use super::*;

#[test]
fn rack_slot_presets_follow_explicit_selection_and_route_to_the_slot() {
    let eng = engine::init_headless_engine(44_100, 2).unwrap();
    struct GraphGuard(sequencer::audiograph::LiveGraphPtr);
    impl Drop for GraphGuard {
        fn drop(&mut self) {
            unsafe {
                sequencer::audiograph::engine_stop_workers();
                sequencer::audiograph::destroy_live_graph(self.0.0);
            }
        }
    }
    let _guard = GraphGuard(eng.lg_ptr);
    let mut app = app::App::new(eng.state, eng.lg_ptr, eng.sample_rate,
        eng.buses, eng.master_recorder, eng.keyboard_tx);
    let name = "factory:Synths/Digi Drift";
    let track = app.add_saved_instrument_track_sync(name).unwrap();
    app.graph_controller().group_track_to_instrument_rack(track).unwrap();
    app.add_saved_instrument_slot_to_rack_sync(track, name).unwrap();
    app.add_builtin_rack_slot_effect_sync(track, 0, "filter").unwrap();
    let presets = sequencer::lisp_host::load_instrument_presets_shared(name).unwrap();
    let preset = presets.iter().find(|preset| !preset.key_locks.is_empty())
        .expect("factory instrument preset with key locks");
    let sibling_before = app.rack_slot_effect_snapshot(track, 1).unwrap().authoring_values();
    let before = app.rack_slot_effect_snapshot(track, 0).unwrap().authoring_values();

    let mut editor = browser_editor_on_instrument_tab();
    register_test_delete_target_natives(&mut editor, app.tracks.len());
    editor.runtime_mut().register_native("seq-preset-tree", |args, _ctx| {
        Ok(build_preset_tree_from_list(args.first(), ""))
    });
    editor.set_layout_viewport(55, 38);
    let rt = editor.runtime_mut();
    rt.set_reactive("SEQ", "num-tracks", Value::Number(app.tracks.len() as f64));
    sync_sidebar_browser(rt, &app, track);
    rt.eval_str("(set! eseq.vanilla/sbrowser-tab \"presets\")").unwrap();
    // An ordinary edit cursor alone must keep the rack-level preset bank.
    assert_eq!(rt.eval_str("(eseq.browser/selected-rack-preset-context)").unwrap(), Some(Value::Nil));
    assert_eq!(rt.eval_str("(eseq.browser/browser-preset-items)").unwrap(),
        rt.reactive_field_value("SEQ", "sidebar-presets").cloned());
    rt.eval_str(&format!("(seq-set-delete-target :rack-slot (dict :track {track} :slot 0))")).unwrap();
    rt.set_reactive("SEQ", "delete-target-version", Value::Number(1.0));
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let layout = editor.widget_layout().expect("preset sidebar layout");
    let tree = find_layout_node_by_stable_key_suffix(&layout, "/presets-tab-tree")
        .expect("selected instrument preset tree");
    assert_finite_nonzero_rect(tree, "preset tree");
    let scroll = find_layout_node_by_stable_key_suffix(&layout, "/presets-tab-scroll").unwrap();
    assert_finite_nonzero_rect(scroll, "preset scroll viewport");
    assert_layout_inside(scroll, &layout, "preset scroll viewport");
    assert!(tree.rect.row < scroll.rect.row + scroll.rect.height);
    assert!(value_contains_string(tree.props.get("items").unwrap(), &preset.name));
    let callback = tree.props.get("on-select").unwrap().clone();
    editor.drain_host_commands();
    editor.runtime_mut().invoke(callback, vec![map_value([
        ("label", Value::String(preset.name.clone())),
    ])]).unwrap();
    let commands = editor.drain_host_commands();
    let payload = commands.iter().find_map(|command| match command {
        eseqlisp::host::HostCommand::Custom { name, payload } if name == "load-instrument-preset" => Some(payload),
        _ => None,
    }).expect("preset load command");
    assert_eq!(extract_usize_from_payload(payload, "track"), Some(track));
    assert_eq!(extract_usize_from_payload(payload, "rack-slot"), Some(0));
    let instrument = extract_string_from_payload(payload, "instrument").unwrap();
    load_instrument_preset_into_rack_slot(&mut app, track, 0, &instrument, &preset.name).unwrap();
    let after = app.rack_slot_effect_snapshot(track, 0).unwrap().authoring_values();
    assert_eq!(after.sound_state.loaded_preset.as_deref(), Some(preset.name.as_str()));
    assert_eq!(after.base_note_offset_bits, preset.base_note_offset.to_bits());
    assert!(app.rack_slot_effect_snapshot(track, 1).unwrap().authoring_values().bit_exact_eq(&sibling_before));
    assert!(after.effect_slots.iter().zip(&before.effect_slots).all(|(a, b)| a.bit_exact_eq(b)));
    assert_eq!(after.gain_bits, before.gain_bits);
    assert_eq!(after.pan_bits, before.pan_bits);
    assert_eq!(after.instrument_slot.plocks, before.instrument_slot.plocks);
    let desc = app.rack_slot_instrument_descriptor(&app.rack_slot_effect_snapshot(track, 0).unwrap()).unwrap();
    for (idx, param) in desc.params.iter().enumerate() {
        assert_eq!(after.instrument_slot.defaults[idx],
            param.clamp(preset.params.get(&param.name).copied().unwrap_or(param.default)));
    }
    let loaded_slot = app.rack_slot_effect_snapshot(track, 0).unwrap();
    let live_slot = sequencer::effects::EffectSlotState::new(&desc, 0);
    loaded_slot.instrument_slot.restore(&live_slot);
    for (&note, locks) in &preset.key_locks {
        for (name, value) in locks {
            let idx = desc.params.iter().position(|param| &param.name == name).unwrap();
            assert_eq!(after.instrument_slot.key_locks[&note][idx], Some(desc.params[idx].clamp(*value)));
            assert_eq!(loaded_slot.instrument_slot.key_lock_param_ids[&note][idx],
                live_slot.param_node_id(idx));
            assert!(live_slot.param_node_id(idx).is_some());
        }
    }
    assert!(matches!(sequencer::app::edit::undo(&mut app), sequencer::app::history::HistoryReplay::Applied(_)));
    assert!(app.rack_slot_effect_snapshot(track, 0).unwrap().authoring_values().bit_exact_eq(&before));
    assert!(matches!(sequencer::app::edit::redo(&mut app), sequencer::app::history::HistoryReplay::Applied(_)));
    assert!(app.rack_slot_effect_snapshot(track, 0).unwrap().authoring_values().bit_exact_eq(&after));
    assert!(load_instrument_preset_into_rack_slot(&mut app, track, 0, "different-instrument", &preset.name).is_err());

    let rt = editor.runtime_mut();
    sync_sidebar_browser(rt, &app, track);
    assert_eq!(rt.eval_str("(eseq.browser/browser-loaded-preset)").unwrap(), Some(Value::String(preset.name.clone())));
    rt.eval_str(&format!("(seq-set-delete-target :rack-slot (dict :track {track} :slot 1))")).unwrap();
    rt.set_reactive("SEQ", "delete-target-version", Value::Number(2.0));
    rt.run_reactive_cycle();
    assert_eq!(rt.eval_str("(get (eseq.browser/selected-rack-preset-context) :slot)").unwrap(), Some(Value::Number(1.0)));
    assert_eq!(rt.eval_str("(eseq.browser/browser-loaded-preset)").unwrap(),
        Some(Value::String(sibling_before.sound_state.loaded_preset.clone().unwrap_or_default())));
    rt.eval_str("(seq-clear-delete-target)").unwrap();
    rt.set_reactive("SEQ", "delete-target-version", Value::Number(3.0));
    rt.run_reactive_cycle();
    assert_eq!(rt.eval_str("(eseq.browser/selected-rack-preset-context)").unwrap(), Some(Value::Nil));
    assert_eq!(rt.eval_str("(eseq.browser/browser-preset-items)").unwrap(),
        rt.reactive_field_value("SEQ", "sidebar-presets").cloned());
}
