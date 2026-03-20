/// Metal GPU backend for eseqlisp.
#[cfg(target_os = "macos")]
mod inner {
    use std::collections::{HashMap, VecDeque};
    use std::ptr::NonNull;
    use std::time::{Duration, Instant};

    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::NSView;
    use objc2_core_foundation::CGSize;
    use objc2_foundation::NSString;
    use objc2_metal::{
        MTLBuffer, MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue,
        MTLCreateSystemDefaultDevice, MTLDevice, MTLLibrary, MTLLoadAction, MTLPixelFormat,
        MTLPrimitiveType, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
        MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLResourceOptions, MTLStoreAction,
        MTLTexture,
    };
    use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
    use winit::{
        dpi::PhysicalSize,
        event::{
            ElementState, Event as WEvent, MouseButton as WMouseButton, MouseScrollDelta,
            TouchPhase, WindowEvent,
        },
        event_loop::{ControlFlow, EventLoop},
        keyboard::{Key, KeyCode as WinitKeyCode, NamedKey, PhysicalKey},
        platform::pump_events::EventLoopExtPumpEvents,
        raw_window_handle::{HasWindowHandle, RawWindowHandle},
        window::Window,
    };

    use crate::backend::{Backend, BackendError, Color, RenderFrame};
    use crate::theme;
    use crate::glyph_atlas::GlyphAtlas;
    use crate::layout::Rect;
    use crate::widget_render::{self, WidgetInstance, WidgetViewport};

    // ── Shader source ─────────────────────────────────────────────────────────
    //
    // Buffer-based vertex input: no vertex descriptor needed.
    // UV.v is flipped in the fragment shader because CoreText rasterizes Y-up
    // but Metal textures are Y-down.
    const SHADER_SRC: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Vertex {
    float2 position;
    float2 uv;
    float4 fg;
    float4 bg;
};

struct Varyings {
    float4 position [[position]];
    float2 uv;
    float4 fg;
    float4 bg;
};

vertex Varyings vert(
    uint                    vid   [[vertex_id]],
    device const Vertex*    verts [[buffer(0)]])
{
    Vertex v = verts[vid];
    Varyings out;
    out.position = float4(v.position, 0.0, 1.0);
    out.uv  = v.uv;
    out.fg  = v.fg;
    out.bg  = v.bg;
    return out;
}

fragment float4 frag(
    Varyings              in    [[stage_in]],
    texture2d<float>      atlas [[texture(0)]])
{
    constexpr sampler s(filter::nearest);
    float coverage = atlas.sample(s, in.uv).r;
    return mix(in.bg, in.fg, coverage);
}
"#;

    // ── Widget shader source ────────────────────────────────────────────────
    //
    // SDF-based rendering for sliders and toggles. Each widget is one instanced
    // quad (6 vertices from vertex_id). The fragment shader decides color per
    // pixel using UV coordinates and per-instance data.
    // ── Shared shader preamble (instance struct, varyings, SDF utils) ──────

    const WIDGET_SHADER_PREAMBLE: &str = r#"
#include <metal_stdlib>
using namespace metal;

// Packed types match Rust's #[repr(C)] layout (4-byte alignment).
struct WidgetInstance {
    packed_float2 ndc_min;
    packed_float2 ndc_max;
    float         value_t;
    float         orientation;
    packed_float4 color_a;
    packed_float4 color_b;
    float         corner_radius;
    float         pixel_aspect;
};

struct WidgetVaryings {
    float4 position [[position]];
    float2 uv;
    float  value_t    [[flat]];
    float4 color_a    [[flat]];
    float4 color_b    [[flat]];
    float  aspect     [[flat]];
};

float sdf_rounded_rect(float2 p, float2 half_size, float radius) {
    float2 d = abs(p) - half_size + radius;
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - radius;
}

float compute_border_mask(float2 localPos, float2 outerSize, float cornerRadius,
                          float borderPixels, thread float& outerMask) {
    float outerDist = sdf_rounded_rect(localPos, outerSize, cornerRadius);
    float outerDeriv = max(fwidth(outerDist), 0.001);
    float borderThickness = borderPixels * outerDeriv;
    float2 innerSize = outerSize - float2(borderThickness);
    float innerDist = sdf_rounded_rect(localPos, innerSize, max(cornerRadius - borderThickness, 0.0));
    float innerDeriv = max(fwidth(innerDist), 0.001);
    outerMask = smoothstep(outerDeriv, -outerDeriv, outerDist);
    float innerMask = smoothstep(innerDeriv, -innerDeriv, innerDist);
    return outerMask * (1.0 - innerMask);
}
"#;

    const DEFAULT_WIDGET_VERTEX_SHADER: &str = r#"
vertex WidgetVaryings widget_vert(
    uint vid [[vertex_id]],
    uint iid [[instance_id]],
    device const WidgetInstance* instances [[buffer(0)]])
{
    float2 corners[6] = {
        float2(0, 0), float2(0, 1), float2(1, 0),
        float2(1, 0), float2(0, 1), float2(1, 1)
    };
    float2 corner = corners[vid];
    WidgetInstance inst = instances[iid];
    float2 ndc = mix(inst.ndc_min, inst.ndc_max, corner);

    WidgetVaryings out;
    out.position = float4(ndc, 0.0, 1.0);
    out.uv = corner;
    out.value_t = inst.value_t;
    out.color_a = inst.color_a;
    out.color_b = inst.color_b;
    out.aspect = inst.pixel_aspect;
    return out;
}
"#;

    // ── Vertex type ───────────────────────────────────────────────────────────

