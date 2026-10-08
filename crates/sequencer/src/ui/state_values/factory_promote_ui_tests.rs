use super::*;

fn eval(editor: &mut Editor, source: &str) -> Value {
    editor.runtime_mut().eval_str(source).unwrap().unwrap_or(Value::Nil)
}

fn find_by_key_suffix<'a>(
    node: &'a eseqlisp::layout::LayoutNode,
    key: &str,
) -> Option<&'a eseqlisp::layout::LayoutNode> {
    if node.stable_key.as_deref().is_some_and(|k| k.ends_with(key)) {
        return Some(node);
    }
    node.children.iter().find_map(|child| find_by_key_suffix(child, key))
}

fn commit_payload(editor: &mut Editor) -> std::collections::HashMap<String, Value> {
    editor.drain_host_commands();
    eval(editor, "(eseq.factory-promote/commit)");
    let payload = editor
        .drain_host_commands()
        .into_iter()
        .find_map(|command| match command {
            HostCommand::Custom { name, payload } if name == "factory-promote-commit" => {
                Some(payload)
            }
            _ => None,
        })
        .expect("commit command");
    let Value::Map(map) = payload else { panic!("payload") };
    map.iter().map(|(key, cell)| (key.clone(), cell.borrow().clone())).collect()
}

/// A field of the modal's own state (`eseq.factory-promote/promote-form`).
fn form(field: &str) -> String {
    format!("(let ((f eseq.factory-promote/promote-form)) f.{field})")
}

/// The M-x commands open through Rust; the modal lists the skipped
/// dependencies, commits the typed name, and turns Promote into Replace once
/// Rust reports the name as taken.
#[test]
fn promote_modal_lists_skips_and_commits_the_name() {
    let mut editor = full_grid_editor_for_scroll_tests();
    eval(&mut editor, "(set-layout (list :buf \"*sequencer*\" :hide-status true))");
    let id = editor.buffers.iter().find(|b| b.name == "*sequencer*").unwrap().id;
    editor.set_active_buffer(id);

    for (command, kind) in [
        ("promote-sound-to-factory", "sound"),
        ("promote-kit-to-factory", "kit"),
        ("promote-preset-to-factory", "preset"),
    ] {
        editor.drain_host_commands();
        eval(&mut editor, &format!("({command})"));
        let sent = editor.drain_host_commands().into_iter().any(|c| matches!(&c,
            HostCommand::Custom { name, payload } if name == "factory-promote-open"
                && extract_string_from_payload(payload, "kind").as_deref() == Some(kind)));
        assert!(sent, "{command} sends factory-promote-open :kind {kind}");
    }

    // What the host kinds push on open (the presented promotion).
    set_kind_field(&mut editor, "factory-promote", "target", Value::String("kit".into()));
    set_kind_field(&mut editor, "factory-promote", "destination", Value::String("content/kits/".into()));
    set_kind_field(
        &mut editor,
        "factory-promote",
        "skipped",
        build_string_list(&["pad 'Kick': skipped slot 1 (instrument 'user:DOOM Kick' is not factory)".into()]),
    );
    eval(&mut editor, "(eseq.factory-promote/open \"Chicken Kit\")");
    assert_eq!(eval(&mut editor, &form("open")), Value::Bool(true));

    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 160, 60);
    let layout = editor.widget_layout().unwrap();
    for key in ["/name", "/skip-0", "/commit", "/cancel"] {
        let node = find_by_key_suffix(&layout, key).unwrap_or_else(|| panic!("{key}"));
        assert_finite_nonzero_rect(node, key);
    }

    eval(&mut editor, "(let ((f eseq.factory-promote/promote-form)) (set! f.name \"Chicken Kit 2\"))");
    let payload = commit_payload(&mut editor);
    assert_eq!(payload["name"], Value::String("Chicken Kit 2".into()));
    assert_eq!(payload["overwrite"], Value::Bool(false));

    // Rust reported the name taken: the next Promote replaces.
    set_kind_field(&mut editor, "factory-promote", "taken", Value::String("chicken kit 2".into()));
    assert_eq!(commit_payload(&mut editor)["overwrite"], Value::Bool(true));

    // A blocked promotion sends nothing.
    set_kind_field(&mut editor, "factory-promote", "blocking", Value::String("not factory".into()));
    editor.drain_host_commands();
    eval(&mut editor, "(eseq.factory-promote/commit)");
    assert!(editor.drain_host_commands().is_empty());

    eval(&mut editor, "(eseq.factory-promote/close)");
    assert_eq!(eval(&mut editor, &form("open")), Value::Bool(false));
    eseqlisp::widget_render::clear_overlay();
}
