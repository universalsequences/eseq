use super::*;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

#[test]
fn group_badge_clicks_ignore_routes_from_unavailable_mod_outputs() {
    let mut editor = full_grid_editor_for_scroll_tests();
    let mut group = rack_group_fixture(false);
    group.members = vec![0];
    let rack = group.rack.as_mut().unwrap();
    rack.pads.truncate(1);
    rack.choke_groups.truncate(1);
    apply_group_bindings(&mut editor, group);
    for (field, value) in [
        ("track-instrument-types", test_string_list(&["sampler"])),
        ("track-mod-output-available", test_bool_list(&[false])),
        ("mod-routes", test_list(vec![map_value([
            ("source", Value::Number(0.0)),
            ("dest-kind", Value::String("bus".into())),
            ("dest", Value::Number(2.0)),
            ("input", Value::Number(0.0)),
        ])])),
        ("selected-mod-routes", test_list(vec![])),
    ] {
        editor.runtime_mut().set_reactive("SEQ", field, value);
    }
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let mixer_id = editor.buffers.iter().find(|b| b.name == "*mixer*").unwrap().id;
    editor.set_active_buffer(mixer_id);

    // Use the real tiled pointer path, including the cable interception
    // before ordinary widget dispatch. Callback-only tests miss this bug.
    for x_fraction in [0.35, 0.5, 0.65, 0.8, 0.95] {
        for y_fraction in [0.2, 0.5, 0.8] {
            editor.runtime_mut().eval_str(
                "(do (seq-clear-delete-target) (set! eseq.seq-core-state/selected-bus -1))",
            ).unwrap();
            editor.refresh_runtime_side_effects();
            let frame = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 140, 50);
            let tile = frame.tiles.iter().find(|t| t.frame.buffer_name == "*mixer*").unwrap();
            let layout = tile.frame.widget_layout.as_deref().unwrap();
            let badge = find_layout_node_by_stable_key_suffix(layout, "/group-badge-7").unwrap();
            assert_finite_nonzero_rect(badge, "group badge");
            let source = find_layout_node_by_stable_key_suffix(layout, "/mod-out-0").unwrap();
            assert_eq!(source.props.get("active"), Some(&Value::Bool(false)));
            let at = (
                tile.rect.col + badge.rect.col + badge.rect.width * x_fraction
                    - tile.frame.widget_layout_scroll_left,
                tile.rect.row + badge.rect.row + badge.rect.height * y_fraction
                    - tile.frame.widget_scroll_top,
            );
            assert!(at.0 >= tile.rect.col && at.0 < tile.rect.col + tile.rect.width);
            assert!(at.1 >= tile.rect.row && at.1 < tile.rect.row + tile.rect.height);
            editor.active_leaf_mut().last_widget_click = None;
            for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
                editor.handle_tiled_mouse_precise(MouseEvent {
                    kind,
                    column: at.0 as u16,
                    row: at.1 as u16,
                    modifiers: KeyModifiers::NONE,
                }, at.0, at.1, 0);
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();
            }
            assert_eq!(
                editor.runtime_mut().eval_str("(seq-active-delete-target-kind)").unwrap(),
                Some(Value::String("mixer-group".into())),
                "badge click at ({x_fraction}, {y_fraction}) must select the group",
            );
            assert_eq!(
                editor.runtime_mut().eval_str("eseq.seq-core-state/selected-bus").unwrap(),
                Some(Value::Number(2.0)),
            );
        }
    }
}
