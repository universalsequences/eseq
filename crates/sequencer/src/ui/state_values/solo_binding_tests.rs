use super::*;

fn assert_binding(layout: &eseqlisp::layout::LayoutNode, key: &str, prop: &str, field: &str, index: usize) {
    let node = find_layout_node_by_stable_key_suffix(layout, key).expect(key);
    assert_finite_nonzero_rect(node, key);
    assert_layout_inside(node, layout, key);
    assert!(matches!(node.props.get(prop), Some(Value::ReactiveRef {
        namespace, field: actual, index: Some(actual_index), ..
    }) if namespace == "SEQ" && actual == field && *actual_index == index),
        "{key} {prop} must bind SEQ.{field}[{index}]: {:?}", node.props.get(prop));
}

fn bound_states<'a>(node: &'a eseqlisp::layout::LayoutNode, states: &mut Vec<(&'a eseqlisp::layout::LayoutNode, &'a str, f32)>) {
    for (prop, value) in &node.props {
        if let Value::ReactiveRef { namespace, field, .. } = value {
            if namespace == "SEQ"
                && matches!(field.as_str(), "track-mutes" | "track-solos" | "track-muted-effective" | "bus-mutes" | "bus-solos")
            {
                states.push((node, prop, eseqlisp::widget_render::get_f32_prop(&node.props, prop, -1.0)));
            }
        }
    }
    for child in &node.children { bound_states(child, states); }
}

#[test]
fn mute_and_solo_repaint_all_channel_views_without_rebuilding_layout() {
    let mut editor = full_grid_editor_for_scroll_tests();
    apply_sequencer_perf_pattern(&mut editor, 8, 64, 0);
    let mut group = regular_group_fixture(false);
    group.members = vec![1, 2];
    apply_group_bindings(&mut editor, group);
    editor.runtime_mut().set_reactive("SEQ", "track-collapsed",
        test_bool_list(&[false, false, false, true, false, false, false, false]));
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();

    let state = Arc::new(SequencerState::new(8, vec![]));
    let mut app = test_app_for_track_visual_state(state.clone());
    for buffer in ["*sequencer*", "*mixer*", "*patch-mixer*", "*step*", "*arrangement*"] {
        let id = editor.buffers.iter().find(|item| item.name == buffer).expect(buffer).id;
        editor.set_active_buffer(id);
        let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 240, 100);
        let layout = editor.widget_layout().expect(buffer);
        match buffer {
            "*sequencer*" => {
                assert_binding(&layout, "/mute-0", "active", "track-mutes", 0);
                assert_binding(&layout, "/solo-0", "active", "track-solos", 0);
                assert_binding(&layout, "/track-name-label-0", "muted", "track-muted-effective", 0);
                assert_binding(&layout, "/group-solo-8", "active", "bus-solos", 2);
            }
            "*mixer*" => {
                assert_binding(&layout, "/track-label-content-0", "muted", "track-muted-effective", 0);
                assert_binding(&layout, "/track-collapsed-label-content-3", "muted", "track-muted-effective", 3);
                assert_binding(&layout, "/group-solo-8", "active", "bus-solos", 2);
            }
            "*patch-mixer*" => assert_binding(&layout, "/track-label-content-0", "muted", "track-muted-effective", 0),
            "*step*" => assert_binding(&layout, "/step-track-badge", "muted", "track-muted-effective", 0),
            "*arrangement*" => {
                assert_binding(&layout, "/solo-0", "active", "track-solos", 0);
                assert_binding(&layout, "/track-name-label-0", "muted", "track-muted-effective", 0);
            }
            _ => unreachable!(),
        }
        for (field, index) in [("track-solos", 0), ("track-mutes", 0),
            ("track-mutes", 3), ("bus-solos", 2), ("bus-mutes", 2)] {
            for enabled in [true, false] {
                let mut bindings = Vec::new();
                bound_states(&layout, &mut bindings);
                let _ = editor.take_dirty_widget_ids();
                let before = editor.runtime().ui_work_counters();
                let start = std::time::Instant::now();
                let rt = editor.runtime_mut();
                let changed = rt.set_reactive_list_index("SEQ", field, index, Value::Bool(enabled));
                assert!(!changed.effects_dirty, "{buffer}: {field} still has a render-time reader");
                match field {
                    "track-mutes" => state.pattern.track_params[index].set_mute(enabled),
                    "track-solos" => state.pattern.track_params[index].set_solo(enabled),
                    "bus-solos" => app.buses[index].solo = enabled,
                    "bus-mutes" => app.buses[index].mute = enabled,
                    _ => unreachable!(),
                }
                assert!(!sync_track_mute_visual_binding_fields(rt, &app, &state, 0..8, true),
                    "{buffer}: effective mute/color fields still have render-time readers");
                let dirty = editor.take_dirty_widget_ids();
                for (node, prop, previous) in bindings {
                    let current = eseqlisp::widget_render::get_f32_prop(&node.props, prop, -1.0);
                    if current != previous {
                        assert!(dirty.contains(&node.widget_id), "{buffer} {field}: bound {prop} must request repaint");
                    }
                    if let Some(Value::ReactiveRef { field: bound_field, index: Some(bound_index), .. }) = node.props.get(prop) {
                        if bound_field == field && *bound_index == index {
                            assert_eq!(current, if enabled { 1.0 } else { 0.0 });
                        }
                    }
                }
                let rt = editor.runtime_mut();
                rt.run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 240, 100);
                let after = editor.runtime().ui_work_counters();
                eprintln!("{buffer} {field}[{index}]={enabled}: {:.3}ms; {:?}",
                    start.elapsed().as_secs_f64() * 1000.0, after);
                assert_eq!(after.full_buffer_reruns, before.full_buffer_reruns,
                    "{buffer} {field} must only repaint bound widgets");
                assert_eq!(after.subtree_reruns, before.subtree_reruns,
                    "{buffer} {field} must not rebuild subtrees");
                assert_eq!(after.relayout_subtree, before.relayout_subtree,
                    "{buffer} {field} must not relayout subtrees");
                assert_eq!(after.relayout_full, before.relayout_full,
                    "{buffer} {field} must not relayout the buffer");
                assert!(Arc::ptr_eq(&layout, &editor.widget_layout().unwrap()),
                    "{buffer} {field} must retain the measured layout");
            }
        }
    }
}