    /// One vertex of a cell quad.  Two triangles (6 vertices) form each cell.
    #[repr(C)]
    #[derive(Clone)]
    pub struct Vertex {
        /// NDC position: X in [-1, +1], Y in [-1, +1] (Y+ = up).
        pub position: [f32; 2],
        /// Atlas UV: (0,0) = top-left of atlas texture.
        pub uv: [f32; 2],
        /// Foreground colour (RGBA linear 0..1).
        pub fg: [f32; 4],
        /// Background colour (RGBA linear 0..1).
        pub bg: [f32; 4],
    }

    /// Layout + colour context threaded into `rasterize_char`.
    struct CharCtx {
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
        fg: [f32; 4],
        bg: [f32; 4],
    }

    // ── Backend ───────────────────────────────────────────────────────────────

    pub struct MetalBackend {
        // Metal state
        device: Retained<ProtocolObject<dyn MTLDevice>>,
        command_queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
        layer: Retained<CAMetalLayer>,
        // Text render pipeline (compiled from SHADER_SRC)
        pipeline: Option<Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
        // Per-widget-type GPU pipelines (hslider, vslider, toggle)
        widget_pipelines: HashMap<String, Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
        // Glyph atlas
        atlas: Option<GlyphAtlas>,
        cached_text_key: Option<u64>,
        cached_text_quads: Vec<Vertex>,
        cached_text_buffer: Option<Retained<ProtocolObject<dyn MTLBuffer>>>,
        cached_text_vertex_count: usize,
        stats: RenderStats,
        // Winit
        event_loop: Option<EventLoop<()>>,
        window: Option<Window>,
        pending: VecDeque<Event>,
        pending_drag: Option<Event>,
        pending_magnify: VecDeque<(f64, (f32, f32))>,
        pending_scroll: VecDeque<((f32, f32), (f32, f32))>,
        modifiers: KeyModifiers,
        pressed_mouse_button: Option<MouseButton>,
        cursor_cell: (u16, u16),
        cursor_pos: (f32, f32),
        last_precise_mouse: Option<(f32, f32)>,
    }

    impl MetalBackend {
        pub fn new() -> Result<Self, BackendError> {
            let device = MTLCreateSystemDefaultDevice().ok_or(BackendError::MetalError)?;
            let command_queue = device.newCommandQueue().ok_or(BackendError::MetalError)?;
            let layer = CAMetalLayer::new();
            layer.setDevice(Some(&device));
            layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
            layer.setFramebufferOnly(false); // atlas upload needs non-framebuffer-only
            Ok(Self {
                device,
                command_queue,
                layer,
                pipeline: None,
                widget_pipelines: HashMap::new(),
                atlas: None,
                cached_text_key: None,
                cached_text_quads: Vec::new(),
                cached_text_buffer: None,
                cached_text_vertex_count: 0,
                stats: RenderStats::new(),
                event_loop: None,
                window: None,
                pending: VecDeque::new(),
                pending_drag: None,
                pending_magnify: VecDeque::new(),
                pending_scroll: VecDeque::new(),
                modifiers: KeyModifiers::NONE,
                pressed_mouse_button: None,
                cursor_cell: (0, 0),
                cursor_pos: (0.0, 0.0),
                last_precise_mouse: None,
            })
        }

        pub fn take_last_precise_mouse(&mut self) -> Option<(f32, f32)> {
            self.last_precise_mouse.take()
        }

        pub fn take_pending_magnify(&mut self) -> Option<(f64, (f32, f32))> {
            self.pending_magnify.pop_front()
        }

        pub fn take_pending_scroll(&mut self) -> Option<((f32, f32), (f32, f32))> {
            self.pending_scroll.pop_front()
        }
    }

    impl Backend for MetalBackend {
        fn initialize(&mut self) -> Result<(), BackendError> {
            // ── Window ───────────────────────────────────────────────────────
            let event_loop = EventLoop::new().map_err(|_| BackendError::MetalError)?;
            let window = winit::window::WindowBuilder::new()
                .with_title("eseqlisp")
                .with_inner_size(PhysicalSize::new(1200u32, 800u32))
                .build(&event_loop)
                .map_err(|_| BackendError::MetalError)?;

            let phys = window.inner_size();
            if let Ok(handle) = window.window_handle()
                && let RawWindowHandle::AppKit(appkit) = handle.as_raw()
            {
                unsafe {
                    let ns_view = appkit.ns_view.as_ptr() as *mut NSView;
                    let ns_view = &*ns_view;
                    ns_view.setWantsLayer(true);
                    ns_view.setLayer(Some(&self.layer));
                }
            }
            // Set drawableSize to physical pixels so the Metal texture is full-res
            // on HiDPI/Retina displays (layer bounds default to logical pixels).
            self.layer.setDrawableSize(CGSize {
                width: phys.width as f64,
                height: phys.height as f64,
            });
            self.event_loop = Some(event_loop);
            self.window = Some(window);

            // ── Glyph atlas ──────────────────────────────────────────────────
            let scale = self
                .window
                .as_ref()
                .map(|w| w.scale_factor())
                .unwrap_or(1.0);
            self.atlas = GlyphAtlas::new(&self.device, "JetBrainsMono-Regular", 14.0 * scale);

            // ── Render pipeline ──────────────────────────────────────────────
            let src = NSString::from_str(SHADER_SRC);
            let library = self
                .device
                .newLibraryWithSource_options_error(&src, None)
                .map_err(|_| BackendError::MetalError)?;

            let vert_fn = library
                .newFunctionWithName(&NSString::from_str("vert"))
                .ok_or(BackendError::MetalError)?;
            let frag_fn = library
                .newFunctionWithName(&NSString::from_str("frag"))
                .ok_or(BackendError::MetalError)?;

            let desc = MTLRenderPipelineDescriptor::new();
            desc.setVertexFunction(Some(&vert_fn));
            desc.setFragmentFunction(Some(&frag_fn));
            let attach = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
            attach.setPixelFormat(MTLPixelFormat::BGRA8Unorm);

            self.pipeline = Some(
                self.device
                    .newRenderPipelineStateWithDescriptor_error(&desc)
                    .map_err(|_| BackendError::MetalError)?,
            );

            // ── Widget render pipelines (one per widget type) ────────────────
            // Each widget gets its own fragment shader but shares the vertex
            // shader and SDF utilities from the preamble.
            for (widget_type, vertex_src, fragment_src) in widget_render::widget_shader_sources() {
                let full_src = format!(
                    "{}{}{}",
                    WIDGET_SHADER_PREAMBLE,
                    vertex_src.unwrap_or(DEFAULT_WIDGET_VERTEX_SHADER),
                    fragment_src
                );
                let src_ns = NSString::from_str(&full_src);
                let wlib = self
                    .device
                    .newLibraryWithSource_options_error(&src_ns, None)
                    .map_err(|_| BackendError::MetalError)?;

                let wvert = wlib
                    .newFunctionWithName(&NSString::from_str("widget_vert"))
                    .ok_or(BackendError::MetalError)?;
                let wfrag = wlib
                    .newFunctionWithName(&NSString::from_str("widget_frag"))
                    .ok_or(BackendError::MetalError)?;

                let wdesc = MTLRenderPipelineDescriptor::new();
                wdesc.setVertexFunction(Some(&wvert));
                wdesc.setFragmentFunction(Some(&wfrag));
                let wattach = unsafe { wdesc.colorAttachments().objectAtIndexedSubscript(0) };
                wattach.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
                wattach.setBlendingEnabled(true);
                {
                    use objc2_metal::{MTLBlendFactor, MTLBlendOperation};
                    wattach.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
                    wattach.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                    wattach.setRgbBlendOperation(MTLBlendOperation::Add);
                    wattach.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                    wattach.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                    wattach.setAlphaBlendOperation(MTLBlendOperation::Add);
                }

                let pipeline_state = self
                    .device
                    .newRenderPipelineStateWithDescriptor_error(&wdesc)
                    .map_err(|_| BackendError::MetalError)?;
                self.widget_pipelines
                    .insert(widget_type.to_string(), pipeline_state);
            }

            Ok(())
        }

