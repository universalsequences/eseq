use super::*;

fn paint_modal(layout: &eseqlisp::layout::LayoutNode) {
    eseqlisp::widget_render::clear_overlay();
    let _ = eseqlisp::widget_render::collect_gpu_primitives(layout,
        eseqlisp::widget_render::WidgetViewport {
            cell_w: 8.0, cell_h: 16.0, vp_w: 1280.0, vp_h: 960.0,
            time_seconds: 0.0, focused_widget_id: None, focused_branch: false,
            overlay_viewport_bottom: 60.0, scroll_top: 0.0, scroll_left: 0.0,
            inherited_hover: false,
        }, 0.0, 60);
}

#[test]
fn retrospective_command_opens_real_capture_and_cancel_keeps_patterns() {
    let mut editor = full_grid_editor_for_scroll_tests();
    let state = Arc::new(SequencerState::new(1, vec![]));
    state.replace_pattern_repository(vec![sequencer::sequencer::PatternSnapshot::new_default(1, &[])], 0);
    let mut app = test_app_for_track_visual_state(state.clone());
    let now = std::time::Instant::now();
    app.retrospective.note_on(1, app.track_registry.id_at(0).unwrap(), 0.0, 0.7, now);
    app.retrospective.note_off(1, now);
    editor.drain_host_commands();
    editor.runtime_mut().eval_str("(capture-midi)").unwrap();
    let command = editor.drain_host_commands().into_iter().find(|command| matches!(command,
        HostCommand::Custom { name, .. } if name == "retrospective-open")).expect("M-x command");
    let HostCommand::Custom { name, payload } = command else { unreachable!() };
    crate::retrospective::handle(&name, payload, &mut app, &mut editor);
    assert_eq!(app.retrospective.draft.as_ref().unwrap().notes.len(), 1);
    assert_eq!(editor.runtime_mut().eval_str("(len RETRO.items)").unwrap(), Some(Value::Number(1.0)));
    assert_eq!(editor.runtime_mut().eval_str("eseq.retrospective/open?").unwrap(), Some(Value::Bool(true)));
    let draft = app.retrospective.draft.as_mut().unwrap();
    draft.duration = 2.0;
    draft.notes[0].start = 0.0;
    draft.notes[0].end = 0.1;
    let crop = editor.runtime_mut().eval_str("(dict :start 0 :end 2 :bars 1)").unwrap().unwrap();
    crate::retrospective::handle("retrospective-audition", crop, &mut app, &mut editor);
    assert_ne!(app.state.note_audition.generation(), 0);
    crate::retrospective::handle("retrospective-close", Value::Nil, &mut app, &mut editor);
    assert!(app.retrospective.draft.is_none());
    assert_eq!(app.state.note_audition.generation(), 0);
    assert_eq!(editor.runtime_mut().eval_str("eseq.retrospective/open?").unwrap(), Some(Value::Bool(false)));
    assert!(!state.pattern.patterns[0].is_active(0));
}

#[test]
fn retrospective_modal_recalculates_usable_bars_when_cropping() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor.runtime_mut().eval_str(include_str!("../../../ui/capture-fixtures/retrospective-preview.lisp")).unwrap();
    editor.runtime_mut().eval_str(
        "(set-layout (list :buf \"*sequencer*\" :hide-status true)) (eseq.retrospective/open 0 30)").unwrap();
    let id = editor.buffers.iter().find(|b| b.name == "*sequencer*").unwrap().id;
    editor.set_active_buffer(id);
    assert_eq!(editor.runtime_mut().eval_str("eseq.retrospective/bars").unwrap(), Some(Value::Number(16.0)));
    // Crop adjustments recompute from the requested meter; an explicit bar
    // choice persists across cropping, while automatic doubling does not stick.
    for (key, value, expected) in [
        ("/retrospective-end", 2.0, 1.0),
        ("/retrospective-end", 5.0, 2.0),
        ("/retrospective-start", 1.0, 2.0),
        ("/retrospective-start", 3.0, 1.0),
        ("/retrospective-bars", 3.0, 3.0),
        ("/retrospective-end", 30.0, 12.0),
        ("/retrospective-end", 5.0, 3.0),
    ] {
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 160, 60);
        let layout = editor.widget_layout().unwrap();
        let picker = find_layout_node_by_stable_key_suffix(&layout, key).unwrap();
        assert_finite_nonzero_rect(picker, key);
        editor.runtime_mut().invoke(picker.props["on-change"].clone(), vec![Value::Number(value)]).unwrap();
        assert_eq!(editor.runtime_mut().eval_str("eseq.retrospective/bars").unwrap(), Some(Value::Number(expected)), "{key} = {value}");
    }
    editor.runtime_mut().eval_str("(eseq.retrospective/open 20 24)").unwrap();
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 160, 60);
    let layout = editor.widget_layout().unwrap();
    let bars = find_layout_node_by_stable_key_suffix(&layout, "/retrospective-bars").unwrap();
    assert_eq!(bars.props.get("value"), Some(&Value::Number(2.0)));
    eseqlisp::widget_render::clear_overlay();
}

