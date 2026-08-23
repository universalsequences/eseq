/// Spike: open a portable winit window and drive the shared platform glue.
///
/// This is the non-Metal counterpart to `metal_window.rs`: it proves winit,
/// the event loop, and `ui::platform::sync_window_theme` work on targets that
/// have no AppKit — Linux in particular — before the wgpu backend lands.
///
/// Run with: cargo run --example winit_window
/// Set `ESEQ_WINIT_SPIKE_FRAMES=<n>` to pump `n` frames and exit (0 = run
/// until the window is closed); it defaults to 60 so the spike doubles as a
/// non-interactive smoke check.
use std::time::Duration;

use eseqlisp::theme;
use eseqlisp::ui::platform;
use winit::{
    dpi::PhysicalSize,
    event::{Event, WindowEvent},
    event_loop::EventLoop,
    platform::pump_events::{EventLoopExtPumpEvents, PumpStatus},
    window::WindowBuilder,
};

fn main() {
    let frames: u32 = std::env::var("ESEQ_WINIT_SPIKE_FRAMES")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(60);

    let mut event_loop = EventLoop::new().expect("failed to create event loop");
    let window = WindowBuilder::new()
        .with_title("eseqlisp — winit spike")
        .with_inner_size(PhysicalSize::new(800u32, 600u32))
        .build(&event_loop)
        .expect("failed to create window");

    // The same call the Metal backend makes every time the UI theme changes.
    // On macOS it also runs the AppKit path; elsewhere winit talks to the
    // native window system on its own.
    let bg = theme::BG();
    platform::sync_window_theme(&window, bg);

    println!(
        "window opened: {}x{} scale {:.2}, bg luma {:.3} -> {:?} (winit reports {:?})",
        window.inner_size().width,
        window.inner_size().height,
        window.scale_factor(),
        bg.luma(),
        platform::window_theme_for_background(bg),
        window.theme(),
    );

    let mut pumped = 0u32;
    loop {
        let status = event_loop.pump_events(Some(Duration::from_millis(16)), |event, elwt| {
            if let Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } = event
            {
                elwt.exit();
            }
        });
        if let PumpStatus::Exit(code) = status {
            std::process::exit(code);
        }

        pumped += 1;
        if frames != 0 && pumped >= frames {
            println!("pumped {pumped} frames without error — exiting");
            return;
        }
        window.request_redraw();
    }
}
