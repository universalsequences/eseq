use super::*;
use crate::reactive_sync::apply_rack_macro_rename_host_command;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
fn rack_macro_typing_preserves_caret_and_only_rerenders_the_name() {
    let mut app = test_app_with_rack_panel_and_slot_fx();
    let selected = Arc::new(Mutex::new(HashSet::new()));
    let mut editor = full_grid_editor_for_scroll_tests();
    let panel = build_instrument_panel_value(&app, 0, &selected);
    editor.runtime_mut().set_reactive("SEQ", "instrument-panel", panel.clone());
    sync_all_rack_macro_name_fields(editor.runtime_mut(), &app);
    sync_rack_macro_value_fields(editor.runtime_mut(), &app, 0, None);
    editor.runtime_mut().eval_str(r#"
        (set-layout (list :buf "*fx*" :hide-status true))
    "#).unwrap();
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let buffer = editor.buffers.iter().find(|b| b.name == "*fx*").unwrap().id;
    editor.set_active_buffer(buffer);
    editor.set_layout_viewport(160, 24);
    for name in ["rack-slot-list-view-toggle", "rack-chain-view-toggle", "rack-macro-view-toggle"] {
        let layout = editor.widget_layout().unwrap();
        let toggle = find_layout_node_by_debug_name(&layout, name).unwrap();
        editor.runtime_mut().invoke(toggle.props["on-click"].clone(),
            vec![Value::Number(0.0), Value::Number(0.0), Value::Nil]).unwrap();
        editor.refresh_runtime_side_effects();
    }
    let layout = editor.widget_layout().unwrap();
    let input = find_layout_node_by_debug_name(&layout, "rack-macro-name-0").unwrap();
    assert_finite_nonzero_rect(input, "rack macro name");
    assert_layout_inside(input, find_layout_node_by_debug_name(&layout, "rack-macro-bank").unwrap(), "macro name");
    let key = input.stable_key.clone().unwrap();
    assert!(editor.focus_widget_by_stable_key(&key, Some("text-input")));
    editor.drain_host_commands();
    editor.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));

    let mut timings = Vec::new();
    for (code, expected) in [
        (KeyCode::Backspace, ""),
        (KeyCode::Char('T'), "T"),
        (KeyCode::Char('o'), "To"),
        (KeyCode::Char('n'), "Ton"),
        (KeyCode::Char('e'), "Tone"),
        (KeyCode::Char(' '), "Tone "),
        (KeyCode::Char('Q'), "Tone Q"),
        (KeyCode::Left, "Tone Q"),
        (KeyCode::Char('é'), "Tone éQ"),
        (KeyCode::Backspace, "Tone Q"),
        (KeyCode::Home, "Tone Q"),
        (KeyCode::Delete, "one Q"),
        (KeyCode::Char(':'), ":one Q"),
    ] {
        let before = editor.runtime().ui_work_counters();
        let started = Instant::now();
        editor.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        let commands = editor.drain_host_commands();
        let edited = !matches!(code, KeyCode::Left | KeyCode::Home);
        assert_eq!(commands.len(), usize::from(edited), "key {code:?}");
        for command in commands {
            let eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(map) } = command else {
                panic!("expected rack rename");
            };
            assert_eq!(name, "rename-rack-macro");
            assert_eq!(*map["name"].borrow(), Value::String(expected.to_string()));
            apply_rack_macro_rename_host_command(&mut editor, &mut app, &map);
        }
        let layout = editor.widget_layout().unwrap();
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
        let input = find_layout_node_by_debug_name(&layout, "rack-macro-name-0").unwrap();
        assert_eq!(input.props.get("value"), Some(&Value::String(expected.to_string())));
        assert!(editor.focused_widget_id().is_some());
        let after = editor.runtime().ui_work_counters();
        assert_eq!(after.full_buffer_reruns, before.full_buffer_reruns, "typing must not rerun a panel");
        assert_eq!(after.subtree_reruns - before.subtree_reruns, u64::from(edited), "only the name subtree needs rebuilding");
        assert_eq!(editor.runtime_mut().eval_str("SEQ.instrument-panel").unwrap(), Some(panel.clone()));
        app.state.with_project_scenes(|scenes| {
            let pattern = scenes.effective_pattern_id(0).unwrap();
            assert_eq!(scenes.track_pools[0].rack_macros(pattern).unwrap()[0].name, expected);
        });
    }
    timings.sort_by(f64::total_cmp);
    eprintln!("rack macro typing: median {:.3} ms, max {:.3} ms (key + host + reactive + layout, {} events)",
        timings[timings.len() / 2], timings.last().unwrap(), timings.len());
}

