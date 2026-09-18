//! Main-thread native integration check. No project, audio, visible window or
//! synthetic OS input; exercises the production driver and queue directly.
use super::*;
use crate::ui::host_loop::{HostLoopAction, HostLoopControl};
use std::sync::mpsc;
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = winit::event_loop::EventLoopBuilder::new();
    builder.with_activation_policy(ActivationPolicy::Prohibited)
        .with_default_menu(false).with_activate_ignoring_other_apps(false);
    let event_loop = builder.build()?;
    let window = winit::window::WindowBuilder::new().with_visible(false)
        .with_inner_size(winit::dpi::PhysicalSize::new(320, 240)).build(&event_loop)?;
    let mut backend = MetalBackend::new_capture(320, 240).map_err(|_| "create Metal backend")?;
    backend.window = Some(window);
    backend.event_loop = Some(event_loop);
    let waker = backend.event_loop_waker().ok_or("create native waker")?;
    let (start_tx, start_rx) = mpsc::channel();
    let (sent_tx, sent_rx) = mpsc::channel();
    let (ack_tx, ack_rx) = mpsc::channel();
    let sender = std::thread::spawn(move || {
        start_rx.recv_timeout(Duration::from_secs(2)).expect("host startup");
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(10));
            sent_tx.send(Instant::now()).unwrap();
            assert!(waker.wake());
            ack_rx.recv_timeout(Duration::from_secs(1)).expect("host wake acknowledgement");
        }
    });
    let started = Instant::now();
    let mut start_tx = Some(start_tx);
    let mut delays = Vec::new();
    let mut resize_callbacks = 0;
    let mut queued_resizes = 0;
    let mut resize_requested = false;
    let mut keys = String::new();
    let mut key_started = None;
    let mut key_elapsed = None;
    let mut paced_ticks = 0;
    let mut next_frame = None;
    let mut ticks = 0;
    backend.run_host_loop(|backend, action| {
        if started.elapsed() > Duration::from_secs(5) { return Err("native host probe timed out".into()); }
        if action == HostLoopAction::LiveResize {
            resize_callbacks += 1;
            let size = backend.window.as_ref().unwrap().inner_size();
            let drawable = backend.layer.drawableSize();
            assert_eq!((drawable.width as u32, drawable.height as u32), (size.width, size.height));
            return Ok(HostLoopControl::WaitUntil(Instant::now()));
        }
        ticks += 1;
        if let Some(start) = start_tx.take() { start.send(()).unwrap(); }
        for sent in sent_rx.try_iter() {
            delays.push(sent.elapsed());
            ack_tx.send(()).unwrap();
        }
        // One event per host tick deliberately leaves an ordering barrier.
        if let Some(event) = backend.next_queued_backend_event() {
            match event {
                BackendEvent::Terminal(Event::Key(key)) => {
                    if let KeyCode::Char(ch) = key.code { keys.push(ch); }
                    if keys == "abc" { key_elapsed = key_started.map(|start: Instant| start.elapsed()); }
                }
                BackendEvent::Terminal(Event::Resize(_, _)) => queued_resizes += 1,
                BackendEvent::Quit => return Ok(HostLoopControl::Exit),
                _ => {}
            }
        }
        if delays.len() == 20 && !resize_requested {
            resize_requested = true;
            let _ = backend.window.as_ref().unwrap().request_inner_size(winit::dpi::PhysicalSize::new(800, 600));
            key_started = Some(Instant::now());
            for ch in ['a', 'b', 'c'] {
                backend.pending.push_back(Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)));
            }
        }
        let now = Instant::now();
        if keys == "abc" && resize_callbacks > 0 && queued_resizes > 0 {
            let deadline = next_frame.get_or_insert(now);
            if now >= *deadline {
                paced_ticks += 1;
                let interval = Duration::from_secs_f64(1.0 / 60.0);
                let late = now.duration_since(*deadline).as_nanos();
                *deadline = now + interval - Duration::from_nanos((late % interval.as_nanos()) as u64);
            }
            if paced_ticks >= 60 {
                // A queued close must also bypass a distant idle deadline.
                backend.close_requested = true;
                return Ok(HostLoopControl::WaitUntil(now + Duration::from_secs(1)));
            }
            return Ok(HostLoopControl::WaitUntil(*deadline));
        }
        Ok(HostLoopControl::WaitUntil(now + Duration::from_millis(200)))
    })?;
    sender.join().map_err(|_| "native wake sender panicked")?;
    assert_eq!(delays.len(), 20);
    assert!(delays.iter().all(|delay| *delay < Duration::from_millis(100)), "wake delayed to idle deadline: {delays:?}");
    assert_eq!(keys, "abc");
    assert!(key_elapsed.is_some_and(|elapsed| elapsed < Duration::from_millis(100)), "queued input waited for idle deadline");
    assert!(resize_callbacks > 0 && queued_resizes > 0);
    assert_eq!(paced_ticks, 60);
    // run_on_demand restores ownership on exit and propagates host failures.
    let error = backend.run_host_loop(|_, _| Err("expected probe error".into())).unwrap_err();
    assert_eq!(error.to_string(), "expected probe error");
    println!("native host probe passed: ticks={ticks}, paced_ticks={paced_ticks}, resize_callbacks={resize_callbacks}, queued_resizes={queued_resizes}, wake_delays_ms={:?}",
        delays.iter().map(|delay| delay.as_secs_f64() * 1000.0).collect::<Vec<_>>());
    Ok(())
}
