use super::*;

fn eval(editor: &mut Editor, source: &str) -> Value {
    editor.runtime_mut().eval_str(source).unwrap().unwrap_or(Value::Nil)
}

fn eval_number(editor: &mut Editor, source: &str) -> f64 {
    match eval(editor, source) {
        Value::Number(value) => value,
        other => panic!("{source} = {other:?}"),
    }
}

/// Module keys end with the widget key; the waveform takes its subtree's
/// print-numbered key, so that one matches by prefix.
fn find_layout_node_by_stable_key_prefix_or_suffix<'a>(
    node: &'a eseqlisp::layout::LayoutNode, key: &str,
) -> Option<&'a eseqlisp::layout::LayoutNode> {
    if node.stable_key.as_deref().is_some_and(|k| k.ends_with(key) || k.starts_with(key)) {
        return Some(node);
    }
    node.children.iter().find_map(|child| find_layout_node_by_stable_key_prefix_or_suffix(child, key))
}

#[test]
fn resample_command_prints_the_ring_and_sends_the_crop_with_name_and_tags() {
    let mut editor = full_grid_editor_for_scroll_tests();
    // The shared tag entry autocompletes through these (the app registers them).
    crate::sample_import_ui::register_sample_import_natives(editor.runtime_mut());
    let state = Arc::new(SequencerState::new(1, vec![]));
    let mut app = test_app_for_track_visual_state(state);
    // 1 kHz ring: 1 s of silence, 2 s of tone, 1 s of silence.
    let recorder = sequencer::recorder::MasterRecorder::with_history(
        1000, 2, sequencer::recorder::RESAMPLE_WINDOW);
    let mut output = vec![0.0f32; 2 * 4000];
    for frame in 1000..3000 {
        let value = (frame as f32 * 0.3).sin() * 0.5;
        output[frame * 2] = value;
        output[frame * 2 + 1] = value;
    }
    recorder.capture(&output);
    app.master_recorder = Arc::new(recorder);

    eval(&mut editor, "(set-layout (list :buf \"*sequencer*\" :hide-status true))");
    let id = editor.buffers.iter().find(|b| b.name == "*sequencer*").unwrap().id;
    editor.set_active_buffer(id);
    editor.drain_host_commands();
    eval(&mut editor, "(capture-resample)");
    assert!(editor.drain_host_commands().iter().any(|command| matches!(command,
        HostCommand::Custom { name, .. } if name == "resample-open")), "M-x command");
    crate::host_commands::resample::open(&app, &mut editor).unwrap();

    assert_eq!(eval(&mut editor, "eseq.resample/open?"), Value::Bool(true));
    assert_eq!(eval_number(&mut editor, "RESAMPLE.duration"), 4.0);
    // The crop opens on the audible span, not the silence around it.
    assert!((eval_number(&mut editor, "eseq.resample/crop-start") - 1.0).abs() < 0.002);
    assert!((eval_number(&mut editor, "eseq.resample/crop-end") - 3.0).abs() < 0.002);
    assert!(matches!(eval(&mut editor, "eseq.resample/name"),
        Value::String(name) if name.starts_with("Resample 20")));
    assert_eq!(eval_number(&mut editor, "(len eseq.resample/tags)"), 1.0);
    assert_eq!(eval(&mut editor, "(first eseq.resample/tags)"), Value::String("Resampled".into()));

    // The modal lays out with the waveform, crop pickers and the Add button.
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 160, 60);
    let layout = editor.widget_layout().unwrap();
    for key in ["/resample-wave-container", "resample-wave-resample://", "/resample-start", "/resample-end", "/resample-name", "/resample-commit"] {
        let node = find_layout_node_by_stable_key_prefix_or_suffix(&layout, key).unwrap_or_else(|| panic!("{key}"));
        assert_finite_nonzero_rect(node, key);
    }

    // Handles crop; duplicate tags (any case) are ignored; a pending typed
    // tag is included on Add.
    eval(&mut editor, "(eseq.resample/action (dict :type :set-selection :start 1.5 :end 2.25))");
    eval(&mut editor, "(eseq.resample/add-tag \"resampled\")");
    eval(&mut editor, "(eseq.resample/add-tag \"loop\")");
    eval(&mut editor, "(set! eseq.resample/name \"Big chord\")");
    eval(&mut editor, "(set! eseq.resample/tag-draft \"dusty\")");
    editor.drain_host_commands();
    eval(&mut editor, "(eseq.resample/commit)");
    let payload = editor.drain_host_commands().into_iter().find_map(|command| match command {
        HostCommand::Custom { name, payload } if name == "resample-commit" => Some(payload),
        _ => None,
    }).expect("commit command");
    let Value::Map(map) = payload else { panic!("payload") };
    let get = |key: &str| map.get(key).unwrap().borrow().clone();
    assert_eq!(get("start"), Value::Number(1.5));
    assert_eq!(get("end"), Value::Number(2.25));
    assert_eq!(get("name"), Value::String("Big chord".into()));
    let Value::List(tags) = get("tags") else { panic!("tags") };
    let tags: Vec<_> = tags.iter().map(|tag| tag.borrow().clone()).collect();
    assert_eq!(tags, ["Resampled", "loop", "dusty"].map(|t| Value::String(t.into())));

    crate::host_commands::resample::close(&mut editor).unwrap();
    assert_eq!(eval(&mut editor, "eseq.resample/open?"), Value::Bool(false));
    eseqlisp::widget_render::clear_overlay();
}

#[test]
fn resample_without_a_ring_reports_instead_of_opening() {
    let mut editor = full_grid_editor_for_scroll_tests();
    let app = test_app_for_track_visual_state(Arc::new(SequencerState::new(1, vec![])));
    assert!(crate::host_commands::resample::open(&app, &mut editor).is_err());
    assert_eq!(eval(&mut editor, "eseq.resample/open?"), Value::Bool(false));
}
