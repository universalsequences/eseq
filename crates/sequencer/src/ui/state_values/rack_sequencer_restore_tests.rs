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
