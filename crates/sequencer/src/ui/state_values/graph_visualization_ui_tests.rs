use super::*;
use sequencer::graph::{GraphVisualizationEdge, GraphVisualizationEvent, GraphVisualizationSnapshot};

fn graph_panel_editor(package: bool) -> (Arc<SequencerState>, Editor, u64) {
    let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
    let mut editor = full_grid_editor_for_scroll_tests();
    let (roots, errors) = sequencer::app_paths::app_paths().module_load_roots();
    assert!(errors.is_empty(), "{errors:?}");
    editor.runtime_mut().set_scoped_module_load_path(roots);
    sequencer::lisp_host::register_graph_authoring_natives(editor.runtime_mut(), Arc::clone(&state));
    let published_state = Arc::clone(&state);
    editor.runtime_mut().register_native("def-sequencer", move |args, ctx| {
        let published = sequencer::lisp_host::published_sequencer_from_def_args_in_module(
            &args, ctx.current_module().as_deref(),
        )?;
        let id = published.id;
        published_state.publish_sequencer(published);
        Ok(Value::Number(id as f64))
    });
    editor.runtime_mut().set_reactive("SEQ", "graph-visualizations", test_list(vec![]));
    editor.runtime_mut().set_reactive("SEQ", "track-active-notes", test_list(vec![]));
    let source = if package {
        "(import alez.neural.variable-reset)"
    } else {
        "(load \"@/scripts/sequencers/graph-neural-variable-reset-demo.lisp\")"
    };
    let overlays = editor.snapshot_file_backed_sources();
    let report = editor.runtime_mut().eval_source_transactional(None, source, overlays);
    assert!(report.success, "{:?}", report.diagnostics);
    editor.process_lisp_reload_report(report);
    let buffer = if package { "*var-reset*" } else { "*variable-reset*" };
    editor.runtime_mut().eval_str(&format!("(set-layout (list :buf \"{buffer}\" :hide-status true))")).unwrap();
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let id = editor.buffers.iter().find(|item| item.name == buffer).expect(buffer).id;
    editor.set_active_buffer(id);
    editor.set_layout_viewport(240, 100);
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let graph_id = state.published_sequencers()[0].id;
    (state, editor, graph_id)
}

fn graph_widget<'a>(layout: &'a eseqlisp::layout::LayoutNode, suffix: &str) -> &'a eseqlisp::layout::LayoutNode {
    let key = format!("graph-variable-reset-{suffix}");
    let node = find_layout_node_by_stable_key_suffix(layout, &key).expect(&key);
    assert_finite_nonzero_rect(node, &key);
    assert_layout_inside(node, layout, &key);
    node
}

fn graph_activity_updates_only_visualizations(package: bool) {
    let (state, mut editor, graph_id) = graph_panel_editor(package);
    let prefix = if package { "alez.neural.variable-reset/" } else { "" };
    // Recreate the parent with a different node count, then update activity
    // again: subtree captures must follow the current graph dimensions.
    for count in [8, 3] {
        if count != 8 {
            editor.runtime_mut().eval_str(&format!("({prefix}gvr-edit-config :node-count {count})")).unwrap();
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
        }
        for beat in [1.0, 2.0] {
            let event = GraphVisualizationEvent {
                node_index: 1, track: Some(0), sample_time: (beat * 24_000.0) as u64,
                beat, transpose: 7.0, velocity: 0.75,
            };
            let mut energy = vec![0.0; count];
            energy[1] = beat;
            let mut triggers = vec![0.0; count];
            triggers[1] = (beat * 0.25) as f32;
            state.set_graph_visualizations(vec![GraphVisualizationSnapshot {
                id: graph_id, name: "test".to_string(), active: true,
                current_beat: beat, num_nodes: count, energy, trigger_activity: triggers,
                event_history: vec![event],
                edges: vec![GraphVisualizationEdge {
                    from: 0, to: 1, weight: 1.0, dampening: beat * 0.25,
                    delay_steps: 1, distribution: Default::default(),
                }],
                ..Default::default()
            }]);
            let before = editor.runtime().ui_work_counters();
            let changed = editor.runtime_mut().set_reactive("SEQ", "graph-visualizations",
                build_graph_visualizations_value(&state));
            assert!(changed.effects_dirty, "graph activity must have live consumers");
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            let layout = editor.widget_layout().expect("updated graph layout");
            let after = editor.runtime().ui_work_counters();
            assert_eq!(after.full_buffer_reruns, before.full_buffer_reruns,
                "graph activity must not rerun the panel or its controls");
            assert_eq!(after.subtree_reruns - before.subtree_reruns, 4,
                "only the firing history and three activity matrices should rerun");
            let events = graph_widget(&layout, "event-view");
            assert_eq!(events.widget_type, "event-view");
            assert_eq!(events.props.get("current-beat"), Some(&Value::Number(beat)));
            assert_eq!(events.props.get("y-max"), Some(&Value::Number((count - 1) as f64)));
            assert_eq!(events.props.get("events"), Some(&test_list(vec![map_value([
                ("node", Value::Number(1.0)), ("track", Value::Number(0.0)),
                ("sample", Value::Number(event.sample_time as f64)), ("beat", Value::Number(beat)),
                ("transpose", Value::Number(7.0)), ("velocity", Value::Number(0.75)),
            ])])));
            let Value::List(snapshots) = build_graph_visualizations_value(&state) else { unreachable!() };
            let Value::Map(snapshot) = &*snapshots[0].borrow() else { unreachable!() };
            for field in ["trigger-matrix", "energy-matrix", "dampening-matrix"] {
                let matrix = graph_widget(&layout, field);
                assert_eq!(matrix.widget_type, "matrix");
                assert_eq!(matrix.props.get("rows"), Some(&Value::Number(count as f64)));
                assert_eq!(matrix.props.get("value"), Some(&*snapshot[field].borrow()));
            }
            graph_widget(&layout, "weight-matrix");
            graph_widget(&layout, "node-count");

            let notes = test_list(vec![test_list(vec![map_value([
                ("note", Value::Number(60.0 + beat)), ("velocity", Value::Number(0.75)),
                ("trigger-id", Value::Number(beat + count as f64)),
            ])])]);
            let before = editor.runtime().ui_work_counters();
            editor.runtime_mut().set_reactive("SEQ", "track-active-notes", notes.clone());
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            let layout = editor.widget_layout().unwrap();
            let after = editor.runtime().ui_work_counters();
            assert_eq!(after.full_buffer_reruns, before.full_buffer_reruns,
                "track notes must not rerun the graph panel");
            assert_eq!(after.subtree_reruns - before.subtree_reruns, 1,
                "track notes should rerun only the piano");
            assert_eq!(graph_widget(&layout, "piano").props.get("notes-by-track"), Some(&notes));
            assert_eq!(graph_widget(&layout, "event-view").props.get("current-beat"), Some(&Value::Number(beat)));
        }
    }
    // Stopping/clearing the visualization must clear the history and notes.
    editor.runtime_mut().set_reactive("SEQ", "graph-visualizations", test_list(vec![]));
    editor.runtime_mut().set_reactive("SEQ", "track-active-notes", test_list(vec![]));
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let layout = editor.widget_layout().unwrap();
    assert_eq!(graph_widget(&layout, "event-view").props.get("events"), Some(&test_list(vec![])));
    assert_eq!(graph_widget(&layout, "piano").props.get("notes-by-track"), Some(&test_list(vec![])));
}

