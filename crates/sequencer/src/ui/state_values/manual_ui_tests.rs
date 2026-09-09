//! The `*manual*` buffer (content/ui/manual.lisp): renders docs/manual/
//! nodes through `parse-manual-page` and navigates by menu order.

use super::*;
use eseqlisp::layout::LayoutNode;

fn labels_containing<'a>(node: &'a LayoutNode, needle: &str, out: &mut Vec<&'a LayoutNode>) {
    if node.widget_type == "label"
        && matches!(node.props.get("text"), Some(Value::String(text)) if text.contains(needle))
    {
        out.push(node);
    }
    for child in &node.children {
        labels_containing(child, needle, out);
    }
}

fn label_texts(node: &LayoutNode, out: &mut Vec<String>) {
    if node.widget_type == "label" {
        if let Some(Value::String(text)) = node.props.get("text") {
            out.push(text.clone());
        }
    }
    for child in &node.children {
        label_texts(child, out);
    }
}

fn manual_layout(editor: &mut eseqlisp::Editor) -> std::sync::Arc<LayoutNode> {
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    if let Some(status) = editor.runtime_mut().take_status_message() {
        panic!("manual status after refresh: {status}");
    }
    let buffer_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*manual*")
        .expect("manual effect buffer")
        .id;
    editor.set_active_buffer(buffer_id);
    editor.set_layout_viewport(100, 40);
    let layout = editor.widget_layout().unwrap_or_else(|| {
        let tree = editor
            .buffers
            .iter()
            .find(|buffer| buffer.id == buffer_id)
            .and_then(|buffer| buffer.widget_tree.as_ref());
        panic!("manual layout; widget tree={tree:#?}")
    });
    assert_finite_layout_tree(&layout);
    layout
}

fn eval(editor: &mut eseqlisp::Editor, form: &str) -> Value {
    editor
        .runtime_mut()
        .eval_str(form)
        .unwrap_or_else(|error| panic!("{form}: {error:?}"))
        .unwrap_or(Value::Nil)
}

fn current_node(editor: &mut eseqlisp::Editor) -> String {
    match eval(editor, "eseq.manual/manual-node") {
        Value::String(node) => node,
        other => panic!("manual-node should be a string, got {other:?}"),
    }
}

#[test]
fn manual_opens_on_the_index_menu_and_remembers_the_source_buffer() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor.drain_host_commands();
    eval(&mut editor, "(switch-to-buffer \"*mixer*\")");
    editor.refresh_runtime_side_effects();
    eval(&mut editor, "(eseq.manual/open-manual)");
    eval(
        &mut editor,
        r#"(set-layout (list :buf "*manual*" :hide-status true :min-width 25))"#,
    );
    let layout = manual_layout(&mut editor);
    assert_eq!(current_node(&mut editor), "index");

    let nav = find_layout_node_by_debug_name(&layout, "manual-nav").expect("nav bar");
    assert_finite_nonzero_rect(nav, "manual nav bar");
    let menu = find_layout_node_by_debug_name(&layout, "manual-menu").expect("index menu");
    assert_finite_nonzero_rect(menu, "index menu");
    let mut texts = Vec::new();
    label_texts(menu, &mut texts);
    for expected in ["Sequencer Tour", "Mixer", "Customization"] {
        assert!(
            texts.iter().any(|text| text == expected),
            "index menu should list {expected}; labels={texts:?}"
        );
    }

    // Prose is flowed as per-word labels; a heading is one wrapped label.
    let mut words = Vec::new();
    labels_containing(&layout, "placeholder", &mut words);
    assert_eq!(
        words.len(),
        1,
        "the word 'placeholder' renders as one word label"
    );
    assert_eq!(
        words[0].props.get("text"),
        Some(&Value::String("placeholder".to_string()))
    );
    let mut title = Vec::new();
    labels_containing(&layout, "eseq Manual", &mut title);
    assert_eq!(title.len(), 1, "h1 renders as a single label");

    assert_eq!(
        eval(&mut editor, "eseq.manual/manual-source-buffer"),
        Value::String("*mixer*".to_string())
    );
    eval(&mut editor, "(eseq.manual/manual-quit)");
    editor.refresh_runtime_side_effects();
    assert_eq!(
        eval(&mut editor, "(current-buffer-name)"),
        Value::String("*mixer*".to_string())
    );
}

