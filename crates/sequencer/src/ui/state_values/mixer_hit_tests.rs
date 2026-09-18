use super::*;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

fn click_at(editor: &mut Editor, at: (f32, f32)) {
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
}

fn first_badge_click(editor: &mut Editor, buffer: &str, key: &str) -> (f32, f32) {
    assert!(editor.switch_active_tile_to_buffer_named(buffer));
    let frame = eseqlisp::frame::build_tiled_render_frame_borderless(editor, 180, 70);
    let tile = frame.tiles.iter().find(|t| t.frame.buffer_name == buffer).unwrap();
    let layout = tile.frame.widget_layout.as_deref().unwrap();
    let badge = find_layout_node_by_stable_key_suffix(layout, key).unwrap();
    assert_finite_nonzero_rect(badge, "badge pointer target");
    let at = (
        tile.rect.col + badge.rect.col + badge.rect.width * 0.7
            - tile.frame.widget_layout_scroll_left,
        tile.rect.row + badge.rect.row + badge.rect.height * 0.5
            - tile.frame.widget_scroll_top,
    );
    assert!(at.0 >= tile.rect.col && at.0 < tile.rect.col + tile.rect.width);
    assert!(at.1 >= tile.rect.row && at.1 < tile.rect.row + tile.rect.height);
    editor.active_leaf_mut().last_widget_click = None;
    click_at(editor, at);
    at
}

fn second_badge_click(editor: &mut Editor, at: (f32, f32)) {
    // Exclude debug-build layout/assertion time from the simulated user's
    // inter-click interval. Keep the real first click's widget and position.
    editor.active_leaf_mut().last_widget_click.as_mut().unwrap().at = std::time::Instant::now();
    click_at(editor, at);
}

fn assert_lower_panel(editor: &mut Editor, buffer: &str) {
    assert_eq!(
        editor.runtime_mut().eval_str("eseq.seq-step-tabs/lower-panel-buffer").unwrap(),
        Some(Value::String(buffer.into())),
    );
}

#[test]
fn track_badge_double_click_toggles_panel_and_clears_delete_arm() {
    let mut editor = full_grid_editor_for_scroll_tests();
    for collapsed in [false, true] {
        editor.runtime_mut().set_reactive("SEQ", "track-collapsed", test_bool_list(&[collapsed]));
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let key = if collapsed { "/track-collapsed-label-0" } else { "mixer-v2-strip-label-0" };
        for (before, after) in [("*fx*", "*piano-roll*"), ("*piano-roll*", "*fx*")] {
            assert_lower_panel(&mut editor, before);
            let at = first_badge_click(&mut editor, "*mixer*", key);
            assert_lower_panel(&mut editor, before);
            assert_eq!(
                editor.runtime_mut().eval_str("(seq-active-delete-target-kind)").unwrap(),
                Some(Value::String("mixer-track".into())),
                "single click still arms deletion",
            );
            second_badge_click(&mut editor, at);
            assert_lower_panel(&mut editor, after);
            assert_eq!(
                editor.runtime_mut().eval_str("(seq-active-delete-target-kind)").unwrap(),
                Some(Value::Bool(false)),
                "double click must clear the first click's deletion arm, collapsed={collapsed}",
            );
        }
    }
}

#[test]
fn group_badge_double_click_opens_own_fx_and_clears_delete_arm() {
    let mut editor = full_grid_editor_for_scroll_tests();
    for rack in [false, true] {
        let mut group = rack_group_fixture(false);
        group.members = vec![0];
        if rack {
            let config = group.rack.as_mut().unwrap();
            config.pads.truncate(1);
            config.choke_groups.truncate(1);
        } else {
            group.rack = None;
        }
        apply_group_bindings(&mut editor, group);
        let sequencer_key = if rack { "/rack-select-7" } else { "/group-select-7" };
        for (buffer, key) in [("*mixer*", "/group-badge-7"), ("*sequencer*", sequencer_key)] {
            editor.runtime_mut().eval_str(
                "(do (eseq.seq-panels/seq-open-piano-roll-bottom-for-track 0)
                     (set! eseq.seq-core-state/selected-bus -1)
                     (seq-clear-delete-target))",
            ).unwrap();
            if buffer == "*sequencer*" {
                // A mixer deletion arm can still be present when navigating
                // through the sequencer's group header.
                editor.runtime_mut().eval_str(
                    "(seq-set-delete-target :mixer-group (dict :group-id 7))",
                ).unwrap();
            }
            editor.refresh_runtime_side_effects();
            let at = first_badge_click(&mut editor, buffer, key);
            assert_lower_panel(&mut editor, "*piano-roll*");
            assert_eq!(
                editor.runtime_mut().eval_str("(seq-active-delete-target-kind)").unwrap(),
                Some(Value::String("mixer-group".into())),
            );
            second_badge_click(&mut editor, at);
            assert_lower_panel(&mut editor, "*fx*");
            assert_eq!(
                editor.runtime_mut().eval_str("eseq.seq-core-state/selected-bus").unwrap(),
                Some(Value::Number(2.0)),
                "FX must belong to the group's backing bus",
            );
            assert_eq!(
                editor.runtime_mut().eval_str("(seq-active-delete-target-kind)").unwrap(),
                Some(Value::Bool(false)),
                "double click must clear deletion arming for {buffer}, rack={rack}",
            );
        }
    }
}

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
