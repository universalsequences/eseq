use super::*;

fn eval(editor: &mut Editor, source: &str) -> Value {
    editor.runtime_mut().eval_str(source).unwrap().unwrap_or(Value::Nil)
}

/// A field of the modal's state (`eseq.sample-region/region-view`).
fn view(field: &str) -> String {
    format!("(let ((v eseq.sample-region/region-view)) v.{field})")
}

fn find_layout_node_by_stable_key_suffix<'a>(
    node: &'a eseqlisp::layout::LayoutNode, key: &str,
) -> Option<&'a eseqlisp::layout::LayoutNode> {
    if node.stable_key.as_deref().is_some_and(|k| k.ends_with(key)) {
        return Some(node);
    }
    node.children.iter().find_map(|child| find_layout_node_by_stable_key_suffix(child, key))
}

#[test]
fn save_region_menu_opens_the_modal_on_the_samplers_range_and_sends_name_and_tags() {
    let mut editor = full_grid_editor_for_scroll_tests();
    // The shared tag entry autocompletes through these (the app registers them).
    crate::sample_import_ui::register_sample_import_natives(editor.runtime_mut());
    editor.runtime_mut().register_native("seq-sample-packs", |_args, _ctx| {
        Ok(test_list(["training set 1", "fills"].map(|name| map_value([
            ("name", Value::String(name.into())),
            ("member", Value::Bool(false)),
        ])).to_vec()))
    });
    let mut app = test_app_for_track_visual_state(Arc::new(SequencerState::new(1, vec![])));
    let dir = std::env::temp_dir().join(format!("eseq-sample-region-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let parent = dir.join("Think Break.wav");
    std::fs::write(&parent, b"not decoded until Save").unwrap();
    app.sampler_paths = vec![Some(parent.clone())];

    eval(&mut editor, "(set-layout (list :buf \"*sequencer*\" :hide-status true))");
    let id = editor.buffers.iter().find(|b| b.name == "*sequencer*").unwrap().id;
    editor.set_active_buffer(id);

    // The waveform's menu item sends the sampler and range it was opened on.
    editor.drain_host_commands();
    eval(&mut editor, "(let ((m eseq.effects.sampler-panel/sampler-region-menu))
                         (set! m.target (dict :track 0 :rack-slot -1 :start 0.25 :end 0.5)))");
    let menu = eval(&mut editor, "(eseq.effects.sampler-panel/sampler-region-context-menu)");
    assert!(!matches!(menu, Value::Nil), "the menu builds");
    let payload = eval(&mut editor, "(let ((m eseq.effects.sampler-panel/sampler-region-menu)) m.target)");
    crate::host_commands::sample_region::handle_for_test("sample-region-open", payload, &mut app, &mut editor);

    assert_eq!(eval(&mut editor, &view("open")), Value::Bool(true));
    assert_eq!(eval(&mut editor, &view("parent")), Value::String("Think Break".into()));
    assert_eq!(eval(&mut editor, &view("span")), Value::String("250–500 ms".into()));
    assert_eq!(eval(&mut editor, &view("name")), Value::String("Think Break 250–500 ms".into()));

    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 160, 60);
    let layout = editor.widget_layout().unwrap();
    for key in ["/sample-region-from", "/sample-region-name", "/sample-region-pack",
                "/sample-region-pack-suggest-fills", "/sample-region-commit"] {
        let node = find_layout_node_by_stable_key_suffix(&layout, key).unwrap_or_else(|| panic!("{key}"));
        assert_finite_nonzero_rect(node, key);
    }
    // Save sits below the scrolling fields, not inside them: however tall
    // tags and suggestions grow the form, the buttons stay in view.
    let fields = find_layout_node_by_stable_key_suffix(&layout, "/sample-region-fields").expect("fields");
    let save = find_layout_node_by_stable_key_suffix(&layout, "/sample-region-commit").unwrap();
    assert!(save.rect.row >= fields.rect.row + fields.rect.height,
        "Save Region {:?} below the fields {:?}", save.rect, fields.rect);

    // Existing packs are offered as you type; a chip fills the field.
    eval(&mut editor, "(let ((v eseq.sample-region/region-view)) (set! v.pack \"Tr\"))");
    assert_eq!(eval(&mut editor, "(str (eseq.sample-region/pack-suggestions))"),
               eval(&mut editor, "(str (list \"training set 1\"))"));
    eval(&mut editor, "(let ((v eseq.sample-region/region-view)) (set! v.pack \"\"))");

    // A pending typed tag is included on Save; duplicates are ignored.
    eval(&mut editor, "(eseq.sample-region/add-tag \"snare\")");
    eval(&mut editor, "(eseq.sample-region/add-tag \"Snare\")");
    eval(&mut editor, "(let ((v eseq.sample-region/region-view)) (set! v.tag-draft \"lyn collins\"))");
    editor.drain_host_commands();
    eval(&mut editor, "(eseq.sample-region/commit)");
    let payload = editor.drain_host_commands().into_iter().find_map(|command| match command {
        HostCommand::Custom { name, payload } if name == "sample-region-commit" => Some(payload),
        _ => None,
    }).expect("commit command");
    let Value::Map(map) = payload else { panic!("payload") };
    // No pack was used yet, so the field starts empty and sends "".
    assert_eq!(map.get("pack").unwrap().borrow().clone(), Value::String(String::new()));
    let Value::List(tags) = map.get("tags").unwrap().borrow().clone() else { panic!("tags") };
    let tags: Vec<_> = tags.iter().map(|tag| tag.borrow().clone()).collect();
    assert_eq!(tags, ["snare", "lyn collins"].map(|t| Value::String(t.into())));

    crate::host_commands::sample_region::handle_for_test(
        "sample-region-close", Value::Nil, &mut app, &mut editor);
    assert_eq!(eval(&mut editor, &view("open")), Value::Bool(false));
    eseqlisp::widget_render::clear_overlay();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn save_region_without_a_range_reports_instead_of_opening() {
    let mut editor = full_grid_editor_for_scroll_tests();
    let mut app = test_app_for_track_visual_state(Arc::new(SequencerState::new(1, vec![])));
    let payload = eval(&mut editor, "(dict :track 0 :rack-slot -1 :start 0.3 :end 0.3)");
    crate::host_commands::sample_region::handle_for_test("sample-region-open", payload, &mut app, &mut editor);
    assert_eq!(eval(&mut editor, &view("open")), Value::Bool(false));
}