#[test]
fn rack_macro_cached_labels_follow_names_without_rebuilding_metadata() {
    use crate::piano_roll::{build_piano_roll_automation_params_value, build_track_automation_value,
        build_track_lock_targets_value, compact_param_label};
    let mut app = test_app_with_rack_panel();
    let id = sequencer::sequencer::RackMacroId::from_index(0).unwrap();
    assert!(app.state.set_rack_macro_plocks_in_current_pattern(0, id, &[0], 0.5));
    let selected = Arc::new(Mutex::new(HashSet::from([0])));
    let mut editor = full_grid_editor_for_scroll_tests();
    let (roots, errors) = sequencer::app_paths::app_paths().module_load_roots();
    assert!(errors.is_empty());
    editor.runtime_mut().set_scoped_module_load_path(roots);
    let overlays = editor.snapshot_file_backed_sources();
    let report = editor.runtime_mut().eval_source_transactional(None, "(import alez.tracker.ui)", overlays);
    assert!(report.success, "{:?}", report.diagnostics);
    editor.process_lisp_reload_report(report);
    sync_all_rack_macro_name_fields(editor.runtime_mut(), &app);
    editor.runtime_mut().register_reactive("NAMES", vec![
        ("panel", build_instrument_panel_value(&app, 0, &selected)),
        ("plocks", build_track_plocks_value(&app, &app.state, 0, &selected)),
        ("params", build_piano_roll_automation_params_value(&app, &app.state, 0)),
        ("columns", build_track_automation_value(&app, &app.state)),
        ("targets", build_track_lock_targets_value(&app, &app.state)),
    ], false);
    editor.runtime_mut().eval_str(r#"
        (def cached-macro (nth (get (nth NAMES.panel 0) :macros) 0))
        (def cached-plock (nth (filter |p| (= (get p :target) "rack-macro") NAMES.plocks) 0))
        (def cached-param (nth (filter |p| (= (get p :key) "rack-macro:0") NAMES.params) 0))
        (def cached-column (nth (filter |p| (= (get p :key) "rack-macro:0") (nth NAMES.columns 0)) 0))
        (def cached-target (nth (get (nth (filter |g| (= (get g :group) "Macros") (nth NAMES.targets 0)) 0) :items) 0))
    "#).unwrap();
    for name in ["", "Tone ", "Écho Depth"] {
        let Value::Map(map) = map_value([
            ("track", Value::Number(0.0)), ("id", Value::Number(0.0)),
            ("name", Value::String(name.to_string())),
        ]) else { unreachable!() };
        apply_rack_macro_rename_host_command(&mut editor, &mut app, &map);
        for expression in [
            "(eseq.macro-state/macro-name cached-macro)",
            "(eseq.effects.track-panels/plock-row-title cached-plock)",
            "(eseq.piano-roll/automation-name cached-param)",
            "(alez.tracker.ui/column-label cached-column)",
            "(alez.tracker.ui/column-label cached-target)",
        ] {
            assert_eq!(editor.runtime_mut().eval_str(expression).unwrap(), Some(Value::String(name.to_string())), "{expression}");
        }
        assert_eq!(editor.runtime_mut().eval_str("(alez.tracker.ui/column-title cached-column)").unwrap(),
            Some(Value::String(compact_param_label(name))));
    }
}
