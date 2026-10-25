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
    eval(editor, "(eseq.modal.save/commit)");
    let payload = editor
        .drain_host_commands()
        .into_iter()
        .find_map(|command| match command {
            HostCommand::Custom { name, payload } if name == "modal-draft-save" => Some(payload),
            _ => None,
        })
        .expect("save command");
    let Value::Map(map) = payload else { panic!("payload") };
    map.iter().map(|(key, cell)| (key.clone(), cell.borrow().clone())).collect()
}

/// A field of the modal's own state (`eseq.modal.save/save-form`).
fn form(field: &str) -> String {
    format!("(let ((f eseq.modal.save/save-form)) f.{field})")
}

/// The host opens the modal on the patch's :name; it commits the typed name
/// for the draft's track and turns Save into Replace once the host reports
/// that name as already saved.
#[test]
fn modal_draft_save_modal_prefills_the_patch_name_and_commits_it() {
    let mut editor = full_grid_editor_for_scroll_tests();
    eval(&mut editor, "(set-layout (list :buf \"*sequencer*\" :hide-status true))");
    let id = editor.buffers.iter().find(|b| b.name == "*sequencer*").unwrap().id;
    editor.set_active_buffer(id);

    eval(&mut editor, "(eseq.modal.save/open 3 \"modal-banjo\")");
    assert_eq!(eval(&mut editor, &form("open")), Value::Bool(true));
    assert_eq!(eval(&mut editor, &form("name")), Value::String("modal-banjo".into()));

    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let _ = eseqlisp::frame::build_tiled_render_frame_borderless(&mut editor, 160, 60);
    let layout = editor.widget_layout().unwrap();
    for key in ["/name", "/commit", "/cancel"] {
        let node = find_by_key_suffix(&layout, key).unwrap_or_else(|| panic!("{key}"));
        assert_finite_nonzero_rect(node, key);
    }

    let payload = commit_payload(&mut editor);
    assert_eq!(payload["track"], Value::Number(3.0));
    assert_eq!(payload["name"], Value::String("modal-banjo".into()));
    assert_eq!(payload["overwrite"], Value::Bool(false));

    // The host found it saved: the next Save replaces, a new name does not.
    eval(&mut editor, "(eseq.modal.save/fail \"Instrument 'modal-banjo' already exists\" \"modal-banjo\")");
    assert_eq!(commit_payload(&mut editor)["overwrite"], Value::Bool(true));
    eval(&mut editor, "(let ((f eseq.modal.save/save-form)) (set! f.name \"banjo 2\"))");
    assert_eq!(commit_payload(&mut editor)["overwrite"], Value::Bool(false));

    // Reopening starts clean.
    eval(&mut editor, "(eseq.modal.save/open 3 \"modal-banjo\")");
    assert_eq!(eval(&mut editor, &form("error")), Value::String(String::new()));
    assert_eq!(eval(&mut editor, &form("taken")), Value::String(String::new()));

    eval(&mut editor, "(eseq.modal.save/close)");
    assert_eq!(eval(&mut editor, &form("open")), Value::Bool(false));
    eseqlisp::widget_render::clear_overlay();
}
