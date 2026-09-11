use super::*;

#[test]
fn polyphony_header_follows_voice_mode_and_toggles_the_selected_owner() {
    let mut editor = full_grid_editor_for_scroll_tests();
    let writes = Rc::new(RefCell::new(Vec::new()));
    let captured = writes.clone();
    editor.runtime_mut().register_native("seq-set-track-param", move |args, _ctx| {
        captured.borrow_mut().push(args.to_vec());
        Ok(Value::Nil)
    });
    editor.runtime_mut().eval_str(r#"
        (effect-buffer "*polyphony-test*"
          (eseq.effects.instrument-panel/instrument-polyphony-control))
    "#).unwrap();
    editor.refresh_runtime_side_effects();
    let buffer = editor.buffers.iter().find(|b| b.name == "*polyphony-test*").unwrap().id;
    editor.set_active_buffer(buffer);
    editor.set_layout_viewport(80, 20);
    for rack in [false, true] {
        for poly in [false, true, false] {
            editor.runtime_mut().set_reactive("SEQ", "tp-is-rack", Value::Bool(rack));
            editor.runtime_mut().set_reactive("SEQ", "tp-poly", Value::Bool(poly));
            editor.runtime_mut().set_reactive("SEQ", "current-track", Value::Number(2.0));
            editor.runtime_mut().set_reactive("SEQ", "tp-rack-slot-idx", Value::Number(3.0));
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            let layout = editor.widget_layout().unwrap();
            let button = find_layout_node_by_debug_name(&layout, "instrument-polyphony").unwrap();
            assert_finite_nonzero_rect(button, "voice mode toggle");
            assert!(find_layout_node_by_text(button, if poly { "poly" } else { "mono" }).is_some(),
                "the caption communicates the current voice mode");
            editor.drain_host_commands();
            writes.borrow_mut().clear();
            editor.runtime_mut().invoke(button.props["on-click"].clone(),
                vec![Value::Number(0.0), Value::Number(0.0), Value::Nil]).unwrap();
            let commands = editor.drain_host_commands();
            if rack {
                assert!(writes.borrow().is_empty(), "rack toggle must not edit the parent track");
                assert!(commands.iter().any(|cmd| matches!(cmd,
                    eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(payload) }
                        if name == "set-rack-slot-max-polyphony"
                            && *payload["track"].borrow() == Value::Number(2.0)
                            && *payload["slot"].borrow() == Value::Number(3.0)
                            && *payload["value"].borrow() == Value::Number(if poly { 1.0 } else { 4.0 }))));
            } else {
                assert_eq!(*writes.borrow(), vec![vec![Value::Keyword("poly".into()),
                    Value::Number(if poly { 0.0 } else { 1.0 })]]);
                assert!(commands.iter().all(|cmd| !matches!(cmd,
                    eseqlisp::host::HostCommand::Custom { name, .. } if name == "set-rack-slot-max-polyphony")));
            }
        }
    }
}

#[test]
fn base_note_header_stays_editable_in_mods_and_routes_rack_targets() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor.set_layout_viewport(100, 20);
    editor.runtime_mut().set_reactive("SEQ", "header-base-note", Value::Number(7.0));
    for rack in [false, true] {
        for mods in [false, true] {
            for locked in [false, true] {
                editor.runtime_mut().register_native("seq-has-selection?", move |_args, _ctx| Ok(Value::Bool(locked)));
                let target = if rack { ":rack-track 2 :rack-slot 3" } else { "" };
                let buffer_name = format!("*base-note-{rack}-{mods}-{locked}*");
                editor.runtime_mut().eval_str(&format!(r#"
                    (set! eseq.effects.state/instrument-mods-open {mods})
                    (effect-buffer "{buffer_name}"
                      (eseq.effects.instrument-panel/instrument-base-note-control
                        (dict :track 2 :rack-slot {slot} :synth
                          (list (dict :name "base_note" :control "base-note"
                            :idx 0 :value 7 :value-field "header-base-note"
                            :min -48 :max 48 :modulatable false {target})))))
                "#, slot = if rack { "3" } else { "nil" })).unwrap();
                editor.refresh_runtime_side_effects();
                let buffer = editor.buffers.iter().find(|b| b.name == buffer_name).unwrap().id;
                editor.set_active_buffer(buffer);
                let layout = editor.widget_layout().unwrap();
                let control = find_layout_node_by_debug_name(&layout, "instrument-base-note").unwrap();
                assert_finite_nonzero_rect(control, "base note header");
                assert!(matches!(control.props.get("value"), Some(Value::ReactiveRef { field, .. }) if field == "header-base-note"));
                assert_eq!(control.props.get("min"), Some(&Value::Number(-48.0)));
                assert_eq!(control.props.get("max"), Some(&Value::Number(48.0)));
                editor.drain_host_commands();
                editor.runtime_mut().invoke(control.props["on-change"].clone(), vec![Value::Number(12.0)]).unwrap();
                let command = if rack {
                    if locked { "set-rack-slot-param-plock" } else { "set-rack-slot-base-note" }
                } else { "set-instrument-base-note" };
                let commands = editor.drain_host_commands();
                assert!(commands.iter().any(|cmd| matches!(cmd,
                    eseqlisp::host::HostCommand::Custom { name, payload: Value::Map(payload) }
                        if name == command && *payload["value"].borrow() == Value::Number(12.0)
                            && (!rack || (*payload["track"].borrow() == Value::Number(2.0)
                                && *payload["slot"].borrow() == Value::Number(3.0))))),
                    "wrong base note target (rack={rack}, mods={mods}, locked={locked}): {commands:?}");
            }
        }
    }
}