#[test]
fn manual_navigation_follows_menu_order_and_history() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor.drain_host_commands();
    eval(&mut editor, "(eseq.manual/open-manual)");
    eval(
        &mut editor,
        r#"(set-layout (list :buf "*manual*" :hide-status true :min-width 25))"#,
    );
    manual_layout(&mut editor);

    // Enter the second menu entry the way a click does, then walk siblings.
    eval(
        &mut editor,
        "(eseq.manual/open-menu-entry \"sequencer-tour\")",
    );
    assert_eq!(current_node(&mut editor), "sequencer-tour");
    eval(&mut editor, "(eseq.manual/manual-next)");
    assert_eq!(current_node(&mut editor), "mixer");
    eval(&mut editor, "(eseq.manual/manual-prev)");
    assert_eq!(current_node(&mut editor), "sequencer-tour");
    eval(&mut editor, "(eseq.manual/manual-prev)");
    assert_eq!(current_node(&mut editor), "concepts");
    eval(&mut editor, "(eseq.manual/manual-prev)");
    assert_eq!(
        current_node(&mut editor),
        "concepts",
        "first sibling stays put"
    );
    editor.runtime_mut().take_status_message();

    eval(&mut editor, "(eseq.manual/manual-up)");
    assert_eq!(current_node(&mut editor), "index");
    eval(&mut editor, "(eseq.manual/manual-back)");
    assert_eq!(current_node(&mut editor), "concepts");

    // The tour page carries a code block and an ordered list.
    eval(&mut editor, "(eseq.manual/open-node \"sequencer-tour\")");
    let layout = manual_layout(&mut editor);
    let mut code = Vec::new();
    labels_containing(&layout, "(seq-roll", &mut code);
    assert_eq!(
        code.len(),
        1,
        "code block line renders verbatim as one label"
    );
    let mut numbered = Vec::new();
    labels_containing(&layout, "3.", &mut numbered);
    assert!(!numbered.is_empty(), "ordered list items are renumbered");

    // A missing node degrades to an in-manual error page, never a Lisp error.
    eval(&mut editor, "(eseq.manual/open-node \"no-such-node\")");
    let layout = manual_layout(&mut editor);
    let mut missing = Vec::new();
    labels_containing(&layout, "Missing page", &mut missing);
    assert_eq!(missing.len(), 1);
    eval(&mut editor, "(eseq.manual/manual-back)");
    assert_eq!(current_node(&mut editor), "sequencer-tour");
}

#[test]
fn manual_action_links_evaluate_only_on_click() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor.drain_host_commands();
    eval(&mut editor, "(eseq.manual/open-manual)");
    eval(&mut editor, "(eseq.manual/open-node \"mixer\")");
    eval(
        &mut editor,
        r#"(set-layout (list :buf "*manual*" :hide-status true :min-width 25))"#,
    );
    manual_layout(&mut editor);
    assert_eq!(
        eval(&mut editor, "(current-buffer-name)"),
        Value::String("*manual*".to_string()),
        "rendering an action link must not run it"
    );
    // The reader has no backslash escapes, so exercise a quote-free form:
    // the raw link text is wrapped in one pair of parens and evaluated.
    eval(
        &mut editor,
        "(eseq.manual/run-action \"eseq.manual/manual-top\")",
    );
    assert_eq!(current_node(&mut editor), "index");

    // External links go to the host, cross-references stay in the manual.
    eval(
        &mut editor,
        "(eseq.manual/follow-link \"https://example.com/x\")",
    );
    let commands = editor.drain_host_commands();
    assert!(
        commands.iter().any(|command| matches!(
            command,
            eseqlisp::host::HostCommand::Custom { name, .. } if name == "open-url"
        )),
        "external link should request open-url; got {commands:?}"
    );
    eval(&mut editor, "(eseq.manual/follow-link \"concepts\")");
    assert_eq!(current_node(&mut editor), "concepts");
}

fn click(editor: &mut eseqlisp::Editor, col: f32, row: f32) {
    for kind in [
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
    ] {
        editor.handle_mouse_precise(
            crossterm::event::MouseEvent {
                kind,
                column: col as u16,
                row: row as u16,
                modifiers: crossterm::event::KeyModifiers::NONE,
            },
            0,
            0,
            100,
            40,
            col,
            row,
        );
    }
}

#[test]
fn manual_menu_entry_click_navigates() {
    let mut editor = full_grid_editor_for_scroll_tests();
    editor.drain_host_commands();
    eval(&mut editor, "(eseq.manual/open-manual)");
    eval(
        &mut editor,
        r#"(set-layout (list :buf "*manual*" :hide-status true :min-width 25))"#,
    );
    let layout = manual_layout(&mut editor);
    let mut mixer = Vec::new();
    labels_containing(&layout, "Mixer", &mut mixer);
    let target = mixer
        .iter()
        .find(|node| node.props.get("text") == Some(&Value::String("Mixer".to_string())))
        .expect("menu entry label");
    let col = target.rect.col + target.rect.width * 0.5;
    let row = target.rect.row + target.rect.height * 0.5;
    click(&mut editor, col, row);
    editor.refresh_runtime_side_effects();
    if let Some(status) = editor.runtime_mut().take_status_message() {
        panic!("status after click: {status}");
    }
    assert_eq!(current_node(&mut editor), "mixer");
}
