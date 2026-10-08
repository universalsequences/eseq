use super::*;

pub(crate) fn auto_follow_enabled(override_until: &Arc<Mutex<Option<Instant>>>) -> bool {
    let guard = override_until.lock().unwrap();
    match *guard {
        Some(until) => Instant::now() >= until,
        None => true,
    }
}

pub(crate) fn poll_pending_compile_status(
    app: &mut app::App,
    editor: &mut Editor,
    fx_epoch: &Arc<AtomicUsize>,
    ui_epoch: &Arc<AtomicUsize>,
) {
    if let Some(status) = app.poll_pending_compile() {
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        editor.refresh_visible_layouts_for_buffer_named("*fx*");
        fx_epoch.fetch_add(1, Ordering::Relaxed);
        ui_epoch.fetch_add(1, Ordering::Relaxed);
        editor.handle_host_event(HostEvent::Status(status));
    }
}
