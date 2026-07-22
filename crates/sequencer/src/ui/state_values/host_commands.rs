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
    state: &Arc<SequencerState>,
    current_track: &Arc<AtomicUsize>,
    selected_steps: &Arc<Mutex<HashSet<usize>>>,
    fx_epoch: &Arc<AtomicUsize>,
    ui_epoch: &Arc<AtomicUsize>,
) {
    if let Some(status) = app.poll_pending_compile() {
        let ct = current_track.load(Ordering::Relaxed);
        let rt = editor.runtime_mut();
        rt.set_reactive("SEQ", "compiling", Value::Bool(false));
        rt.set_reactive(
            "SEQ",
            "effects",
            if app.tracks.is_empty() {
                Value::List(vec![])
            } else {
                build_effects_value(&state, ct, &app.graph.effect_descriptors, &selected_steps)
            },
        );
        rt.set_reactive(
            "SEQ",
            "midi-effects",
            if app.tracks.is_empty() {
                Value::List(vec![])
            } else {
                build_midi_effects_value(&state, ct, &selected_steps)
            },
        );
        rt.set_reactive(
            "SEQ",
            "instrument-panel",
            if app.tracks.is_empty() {
                Value::List(vec![])
            } else {
                build_instrument_panel_value(&app, ct, &selected_steps)
            },
        );
        sync_fx_param_binding_fields(rt, app, state, ct, selected_steps);
        rt.set_reactive(
            "SEQ",
            "step-has-plocks",
            if app.tracks.is_empty() {
                Value::List(vec![])
            } else {
                build_step_has_plocks(state, ct, &app.graph.effect_descriptors)
            },
        );
        rt.run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        editor.refresh_visible_layouts_for_buffer_named("*fx*");
        fx_epoch.fetch_add(1, Ordering::Relaxed);
        ui_epoch.fetch_add(1, Ordering::Relaxed);
        editor.handle_host_event(HostEvent::Status(status));
    }
}
