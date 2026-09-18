//! The Customize modal (content/ui/customize.lisp): lists `defcustom`
//! knobs grouped by module with live editors, and override registrations by
//! the module that installed them with enable/disable toggles.

use super::*;
use eseqlisp::layout::LayoutNode;

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

fn widgets_with_key<'a>(node: &'a LayoutNode, key: &str, out: &mut Vec<&'a LayoutNode>) {
    if matches!(node.props.get("key"), Some(Value::String(k)) if k == key) {
        out.push(node);
    }
    for child in &node.children {
        widgets_with_key(child, key, out);
    }
}

fn eval(editor: &mut eseqlisp::Editor, form: &str) -> Value {
    editor
        .runtime_mut()
        .eval_str(form)
        .unwrap_or_else(|error| panic!("{form}: {error:?}"))
        .unwrap_or(Value::Nil)
}

fn customize_layout(editor: &mut eseqlisp::Editor) -> std::sync::Arc<LayoutNode> {
    // The modal is mounted by eseq.file-dialogs in the step-panel buffers.
    let sequencer_id = editor
        .buffers
        .iter()
        .find(|buffer| buffer.name == "*sequencer*")
        .expect("sequencer buffer")
        .id;
    editor.set_active_buffer(sequencer_id);
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    if let Some(status) = editor.runtime_mut().take_status_message() {
        panic!("customize status after refresh: {status}");
    }
    editor.set_layout_viewport(160, 60);
    let layout = editor
        .widget_layout()
        .expect("sequencer layout with the customize modal open");
    assert_finite_layout_tree(&layout);
    layout
}

fn texts(layout: &LayoutNode) -> Vec<String> {
    let mut out = Vec::new();
    label_texts(layout, &mut out);
    out
}

#[test]
fn customize_lists_the_mixer_clip_knob_under_its_module_and_edits_resize_the_mixer_live() {
    let mut editor = full_grid_editor_for_scroll_tests();
    eval(&mut editor, "(eseq.customize/open-customize)");
    let layout = customize_layout(&mut editor);
    let labels = texts(&layout);
    assert!(
        labels.iter().any(|text| text == "eseq.seq-core-state"),
        "module header missing: {labels:?}"
    );
    assert!(
        labels.iter().any(|text| text == "mixer-clip-area-height"),
        "knob row missing: {labels:?}"
    );
    assert!(
        !labels.iter().any(|text| text == "(customized)"),
        "a fresh knob must not read as customized: {labels:?}"
    );
    // Factory sizing knobs seed the surface, grouped under their modules.
    for (module, knob) in [
        ("eseq.seq-layout", "samples-sidebar-ratio"),
        ("eseq.seq-layout", "transport-bar-height"),
        ("eseq.seq-layout", "lower-panel-ratio"),
        ("eseq.seq-step-tabs", "lower-fx-layout-height"),
        ("eseq.seq-step-tabs", "seq-tile-border-width"),
        ("eseq.mixer", "track-strip-width"),
        ("eseq.mixer", "bus-strip-width"),
    ] {
        assert!(
            labels.iter().any(|text| text == module),
            "{module} header: {labels:?}"
        );
        assert!(
            labels.iter().any(|text| text == knob),
            "{knob} row: {labels:?}"
        );
    }
    let mut pickers = Vec::new();
    widgets_with_key(
        &layout,
        "customize-eseq.seq-core-state/mixer-clip-area-height",
        &mut pickers,
    );
    assert_eq!(pickers.len(), 1, "one number-picker for the knob");
    assert_eq!(pickers[0].widget_type, "number-picker");
    assert_eq!(pickers[0].props.get("value"), Some(&Value::Number(4.0)));

    assert_eq!(
        eval(&mut editor, "(eseq.seq-core-state/mixer-panel-height)"),
        Value::Number(14.5)
    );
    // The picker's on-change path.
    eval(
        &mut editor,
        "(eseq.customize/set-knob \"eseq.seq-core-state/mixer-clip-area-height\" 9)",
    );
    assert_eq!(
        eval(&mut editor, "(eseq.seq-core-state/mixer-panel-height)"),
        Value::Number(19.5),
        "the mixer derives its height from the knob"
    );
    assert_eq!(
        eval(&mut editor, "eseq.customize/customize-dirty"),
        Value::Bool(true)
    );
    let layout = customize_layout(&mut editor);
    let labels = texts(&layout);
    assert!(
        labels.iter().any(|text| text == "(customized)"),
        "an edited knob is marked: {labels:?}"
    );
    let mut pickers = Vec::new();
    widgets_with_key(
        &layout,
        "customize-eseq.seq-core-state/mixer-clip-area-height",
        &mut pickers,
    );
    assert_eq!(pickers[0].props.get("value"), Some(&Value::Number(9.0)));

    // Reset returns to the declared default.
    eval(
        &mut editor,
        "(eseq.customize/reset-knob (find-by-key (custom-declarations) :name \"eseq.seq-core-state/mixer-clip-area-height\"))",
    );
    assert_eq!(
        eval(&mut editor, "(eseq.seq-core-state/mixer-panel-height)"),
        Value::Number(14.5)
    );
}