        fn teardown(&mut self) -> Result<(), BackendError> {
            self.window = None;
            self.event_loop = None;
            Ok(())
        }

        fn viewport_size(&self) -> (usize, usize) {
            let Some(window) = &self.window else {
                return (80, 24);
            };
            let size = window.inner_size();
            let (cell_w, cell_h) = self
                .atlas
                .as_ref()
                .map(|a| (a.cell_w.max(1), a.cell_h.max(1)))
                .unwrap_or((8, 16));
            let cols = (size.width as usize / cell_w).max(1);
            let rows = (size.height as usize / cell_h).max(1);
            (cols, rows)
        }

        fn poll_event(&mut self, timeout: Duration) -> Option<Event> {
            if let Some(ev) = self.pending.pop_front() {
                if matches!(ev, Event::Mouse(_)) {
                    self.last_precise_mouse = Some(self.cursor_pos);
                }
                return Some(ev);
            }
            if let Some(ev) = self.pending_drag.take() {
                self.last_precise_mouse = Some(self.cursor_pos);
                return Some(ev);
            }
            let Some(event_loop) = &mut self.event_loop else {
                return None;
            };
            let pending = &mut self.pending;
            let pending_drag = &mut self.pending_drag;
            let pending_magnify = &mut self.pending_magnify;
            let pending_scroll = &mut self.pending_scroll;
            let modifiers = &mut self.modifiers;
            let pressed_mouse_button = &mut self.pressed_mouse_button;
            let cursor_cell = &mut self.cursor_cell;
            let cursor_pos = &mut self.cursor_pos;
            let layer_ref = &self.layer;
            let window_ref = self.window.as_ref();
            let cell_size = self
                .atlas
                .as_ref()
                .map(|a| (a.cell_w.max(1) as f64, a.cell_h.max(1) as f64))
                .unwrap_or((8.0, 16.0));
            event_loop.pump_events(Some(timeout), |event, elwt| {
                elwt.set_control_flow(ControlFlow::Wait);
                let WEvent::WindowEvent { event, .. } = event else {
                    return;
                };
                match event {
                    WindowEvent::CloseRequested => {
                        pending.push_back(Event::Key(KeyEvent::new(
                            KeyCode::Char('c'),
                            KeyModifiers::CONTROL,
                        )));
                    }
                    WindowEvent::Resized(new_size) => {
                        layer_ref.setDrawableSize(CGSize {
                            width: new_size.width as f64,
                            height: new_size.height as f64,
                        });
                        // Ask macOS to send RedrawRequested during the modal drag loop.
                        if let Some(w) = window_ref {
                            w.request_redraw();
                        }
                        pending.push_back(Event::Resize(
                            new_size.width as u16,
                            new_size.height as u16,
                        ));
                    }
                    WindowEvent::RedrawRequested => {
                        pending.push_back(Event::Resize(0, 0));
                    }
                    WindowEvent::ModifiersChanged(mods) => {
                        *modifiers = winit_mods_to_crossterm(mods.state());
                    }
                    WindowEvent::KeyboardInput { event: kev, .. } => {
                        if kev.state != ElementState::Pressed {
                            return;
                        }
                        if let Some(ev) =
                            translate_key(&kev.logical_key, &kev.physical_key, *modifiers)
                        {
                            pending.push_back(ev);
                        }
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        let exact_col = (position.x / cell_size.0).max(0.0) as f32;
                        let exact_row = (position.y / cell_size.1).max(0.0) as f32;
                        let col = exact_col.floor() as u16;
                        let row = exact_row.floor() as u16;
                        *cursor_pos = (exact_col, exact_row);
                        *cursor_cell = (col, row);
                        if let Some(button) = pressed_mouse_button {
                            *pending_drag = Some(Event::Mouse(MouseEvent {
                                kind: MouseEventKind::Drag(*button),
                                column: col,
                                row,
                                modifiers: *modifiers,
                            }));
                        }
                    }
                    WindowEvent::MouseInput { state, button, .. } => {
                        let Some(button) = translate_mouse_button(button) else {
                            return;
                        };
                        match state {
                            ElementState::Pressed => {
                                *pressed_mouse_button = Some(button);
                                pending.push_back(Event::Mouse(MouseEvent {
                                    kind: MouseEventKind::Down(button),
                                    column: cursor_cell.0,
                                    row: cursor_cell.1,
                                    modifiers: *modifiers,
                                }));
                            }
                            ElementState::Released => {
                                pending.push_back(Event::Mouse(MouseEvent {
                                    kind: MouseEventKind::Up(button),
                                    column: cursor_cell.0,
                                    row: cursor_cell.1,
                                    modifiers: *modifiers,
                                }));
                                if pressed_mouse_button.as_ref() == Some(&button) {
                                    *pressed_mouse_button = None;
                                }
                            }
                        }
                    }
                    WindowEvent::MouseWheel { delta, phase, .. } => {
                        if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                            return;
                        }
                        let kind = match delta {
                            MouseScrollDelta::LineDelta(x, y) => {
                                if y > 0.0 {
                                    Some(MouseEventKind::ScrollUp)
                                } else if y < 0.0 {
                                    Some(MouseEventKind::ScrollDown)
                                } else if x > 0.0 {
                                    Some(MouseEventKind::ScrollLeft)
                                } else if x < 0.0 {
                                    Some(MouseEventKind::ScrollRight)
                                } else {
                                    None
                                }
                            }
                            MouseScrollDelta::PixelDelta(delta) => {
                                pending_scroll.push_back((
                                    (delta.x as f32, delta.y as f32),
                                    *cursor_pos,
                                ));
                                None
                            }
                        };
                        if let Some(kind) = kind {
                            pending.push_back(Event::Mouse(MouseEvent {
                                kind,
                                column: cursor_cell.0,
                                row: cursor_cell.1,
                                modifiers: *modifiers,
                            }));
                        }
                    }
                    WindowEvent::TouchpadMagnify { delta, phase, .. } => {
                        if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                            return;
                        }
                        pending_magnify.push_back((delta, *cursor_pos));
                    }
                    _ => {}
                }
            });
            if let Some(ev) = self.pending.pop_front() {
                if matches!(ev, Event::Mouse(_)) {
                    self.last_precise_mouse = Some(self.cursor_pos);
                }
                Some(ev)
            } else if let Some(ev) = self.pending_drag.take() {
                self.last_precise_mouse = Some(self.cursor_pos);
                Some(ev)
            } else {
                None
            }
        }

        fn render(&mut self, frame: &RenderFrame) -> Result<(), BackendError> {
            let (Some(pipeline), Some(atlas)) = (&self.pipeline, &mut self.atlas) else {
                return Ok(());
            };

            // ── Draw ─────────────────────────────────────────────────────────
            // Get the drawable first so we know the exact texture dimensions.
            let Some(drawable) = self.layer.nextDrawable() else {
                return Ok(());
            };
            let texture = drawable.texture();
            let vp_w = texture.width() as f32;
            let vp_h = texture.height() as f32;

            // ── Build/cached text vertex data ───────────────────────────────
            let mut text_upload_bytes = 0;
            if self.cached_text_key != Some(frame.text_cache_key) {
                self.cached_text_quads = build_text_quads(frame, atlas, vp_w, vp_h);
                self.cached_text_key = Some(frame.text_cache_key);
                self.cached_text_vertex_count = self.cached_text_quads.len();
                self.cached_text_buffer = if self.cached_text_quads.is_empty() {
                    None
                } else {
                    let byte_len = std::mem::size_of_val(self.cached_text_quads.as_slice());
                    text_upload_bytes = byte_len;
                    unsafe {
                        self.device.newBufferWithBytes_length_options(
                            NonNull::new(self.cached_text_quads.as_ptr() as *mut _).unwrap(),
                            byte_len,
                            MTLResourceOptions(0),
                        )
                    }
                };
            }
            let max_rows = (vp_h / atlas.cell_h as f32).floor() as u16 - 1;
            let primitive_scene = frame
                .widget_layout
                .as_ref()
                .map(|layout| {
                    widget_render::collect_metal_primitives(
                        layout,
                        WidgetViewport {
                            cell_w: atlas.cell_w as f32,
                            cell_h: atlas.cell_h as f32,
                            vp_w,
                            vp_h,
                            focused_widget_id: frame.focused_widget_id,
                        },
                        frame.widget_scroll_top,
                        max_rows,
                    )
                })
                .unwrap_or_default();
            let primitive_quads =
                build_widget_primitive_quads(&primitive_scene, atlas, vp_w, vp_h);
            let primitive_instance_batches = group_widget_instances(&primitive_scene);

            // ── Vertex buffer ────────────────────────────────────────────────
            let text_vbuf = self.cached_text_buffer.as_ref();
            let label_vbuf = if primitive_quads.is_empty() {
                None
            } else {
                let byte_len = std::mem::size_of_val(primitive_quads.as_slice());
                unsafe {
                    self.device.newBufferWithBytes_length_options(
                        NonNull::new(primitive_quads.as_ptr() as *mut _).unwrap(),
                        byte_len,
                        MTLResourceOptions(0),
                    )
                }
            };

            let desc = MTLRenderPassDescriptor::new();
            let attach = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
            attach.setTexture(Some(&texture));
            attach.setLoadAction(MTLLoadAction::Clear);
            attach.setClearColor(MTLClearColor {
                red: theme::BG.r as f64,
                green: theme::BG.g as f64,
                blue: theme::BG.b as f64,
                alpha: 1.0,
            });
            attach.setStoreAction(MTLStoreAction::Store);

            let buf = self
                .command_queue
                .commandBuffer()
                .ok_or(BackendError::MetalError)?;
            let enc = buf
                .renderCommandEncoderWithDescriptor(&desc)
                .ok_or(BackendError::MetalError)?;

            if let Some(vbuf) = &text_vbuf {
                enc.setRenderPipelineState(pipeline);
                unsafe {
                    enc.setVertexBuffer_offset_atIndex(Some(vbuf), 0, 0);
                    enc.setFragmentTexture_atIndex(Some(&atlas.texture), 0);
                    enc.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        self.cached_text_vertex_count as _,
                    );
                }
            }

            if let Some(vbuf) = &label_vbuf {
                enc.setRenderPipelineState(pipeline);
                unsafe {
                    enc.setVertexBuffer_offset_atIndex(Some(vbuf), 0, 0);
                    enc.setFragmentTexture_atIndex(Some(&atlas.texture), 0);
                    enc.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        primitive_quads.len() as _,
                    );
                }
            }

            let mut widget_upload_bytes = 0;
            for (widget_type, instances) in &primitive_instance_batches {
                let Some(wpipe) = self.widget_pipelines.get(widget_type) else {
                    continue;
                };
                if instances.is_empty() {
                    continue;
                }
                let byte_len = std::mem::size_of_val(instances.as_slice());
                widget_upload_bytes += byte_len;
                let Some(wbuf) = (unsafe {
                    self.device.newBufferWithBytes_length_options(
                        NonNull::new(instances.as_ptr() as *mut _).unwrap(),
                        byte_len,
                        MTLResourceOptions(0),
                    )
                }) else {
                    continue;
                };
                enc.setRenderPipelineState(wpipe);
                unsafe {
                    enc.setVertexBuffer_offset_atIndex(Some(&wbuf), 0, 0);
                    enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        6,
                        instances.len() as _,
                    );
                }
            }

            let text_bytes = text_upload_bytes;
            let label_bytes = primitive_quads.len() * std::mem::size_of::<Vertex>();
            let widget_bytes = widget_upload_bytes;

            enc.endEncoding();
            buf.presentDrawable(objc2::runtime::ProtocolObject::from_ref(&*drawable));
            buf.commit();
            self.stats.note_frame(text_bytes, label_bytes, widget_bytes);
            Ok(())
        }
    }

    struct RenderStats {
        window_start: Instant,
        frames: u64,
        text_bytes: usize,
        label_bytes: usize,
        widget_bytes: usize,
    }

    impl RenderStats {
        fn new() -> Self {
            Self {
                window_start: Instant::now(),
                frames: 0,
                text_bytes: 0,
                label_bytes: 0,
                widget_bytes: 0,
            }
        }

        fn note_frame(&mut self, text_bytes: usize, label_bytes: usize, widget_bytes: usize) {
            self.frames += 1;
            self.text_bytes += text_bytes;
            self.label_bytes += label_bytes;
            self.widget_bytes += widget_bytes;

            let elapsed = self.window_start.elapsed();
            if elapsed.as_secs_f64() < 1.0 {
                return;
            }

            let secs = elapsed.as_secs_f64();
            let fps = self.frames as f64 / secs;
            let total_mb =
                (self.text_bytes + self.label_bytes + self.widget_bytes) as f64 / (1024.0 * 1024.0);
            let mbps = total_mb / secs;
            eprintln!(
                "[metal-stats] fps={fps:.1} upload={mbps:.2}MB/s text={:.2}MB/s labels={:.2}MB/s widgets={:.2}MB/s",
                self.text_bytes as f64 / (1024.0 * 1024.0) / secs,
                self.label_bytes as f64 / (1024.0 * 1024.0) / secs,
                self.widget_bytes as f64 / (1024.0 * 1024.0) / secs,
            );

            self.window_start = Instant::now();
            self.frames = 0;
            self.text_bytes = 0;
            self.label_bytes = 0;
            self.widget_bytes = 0;
        }
    }

    fn rasterize_char(
        atlas: &mut GlyphAtlas,
        ch: char,
        (col, row): (usize, usize),
        ctx: &CharCtx,
        out: &mut Vec<Vertex>,
    ) {
        let Some(entry) = atlas.get_or_rasterize(ch) else {
            return;
        };
        let [u0, v0] = entry.uv_min;
        let [u1, v1] = entry.uv_max;

        let ndc_x = |px: f32| px / ctx.vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / ctx.vp_h * 2.0;
        let x0 = ndc_x(col as f32 * ctx.cell_w);
        let x1 = ndc_x((col + 1) as f32 * ctx.cell_w);
        let y0 = ndc_y(row as f32 * ctx.cell_h);
        let y1 = ndc_y((row + 1) as f32 * ctx.cell_h);

        let gv = |px, py, u, v| Vertex {
            position: [px, py],
            uv: [u, v],
            fg: ctx.fg,
            bg: ctx.bg,
        };
        out.extend_from_slice(&[
            gv(x0, y0, u0, v0),
            gv(x0, y1, u0, v1),
            gv(x1, y0, u1, v0),
            gv(x1, y0, u1, v0),
            gv(x0, y1, u0, v1),
            gv(x1, y1, u1, v1),
        ]);
    }

    // ── Quad builder ──────────────────────────────────────────────────────────

    /// Convert a `RenderFrame` into a flat list of triangle vertices.
    ///
    /// Each cell becomes 6 vertices (2 triangles).
    /// Coordinate system:
    ///   - Screen pixel (0, 0) = top-left of window.
    ///   - Metal NDC: X ∈ [-1, +1] left→right, Y ∈ [-1, +1] bottom→top.
    ///   - Conversion: ndc_x = (px_x / vp_w) * 2 - 1
    ///                 ndc_y = 1 - (px_y / vp_h) * 2
    fn build_text_quads(
        frame: &RenderFrame,
        atlas: &mut GlyphAtlas,
        vp_w: f32,
        vp_h: f32,
    ) -> Vec<Vertex> {
        let cell_w = atlas.cell_w as f32;
        let cell_h = atlas.cell_h as f32;
        let mut verts = Vec::with_capacity(frame.lines.len() * 80 * 6);

        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        let to_rgba = |c: Color| [c.r, c.g, c.b, c.a];

        for (row, line) in frame.lines.iter().enumerate() {
            for (col, cell) in line.iter().enumerate() {
                let is_cursor = frame.cursor == Some((row, col));

                let x0 = ndc_x(col as f32 * cell_w);
                let x1 = ndc_x((col + 1) as f32 * cell_w);
                let y0 = ndc_y(row as f32 * cell_h); // top (larger NDC Y)
                let y1 = ndc_y((row + 1) as f32 * cell_h); // bottom

                // Cursor inverts fg/bg; otherwise use cell style.
                let (fg, bg) = if is_cursor {
                    let cell_fg = cell.style.fg;
                    let cell_bg = cell.style.bg.unwrap_or(theme::BG);
                    (to_rgba(cell_bg), to_rgba(cell_fg))
                } else {
                    (
                        to_rgba(cell.style.fg),
                        to_rgba(cell.style.bg.unwrap_or(theme::BG)),
                    )
                };

                // Background quad — solid color, zero coverage glyph UV.
                let bg_v = |px, py| Vertex {
                    position: [px, py],
                    uv: [0.0, 0.0],
                    fg: bg,
                    bg,
                };
                verts.extend_from_slice(&[
                    bg_v(x0, y0),
                    bg_v(x0, y1),
                    bg_v(x1, y0),
                    bg_v(x1, y0),
                    bg_v(x0, y1),
                    bg_v(x1, y1),
                ]);

                // Glyph quad — skip spaces (cursor on space is handled by bg inversion above).
                if cell.ch == ' ' {
                    continue;
                }

                rasterize_char(
                    atlas,
                    cell.ch,
                    (col, row),
                    &CharCtx {
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                        fg,
                        bg,
                    },
                    &mut verts,
                );
            }
        }

        // ── Status bar (bottom row) ───────────────────────────────────────────
        let total_rows = (vp_h / cell_h).floor() as usize;
        let status_row = total_rows.saturating_sub(1);
        let status_fg = to_rgba(theme::STATUS_FG);
        let status_bg = to_rgba(theme::STATUS_BG);

        // ── Completion popup ─────────────────────────────────────────────────
        if let Some(comp) = &frame.completion {
            let label_w = comp
                .entries
                .iter()
                .map(|e| e.label.len())
                .max()
                .unwrap_or(0)
                .max(12);
            let popup_col = comp.anchor.1;
            let popup_row = comp.anchor.0 + 1; // one row below the cursor

            let sel_bg = to_rgba(theme::COMP_SELECTED_BG);
            let unsel_bg = to_rgba(theme::COMP_UNSELECTED_BG);
            let pop_fg = to_rgba(theme::COMP_FG);

            let x0 = ndc_x(popup_col as f32 * cell_w);
            let x1 = ndc_x((popup_col + label_w) as f32 * cell_w);
            for (i, entry) in comp.entries.iter().enumerate() {
                let row = popup_row + i;
                if row >= status_row {
                    break;
                }
                let y0 = ndc_y(row as f32 * cell_h); // top (larger NDC Y)
                let y1 = ndc_y((row + 1) as f32 * cell_h); // bottom
                let bg = if entry.selected { sel_bg } else { unsel_bg };
                let gv = |px, py, u, v| Vertex {
                    position: [px, py],
                    uv: [u, v],
                    fg: pop_fg,
                    bg,
                };

                verts.extend_from_slice(&[
                    gv(x0, y0, 0.0, 0.0),
                    gv(x0, y1, 0.0, 0.0),
                    gv(x1, y0, 0.0, 0.0),
                    gv(x1, y0, 0.0, 0.0),
                    gv(x0, y1, 0.0, 0.0),
                    gv(x1, y1, 0.0, 0.0),
                ]);

                for (j, ch) in entry.label.chars().enumerate() {
                    let ch_row = row;
                    let ch_col = popup_col + j;

                    rasterize_char(
                        atlas,
                        ch,
                        (ch_col, ch_row),
                        &CharCtx {
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                            fg: pop_fg,
                            bg,
                        },
                        &mut verts,
                    );
                }
            }

            // ── Doc panel (right of the list) ────────────────────────────────
            if let Some((title, body)) = &comp.doc {
                let doc_col = popup_col + label_w + 1;
                let doc_w: usize = 44;
                let doc_h = comp.entries.len().max(4);
                let doc_bg = to_rgba(theme::COMP_DOC_BG);
                let doc_fg = to_rgba(theme::COMP_DOC_FG);
                let title_fg = to_rgba(theme::COMP_DOC_TITLE_FG);

                // Background for the whole panel.
                let dx0 = ndc_x(doc_col as f32 * cell_w);
                let dx1 = ndc_x((doc_col + doc_w) as f32 * cell_w);
                let dy0 = ndc_y(popup_row as f32 * cell_h);
                let dy1 = ndc_y((popup_row + doc_h) as f32 * cell_h);
                let db = |px, py| Vertex {
                    position: [px, py],
                    uv: [0.0, 0.0],
                    fg: doc_bg,
                    bg: doc_bg,
                };
                verts.extend_from_slice(&[
                    db(dx0, dy0),
                    db(dx0, dy1),
                    db(dx1, dy0),
                    db(dx1, dy0),
                    db(dx0, dy1),
                    db(dx1, dy1),
                ]);

                // Title on row 0.
                let title_row = popup_row;
                if title_row < status_row {
                    for (j, ch) in title.chars().take(doc_w).enumerate() {
                        if ch == ' ' {
                            continue;
                        }
                        rasterize_char(
                            atlas,
                            ch,
                            (doc_col + j, title_row),
                            &CharCtx {
                                cell_w,
                                cell_h,
                                vp_w,
                                vp_h,
                                fg: title_fg,
                                bg: doc_bg,
                            },
                            &mut verts,
                        );
                    }
                }

                // Body lines starting at row 2 (row 1 is the blank separator).
                for (li, line) in body.iter().enumerate() {
                    let doc_row = popup_row + 2 + li;
                    if doc_row >= popup_row + doc_h || doc_row >= status_row {
                        break;
                    }
                    for (j, ch) in line.chars().take(doc_w).enumerate() {
                        if ch == ' ' {
                            continue;
                        }
                        rasterize_char(
                            atlas,
                            ch,
                            (doc_col + j, doc_row),
                            &CharCtx {
                                cell_w,
                                cell_h,
                                vp_w,
                                vp_h,
                                fg: doc_fg,
                                bg: doc_bg,
                            },
                            &mut verts,
                        );
                    }
                }
            }
        }

        // Fill the whole status row with background first.
        let total_cols = (vp_w / cell_w).floor() as usize;
        let sx0 = ndc_x(0.0);
        let sx1 = ndc_x(total_cols as f32 * cell_w);
        let sy0 = ndc_y(status_row as f32 * cell_h);
        let sy1 = ndc_y((status_row + 1) as f32 * cell_h);
        let sb = |px, py| Vertex {
            position: [px, py],
            uv: [0.0, 0.0],
            fg: status_bg,
            bg: status_bg,
        };
        verts.extend_from_slice(&[
            sb(sx0, sy0),
            sb(sx0, sy1),
            sb(sx1, sy0),
            sb(sx1, sy0),
            sb(sx0, sy1),
            sb(sx1, sy1),
        ]);

        // Render each character in frame.status.
        for (col, ch) in frame.status.chars().enumerate() {
            if col >= total_cols {
                break;
            }
            if ch == ' ' {
                continue;
            }

            rasterize_char(
                atlas,
                ch,
                (col, status_row),
                &CharCtx {
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                    fg: status_fg,
                    bg: status_bg,
                },
                &mut verts,
            );
        }

        verts
    }

    fn build_widget_primitive_quads(
        primitives: &[widget_render::MetalPrimitive],
        atlas: &mut GlyphAtlas,
        vp_w: f32,
        vp_h: f32,
    ) -> Vec<Vertex> {
        let cell_w = atlas.cell_w as f32;
        let cell_h = atlas.cell_h as f32;
        let mut verts = Vec::new();
        for primitive in primitives {
            match primitive {
                widget_render::MetalPrimitive::Rect(rect) => {
                    push_solid_rect_vertices(rect.rect, rect.color, cell_w, cell_h, vp_w, vp_h, &mut verts);
                }
                widget_render::MetalPrimitive::Quad(quad) => {
                    push_solid_quad_vertices(*quad, cell_w, cell_h, vp_w, vp_h, &mut verts);
                }
                widget_render::MetalPrimitive::GlyphRun(run) => {
                    for (idx, ch) in run.text.chars().enumerate() {
                        if ch == ' ' {
                            continue;
                        }
                        rasterize_char(
                            atlas,
                            ch,
                            (run.col as usize + idx, run.row as usize),
                            &CharCtx {
                                cell_w,
                                cell_h,
                                vp_w,
                                vp_h,
                                fg: run.fg.to_rgba(),
                                bg: run.bg.to_rgba(),
                            },
                            &mut verts,
                        );
                    }
                }
                widget_render::MetalPrimitive::WidgetInstance { .. } => {}
            }
        }
        verts
    }

    fn push_solid_rect_vertices(
        rect: Rect,
        color: Color,
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
        verts: &mut Vec<Vertex>,
    ) {
        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        let x0 = ndc_x(rect.col as f32 * cell_w);
        let x1 = ndc_x((rect.col + rect.width) as f32 * cell_w);
        let y0 = ndc_y(rect.row as f32 * cell_h);
        let y1 = ndc_y((rect.row + rect.height) as f32 * cell_h);
        let rgba = color.to_rgba();
        let v = |px, py| Vertex {
            position: [px, py],
            uv: [0.0, 0.0],
            fg: rgba,
            bg: rgba,
        };
        verts.extend_from_slice(&[
            v(x0, y0),
            v(x0, y1),
            v(x1, y0),
            v(x1, y0),
            v(x0, y1),
            v(x1, y1),
        ]);
    }

    fn push_solid_quad_vertices(
        quad: widget_render::MetalQuadPrimitive,
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
        verts: &mut Vec<Vertex>,
    ) {
        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        let x0 = ndc_x(quad.x * cell_w);
        let x1 = ndc_x((quad.x + quad.width) * cell_w);
        let y0 = ndc_y(quad.y * cell_h);
        let y1 = ndc_y((quad.y + quad.height) * cell_h);
        let rgba = quad.color.to_rgba();
        let v = |px, py| Vertex {
            position: [px, py],
            uv: [0.0, 0.0],
            fg: rgba,
            bg: rgba,
        };
        verts.extend_from_slice(&[
            v(x0, y0),
            v(x0, y1),
            v(x1, y0),
            v(x1, y0),
            v(x0, y1),
            v(x1, y1),
        ]);
    }

    fn group_widget_instances(
        primitives: &[widget_render::MetalPrimitive],
    ) -> HashMap<String, Vec<WidgetInstance>> {
        let mut batches = HashMap::new();
        for primitive in primitives {
            if let widget_render::MetalPrimitive::WidgetInstance {
                widget_type,
                instance,
            } = primitive
            {
                batches
                    .entry(widget_type.clone())
                    .or_insert_with(Vec::new)
                    .push(*instance);
            }
        }
        batches
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn winit_mods_to_crossterm(mods: winit::keyboard::ModifiersState) -> KeyModifiers {
        let mut out = KeyModifiers::NONE;
        if mods.shift_key() {
            out |= KeyModifiers::SHIFT;
        }
        if mods.control_key() {
            out |= KeyModifiers::CONTROL;
        }
        if mods.alt_key() {
            out |= KeyModifiers::ALT;
        }
        out
    }

    fn translate_key(key: &Key, physical_key: &PhysicalKey, mods: KeyModifiers) -> Option<Event> {
        let code = if mods.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) {
            translate_physical_shortcut_key(physical_key).or_else(|| translate_logical_key(key))?
        } else {
            translate_logical_key(key)?
        };
        Some(Event::Key(KeyEvent::new(code, mods)))
    }

    fn translate_logical_key(key: &Key) -> Option<KeyCode> {
        let code = match key {
            Key::Named(named) => match named {
                NamedKey::Enter => KeyCode::Enter,
                NamedKey::Escape => KeyCode::Esc,
                NamedKey::Backspace => KeyCode::Backspace,
                NamedKey::Delete => KeyCode::Delete,
                NamedKey::Tab => KeyCode::Tab,
                NamedKey::Space => KeyCode::Char(' '),
                NamedKey::ArrowUp => KeyCode::Up,
                NamedKey::ArrowDown => KeyCode::Down,
                NamedKey::ArrowLeft => KeyCode::Left,
                NamedKey::ArrowRight => KeyCode::Right,
                NamedKey::Home => KeyCode::Home,
                NamedKey::End => KeyCode::End,
                NamedKey::PageUp => KeyCode::PageUp,
                NamedKey::PageDown => KeyCode::PageDown,
                _ => return None,
            },
            Key::Character(s) => KeyCode::Char(s.chars().next()?),
            _ => return None,
        };
        Some(code)
    }

    fn translate_physical_shortcut_key(key: &PhysicalKey) -> Option<KeyCode> {
        let PhysicalKey::Code(code) = key else {
            return None;
        };
        let code = match code {
            WinitKeyCode::KeyA => KeyCode::Char('a'),
            WinitKeyCode::KeyB => KeyCode::Char('b'),
            WinitKeyCode::KeyC => KeyCode::Char('c'),
            WinitKeyCode::KeyD => KeyCode::Char('d'),
            WinitKeyCode::KeyE => KeyCode::Char('e'),
            WinitKeyCode::KeyF => KeyCode::Char('f'),
            WinitKeyCode::KeyG => KeyCode::Char('g'),
            WinitKeyCode::KeyH => KeyCode::Char('h'),
            WinitKeyCode::KeyI => KeyCode::Char('i'),
            WinitKeyCode::KeyJ => KeyCode::Char('j'),
            WinitKeyCode::KeyK => KeyCode::Char('k'),
            WinitKeyCode::KeyL => KeyCode::Char('l'),
            WinitKeyCode::KeyM => KeyCode::Char('m'),
            WinitKeyCode::KeyN => KeyCode::Char('n'),
            WinitKeyCode::KeyO => KeyCode::Char('o'),
            WinitKeyCode::KeyP => KeyCode::Char('p'),
            WinitKeyCode::KeyQ => KeyCode::Char('q'),
            WinitKeyCode::KeyR => KeyCode::Char('r'),
            WinitKeyCode::KeyS => KeyCode::Char('s'),
            WinitKeyCode::KeyT => KeyCode::Char('t'),
            WinitKeyCode::KeyU => KeyCode::Char('u'),
            WinitKeyCode::KeyV => KeyCode::Char('v'),
            WinitKeyCode::KeyW => KeyCode::Char('w'),
            WinitKeyCode::KeyX => KeyCode::Char('x'),
            WinitKeyCode::KeyY => KeyCode::Char('y'),
            WinitKeyCode::KeyZ => KeyCode::Char('z'),
            WinitKeyCode::ArrowUp => KeyCode::Up,
            WinitKeyCode::ArrowDown => KeyCode::Down,
            WinitKeyCode::ArrowLeft => KeyCode::Left,
            WinitKeyCode::ArrowRight => KeyCode::Right,
            WinitKeyCode::Space => KeyCode::Char(' '),
            WinitKeyCode::Tab => KeyCode::Tab,
            WinitKeyCode::Backspace => KeyCode::Backspace,
            WinitKeyCode::Delete => KeyCode::Delete,
            WinitKeyCode::Enter => KeyCode::Enter,
            WinitKeyCode::Home => KeyCode::Home,
            WinitKeyCode::End => KeyCode::End,
            WinitKeyCode::PageUp => KeyCode::PageUp,
            WinitKeyCode::PageDown => KeyCode::PageDown,
            _ => return None,
        };
        Some(code)
    }

    fn translate_mouse_button(button: WMouseButton) -> Option<MouseButton> {
        match button {
            WMouseButton::Left => Some(MouseButton::Left),
            WMouseButton::Right => Some(MouseButton::Right),
            WMouseButton::Middle => Some(MouseButton::Middle),
            _ => None,
        }
    }
}

#[cfg(target_os = "macos")]
pub use inner::MetalBackend;
