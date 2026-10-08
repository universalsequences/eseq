use super::*;

fn assert_change(editor: &mut eseqlisp::Editor, idx: usize, value: f64) {
    let commands = editor.drain_host_commands();
    let payload = commands
        .iter()
        .find_map(|command| match command {
            eseqlisp::host::HostCommand::Custom {
                name,
                payload: Value::Map(p),
            } if name == "set-effect-param" => Some(p),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing parameter command: {commands:?}"));
    assert_eq!(*payload["slot-idx"].borrow(), Value::Number(0.0));
    assert_eq!(*payload["param-idx"].borrow(), Value::Number(idx as f64));
    assert_eq!(*payload["value"].borrow(), Value::Number(value));
}

#[test]
fn chorus_filters_controls_and_modulation_have_live_bindings_and_visible_geometry() {
    let desc = sequencer::effects::EffectDescriptor::builtin_insert("Chorus").unwrap();
    let state = Arc::new(SequencerState::new(
        1,
        vec![sequencer::sequencer::default_empty_effect_chain()],
    ));
    state.pattern.effect_chains[0][0].apply_descriptor(&desc, 42);
    let mut editor = full_grid_editor_for_scroll_tests();
    let mut projection_app = test_app_for_track_visual_state(Arc::clone(&state));
    projection_app.graph.effect_descriptors = vec![vec![desc.clone()]];
    seed_app_panels(&mut editor, &projection_app, 0);
    editor
        .runtime_mut()
        .eval_str(r#"(set-layout (list :buf "*fx*" :hide-status true))"#)
        .unwrap();
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let id = editor.buffers.iter().find(|b| b.name == "*fx*").unwrap().id;
    editor.set_active_buffer(id);
    editor.set_layout_viewport(220, 30);
    for tab in 0..2 {
        editor
            .runtime_mut()
            .eval_str(&format!(
                "(eseq.effects.builtin.chorus/select-filter (first (eseq.effects.panel-data/current-effect-panels)) {tab})"
            ))
            .unwrap();
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let layout = editor.widget_layout().expect("Chorus layout");
        assert_finite_layout_tree(&layout);
        let panel = find_layout_node_by_debug_name(&layout, "audio-fx-panel-root-0-Chorus")
            .expect("Chorus panel");
        assert_finite_nonzero_rect(panel, "Chorus");
        assert_layout_inside(panel, &layout, "Chorus visible");
        let indexes = if tab == 0 {
            vec![1, 2, 3, 4, 5, 6, 8, 9]
        } else {
            vec![1, 2, 3, 4, 7, 8, 9]
        };
        for idx in indexes {
            let row = find_layout_node_by_debug_name(panel, &format!("chorus-param-{idx}"))
                .expect("control row");
            let knob = find_layout_node_by_widget_type(row, "knob-number").expect("knob");
            assert_finite_nonzero_rect(knob, &desc.params[idx].name);
            assert_layout_inside(knob, panel, &desc.params[idx].name);
            assert!(
                matches!(knob.props.get("value"), Some(Value::ReactiveRef { .. })),
                "live binding: {}",
                desc.params[idx].name
            );
        }
        let curve = find_layout_node_by_debug_name(
            panel,
            if tab == 0 {
                "chorus-input-curve"
            } else {
                "chorus-wet-curve"
            },
        )
        .expect("filter editor");
        assert_eq!(curve.widget_type, "response-curve-editor");
        assert_finite_nonzero_rect(curve, "filter editor");
        assert_layout_inside(curve, panel, "filter editor");
        editor.drain_host_commands();
        let event = editor
            .runtime_mut()
            .eval_str("(dict :type :change-band :id 0 :freq 4200)")
            .unwrap()
            .unwrap();
        editor
            .runtime_mut()
            .invoke(curve.props["on-action"].clone(), vec![event])
            .unwrap();
        assert_change(&mut editor, if tab == 0 { 6 } else { 7 }, 4200.0);
        if tab == 0 {
            let event = editor
                .runtime_mut()
                .eval_str("(dict :type :commit-band :id 1 :freq 150)")
                .unwrap()
                .unwrap();
            editor
                .runtime_mut()
                .invoke(curve.props["on-action"].clone(), vec![event])
                .unwrap();
            assert_change(&mut editor, 5, 150.0);
        }
    }
    editor
        .runtime_mut()
        .eval_str(
            r#"
        (let ((m eseq.effects.state/effect-mods))
          (do (set! m.chain "audio") (set! m.track 0) (set! m.slot 0)
              (set! m.rack-slot -1) (set! m.bus -1) (set! m.open true)))
    "#,
        )
        .unwrap();
    for slot in 1..=4 {
        editor
            .runtime_mut()
            .eval_str(&format!(
                "(let ((m eseq.effects.state/effect-mods)) (set! m.mod-slot {slot}))"
            ))
            .unwrap();
        for tab in 0..2 {
            editor
                .runtime_mut()
                .eval_str(&format!(
                    "(eseq.effects.builtin.chorus/select-filter (first (eseq.effects.panel-data/current-effect-panels)) {tab})"
                ))
                .unwrap();
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            let layout = editor.widget_layout().unwrap();
            for idx in if tab == 0 {
                vec![1, 2, 3, 4, 5, 6, 8, 9]
            } else {
                vec![7]
            } {
                let row = find_layout_node_by_debug_name(&layout, &format!("chorus-param-{idx}"))
                    .unwrap();
                let knob = find_layout_node_by_widget_type(row, "knob-number").unwrap();
                assert_finite_nonzero_rect(knob, "modulation control");
                editor.drain_host_commands();
                editor
                    .runtime_mut()
                    .invoke(knob.props["on-change"].clone(), vec![Value::Number(0.25)])
                    .unwrap();
                let target = desc
                    .instrument_modulation_targets
                    .iter()
                    .find(|t| t.base_param_idx == idx && t.modulator_slot == slot)
                    .unwrap();
                assert_change(&mut editor, target.depth_param_idx, 0.25);
            }
        }
    }
}
