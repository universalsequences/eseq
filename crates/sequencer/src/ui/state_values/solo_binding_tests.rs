use super::*;

fn bound_states<'a>(node: &'a eseqlisp::layout::LayoutNode, states: &mut Vec<(&'a eseqlisp::layout::LayoutNode, &'a str, f32)>) {
    for (prop, value) in &node.props {
        if let Value::ReactiveRef { namespace, field, .. } = value {
            if namespace == "SEQ" && field == "track-muted-effective" {
                states.push((node, prop, eseqlisp::widget_render::get_f32_prop(&node.props, prop, -1.0)));
            }
        }
    }
    for child in &node.children { bound_states(child, states); }
}

/// The *step* panel's track badge binds the kinds `audible`, like every
/// other channel view (`mute_and_solo_only_repaint_through_kinds`). A mute or
/// solo republishes the legacy effective-mute field without a render-time
/// reader and never rebuilds the layout.
#[test]
fn mute_and_solo_repaint_the_step_panel_badge_without_rebuilding_layout() {
    let mut editor = full_grid_editor_for_scroll_tests();
    apply_sequencer_perf_pattern(&mut editor, 8, 64, 0);
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();

    let state = Arc::new(SequencerState::new(8, vec![]));
    let app = test_app_for_track_visual_state(state.clone());
    let buffer = "*step*";
    let id = editor.buffers.iter().find(|item| item.name == buffer).expect(buffer).id;
    editor.set_active_buffer(id);
    let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 240, 100);
    let layout = editor.widget_layout().expect(buffer);
    // The step panel's track chip binds the current track's eseq.kinds
    // `audible` (no SEQ mute field).
    let badge = find_layout_node_by_stable_key_suffix(&layout, "/step-track-badge")
        .expect("step track badge");
    assert!(
        matches!(badge.props.get("muted"), Some(Value::ReactiveRef { field, .. }) if field == "audible"),
        "{:?}",
        badge.props.get("muted")
    );
    for (edit, index) in [("solo", 0), ("mute", 0), ("mute", 3)] {
        for enabled in [true, false] {
            let mut bindings = Vec::new();
            bound_states(&layout, &mut bindings);
            let _ = editor.take_dirty_widget_ids();
            let before = editor.runtime().ui_work_counters();
            match edit {
                "mute" => state.pattern.track_params[index].set_mute(enabled),
                _ => state.pattern.track_params[index].set_solo(enabled),
            }
            let rt = editor.runtime_mut();
            assert!(!sync_track_mute_visual_binding_fields(rt, &app, &state, 0..8, true),
                "{edit}: effective mute fields still have render-time readers");
            let dirty = editor.take_dirty_widget_ids();
            for (node, prop, previous) in bindings {
                let current = eseqlisp::widget_render::get_f32_prop(&node.props, prop, -1.0);
                if current != previous {
                    assert!(dirty.contains(&node.widget_id), "{edit}: bound {prop} must request repaint");
                }
            }
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 240, 100);
            let after = editor.runtime().ui_work_counters();
            assert_eq!(after.full_buffer_reruns, before.full_buffer_reruns,
                "{edit} must only repaint bound widgets");
            assert_eq!(after.subtree_reruns, before.subtree_reruns, "{edit} must not rebuild subtrees");
            assert_eq!(after.relayout_subtree, before.relayout_subtree, "{edit} must not relayout subtrees");
            assert_eq!(after.relayout_full, before.relayout_full, "{edit} must not relayout the buffer");
            assert!(Arc::ptr_eq(&layout, &editor.widget_layout().unwrap()),
                "{edit} must retain the measured layout");
        }
    }
}

/// `key`'s `prop` binds `field` of instance `id` (a `#'` binding).
fn assert_kind_binding(layout: &eseqlisp::layout::LayoutNode, key: &str, prop: &str, id: eseqlisp::vm::InstanceId, field: &str) {
    let node = find_layout_node_by_stable_key_suffix(layout, key).expect(key);
    assert_finite_nonzero_rect(node, key);
    assert_layout_inside(node, layout, key);
    let namespace = format!("%instance/{id}");
    assert!(matches!(node.props.get(prop), Some(Value::ReactiveRef {
        namespace: actual, field: actual_field, ..
    }) if *actual == namespace && actual_field == field),
        "{key} {prop} must bind {namespace}.{field}: {:?}", node.props.get(prop));
}

/// Every widget prop bound to a field of `ids`, with the value it shows.
fn bound_kind_states<'a>(
    node: &'a eseqlisp::layout::LayoutNode,
    namespaces: &[String],
    states: &mut Vec<(&'a eseqlisp::layout::LayoutNode, &'a str, f32)>,
) {
    for (prop, value) in &node.props {
        if let Value::ReactiveRef { namespace, .. } = value {
            if namespaces.contains(namespace) {
                states.push((node, prop, eseqlisp::widget_render::get_f32_prop(&node.props, prop, -1.0)));
            }
        }
    }
    for child in &node.children { bound_kind_states(child, namespaces, states); }
}

