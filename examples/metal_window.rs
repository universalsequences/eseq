/// Spike: open a Metal-backed window and clear it each frame.
/// Run with: cargo run --example metal_window
#[cfg(target_os = "macos")]
fn main() {
    use objc2_app_kit::NSView;
    use objc2_metal::{
        MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue,
        MTLCreateSystemDefaultDevice, MTLDevice, MTLLoadAction, MTLPixelFormat,
        MTLRenderPassDescriptor, MTLStoreAction,
    };
    use objc2_quartz_core::CAMetalLayer;
    use winit::{
        dpi::PhysicalSize,
        event::{Event, WindowEvent},
        event_loop::{ControlFlow, EventLoop},
        raw_window_handle::{HasWindowHandle, RawWindowHandle},
        window::WindowBuilder,
    };

    // ── Metal setup ───────────────────────────────────────────────────────────
    let device = MTLCreateSystemDefaultDevice().expect("no Metal-capable GPU found");
    let command_queue = device
        .newCommandQueue()
        .expect("failed to create command queue");

    let layer = CAMetalLayer::new();
    layer.setDevice(Some(&device));
    layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
    layer.setFramebufferOnly(true);

    println!("Metal device: {:?}", device.name());

    // ── Window ────────────────────────────────────────────────────────────────
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let window = WindowBuilder::new()
        .with_title("eseqlisp — Metal spike")
        .with_inner_size(PhysicalSize::new(1200u32, 800u32))
        .build(&event_loop)
        .expect("failed to create window");

    // Attach the CAMetalLayer to the window's NSView.
    if let Ok(handle) = window.window_handle()
        && let RawWindowHandle::AppKit(appkit) = handle.as_raw()
    {
        unsafe {
            let ns_view = appkit.ns_view.as_ptr() as *mut NSView;
            let ns_view = &*ns_view;
            ns_view.setWantsLayer(true);
            ns_view.setLayer(Some(&layer));
        }
    }

    println!("Window ready — close it to exit.");

    // ── Event loop ────────────────────────────────────────────────────────────
    event_loop
        .run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::Poll);

            match event {
                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::CloseRequested => elwt.exit(),

                    WindowEvent::RedrawRequested => {
                        use objc2_quartz_core::CAMetalDrawable;

                        let Some(drawable) = layer.nextDrawable() else { return; };
                        let desc = MTLRenderPassDescriptor::new();
                        let attach = unsafe {
                            desc.colorAttachments().objectAtIndexedSubscript(0)
                        };
                        let texture = drawable.texture();
                        attach.setTexture(Some(&texture));
                        attach.setLoadAction(MTLLoadAction::Clear);
                        attach.setClearColor(MTLClearColor {
                            red: 0.1,
                            green: 0.05,
                            blue: 0.15,
                            alpha: 1.0,
                        });
                        attach.setStoreAction(MTLStoreAction::Store);

                        let buf = command_queue.commandBuffer().unwrap();
                        let enc = buf.renderCommandEncoderWithDescriptor(&desc).unwrap();
                        enc.endEncoding();
                        buf.presentDrawable(
                            objc2::runtime::ProtocolObject::from_ref(&*drawable),
                        );
                        buf.commit();
                    }

                    _ => {}
                },

                // AboutToWait fires once the event queue drains in Poll mode —
                // use it to drive a continuous redraw.
                Event::AboutToWait => window.request_redraw(),

                _ => {}
            }
        })
        .expect("event loop error");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Metal backend is macOS only.");
}
