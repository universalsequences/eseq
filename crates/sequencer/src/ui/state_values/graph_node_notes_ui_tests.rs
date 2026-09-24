use super::*;
use sequencer::graph::{GraphSoundingNote, GraphVisualizationSnapshot};

/// A `neural` instance's panel (alez/neural), rendered in its host-created
/// buffer, with the instance's graph published into `state`.
fn var_reset_panel_editor() -> (Arc<SequencerState>, Editor, u64) {
    let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
    let mut editor = full_grid_editor_for_scroll_tests();
    let (roots, errors) = sequencer::app_paths::app_paths().module_load_roots();
    assert!(errors.is_empty(), "{errors:?}");
    editor.runtime_mut().set_scoped_module_load_path(roots);
    sequencer::lisp_host::register_graph_authoring_natives(editor.runtime_mut(), Arc::clone(&state));
    editor.runtime_mut().set_reactive("SEQ", "graph-visualizations", test_list(vec![]));
    editor.runtime_mut().set_reactive("SEQ", "track-active-notes", test_list(vec![]));
    let buffer = super::graph_visualization_ui_tests::neural_instance_view(&state, &mut editor, 1, "neural 1");
    editor.runtime_mut().eval_str(&format!("(set-layout (list :buf \"{buffer}\" :hide-status true))")).unwrap();
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let id = editor.buffers.iter().find(|item| item.name == buffer).expect(&buffer).id;
    editor.set_active_buffer(id);
    editor.set_layout_viewport(240, 100);
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let graph_id = state.published_sequencers()[0].id;
    (state, editor, graph_id)
}

#[test]
fn neural_package_sounding_notes_repaint_only_their_row_widget() {
    let (state, mut editor, graph_id) = var_reset_panel_editor();
    let note = |note: f32, start_sample: u64, end_sample: u64| GraphSoundingNote {
        note, velocity: 0.25 + note.abs() / 100.0, start_sample, end_sample,
    };
    let mut node_sounding = vec![Vec::new(); 8];
    // Two overlapping gates on node 1 plus one that already closed and one
    // that has not opened yet at the audio clock.
    node_sounding[1] = vec![note(9.0, 0, 500), note(-3.0, 600, 2_000), note(5.0, 900, 1_200), note(7.0, 1_500, 1_900)];
    state.set_graph_visualizations(vec![GraphVisualizationSnapshot {
        id: graph_id, name: "test".to_string(), active: true, num_nodes: 8, node_sounding,
        ..Default::default()
    }]);
    state.set_audio_rendered_sample(1_000);
    // Baseline while stopped: the field's first publish touches every row.
    let mut previous = HashMap::new();
    sync_graph_node_notes_fields(editor.runtime_mut(), &state, &mut previous);
    let _ = editor.runtime_mut().take_dirty_widget_ids();
    state.transport.playing.store(true, std::sync::atomic::Ordering::Relaxed);

    let before = editor.runtime().ui_work_counters();
    let effects_dirty = sync_graph_node_notes_fields(editor.runtime_mut(), &state, &mut previous);
    assert!(!effects_dirty, "sounding notes are read through element bindings only");
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let after = editor.runtime().ui_work_counters();
    assert_eq!(after.full_buffer_reruns, before.full_buffer_reruns);
    assert_eq!(after.subtree_reruns, before.subtree_reruns);

    let layout = editor.widget_layout().unwrap();
    let row = find_layout_node_by_stable_key_suffix(&layout, "graph-variable-reset-sounding-1")
        .expect("node 1 sounding readout");
    assert_finite_nonzero_rect(row, "sounding-1");
    assert_eq!(row.widget_type, "number-list");
    assert_eq!(editor.runtime_mut().take_dirty_widget_ids(), vec![row.widget_id],
        "only node 1's readout changed");
    let read = |value: &Value| match value {
        Value::ReactiveRef { slot, .. } => eseqlisp::reactive::read_float_slot(slot),
        other => panic!("expected a binding, got {other:?}"),
    };
    assert_eq!(read(&row.props["count"]), 2.0);
    let Value::List(values) = &row.props["values"] else { panic!("values list") };
    assert_eq!((read(&values[0].borrow()), read(&values[1].borrow())), (-3.0, 5.0));
    let Value::List(levels) = &row.props["levels"] else { panic!("levels list") };
    let level = |idx: usize| read(&levels[idx].borrow());
    assert!((level(0) - 0.28).abs() < 1e-6 && (level(1) - 0.30).abs() < 1e-6,
        "each chip's level is its note's velocity");

    // An unchanged clock publishes nothing; stopping clears the row.
    assert!(!sync_graph_node_notes_fields(editor.runtime_mut(), &state, &mut previous));
    assert!(editor.runtime_mut().take_dirty_widget_ids().is_empty());
    state.transport.playing.store(false, std::sync::atomic::Ordering::Relaxed);
    sync_graph_node_notes_fields(editor.runtime_mut(), &state, &mut previous);
    assert_eq!(editor.runtime_mut().take_dirty_widget_ids(), vec![row.widget_id]);
    assert_eq!(read(&row.props["count"]), 0.0);
}
