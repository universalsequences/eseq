use super::*;

const GRAPH_SOURCE: &str = "(load \"@/scripts/sequencers/graph-neural-variable-reset-demo.lisp\")";

fn rack_project_editor() -> (app::App, Editor) {
    let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
    let mut app = test_app_for_track_visual_state(Arc::clone(&state));
    app.groups = serde_json::from_value(serde_json::json!([{
        "id": 1, "name": "Restored kit", "members": [0], "bus_id": 0,
        "rack": {"pads": [], "sequencers": [{
            "sequencer_id": sequencer::lisp_host::graph_instance_id("neural-variable-reset-demo", Some(1)),
            "sequencer_name": "neural-variable-reset-demo", "source": GRAPH_SOURCE
        }]}
    }])).unwrap();
    app.state.set_rack_memberships(app.rack_memberships());
    app.publish_rack_owner_modules();

    let mut editor = full_grid_editor_for_scroll_tests();
    let (roots, errors) = sequencer::app_paths::app_paths().module_load_roots();
    assert!(errors.is_empty(), "{errors:?}");
    editor.runtime_mut().set_scoped_module_load_path(roots);
    sequencer::lisp_host::register_graph_authoring_natives(editor.runtime_mut(), Arc::clone(&state));
    editor.runtime_mut().register_native("def-sequencer", move |args, ctx| {
        let published = sequencer::lisp_host::published_sequencer_from_def_args_in_module(
            &args, ctx.current_module().as_deref(),
        )?;
        let id = published.id;
        state.publish_sequencer(published);
        Ok(Value::Number(id as f64))
    });
    sync_groups_bindings(editor.runtime_mut(), &app.groups);
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    (app, editor)
}

#[test]
fn empty_scratch_restores_rack_graph_tab_and_controls() {
    let (app, mut editor) = rack_project_editor();
    assert!(app.state.scratch_source().is_empty());
    // Reopen with empty scratch, then with unrelated project source, then
    // empty again. Each transition must recreate one usable rack tab.
    for scratch in ["", "(def restored-project-marker 1)", ""] {
        clear_project_script_tabs(&mut editor).unwrap();
        app.state.set_scratch_source(scratch);
        evaluate_project_scratch_on_ui_runtime(&mut editor, &app).expect("restore rack scripts");
        assert_eq!(app.state.published_sequencers().len(), 1);
        assert_eq!(app.state.published_sequencers()[0].graph.as_ref().unwrap().owner_rack, Some(1));
        let tabs = tile_tabs_for_buffer(&editor, "*sequencer*");
        assert_eq!(tabs.len(), 2, "step tab plus exactly one restored graph tab");
        assert_eq!(tabs[1].1, "*variable-reset*");
        assert!(editor.buffers.iter().any(|buffer| buffer.name == "*variable-reset*"),
            "restored tab must have a materialized editor buffer");
        editor.runtime_mut().eval_str("(eseq.seq-step-tabs/seq-select-main-step-tab-by-index 2)").unwrap();
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        assert_eq!(editor.active_buffer().name, "*variable-reset*");
        let layout = editor.widget_layout().expect("restored graph layout");
        let matrix = find_layout_node_by_stable_key_suffix(&layout, "graph-variable-reset-weight-matrix")
            .expect("restored graph weight editor");
        assert_finite_nonzero_rect(matrix, "graph weight editor");
    }
}

/// A rack-owned graph tab must stay renderable after the rack's membership
/// changes underneath it: members leaving and a track joining must never
/// leave the panel's `node-count` loop disagreeing with `bind-graph`'s
/// active-node range, which is what floods `[lisp-error][each]` and stalls the
/// tab.
#[test]
fn rack_member_churn_keeps_graph_tab_node_rows_consistent_legacy_rack() {
    rack_member_churn_keeps_graph_tab_node_rows_consistent(false);
}

#[test]
fn rack_member_churn_keeps_graph_tab_node_rows_consistent_clip_rack() {
    rack_member_churn_keeps_graph_tab_node_rows_consistent(true);
}