#[test]
fn customize_lists_overrides_by_module_and_a_module_toggle_reverts_to_factory() {
    let mut editor = full_grid_editor_for_scroll_tests();
    let package = std::env::temp_dir().join(format!(
        "customize-override-pkg-{}.lisp",
        std::process::id()
    ));
    let overlays = editor.snapshot_file_backed_sources();
    let report = editor.runtime_mut().eval_source_transactional(
        Some(package.clone()),
        "(module test.customize-pkg)\n\
         (override eseq.seq-layout/transport-height :around (original) (+ (original) 3))",
        overlays,
    );
    assert!(report.success, "{:?}", report.diagnostics);
    assert_eq!(
        eval(&mut editor, "(eseq.seq-layout/transport-height)"),
        Value::Number(5.4)
    );

    eval(&mut editor, "(eseq.customize/open-customize)");
    let layout = customize_layout(&mut editor);
    let labels = texts(&layout);
    assert!(
        labels.iter().any(|text| text == "test.customize-pkg"),
        "{labels:?}"
    );
    assert!(
        labels
            .iter()
            .any(|text| text == "eseq.seq-layout/transport-height"),
        "{labels:?}"
    );
    assert!(labels.iter().any(|text| text == "around"), "{labels:?}");
    let mut toggles = Vec::new();
    widgets_with_key(
        &layout,
        "customize-override-module-toggle-test.customize-pkg",
        &mut toggles,
    );
    assert_eq!(toggles.len(), 1);
    assert_eq!(toggles[0].props.get("value"), Some(&Value::Bool(true)));

    eval(
        &mut editor,
        "(eseq.customize/set-module-overrides-enabled \"test.customize-pkg\" false)",
    );
    assert_eq!(
        eval(&mut editor, "(eseq.seq-layout/transport-height)"),
        Value::Number(2.4),
        "disabling the package returns the factory value"
    );
    let disabled = eval(&mut editor, "(disabled-override-modules)");
    assert!(
        matches!(&disabled, Value::List(items) if items.len() == 1),
        "{disabled:?}"
    );
    let layout = customize_layout(&mut editor);
    let labels = texts(&layout);
    assert!(
        labels.iter().any(|text| text == "off (factory)"),
        "{labels:?}"
    );
    let mut toggles = Vec::new();
    widgets_with_key(
        &layout,
        "customize-override-module-toggle-test.customize-pkg",
        &mut toggles,
    );
    assert_eq!(toggles[0].props.get("value"), Some(&Value::Bool(false)));

    // Per-entry re-enable through the row toggle path.
    eval(
        &mut editor,
        "(eseq.customize/set-override-enabled (first (override-declarations)) true)",
    );
    assert_eq!(
        eval(&mut editor, "(eseq.seq-layout/transport-height)"),
        Value::Number(5.4)
    );
    let _ = std::fs::remove_file(package);
}

