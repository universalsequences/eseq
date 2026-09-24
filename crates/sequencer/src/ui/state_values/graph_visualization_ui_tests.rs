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
    editor.runtime_mut().register_native("def-sequencer", move |args, _ctx| {
        let published = sequencer::lisp_host::published_sequencer_from_def_args(&args)?;
        let id = published.id;
        published_state.publish_sequencer(published);
        Ok(Value::Number(id as f64))
    });
    editor.runtime_mut().set_reactive("SEQ", "graph-visualizations", test_list(vec![]));
    editor.runtime_mut().set_reactive("SEQ", "track-active-notes", test_list(vec![]));
    let buffer = if package {
        neural_instance_view(&state, &mut editor, 1, "neural 1")
    } else {
        let source = "(load \"@/scripts/sequencers/graph-neural-variable-reset-demo.lisp\")";
        let overlays = editor.snapshot_file_backed_sources();
        let report = editor.runtime_mut().eval_source_transactional(None, source, overlays);
        assert!(report.success, "{:?}", report.diagnostics);
        editor.process_lisp_reload_report(report);
        "*variable-reset*".to_string()
    };
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

/// Import alez/neural (registering its `neural` kind) unless it is
/// registered already, then give it instance `id` the way the host does
/// (instance-kinds spec §5/§7): publish the instance's sequencer, create
/// its record, bind its view buffer. Returns the buffer name.
pub(super) fn neural_instance_view(
    state: &Arc<SequencerState>,
    editor: &mut Editor,
    id: u64,
    label: &str,
) -> String {
    if editor.runtime().instance_kind_schema("alez/neural:neural").is_none() {
        let overlays = editor.snapshot_file_backed_sources();
        let report = editor.runtime_mut().eval_source_transactional(
            None,
            "(import alez.neural.variable-reset)",
            overlays,
        );
        assert!(report.success, "{:?}", report.diagnostics);
        editor.process_lisp_reload_report(report);
    }
    let kind = sequencer::lisp_host::registered_kind("alez/neural:neural")
        .expect("importing alez.neural.variable-reset registers alez/neural:neural");
    let mut instances = sequencer::project::ProjectInstances::default();
    for live in editor.runtime().live_instances() {
        if live != id {
            instances.list.push(sequencer::project::ProjectInstance {
                id: live,
                kind: kind.id.clone(),
                owner: sequencer::project::ProjectInstanceOwner::Project,
                label: match editor.runtime().instance_field(live, "label") {
                    Ok(Value::String(label)) => label,
                    _ => format!("neural {live}"),
                },
            });
        }
    }
    instances.list.push(sequencer::project::ProjectInstance {
        id,
        kind: kind.id.clone(),
        owner: sequencer::project::ProjectInstanceOwner::Project,
        label: label.to_string(),
    });
    state.publish_sequencer(
        sequencer::lisp_host::instance_published_sequencer(&kind, id, None).expect("a :sequencer"),
    );
    sequencer::lisp_host::sync_instance_records(editor.runtime_mut(), &instances);
    crate::host_commands::instances::sync_instance_views(editor, &instances, false);
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    sequencer::lisp_host::instance_view_buffer_name("neural", label)
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
    let edit_count = |count: usize| {
        if package {
            format!("(alez.neural.variable-reset/gvr-edit-config (instance-ref {graph_id}) :node-count {count})")
        } else {
            format!("(gvr-edit-config :node-count {count})")
        }
    };
    // Recreate the parent with a different node count, then update activity
    // again: subtree captures must follow the current graph dimensions.
    for count in [8, 3] {
        if count != 8 {
            editor.runtime_mut().eval_str(&edit_count(count)).unwrap();
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
/// click of the "× cable" chip and on a real Backspace in the instance's
/// `*neural · neural 1*` buffer (the kind's :keymap).
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
    eval(&mut editor, &format!("(alez.neural.variable-reset/gvr-expand-node (instance-ref {graph_id}) 1)"));
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
    // Node 1's own namespace (derived from the graph id, instance-kinds
    // spec §7), slot 0, ordinal 0: rand's wire port.
    let ns = match eval(&mut editor, &format!("(graph-node-patch-namespace {graph_id} 1)")) {
        Some(Value::Number(ns)) => ns as usize,
        other => panic!("node namespace: {other:?}"),
    };
    assert_eq!(ns, sequencer::lisp_host::graph_node_lane_patch_namespace(graph_id, 1));
    let port_id = (ns * 4096) * 16;
    let wire_and_select = |editor: &mut Editor| {
        eval(editor, &format!(
            "(do (graph-node-process-wire {graph_id} 1 {rand} \"wire\" {cmp} \"a\") (eseq.sequencer/lane-patch-node-touch))"
        ));
        assert_eq!(readers(editor), Some(Value::Number(1.0)), "rand -> cmp a is wired");
        eval(editor, &format!("(eseq.sequencer/lane-patch-select-cable {ns} {port_id} 1 0)"));
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

/// Importing the package registers its `neural` kind (instance-kinds spec
/// §3/§8.1) and publishes nothing: the id comes from the package manifest,
/// the kind carries its :state view cells, :view, :keymap and :on-create,
/// and the manifest's declared kinds pass the attach check. An instance
/// publishes the kind's graph under its own id.
#[test]
fn importing_the_neural_package_registers_its_kind() {
    sequencer::lisp_host::clear_kind_registry();
    let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
    let mut editor = full_grid_editor_for_scroll_tests();
    let (roots, errors) = sequencer::app_paths::app_paths().module_load_roots();
    assert!(errors.is_empty(), "{errors:?}");
    editor.runtime_mut().set_scoped_module_load_path(roots);
    sequencer::lisp_host::register_graph_authoring_natives(editor.runtime_mut(), Arc::clone(&state));
    let overlays = editor.snapshot_file_backed_sources();
    let report = editor.runtime_mut().eval_source_transactional(
        None,
        "(import alez.neural.variable-reset)",
        overlays,
    );
    assert!(report.success, "{:?}", report.diagnostics);
    editor.process_lisp_reload_report(report);
    assert!(state.published_sequencers().is_empty(), "importing a kind creates no instance");
    let kind = sequencer::lisp_host::registered_kind("alez/neural:neural")
        .expect("alez/neural:neural registered on import");
    assert_eq!(kind.module.as_deref(), Some("alez.neural.variable-reset"));
    assert_eq!(
        kind.state_fields,
        ["expanded-node", "selected-neuron", "add-class", "map-slot", "map-port", "piano-depth"]
            .map(str::to_string)
            .to_vec()
    );
    assert!(kind.has_view);
    assert_eq!(kind.keymap.as_deref(), Some("eseq.sequencer-keys/sequencer-keys"));
    let schema = editor.runtime().instance_kind_schema("alez/neural:neural").expect("schema");
    assert!(schema.on_create.is_some(), "the ring default is the kind's :on-create");
    let manifest = kind.sequencer.clone().expect("the kind carries a :sequencer");
    assert_eq!(manifest.max_poly, 4);
    assert_eq!(manifest.node.params.len(), 9);
    let published = sequencer::lisp_host::instance_published_sequencer(&kind, 7, None)
        .expect("instance sequencer");
    assert_eq!((published.id, published.name.as_str()), (7, "neural#7"));
    assert_eq!(published.graph.expect("graph").node, manifest.node);
    let catalog = sequencer::app_paths::app_paths().package_catalog();
    let package = catalog
        .package_for_module("alez.neural.variable-reset")
        .expect("alez/neural installed");
    assert_eq!(
        sequencer::lisp_host::check_manifest_kinds(&package.manifest, "alez.neural.variable-reset"),
        Ok(Vec::new())
    );
}

/// Two `neural` instances of the real package (instance-kinds spec §10/§11):
/// `:on-create` writes the ring into a fresh instance only, and document
/// edits, view state and widget keys stay per instance. The panel re-reads
/// every edit through tracked graph reads, with no echo or cache.
#[test]
fn neural_instances_get_the_ring_on_create_and_stay_independent() {
    let (state, mut editor, first) = graph_panel_editor(true);
    let second_buffer = neural_instance_view(&state, &mut editor, 2, "neural 2");
    let eval = |editor: &mut Editor, source: &str| {
        let value = editor.runtime_mut().eval_str(source).unwrap_or_else(|e| panic!("{source}: {e:?}"));
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        value
    };
    let weight = |editor: &mut Editor, id: u64, from: usize, to: usize| {
        eval(editor, &format!("(graph-edge-value (instance-ref {id}) {from} {to} :weight)"))
    };
    // Only instance 1 is freshly created here; instance 2 stands in for a
    // duplicate or kit load, which never runs the hook.
    crate::host_commands::instances::run_on_create(&mut editor, first);
    for from in 0..8 {
        assert_eq!(weight(&mut editor, first, from, (from + 1) % 8), Some(Value::Number(1.0)), "ring {from}");
        assert_eq!(weight(&mut editor, first, from, (from + 2) % 8), Some(Value::Number(0.0)));
        assert_eq!(weight(&mut editor, 2, from, (from + 1) % 8), Some(Value::Number(0.0)), "no hook on 2");
    }
    assert_eq!(
        eval(&mut editor, &format!("(graph-node-value (instance-ref {first}) 0 :seed-route)")),
        Some(Value::Number(1.0)),
        "node 0 seeds from its route"
    );

    // The rendered weight matrix follows a write with no echo.
    let matrix_value = |editor: &Editor, buffer: &str| {
        let item = editor.buffers.iter().find(|item| item.name == buffer).expect(buffer);
        let tree = item.widget_tree.as_ref().expect("rendered");
        let mut stack = vec![tree.clone()];
        while let Some(node) = stack.pop() {
            let Value::Map(map) = &node else { continue };
            let key = map.get("__stable-key").map(|key| key.borrow().clone());
            if let Some(Value::String(key)) = key {
                if key.ends_with("graph-variable-reset-weight-matrix") && map.contains_key("rows") {
                    return (key, map.get("value").map(|value| value.borrow().clone()));
                }
            }
            if let Some(children) = map.get("children") {
                if let Value::List(children) = &*children.borrow() {
                    stack.extend(children.iter().map(|child| child.borrow().clone()));
                }
            }
        }
        panic!("{buffer} has no weight matrix");
    };
    let first_buffer = sequencer::lisp_host::instance_view_buffer_name("neural", "neural 1");
    for buffer in [&first_buffer, &second_buffer] {
        eval(&mut editor, &format!("(set-layout (list :buf \"{buffer}\" :hide-status true))"));
    }
    eval(&mut editor, "(graph-edge (instance-ref 2) :from 3 :to 5 :weight 0.75)");
    eval(&mut editor, &format!("(set-layout (list :buf \"{second_buffer}\" :hide-status true))"));
    let (key_2, value_2) = matrix_value(&editor, &second_buffer);
    assert!(key_2.starts_with("instance:2::"), "{key_2}");
    let row_3 = match &value_2 {
        Some(Value::List(rows)) => rows[3].borrow().clone(),
        other => panic!("matrix value {other:?}"),
    };
    let Value::List(row_3) = row_3 else { panic!("row") };
    assert_eq!(*row_3[5].borrow(), Value::Number(0.75), "instance 2's matrix shows its own write");
    assert_eq!(weight(&mut editor, first, 3, 5), Some(Value::Number(0.0)), "instance 1 is untouched");
    eval(&mut editor, &format!("(set-layout (list :buf \"{first_buffer}\" :hide-status true))"));
    let (key_1, value_1) = matrix_value(&editor, &first_buffer);
    assert!(key_1.starts_with(&format!("instance:{first}::")), "{key_1}");
    let Some(Value::List(rows)) = value_1 else { panic!("matrix value") };
    let Value::List(row_0) = rows[0].borrow().clone() else { panic!("row") };
    assert_eq!(*row_0[1].borrow(), Value::Number(1.0), "instance 1's matrix shows its ring");

    // View state is per instance: expanding a node in one leaves the other.
    eval(&mut editor, &format!("(alez.neural.variable-reset/gvr-expand-node (instance-ref {first}) 2)"));
    assert_eq!(
        eval(&mut editor, &format!("(let ((i (instance-ref {first}))) i.expanded-node)")),
        Some(Value::Number(2.0))
    );
    assert_eq!(eval(&mut editor, "(let ((i (instance-ref 2))) i.expanded-node)"), Some(Value::Number(-1.0)));
}