#[test]
fn neural_package_activity_updates_only_visualizations() {
    graph_activity_updates_only_visualizations(true);
}

#[test]
fn legacy_graph_activity_updates_only_visualizations() {
    graph_activity_updates_only_visualizations(false);
}

/// A node's patch bay in the package panel: a selected cable goes on a real
/// click of the "× cable" chip and on a real Backspace in *var-reset*.
#[test]
fn neural_package_node_bay_removes_selected_cable_by_chip_and_backspace() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let (state, mut editor, graph_id) = graph_panel_editor(true);
    let mut authoring = Runtime::new();
    sequencer::lisp_host::register_published_process_authoring_natives(
        &mut authoring,
        Arc::clone(&state),
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    );
    authoring
        .eval_str(&sequencer::lisp_host::load_process_library_source())
        .expect("builtin process library");

    let eval = |editor: &mut Editor, source: &str| {
        let value = editor.runtime_mut().eval_str(source).unwrap_or_else(|e| panic!("{source}: {e:?}"));
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        value
    };
    eval(&mut editor, "(alez.neural.variable-reset/gvr-expand-node 1)");
    let add = |editor: &mut Editor, class: &str| match eval(editor, &format!("(graph-node-process-add {graph_id} 1 \"{class}\")")) {
        Some(Value::Number(id)) => id as u64,
        other => panic!("add {class}: {other:?}"),
    };
    let (rand, cmp) = (add(&mut editor, "lane-rand"), add(&mut editor, "lane-cmp"));
    let readers = |editor: &mut Editor| {
        eval(editor, &format!(
            "(len (get (nth (get (nth (graph-node-lane-patch {graph_id} 1) 0) :out-ports) 0) :readers))"
        ))
    };
    // Node namespace 1024 + 1, slot 0, ordinal 0: rand's wire port.
    let port_id = (1025 * 4096) * 16;
    let wire_and_select = |editor: &mut Editor| {
        eval(editor, &format!(
            "(do (graph-node-process-wire {graph_id} 1 {rand} \"wire\" {cmp} \"a\") (eseq.sequencer/lane-patch-node-touch))"
        ));
        assert_eq!(readers(editor), Some(Value::Number(1.0)), "rand -> cmp a is wired");
        eval(editor, &format!("(eseq.sequencer/lane-patch-select-cable 1025 {port_id} 1 0)"));
        assert_eq!(eval(editor, "(eseq.sequencer/lane-patch-cable-selected?)"), Some(Value::Bool(true)));
    };

    wire_and_select(&mut editor);
    let layout = editor.widget_layout().expect("node bay layout");
    let chip = find_layout_node_by_stable_key_suffix(&layout, "lane-patch-remove-cable")
        .expect("× chip shows while a cable is selected")
        .clone();
    let (col, row) = (chip.rect.col + chip.rect.width * 0.5, chip.rect.row + chip.rect.height * 0.5);
    editor.handle_mouse_precise(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col.floor() as u16,
            row: row.floor() as u16,
            modifiers: KeyModifiers::NONE,
        },
        0, 0, 240, 100, col, row,
    );
    editor.refresh_runtime_side_effects();
    assert_eq!(readers(&mut editor), Some(Value::Number(0.0)), "× removed the cable");

    wire_and_select(&mut editor);
    editor.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    editor.refresh_runtime_side_effects();
    assert_eq!(readers(&mut editor), Some(Value::Number(0.0)), "Backspace removed the cable");
    assert_eq!(eval(&mut editor, "(eseq.sequencer/lane-patch-cable-selected?)"), Some(Value::Bool(false)));
}