#[test]
fn retrospective_modal_crops_unsnapped_time_and_sends_one_import_command() {
    let mut editor = full_grid_editor_for_scroll_tests();
    crate::natives::register_transport_toggle_play_native(editor.runtime_mut(),
        Arc::new(SequencerState::new(1, vec![])));
    editor.runtime_mut().eval_str(include_str!("../../../ui/capture-fixtures/retrospective-preview.lisp"))
        .expect("seed the capture view");
    for buffer in ["*sequencer*", "*arrangement*"] {
        editor.runtime_mut().eval_str(&format!(
            "(set-layout (list :buf \"{buffer}\" :hide-status true))")).unwrap();
        let id = editor.buffers.iter().find(|b| b.name == buffer).unwrap().id;
        editor.set_active_buffer(id);
        editor.runtime_mut().eval_str("(eseq.retrospective/open 20.0 24.0)").unwrap();
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let frame = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 160, 60);
        let tile = frame.tiles.iter().find(|tile| tile.frame.buffer_name == buffer).unwrap();
        let offset = (tile.rect.col, tile.rect.row);
        let layout = editor.widget_layout().unwrap();
        assert_finite_layout_tree(&layout);
        for key in ["/retrospective-roll", "/retrospective-start", "/retrospective-end",
            "/retrospective-bars", "/retrospective-send", "/retrospective-audition"] {
            let node = find_layout_node_by_stable_key_suffix(&layout, key)
                .unwrap_or_else(|| panic!("missing {key} in {buffer}"));
            assert_finite_nonzero_rect(node, key);
            assert!(node.rect.col >= 0.0 && node.rect.row >= 0.0);
            assert!(node.rect.col + node.rect.width <= 160.0);
            assert!(node.rect.row + node.rect.height <= 60.0);
        }
        let roll = find_layout_node_by_stable_key_suffix(&layout, "/retrospective-roll").unwrap();
        editor.runtime_mut().take_status_message();
        paint_modal(&layout);
        assert_eq!(roll.props.get("snap"), Some(&Value::Number(0.0)));
        let col_at = |time: f32| offset.0 + roll.rect.col + 14.0 + (roll.rect.width - 14.0) * time / 30.0;
        let row = offset.1 + roll.rect.row + 2.2;
        for (kind, col) in [
            (crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left), col_at(20.1)),
            (crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left), col_at(23.7)),
            (crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left), col_at(23.7)),
        ] {
            editor.handle_tiled_mouse_precise(crossterm::event::MouseEvent {
                kind, column: col as u16, row: row as u16,
                modifiers: crossterm::event::KeyModifiers::NONE,
            }, col, row, 0);
        }
        let Some(Value::Number(start)) = editor.runtime_mut().eval_str("eseq.retrospective/crop-start").unwrap()
            else { panic!("crop start"); };
        assert!(start > 20.0 && start < 21.0 && (start - start.round()).abs() > 0.001,
            "pointer drag must produce an unsnapped crop in {buffer}: {start}");
        editor.runtime_mut().eval_str(
            "(eseq.retrospective/action (dict :type :finish-marquee-select :time-a 20.123 :time-b 23.789))").unwrap();
        assert_eq!(editor.runtime_mut().eval_str("eseq.retrospective/crop-start").unwrap(), Some(Value::Number(20.123)));
        assert_eq!(editor.runtime_mut().eval_str("eseq.retrospective/crop-end").unwrap(), Some(Value::Number(23.789)));
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 160, 60);
        let layout = editor.widget_layout().unwrap();
        paint_modal(&layout);
        let audition = find_layout_node_by_stable_key_suffix(&layout, "/retrospective-audition").unwrap();
        let col = offset.0 + audition.rect.col + audition.rect.width * 0.5;
        let row = offset.1 + audition.rect.row + audition.rect.height * 0.5;
        editor.drain_host_commands();
        for kind in [crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left)] {
            editor.handle_tiled_mouse_precise(crossterm::event::MouseEvent {
                kind, column: col as u16, row: row as u16,
                modifiers: crossterm::event::KeyModifiers::NONE,
            }, col, row, 0);
        }
        let auditions: Vec<_> = editor.drain_host_commands().into_iter().filter_map(|command| match command {
            HostCommand::Custom { name, payload: Value::Map(map) } if name == "retrospective-audition" => Some(map),
            _ => None,
        }).collect();
        assert_eq!(auditions.len(), 1);
        assert_eq!(*auditions[0]["bars"].borrow(), Value::Number(2.0));
        editor.runtime_mut().set_reactive("RETRO", "playing", Value::Bool(true));
        editor.runtime_mut().set_reactive("RETRO", "position", Value::Number(0.25));
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 160, 60);
        let layout = editor.widget_layout().unwrap();
        let roll = find_layout_node_by_stable_key_suffix(&layout, "/retrospective-roll").unwrap();
        let Some(Value::Number(playhead)) = roll.props.get("playhead-time") else { panic!("preview playhead"); };
        assert!((*playhead - (20.123 + (23.789 - 20.123) * 0.25)).abs() < 1e-6, "preview playhead in {buffer}: {playhead}");
        editor.runtime_mut().eval_str(
            "(eseq.retrospective/action (dict :type :finish-marquee-select :time-a 20.123 :time-b 23.789))").unwrap();
        assert_eq!(editor.drain_host_commands().into_iter().filter(|command| matches!(command,
            HostCommand::Custom { name, .. } if name == "retrospective-stop")).count(), 1);
        // The host owns playback state and publishes the stopped state.
        editor.runtime_mut().set_reactive("RETRO", "playing", Value::Bool(false));
        // Scroll events may carry only one axis; zoom carries anchor + factor.
        editor.runtime_mut().eval_str(
            "(eseq.retrospective/action (dict :type :scroll-view :lane-scroll 1))").unwrap();
        editor.runtime_mut().eval_str(
            "(eseq.retrospective/action (dict :type :zoom-view :anchor-time 22 :factor 2))").unwrap();
        // Stop uses the normal transport state machine and keeps the crop.
        editor.runtime_mut().set_reactive("SEQ", "playing", Value::Bool(true));
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 160, 60);
        let layout = editor.widget_layout().unwrap();
        paint_modal(&layout);
        let stop = find_layout_node_by_stable_key_suffix(&layout, "/retrospective-stop").unwrap();
        assert_finite_nonzero_rect(stop, "stop playback");
        let col = offset.0 + stop.rect.col + stop.rect.width * 0.5;
        let row = offset.1 + stop.rect.row + stop.rect.height * 0.5;
        editor.drain_host_commands();
        for kind in [crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left)] {
            editor.handle_tiled_mouse_precise(crossterm::event::MouseEvent {
                kind, column: col as u16, row: row as u16,
                modifiers: crossterm::event::KeyModifiers::NONE,
            }, col, row, 0);
        }
        let stops = editor.drain_host_commands().into_iter().filter(|command| matches!(command,
            HostCommand::Custom { name, .. } if name == "song-transport-toggle-play")).count();
        assert_eq!(stops, 1);
        editor.runtime_mut().set_reactive("SEQ", "playing", Value::Bool(false));
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 160, 60);
        let layout = editor.widget_layout().unwrap();
        paint_modal(&layout);
        let send = find_layout_node_by_stable_key_suffix(&layout, "/retrospective-send").unwrap();
        let col = offset.0 + send.rect.col + send.rect.width * 0.5;
        let row = offset.1 + send.rect.row + send.rect.height * 0.5;
        editor.drain_host_commands();
        for kind in [crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left)] {
            editor.handle_tiled_mouse_precise(crossterm::event::MouseEvent {
                kind, column: col as u16, row: row as u16,
                modifiers: crossterm::event::KeyModifiers::NONE,
            }, col, row, 0);
        }
        let commands = editor.drain_host_commands();
        let imports: Vec<_> = commands.iter().filter_map(|command| match command {
            HostCommand::Custom { name, payload: Value::Map(map) } if name == "retrospective-import" => Some(map),
            _ => None,
        }).collect();
        assert_eq!(imports.len(), 1);
        assert_eq!(*imports[0]["start"].borrow(), Value::Number(20.123));
        assert_eq!(*imports[0]["end"].borrow(), Value::Number(23.789));
        assert_eq!(*imports[0]["bars"].borrow(), Value::Number(2.0));
        eseqlisp::widget_render::clear_overlay();
        editor.runtime_mut().eval_str("(eseq.retrospective/close)").unwrap();
    }
}
