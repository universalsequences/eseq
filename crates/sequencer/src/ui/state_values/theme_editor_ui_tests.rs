//! The `*theme-editor*` buffer (content/ui/theme-editor.lisp): a picker edit
//! applies to the live theme at once, and Save writes only the edited slots
//! back into the theme file, in place.

use super::*;
use eseqlisp::layout::LayoutNode;

fn eval(editor: &mut eseqlisp::Editor, form: &str) -> Value {
    editor
        .runtime_mut()
        .eval_str(form)
        .unwrap_or_else(|error| panic!("{form}: {error:?}"))
        .unwrap_or(Value::Nil)
}

fn find_widget<'a>(node: &'a LayoutNode, widget_type: &str) -> Option<&'a LayoutNode> {
    if node.widget_type == widget_type {
        return Some(node);
    }
    node.children.iter().find_map(|child| find_widget(child, widget_type))
}

fn theme_editor_layout(editor: &mut eseqlisp::Editor) -> std::sync::Arc<LayoutNode> {
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    let buffer_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*theme-editor*")
        .expect("theme editor effect buffer")
        .id;
    editor.set_active_buffer(buffer_id);
    editor.set_layout_viewport(60, 60);
    let layout = editor.widget_layout().expect("theme editor layout");
    assert_finite_layout_tree(&layout);
    layout
}

#[test]
fn picker_edit_previews_live_and_save_rewrites_only_edited_slots() {
    let mut editor = full_grid_editor_for_scroll_tests();
    eval(&mut editor, r#"(eseq.theme-editor/load-theme-for-editing "mac-osx-rune")"#);

    // Edit a scratch copy of the theme, never the checked-in file.
    let dir = tempfile::tempdir().expect("tempdir");
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/ui/themes/mac-osx-rune.lisp");
    let copy = dir.path().join("rune-copy.lisp");
    std::fs::copy(&source, &copy).expect("copy theme");
    eval(
        &mut editor,
        &format!("(set! eseq.theme-editor/te-file {})", serde_json::to_string(&copy).unwrap()),
    );

    // Rune sets `accent`; it does not set `process-lane-accent`, which the
    // table must mark as inherited.
    let layout = theme_editor_layout(&mut editor);
    let rows = eval(&mut editor, "(len eseq.theme-editor/te-rows)");
    assert!(matches!(rows, Value::Number(n) if n > 300.0), "every slot is listed: {rows:?}");
    assert_eq!(
        eval(&mut editor, r#"(get (first (filter |r| (= (get r :name) "process-lane-accent") eseq.theme-editor/te-rows)) :in-file)"#),
        Value::Bool(false)
    );
    assert!(find_widget(&layout, "color-picker").is_none(), "picker starts closed");

    eval(&mut editor, r#"(eseq.theme-editor/open-picker "accent")"#);
    eval(&mut editor, "(eseq.theme-editor/set-picker-hsva 0 1 1 1)");
    let layout = theme_editor_layout(&mut editor);
    let picker = find_widget(&layout, "color-picker").expect("picker open in the modal");
    assert_finite_nonzero_rect(picker, "color picker");
    let accent = eseqlisp::theme::ACCENT();
    assert_eq!((accent.r, accent.g, accent.b), (1.0, 0.0, 0.0), "edit applies to the live theme");

    eval(&mut editor, "(eseq.theme-editor/close-picker)");
    eval(&mut editor, "(eseq.theme-editor/theme-editor-save)");
    let saved = std::fs::read_to_string(&copy).unwrap();
    let original = std::fs::read_to_string(&source).unwrap();
    let changed: Vec<_> = saved
        .lines()
        .zip(original.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(changed.len(), 1, "only the accent line changes: {changed:#?}");
    assert!(changed[0].0.contains(":accent") && changed[0].0.contains("'(1.0 0.0 0.0 1.0)"));
    assert_eq!(saved.lines().count(), original.lines().count());
    assert_eq!(eval(&mut editor, "(len eseq.theme-editor/te-edits)"), Value::Number(0.0));
}

#[test]
fn edit_theme_opens_in_the_side_pane_and_close_removes_it() {
    let mut editor = full_grid_editor_for_scroll_tests();
    let visible = |editor: &mut eseqlisp::Editor| {
        editor.runtime_mut().run_reactive_cycle();
        editor.refresh_runtime_side_effects();
        format!("{:?}", eval(editor, "(visible-buffer-list)"))
    };
    let before = visible(&mut editor);
    assert!(!before.contains("*theme-editor*"));
    eval(&mut editor, "(eseq.theme-editor/edit-theme)");
    let open = visible(&mut editor);
    assert!(open.contains("*theme-editor*"), "{open}");
    assert_eq!(open.replace(" \"*theme-editor*\"", ""), before, "existing tiles stay");
    eval(&mut editor, "(eseq.theme-editor/theme-editor-close)");
    assert_eq!(visible(&mut editor), before);
}

#[test]
fn typing_or_pasting_a_hex_color_applies_without_pressing_enter() {
    let mut editor = full_grid_editor_for_scroll_tests();
    eval(&mut editor, r#"(eseq.theme-editor/load-theme-for-editing "mac-osx-rune")"#);
    eval(&mut editor, r#"(eseq.theme-editor/open-picker "buffer-bg")"#);
    editor.refresh_runtime_side_effects();
    let before = eseqlisp::theme::BUFFER_BG();

    // Half-typed text is not a color yet: nothing changes.
    eval(&mut editor, r##"(eseq.theme-editor/hex-typed "#ff00")"##);
    editor.refresh_runtime_side_effects();
    assert_eq!(eseqlisp::theme::BUFFER_BG(), before);

    // A pasted value without "#" applies at once and stays as typed.
    eval(&mut editor, r#"(eseq.theme-editor/hex-typed "ff0000")"#);
    editor.refresh_runtime_side_effects();
    let now = eseqlisp::theme::BUFFER_BG();
    assert_eq!((now.r, now.g, now.b), (1.0, 0.0, 0.0));
    assert_eq!(eval(&mut editor, "eseq.theme-editor/te-hex-draft"), Value::String("ff0000".into()));
    assert_eq!(eval(&mut editor, "(len eseq.theme-editor/te-edits)"), Value::Number(1.0));
}
