use super::*;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

#[test]
fn track_menu_ungroups_only_the_clicked_group_or_rack_member() {
    let mut editor = full_grid_editor_for_scroll_tests();
    set_full_grid_track_count(&mut editor, 3, 16);
    let mixer_id = editor.buffers.iter().find(|b| b.name == "*mixer*").unwrap().id;
    editor.set_active_buffer(mixer_id);
    editor.set_layout_viewport(140, 30);

    for rack in [false, true] {
        let mut group = rack_group_fixture(false);
        if !rack {
            group.rack = None;
        }
        apply_group_bindings(&mut editor, group);
        for collapsed in [false, true] {
            editor.runtime_mut().set_reactive(
                "SEQ", "track-collapsed", test_bool_list(&[collapsed; 3]),
            );
            for selected in [&[0.0][..], &[1.0, 2.0][..]] {
                editor.runtime_mut().set_reactive(
                    "SEQ", "selected-tracks", test_number_list(selected),
                );
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                for track in [0, 1] {
                    let layout = editor.widget_layout().expect("mixer layout");
                    let strip = find_layout_node_by_stable_key(
                        &layout, &format!("mixer-v2-track-{track}"),
                    ).expect("visible track strip");
                    assert_finite_nonzero_rect(strip, "right-click track strip");
                    editor.runtime_mut().invoke(
                        strip.props["on-right-click"].clone(),
                        vec![map_value([
                            ("col", Value::Number(30.0)),
                            ("row", Value::Number(8.0)),
                        ])],
                    ).expect("open clicked track menu");
                    editor.refresh_runtime_side_effects();
                    let layout = editor.widget_layout().expect("track menu layout");
                    let ungroup = find_layout_node_by_stable_key_suffix(
                        &layout, "/track-menu-:ungroup-track",
                    );
                    if track == 0 {
                        assert!(ungroup.is_none(), "ungroup is absent for a loose track");
                        editor.runtime_mut().eval_str(
                            "(set! eseq.mixer/track-menu-open false)",
                        ).unwrap();
                    } else {
                        let ungroup = ungroup.expect("member track offers ungroup");
                        assert_finite_nonzero_rect(ungroup, "ungroup track menu item");
                        assert!(ungroup.rect.col >= 0.0 && ungroup.rect.row >= 0.0);
                        assert!(ungroup.rect.col + ungroup.rect.width <= 140.0);
                        assert!(ungroup.rect.row + ungroup.rect.height <= 30.0);
                        editor.drain_host_commands();
                        editor.runtime_mut().invoke(
                            ungroup.props["on-select"].clone(), vec![Value::Nil],
                        ).expect("select ungroup track");
                        let commands = editor.drain_host_commands();
                        assert_eq!(commands.len(), 1);
                        let eseqlisp::host::HostCommand::Custom { name, payload } = &commands[0]
                        else { panic!("expected ungroup track host command: {commands:?}") };
                        assert_eq!(name, "remove-track-from-group");
                        assert_eq!(payload, &map_value([("track", Value::Number(1.0))]));
                        assert_eq!(
                            editor.runtime_mut().eval_str("eseq.mixer/track-menu-open").unwrap(),
                            Some(Value::Bool(false)),
                        );
                    }
                    editor.refresh_runtime_side_effects();
                }
            }
        }
    }
}