#[test]
fn customize_lists_an_attached_package_knob_under_its_module_and_setopt_reaches_it() {
    let mut editor = full_grid_editor_for_scroll_tests();
    let package =
        std::env::temp_dir().join(format!("customize-knob-pkg-{}.lisp", std::process::id()));
    let overlays = editor.snapshot_file_backed_sources();
    let report = editor.runtime_mut().eval_source_transactional(
        Some(package.clone()),
        "(module test.customize-knobs)\n\
         (defcustom columns 4 :type :number :doc \"Channel cells per row.\")\n\
         (defcustom flavor \"soft\" :type :string :doc \"Flavor\" :choices (list \"soft\" \"hard\"))\n\
         (defcustom dark true :type :bool :doc \"Dark\")\n\
         (def column-width () (/ 100 columns))",
        overlays,
    );
    assert!(report.success, "{:?}", report.diagnostics);

    eval(&mut editor, "(eseq.customize/open-customize)");
    let layout = customize_layout(&mut editor);
    let labels = texts(&layout);
    assert!(
        labels.iter().any(|text| text == "test.customize-knobs"),
        "{labels:?}"
    );
    for knob in ["columns", "flavor", "dark", "Channel cells per row."] {
        assert!(labels.iter().any(|text| text == knob), "{knob}: {labels:?}");
    }
    let widget_type = |layout: &LayoutNode, key: &str| {
        let mut found = Vec::new();
        widgets_with_key(layout, key, &mut found);
        assert_eq!(found.len(), 1, "{key}");
        found[0].widget_type.clone()
    };
    assert_eq!(
        widget_type(&layout, "customize-test.customize-knobs/columns"),
        "number-picker"
    );
    assert_eq!(
        widget_type(&layout, "customize-test.customize-knobs/flavor"),
        "dropdown"
    );
    assert_eq!(
        widget_type(&layout, "customize-test.customize-knobs/dark"),
        "toggle"
    );

    eval(
        &mut editor,
        "(eseq.customize/set-knob \"test.customize-knobs/columns\" 5)",
    );
    assert_eq!(
        eval(&mut editor, "(test.customize-knobs/column-width)"),
        Value::Number(20.0),
        "setopt from the buffer reaches the package"
    );
    let _ = std::fs::remove_file(package);
}

#[test]
fn compact_mixer_knob_drops_the_clip_grid_and_shrinks_the_mixer_panel() {
    let mut editor = full_grid_editor_for_scroll_tests();
    assert_eq!(
        eval(&mut editor, "(eseq.seq-core-state/mixer-panel-height)"),
        Value::Number(14.5)
    );
    eval(
        &mut editor,
        "(eseq.customize/set-knob \"eseq.seq-core-state/mixer-show-clip-grid\" false)",
    );
    assert_eq!(
        eval(&mut editor, "(eseq.seq-core-state/mixer-panel-height)"),
        Value::Number(10.5),
        "compact mode drops the whole clip area from the tile height"
    );
    assert_eq!(
        eval(&mut editor, "(eseq.mixer/strip-height)"),
        Value::Number(9.8)
    );
    // Cranking the clip height while compact changes nothing.
    eval(
        &mut editor,
        "(setopt eseq.seq-core-state/mixer-clip-area-height 9)",
    );
    assert_eq!(
        eval(&mut editor, "(eseq.seq-core-state/mixer-panel-height)"),
        Value::Number(10.5)
    );
    eval(
        &mut editor,
        "(setopt eseq.seq-core-state/mixer-show-clip-grid true)",
    );
    assert_eq!(
        eval(&mut editor, "(eseq.seq-core-state/mixer-panel-height)"),
        Value::Number(19.5)
    );
}

#[test]
fn clip_launch_cells_scale_with_the_track_strip_width() {
    let mut editor = full_grid_editor_for_scroll_tests();
    assert_eq!(eval(&mut editor, "(eseq.mixer/clip-cell-scale)"), Value::Number(1.0));
    eval(&mut editor, "(setopt eseq.mixer/track-strip-width 25.8)");
    assert_eq!(eval(&mut editor, "(eseq.mixer/clip-cell-scale)"), Value::Number(2.0));
    // Six scaled columns still fit the strip at the knob's floor.
    eval(&mut editor, "(setopt eseq.mixer/track-strip-width 11)");
    let scale = match eval(&mut editor, "(eseq.mixer/clip-cell-scale)") {
        Value::Number(n) => n,
        other => panic!("{other:?}"),
    };
    assert!(6.0 * 2.0 * scale <= 11.0, "scale {scale}");
}
