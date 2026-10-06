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
        let instrument = match args.get(2) {
            Some(Value::String(instrument)) => instrument.clone(),
            _ => String::new(),
        };
        Ok(build_preset_tree_from_list(args.first(), "", &instrument, args.get(3)))
    });
    editor.set_layout_viewport(55, 38);
    seed_browser_tracks(&mut editor, &vec!["rack"; app.tracks.len()], track);
    sync_sidebar_browser(editor.runtime_mut(), &app, track);
    let slot_devices = push_presented_sidebar(&mut editor);
    // The rack slot the delete target selects, as the host pushes
    // `device.delete-target`.
    let target_slot = |editor: &mut Editor, slot: Option<usize>| {
        for (index, &device) in slot_devices.iter().enumerate() {
            set_field(editor.runtime_mut(), device, "delete-target", Value::Bool(slot == Some(index)));
        }
        editor.runtime_mut().run_reactive_cycle();
    };
    let rack_presets = "(let ((b eseq.kinds/browser)) b.presets)";
    set_browser_view_field(&mut editor, "browser-view", "tab", r#""presets""#);
    let rt = editor.runtime_mut();
    // An ordinary edit cursor alone must keep the rack-level preset bank.
    assert_eq!(rt.eval_str("(eseq.browser/selected-rack-preset-context)").unwrap(), Some(Value::Nil));
    assert_eq!(rt.eval_str("(eseq.browser/browser-preset-items)").unwrap(),
        rt.eval_str(rack_presets).unwrap());
    rt.eval_str(&format!("(seq-set-delete-target :rack-slot (dict :track {track} :slot 0))")).unwrap();
    target_slot(&mut editor, Some(0));
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
    // Rows carry the slot's instrument so a dragged preset can load it.
    let Some(Value::List(rows)) = tree.props.get("items") else {
        panic!("preset tree items should be a list");
    };
    // Shipped presets come first under Factory (a Library section follows
    // only when the user has saved presets of their own for this instrument).
    let is_header = |row: &Rc<RefCell<Value>>| matches!(&*row.borrow(), Value::Map(row)
        if row.get("kind").is_some_and(|kind| *kind.borrow() == Value::String("header".to_string())));
    assert!(matches!(&*rows[0].borrow(), Value::Map(row)
        if row.get("label").is_some_and(|label| *label.borrow() == Value::String("Factory".to_string()))));
    assert!(rows.iter().filter(|row| !is_header(row)).all(|row| matches!(&*row.borrow(), Value::Map(row)
        if row.get("instrument").is_some_and(|value| *value.borrow() == Value::String(name.to_string()))
            && row.get("preset") == row.get("label"))));
    assert_eq!(tree.props.get("drag-type"), Some(&Value::String("instrument-preset".to_string())));
    // A click only selects (so the row can be dragged); double-click/Enter loads.
    assert!(tree.props.get("on-select").is_none());
    let callback = tree.props.get("on-activate").unwrap().clone();
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
    // A dropped preset of the layer's own instrument switches in place; any
    // other instrument, or the rack track itself, takes the replace path.
    assert!(rack_slot_runs_instrument(&app, track, 0, name));
    assert!(!rack_slot_runs_instrument(&app, track, 0, "different-instrument"));
    assert!(!rack_slot_runs_instrument(&app, track, 9, name));
    assert!(!track_runs_instrument(&app, track, name));

    sync_sidebar_browser(editor.runtime_mut(), &app, track);
    push_presented_sidebar(&mut editor);
    let rt = editor.runtime_mut();
    assert_eq!(rt.eval_str("(eseq.browser/browser-loaded-preset)").unwrap(), Some(Value::String(preset.name.clone())));
    rt.eval_str(&format!("(seq-set-delete-target :rack-slot (dict :track {track} :slot 1))")).unwrap();
    target_slot(&mut editor, Some(1));
    let rt = editor.runtime_mut();
    assert_eq!(
        rt.eval_str("(let ((s (eseq.browser/selected-rack-preset-context))) s.index)").unwrap(),
        Some(Value::Number(1.0))
    );
    assert_eq!(rt.eval_str("(eseq.browser/browser-loaded-preset)").unwrap(),
        Some(Value::String(sibling_before.sound_state.loaded_preset.clone().unwrap_or_default())));
    rt.eval_str("(seq-clear-delete-target)").unwrap();
    target_slot(&mut editor, None);
    let rt = editor.runtime_mut();
    assert_eq!(rt.eval_str("(eseq.browser/selected-rack-preset-context)").unwrap(), Some(Value::Nil));
    assert_eq!(rt.eval_str("(eseq.browser/browser-preset-items)").unwrap(),
        rt.eval_str(rack_presets).unwrap());
}

#[test]
fn dropped_preset_of_the_running_instrument_loads_in_place() {
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
    assert!(track_runs_instrument(&app, track, name));
    assert!(!track_runs_instrument(&app, track, "factory:Synths/Other"));
    assert!(!track_runs_instrument(&app, track + 1, name));

    // The browser stamps the track's instrument on its preset rows.
    let mut editor = browser_editor_on_instrument_tab();
    seed_browser_tracks(&mut editor, &vec!["custom"; app.tracks.len()], track);
    sync_sidebar_browser(editor.runtime_mut(), &app, track);
    push_presented_sidebar(&mut editor);
    let rt = editor.runtime_mut();
    assert_eq!(rt.eval_str("(eseq.browser/browser-preset-instrument)").unwrap(),
        Some(Value::String(name.to_string())));

    let presets = sequencer::lisp_host::load_instrument_presets_shared(name).unwrap();
    let preset = presets.first().expect("factory instrument preset");
    load_instrument_preset_into_track(&mut app, track, &preset.name).unwrap();
    let loaded = app.state.pattern.track_sound_state.lock().unwrap()
        .get(track).and_then(|meta| meta.loaded_preset.clone());
    assert_eq!(loaded.as_deref(), Some(preset.name.as_str()));
}