#[test]
fn rack_clip_scroll_owns_vertical_gestures_across_the_visible_list() {
    use eseqlisp::widget_render::scroll::{get_scroll_state, scroll_state_key, set_scroll_state};

    let mut editor = full_grid_editor_for_scroll_tests();
    set_full_grid_track_count(&mut editor, 12, 16);
    let mut group = rack_group_fixture(true);
    group.members = vec![9, 10];
    apply_group_bindings(&mut editor, group);
    let clips = test_list((1..=20).map(|id| map_value([
        ("id", Value::Number(id as f64)),
        ("name", Value::String(format!("Clip {id}"))),
    ])).collect());
    for field in ["rack-clips", "rack-clip-banks"] {
        editor.runtime_mut().set_reactive("SEQ", field, test_list(vec![map_value([
            ("group-id", Value::Number(7.0)),
            ("active", Value::Number(1.0)),
            ("clips", clips.clone()),
        ])]));
    }
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    assert!(editor.switch_active_tile_to_buffer_named("*mixer*"));
    let frame = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 70);
    let tile = frame.tiles.iter().find(|tile| tile.frame.buffer_name == "*mixer*").unwrap();
    let scroll = find_layout_node_by_stable_key_suffix(
        tile.frame.widget_layout.as_deref().unwrap(), "/rack-clip-scroll-7",
    ).unwrap();
    assert_finite_nonzero_rect(scroll, "rack clip viewport");
    let rect = scroll.rect;
    let key = scroll_state_key(scroll);
    let state = get_scroll_state(key);
    assert!(state.content_height > state.viewport_height);

    // Put this later rack inside the viewport using the same horizontal
    // panning as the app, then test the full painted list, including its rim.
    editor.apply_smooth_widget_scroll(-(rect.col - 10.0), 0.0);
    let frame = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 180, 70);
    let tile = frame.tiles.iter().find(|tile| tile.frame.buffer_name == "*mixer*").unwrap();
    let scroll_left = tile.frame.widget_layout_scroll_left;
    assert!(scroll_left > 0.0);
    let (cell_w, cell_h) = editor.runtime().layout_cell_dims();
    let border = eseqlisp::widget_render::ui_design_px(tile.border_width_px);
    let origin = (
        tile.rect.col + border / cell_w + rect.col - scroll_left,
        tile.rect.row + border / cell_h + rect.row - tile.frame.widget_scroll_top,
    );
    for x_fraction in [0.001, 0.2, 0.5, 0.8, 0.999] {
        for y_fraction in [0.001, 0.2, 0.5, 0.8, 0.999] {
            let at = (origin.0 + rect.width * x_fraction, origin.1 + rect.height * y_fraction);
            assert!(at.0 >= tile.rect.col && at.0 < tile.rect.col + tile.rect.width);
            assert!(at.1 >= tile.rect.row && at.1 < tile.rect.row + tile.rect.height);
            set_scroll_state(key, state.clone());
            let handled = editor.handle_tiled_touchpad_scroll(at.0, at.1, 0, -2.0, -20.0);
            if !handled {
                editor.apply_smooth_widget_scroll(-0.1, -1.0);
            }
            assert!(handled, "clip list missed vertical scroll at ({x_fraction}, {y_fraction})");
            assert!(get_scroll_state(key).offset_y > state.offset_y,
                "clip list did not move at ({x_fraction}, {y_fraction})");
            assert_eq!(editor.widget_scroll_left(), scroll_left, "vertical gesture moved mixer");
        }
    }

    // Gaps between clickable rows are still part of the clip viewport.
    let layout = tile.frame.widget_layout.as_deref().unwrap();
    let first = find_layout_node_by_stable_key_suffix(layout, "/mixer-rack-clip-7-1").unwrap();
    let second = find_layout_node_by_stable_key_suffix(layout, "/mixer-rack-clip-7-2").unwrap();
    let gap_row = (first.rect.row + first.rect.height + second.rect.row) * 0.5;
    let at = (origin.0 + rect.width * 0.5, origin.1 + gap_row - rect.row);
    set_scroll_state(key, state.clone());
    assert!(editor.handle_tiled_touchpad_scroll(at.0, at.1, 0, -2.0, -20.0),
        "the row gap must scroll the list");
    assert!(get_scroll_state(key).offset_y > state.offset_y);

    let at = (origin.0 + rect.width * 0.5, origin.1 + rect.height * 0.5);
    for offset in [0.0, (state.content_height - state.viewport_height) * 0.5,
        state.content_height - state.viewport_height]
    {
        let mut positioned = state.clone();
        positioned.offset_y = offset;
        set_scroll_state(key, positioned);
        for delta_y in [-20.0, 20.0] {
            assert!(editor.handle_tiled_touchpad_scroll(at.0, at.1, 0, -2.0, delta_y),
                "vertical gestures stay with the list at its limits too");
            assert_eq!(editor.widget_scroll_left(), scroll_left);
        }
    }

    let before = get_scroll_state(key).offset_y;
    assert!(!editor.handle_tiled_touchpad_scroll(at.0, at.1, 0, 20.0, -2.0),
        "intentional sideways gestures must still reach the mixer");
    editor.apply_smooth_widget_scroll(1.0, -0.1);
    assert!(editor.widget_scroll_left() < scroll_left);
    assert_eq!(get_scroll_state(key).offset_y, before);
}

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