/// The mixer, the patch mixer, the sequencer grid and the arrangement's track
/// headers bind mute, solo, audibility, arm, the selection, the faders and
/// the meters through kinds: a change of any repaints the bound widgets and
/// never re-runs, rebuilds or relayouts the view.
#[test]
fn mute_and_solo_only_repaint_through_kinds() {
    let mut editor = full_grid_editor_for_scroll_tests();
    apply_sequencer_perf_pattern(&mut editor, 8, 64, 0);
    let mut group = regular_group_fixture(false);
    group.members = vec![1, 2];
    apply_group_bindings(&mut editor, group);
    let rt = editor.runtime_mut();
    let (t0, t3) = (kind_track(rt, 0), kind_track(rt, 3));
    let bus = rt.keyed_instance("eseq.kinds:bus", &[2]).unwrap();
    set_field(rt, t3, "collapsed", Value::Bool(true));
    rt.run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let namespaces: Vec<String> = [t0, t3, bus].iter().map(|id| format!("%instance/{id}")).collect();

    for buffer in ["*mixer*", "*patch-mixer*", "*sequencer*", "*arrangement*"] {
        let id = editor.buffers.iter().find(|item| item.name == buffer).expect(buffer).id;
        editor.set_active_buffer(id);
        let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 240, 100);
        let layout = editor.widget_layout().expect(buffer);
        if buffer == "*mixer*" || buffer == "*patch-mixer*" {
            assert_kind_binding(&layout, "/track-label-content-0", "muted", t0, "audible");
            assert_kind_binding(
                &layout,
                "/track-label-content-0",
                "active",
                t0,
                "delete-target",
            );
        } else {
            assert_kind_binding(&layout, "/solo-0", "active", t0, "soloed");
            assert_kind_binding(&layout, "/track-name-label-0", "muted", t0, "audible");
        }
        if buffer == "*mixer*" {
            assert_kind_binding(&layout, "/track-collapsed-label-content-3", "muted", t3, "audible");
        }
        if buffer == "*mixer*" || buffer == "*sequencer*" {
            assert_kind_binding(&layout, "/group-solo-8", "active", bus, "soloed");
            assert_kind_binding(&layout, "/group-mute-8", "active", bus, "muted");
        }
        if buffer == "*sequencer*" {
            assert_kind_binding(&layout, "/mute-0", "active", t0, "muted");
            assert_kind_binding(&layout, "/arm-0", "active", t0, "armed");
            assert_kind_binding(&layout, "/track-volume-control-0", "volume", t0, "volume");
            assert_kind_binding(&layout, "/track-volume-control-0", "level", t0, "peak");
        }
        for (id, field, on, off) in [
            (t0, "audible", Value::Bool(false), Value::Bool(true)),
            (t0, "muted", Value::Bool(true), Value::Bool(false)),
            (t0, "soloed", Value::Bool(true), Value::Bool(false)),
            (t0, "armed", Value::Bool(true), Value::Bool(false)),
            (t0, "in-selection", Value::Bool(false), Value::Bool(true)),
            (t0, "delete-target", Value::Bool(true), Value::Bool(false)),
            (t0, "volume", Value::Number(0.3), Value::Number(1.0)),
            (t0, "peak", Value::Number(0.6), Value::Number(0.0)),
            (t3, "audible", Value::Bool(false), Value::Bool(true)),
            (bus, "soloed", Value::Bool(true), Value::Bool(false)),
            (bus, "muted", Value::Bool(true), Value::Bool(false)),
            (bus, "volume", Value::Number(0.4), Value::Number(1.0)),
        ] {
            for value in [on, off] {
                let mut bindings = Vec::new();
                bound_kind_states(&layout, &namespaces, &mut bindings);
                let _ = editor.take_dirty_widget_ids();
                let before = editor.runtime().ui_work_counters();
                set_field(editor.runtime_mut(), id, field, value.clone());
                let dirty = editor.take_dirty_widget_ids();
                for (node, prop, previous) in bindings {
                    let current = eseqlisp::widget_render::get_f32_prop(&node.props, prop, -1.0);
                    if current != previous {
                        assert!(dirty.contains(&node.widget_id), "{buffer} {field}: bound {prop} must request repaint");
                    }
                }
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 240, 100);
                let after = editor.runtime().ui_work_counters();
                assert_eq!(after.full_buffer_reruns, before.full_buffer_reruns,
                    "{buffer} {field}={value:?} must only repaint bound widgets");
                assert_eq!(after.subtree_reruns, before.subtree_reruns,
                    "{buffer} {field}={value:?} must not rebuild subtrees");
                assert_eq!(after.relayout_subtree, before.relayout_subtree,
                    "{buffer} {field}={value:?} must not relayout subtrees");
                assert_eq!(after.relayout_full, before.relayout_full,
                    "{buffer} {field}={value:?} must not relayout the buffer");
                assert!(Arc::ptr_eq(&layout, &editor.widget_layout().unwrap()),
                    "{buffer} {field}={value:?} must retain the measured layout");
            }
        }
    }
}
