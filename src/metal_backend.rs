/// Metal GPU backend for eseqlisp.
#[cfg(target_os = "macos")]
mod inner {
    use std::collections::VecDeque;
    use std::ptr::NonNull;
    use std::time::Duration;

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::NSView;
    use objc2_core_foundation::CGSize;
    use objc2_foundation::NSString;
    use objc2_metal::{
        MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue,
        MTLCreateSystemDefaultDevice, MTLDevice, MTLLibrary, MTLLoadAction, MTLPixelFormat,
        MTLPrimitiveType, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
        MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLResourceOptions, MTLStoreAction,
        MTLTexture,
    };
    use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
    use winit::{
        dpi::PhysicalSize,
        event::{ElementState, Event as WEvent, WindowEvent},
        event_loop::{ControlFlow, EventLoop},
        keyboard::{Key, NamedKey},
        platform::pump_events::EventLoopExtPumpEvents,
        raw_window_handle::{HasWindowHandle, RawWindowHandle},
        window::Window,
    };

    use crate::backend::{Backend, BackendError, Color, RenderFrame};
    use crate::glyph_atlas::GlyphAtlas;

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
        // Render pipeline (compiled from SHADER_SRC in initialize())
        pipeline: Option<Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
        // Glyph atlas (created in initialize())
        atlas: Option<GlyphAtlas>,
        // Winit
        event_loop: Option<EventLoop<()>>,
        window: Option<Window>,
        pending: VecDeque<Event>,
        modifiers: KeyModifiers,
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
                atlas: None,
                event_loop: None,
                window: None,
                pending: VecDeque::new(),
                modifiers: KeyModifiers::NONE,
            })
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
            self.atlas = GlyphAtlas::new(&self.device, "Menlo-Regular", 14.0 * scale);

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
                return Some(ev);
            }
            let Some(event_loop) = &mut self.event_loop else {
                return None;
            };
            let pending = &mut self.pending;
            let modifiers = &mut self.modifiers;
            let layer_ref = &self.layer;
            let window_ref = self.window.as_ref();
            event_loop.pump_events(Some(timeout), |event, elwt| {
                elwt.set_control_flow(ControlFlow::Poll);
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
                        if let Some(ev) = translate_key(&kev.logical_key, *modifiers) {
                            pending.push_back(ev);
                        }
                    }
                    _ => {}
                }
            });
            self.pending.pop_front()
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

            // ── Build vertex data ────────────────────────────────────────────
            let quads = build_quads(frame, atlas, vp_w, vp_h);

            // ── Vertex buffer ────────────────────────────────────────────────
            let vbuf = if quads.is_empty() {
                None
            } else {
                let byte_len = std::mem::size_of_val(quads.as_slice());
                unsafe {
                    self.device.newBufferWithBytes_length_options(
                        NonNull::new(quads.as_ptr() as *mut _).unwrap(),
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
                red: 0.05,
                green: 0.05,
                blue: 0.07,
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

            if let Some(vbuf) = &vbuf {
                enc.setRenderPipelineState(pipeline);
                unsafe {
                    enc.setVertexBuffer_offset_atIndex(Some(vbuf), 0, 0);
                    enc.setFragmentTexture_atIndex(Some(&atlas.texture), 0);
                    enc.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        quads.len() as _,
                    );
                }
            }

            enc.endEncoding();
            buf.presentDrawable(objc2::runtime::ProtocolObject::from_ref(&*drawable));
            buf.commit();
            Ok(())
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
    fn build_quads(
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
                    let cell_bg = cell.style.bg.unwrap_or(Color::BLACK);
                    (to_rgba(cell_bg), to_rgba(cell_fg))
                } else {
                    (
                        to_rgba(cell.style.fg),
                        to_rgba(cell.style.bg.unwrap_or(Color::BLACK)),
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
                    &CharCtx { cell_w, cell_h, vp_w, vp_h, fg, bg },
                    &mut verts,
                );
            }
        }

        // ── Status bar (bottom row) ───────────────────────────────────────────
        let total_rows = (vp_h / cell_h).floor() as usize;
        let status_row = total_rows.saturating_sub(1);
        let status_fg = to_rgba(Color::WHITE);
        let status_bg = to_rgba(Color::rgb(0.25, 0.25, 0.28));

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

            let sel_bg = to_rgba(Color::rgb(0.33, 0.30, 0.59));
            let unsel_bg = to_rgba(Color::rgb(0.15, 0.15, 0.22));
            let pop_fg = to_rgba(Color::WHITE);

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
                        &CharCtx { cell_w, cell_h, vp_w, vp_h, fg: pop_fg, bg },
                        &mut verts,
                    );
                }
            }

            // ── Doc panel (right of the list) ────────────────────────────────
            if let Some((title, body)) = &comp.doc {
                let doc_col = popup_col + label_w + 1;
                let doc_w: usize = 44;
                let doc_h = comp.entries.len().max(4);
                let doc_bg = to_rgba(Color::rgb(0.06, 0.06, 0.09));
                let doc_fg = to_rgba(Color::WHITE);
                let title_fg = to_rgba(Color::rgb(0.73, 0.51, 1.0));

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
                            &CharCtx { cell_w, cell_h, vp_w, vp_h, fg: title_fg, bg: doc_bg },
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
                            &CharCtx { cell_w, cell_h, vp_w, vp_h, fg: doc_fg, bg: doc_bg },
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
                &CharCtx { cell_w, cell_h, vp_w, vp_h, fg: status_fg, bg: status_bg },
                &mut verts,
            );
        }

        verts
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

    fn translate_key(key: &Key, mods: KeyModifiers) -> Option<Event> {
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
        Some(Event::Key(KeyEvent::new(code, mods)))
    }
}

#[cfg(target_os = "macos")]
pub use inner::MetalBackend;
