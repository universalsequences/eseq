use super::*;
use crate::reactive_sync::apply_rack_macro_rename_host_command;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
fn rack_macro_typing_preserves_caret_and_only_rerenders_the_name() {
    let mut app = test_app_with_rack_panel_and_slot_fx();
    let mut editor = full_grid_editor_for_scroll_tests();
    editor.runtime_mut().eval_str(r#"
        (set-layout (list :buf "*fx*" :hide-status true))
    "#).unwrap();
    // The rack's macros are eseq.kinds rack-macros: the name field shows
    // rm.name, which the host pushes after each rename lands.
    seed_app_panels(&mut editor, &app, 0);
    let rack = {
        let rt = editor.runtime();
        rt.keyed_instance("eseq.kinds:device", &[kind_track(rt, 0), 0])
            .unwrap()
    };
    let rm = editor
        .runtime()
        .keyed_instance("eseq.kinds:rack-macro", &[rack, 0])
        .unwrap();
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
            // (set! rm.name …): the rack macro's setter.
            assert_eq!(name, "set-rack-macro");
            assert_eq!(*map["field"].borrow(), Value::String("name".to_string()));
            assert_eq!(*map["value"].borrow(), Value::String(expected.to_string()));
            let Value::Map(rename) = map_value([
                ("track", Value::Number(0.0)),
                ("id", Value::Number(0.0)),
                ("name", Value::String(expected.to_string())),
            ]) else {
                unreachable!()
            };
            let epoch = AtomicUsize::new(0);
            apply_rack_macro_rename_host_command(&mut app, &rename, &epoch);
            // The host kinds push the new name (rm.name) after the edit
            // lands, as the frame's sync does before the next key.
            set_field(
                editor.runtime_mut(),
                rm,
                "name",
                Value::String(expected.to_string()),
            );
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
        }
        let layout = editor.widget_layout().unwrap();
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
        let input = find_layout_node_by_debug_name(&layout, "rack-macro-name-0").unwrap();
        assert_eq!(input.props.get("value"), Some(&Value::String(expected.to_string())));
        assert!(editor.focused_widget_id().is_some());
        let after = editor.runtime().ui_work_counters();
        assert_eq!(after.full_buffer_reruns, before.full_buffer_reruns, "typing must not rerun a panel");
        assert_eq!(after.subtree_reruns - before.subtree_reruns, u64::from(edited), "only the name subtree needs rebuilding");
        app.state.with_project_scenes(|scenes| {
            let pattern = scenes.effective_pattern_id(0).unwrap();
            assert_eq!(scenes.track_pools[0].rack_macros(pattern).unwrap()[0].name, expected);
        });
    }
    timings.sort_by(f64::total_cmp);
    eprintln!("rack macro typing: median {:.3} ms, max {:.3} ms (key + host + reactive + layout, {} events)",
        timings[timings.len() / 2], timings.last().unwrap(), timings.len());
}