fn rack_member_churn_keeps_graph_tab_node_rows_consistent(clips: bool) {
    const NAME: &str = "neural-variable-reset-demo";
    let state = Arc::new(SequencerState::new(
        6,
        (0..6).map(|_| default_empty_effect_chain()).collect(),
    ));
    let mut app = test_app_for_track_visual_state(Arc::clone(&state));
    app.groups = serde_json::from_value(serde_json::json!([{
        "id": 1, "name": "Kit", "members": [0, 1, 2, 3], "bus_id": 0,
        "rack": {"pads": [
            {"pad_note": 36, "member": 0}, {"pad_note": 37, "member": 1},
            {"pad_note": 38, "member": 2}, {"pad_note": 39, "member": 3}
        ], "sequencers": [{
            "sequencer_id": sequencer::lisp_host::graph_instance_id(NAME, Some(1)),
            "sequencer_name": NAME, "source": GRAPH_SOURCE
        }]}
    }])).unwrap();
    app.state.set_rack_memberships(app.rack_memberships());
    app.publish_rack_owner_modules();

    let mut editor = full_grid_editor_for_scroll_tests();
    let (roots, errors) = sequencer::app_paths::app_paths().module_load_roots();
    assert!(errors.is_empty(), "{errors:?}");
    editor.runtime_mut().set_scoped_module_load_path(roots);
    sequencer::lisp_host::register_graph_authoring_natives(editor.runtime_mut(), Arc::clone(&state));
    let def_state = Arc::clone(&state);
    editor.runtime_mut().register_native("def-sequencer", move |args, ctx| {
        let published = sequencer::lisp_host::published_sequencer_from_def_args_in_module(
            &args, ctx.current_module().as_deref(),
        )?;
        let id = published.id;
        def_state.publish_sequencer(published);
        Ok(Value::Number(id as f64))
    });
    sync_groups_bindings(editor.runtime_mut(), &app.groups);
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    evaluate_project_scratch_on_ui_runtime(&mut editor, &app).expect("load rack script");
    if clips {
        app.convert_rack_to_clips_recorded(1).expect("convert to clips");
    }

    // Author the graph the way the panel does: 16 active nodes, every node
    // routed to a member.
    sequencer::lisp_host::with_graph_owner_rack(Some(1), || {
        editor.runtime_mut().take_status_message();
        editor
            .runtime_mut()
            .eval_str(&format!("(graph-config \"{NAME}\" :node-count 16)"))
            .expect("node-count 16");
        for node in 0..16 {
            editor
                .runtime_mut()
                .eval_str(&format!("(graph-node \"{NAME}\" {node} :route {})", node % 4))
                .expect("route");
        }
    });

    let consistent = |editor: &mut Editor, label: &str| {
        sequencer::lisp_host::with_graph_owner_rack(Some(1), || {
            editor.runtime_mut().take_status_message();
            let count = editor
                .runtime_mut()
                .eval_str(&format!("(graph-config-value \"{NAME}\" :node-count)"))
                .expect("node-count");
            let Some(Value::Number(count)) = count else {
                panic!("{label}: node-count not numeric: {count:?}");
            };
            for node in 0..(count as usize) {
                let value = editor
                    .runtime_mut()
                    .eval_str(&format!(
                        "(reactive-value (bind-graph \"{NAME}\" {node} :seed-route))"
                    ))
                    .expect("bind-graph");
                let status = editor.runtime_mut().take_status_message();
                assert!(
                    matches!(value, Some(Value::Number(_))),
                    "{label}: node {node} of {count}: bind-graph gave {value:?}, status {status:?}"
                );
            }
        });
    };
    consistent(&mut editor, "before churn");

    // Two members leave (their pads drop), then a loose track joins.
    app.remove_track_from_group_recorded(1).expect("member 1 leaves");
    app.remove_track_from_group_recorded(2).expect("member 2 leaves");
    sync_groups_bindings(editor.runtime_mut(), &app.groups);
    consistent(&mut editor, "after two members left");
    app.attach_track_to_group(4, 1, None).expect("track joins the rack");
    app.publish_rack_choke_runtime();
    sync_groups_bindings(editor.runtime_mut(), &app.groups);
    consistent(&mut editor, "after a track joined");

    // The tab itself renders every active row.
    editor.runtime_mut().eval_str("(eseq.seq-step-tabs/seq-select-main-step-tab-by-index 2)").unwrap();
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    assert_eq!(editor.active_buffer().name, "*variable-reset*");
    let layout = editor.widget_layout().expect("graph layout");
    assert!(
        find_layout_node_by_stable_key_suffix(&layout, "graph-variable-reset-seed-route-15").is_some(),
        "row 15 must render"
    );
}
