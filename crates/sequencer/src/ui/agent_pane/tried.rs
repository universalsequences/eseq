//! What the agent is trying in the running app (its `try` tool): forms
//! evaluated as module `user.try`, which stay until the user keeps them
//! (into init.lisp) or reverts them. Overrides belong to the module that
//! wrote them, so reverting removes exactly `user.try`'s.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use eseqlisp::Editor;
use sequencer::agent::forms::{self, TopForm};
use sequencer::agent::harness::LiveAction;

use crate::host_commands::packages::{edit_user_init, user_init_path, write_text_atomically};

pub(super) const MODULE: &str = "user.try";
const IMPORT: &str = "import";

thread_local! {
    /// The tried forms, the module's source after its `(module …)` line.
    static TRIED: RefCell<String> = const { RefCell::new(String::new()) };
    /// The labels of the tried forms open in a tile beside the pane.
    static VIEWED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn viewed() -> Vec<String> {
    VIEWED.with(|viewed| viewed.borrow().clone())
}

fn set_viewed(labels: Vec<String>) {
    VIEWED.with(|viewed| *viewed.borrow_mut() = labels);
}

fn tried() -> String {
    TRIED.with(|tried| tried.borrow().clone())
}

fn set_tried(source: String) {
    TRIED.with(|tried| *tried.borrow_mut() = source);
}

/// The file `user.try` is evaluated from, so its errors name a place.
fn try_file() -> PathBuf {
    sequencer::app_paths::app_paths().user_data_root().join("agent").join("try.lisp")
}

fn module_source(body: &str) -> String {
    if body.trim().is_empty() {
        format!("(module {MODULE})\n")
    } else {
        format!("(module {MODULE})\n\n{}\n", body.trim_end())
    }
}

/// `source` with each form of `code` defined into it as `define_form` does
/// (a definition replaces the one of the same name), skipping a form
/// `source` already holds word for word, such as a repeated `(import …)`.
fn merged(source: &str, code: &str) -> Result<String, String> {
    let new = forms::top_level_forms(code).map_err(|error| format!("the code, line {error}"))?;
    if new.is_empty() {
        return Err("the code holds no form".to_string());
    }
    let mut out = source.to_string();
    for form in &new {
        let text = &code[form.start_byte..form.end_byte];
        if form.head == "module" {
            return Err(format!("leave out `{text}`: tried forms run as module {MODULE}"));
        }
        let held = forms::top_level_forms(&out).unwrap_or_default();
        if held.iter().any(|old| &out[old.start_byte..old.end_byte] == text) {
            continue;
        }
        out = forms::define_form(&out, text)?.0;
    }
    Ok(out)
}

fn settle(editor: &mut Editor) {
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}

/// What a form shows as in a list: `override eseq.mixer/strip-buttons`.
fn label(form: &TopForm) -> String {
    match &form.name {
        Some(name) => format!("{} {name}", form.head),
        None => form.head.clone(),
    }
}

/// Each form of `source` but its imports, with its label: a repeated one
/// (two `bind-key`s) numbered from its second on, so each row is its own.
fn labeled_forms(source: &str) -> Vec<(String, TopForm)> {
    let mut seen: Vec<String> = Vec::new();
    let forms = forms::top_level_forms(source).unwrap_or_default();
    forms
        .into_iter()
        .filter(|form| form.head != IMPORT)
        .map(|form| {
            let base = label(&form);
            let repeats = seen.iter().filter(|label| **label == base).count();
            seen.push(base.clone());
            let label = if repeats == 0 { base } else { format!("{base} #{}", repeats + 1) };
            (label, form)
        })
        .collect()
}

/// Each form of `source` but its imports, as its label and its text.
fn forms_of(source: &str) -> Vec<(String, String)> {
    labeled_forms(source)
        .into_iter()
        .map(|(label, form)| (label, source[form.start_byte..form.end_byte].to_string()))
        .collect()
}

/// One label per form of `source` but its imports.
fn labels_of(source: &str) -> Vec<String> {
    labeled_forms(source).into_iter().map(|(label, _)| label).collect()
}

/// What is being tried, for the pane: each form's label (imports left out)
/// and whether its code is open beside the pane.
pub(super) fn shown_forms() -> Vec<(String, bool)> {
    let viewed = viewed();
    labels_of(&tried()).into_iter().map(|label| {
        let open = viewed.contains(&label);
        (label, open)
    }).collect()
}

/// The buffer a tried form's code shows in.
fn view_buffer(label: &str) -> String {
    format!("*try: {label}*")
}

/// Open the code of the tried form `label` in a tile beside the pane, or
/// close it when it is open.
pub(super) fn toggle_view(editor: &mut Editor, label: &str) -> Result<(), String> {
    let name = view_buffer(label);
    let mut viewed = viewed();
    if editor.buffer_named_is_showing(&name) {
        close_view(editor, &name);
        viewed.retain(|open| open != label);
    } else {
        let forms = forms_of(&tried());
        let (_, source) = forms
            .iter()
            .find(|(form, _)| form == label)
            .ok_or_else(|| format!("`{label}` is no longer being tried"))?;
        editor.upsert_read_only_lisp_buffer(&name, source);
        if !editor.show_buffer_beside(super::PANE_BUFFER, &name) {
            return Err("Open the agent pane to see the code.".to_string());
        }
        if !viewed.iter().any(|open| open == label) {
            viewed.push(label.to_string());
        }
    }
    set_viewed(viewed);
    Ok(())
}

/// Close a view's tile and drop its buffer.
fn close_view(editor: &mut Editor, name: &str) {
    editor.close_tile_showing_buffer_named(name);
    editor.remove_buffer_by_name(name);
}

/// Forget the views whose tile was closed some other way (a drag, `C-x 0`),
/// so their rows show closed. Whether any was; cheap enough for every frame.
pub(super) fn forget_closed_views(editor: &Editor) -> bool {
    VIEWED.with(|viewed| {
        let mut viewed = viewed.borrow_mut();
        let before = viewed.len();
        viewed.retain(|label| editor.buffer_named_is_showing(&view_buffer(label)));
        viewed.len() != before
    })
}

/// Bring the open views in line with what is tried: refill each with its
/// form's current code and close those whose form is gone.
fn sync_views(editor: &mut Editor) {
    forget_closed_views(editor);
    let before = viewed();
    if before.is_empty() {
        return;
    }
    let forms = forms_of(&tried());
    let mut kept = Vec::new();
    for label in before {
        let name = view_buffer(&label);
        match forms.iter().find(|(form, _)| *form == label) {
            Some((_, source)) => {
                if editor.read_buffer_text(&name).as_deref() != Some(source.as_str()) {
                    editor.upsert_read_only_lisp_buffer(&name, source);
                }
                kept.push(label);
            }
            None => close_view(editor, &name),
        }
    }
    set_viewed(kept);
}

/// Run one of the agent's live tool calls.
pub(super) fn serve(editor: &mut Editor, action: &LiveAction) -> Result<String, String> {
    match action {
        LiveAction::Try(code) => try_forms(editor, &try_file(), code),
        LiveAction::Keep => keep(editor, &try_file(), &user_init_path()),
        LiveAction::Look { .. } | LiveAction::WhatDraws { .. } => {
            Err("There is no window to look at here.".to_string())
        }
    }
}

/// Evaluate `code` in the running app, from `file`, with what was tried
/// before; a failure leaves the app as it was and returns the errors.
pub(super) fn try_forms(editor: &mut Editor, file: &Path, code: &str) -> Result<String, String> {
    let next = merged(&tried(), code)?;
    let source = module_source(&next);
    write_text_atomically(file, &source)?;
    let report = editor.runtime_mut().eval_source_transactional(Some(file.to_path_buf()), &source, Vec::new());
    if !report.success {
        write_text_atomically(file, &module_source(&tried()))?;
        return Err(format!("Nothing changed; the forms failed:\n{}", report.diagnostics.join("\n")));
    }
    set_tried(next);
    sync_views(editor);
    settle(editor);
    Ok(format!(
        "The app now shows: {}. The user can Keep it (into init.lisp) or Revert it from the pane; \
         tell them what to look at.",
        labels_of(&tried()).join(", ")
    ))
}

/// Take `user.try`'s overrides off and forget the tried forms.
fn clear(editor: &mut Editor, file: &Path) -> Vec<String> {
    let targets = editor.runtime_mut().remove_module_overrides(MODULE);
    set_tried(String::new());
    sync_views(editor);
    let _ = write_text_atomically(file, &module_source(""));
    settle(editor);
    targets
}

fn nothing_tried() -> Result<String, String> {
    Err("Nothing is being tried.".to_string())
}

/// Undo what is tried. Overrides go; anything else the forms did (a key
/// binding, a theme change) stays until the app restarts.
pub(super) fn revert(editor: &mut Editor, file: &Path) -> Result<String, String> {
    let source = tried();
    if source.trim().is_empty() {
        return nothing_tried();
    }
    let others: Vec<String> = forms::top_level_forms(&source)
        .unwrap_or_default()
        .iter()
        .filter(|form| form.head != IMPORT && !forms::is_defining(&form.head))
        .map(label)
        .collect();
    let targets = clear(editor, file);
    let mut message = if targets.is_empty() {
        "Reverted what was tried.".to_string()
    } else {
        format!("Reverted: {} back to stock.", targets.join(", "))
    };
    if !others.is_empty() {
        message.push_str(&format!(" ({} stays until the app restarts.)", others.join(", ")));
    }
    Ok(message)
}

/// Add the tried forms to the user init at `init` and reload it, then drop
/// `user.try`: the init's overrides take over.
pub(super) fn keep(editor: &mut Editor, file: &Path, init: &Path) -> Result<String, String> {
    let source = tried();
    if source.trim().is_empty() {
        return nothing_tried();
    }
    edit_user_init(editor, init, |before| {
        merged(if before.trim().is_empty() { "(module user.init)\n" } else { before }, &source)
    })?;
    clear(editor, file);
    Ok(format!("Kept in init.lisp: {}.", labels_of(&source).join(", ")))
}

/// The pane's Keep and Revert.
pub(super) fn keep_in_init(editor: &mut Editor) -> Result<String, String> {
    keep(editor, &try_file(), &user_init_path())
}

pub(super) fn revert_tried(editor: &mut Editor) -> Result<String, String> {
    revert(editor, &try_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use eseqlisp::{EditorConfig, Runtime};

    fn greeting(editor: &mut Editor) -> String {
        let value = editor.runtime_mut().eval_str("(demo.panel/greet)").unwrap().unwrap();
        editor.runtime().format_value(&value)
    }

    #[test]
    fn a_tried_override_shows_until_reverted_or_kept_in_init() {
        let dir = std::env::temp_dir().join(format!("eseq-tried-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("try.lisp");
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        let panel = "(module demo.panel)\n(export greet)\n(def greet () \"hi\")\n";
        let report = editor.runtime_mut().eval_source_transactional(Some(dir.join("panel.lisp")), panel, Vec::new());
        assert!(report.success, "{:?}", report.diagnostics);
        assert_eq!(greeting(&mut editor), "\"hi\"");

        // A failed try changes nothing.
        assert!(try_forms(&mut editor, &file, "(override demo.panel/greet () (undefined-thing))\n(def").is_err());
        assert!(tried().is_empty());

        try_forms(&mut editor, &file, "(override demo.panel/greet () \"yo\")").unwrap();
        assert_eq!(greeting(&mut editor), "\"yo\"");
        // A second try replaces the override rather than stacking.
        let shown = try_forms(&mut editor, &file, "(override demo.panel/greet :around (original) (str (original) \"!\"))").unwrap();
        assert!(shown.contains("override demo.panel/greet"), "{shown}");
        assert_eq!(greeting(&mut editor), "\"hi!\"");

        let reverted = revert(&mut editor, &file).unwrap();
        assert_eq!(reverted, "Reverted: demo.panel/greet back to stock.");
        assert_eq!(greeting(&mut editor), "\"hi\"");
        assert!(revert(&mut editor, &file).is_err(), "nothing left to revert");

        // Kept: the override joins what init already holds.
        let init = dir.join("init.lisp");
        std::fs::write(&init, "(module user.init)\n\n(def x 0)\n").unwrap();
        try_forms(&mut editor, &file, "(override demo.panel/greet () \"kept\")").unwrap();
        let kept = keep(&mut editor, &file, &init).unwrap();
        assert_eq!(kept, "Kept in init.lisp: override demo.panel/greet.");
        assert_eq!(greeting(&mut editor), "\"kept\"");
        assert!(tried().is_empty());
        let saved = std::fs::read_to_string(&init).unwrap();
        assert_eq!(saved, "(module user.init)\n\n(def x 0)\n\n(override demo.panel/greet () \"kept\")\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tried_form_opens_beside_the_pane_and_follows_later_tries() {
        let dir = std::env::temp_dir().join(format!("eseq-tried-view-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("try.lisp");
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_scratch_buffer(super::super::PANE_BUFFER, "");
        try_forms(&mut editor, &file, "(def shade 1)").unwrap();
        let name = view_buffer("def shade");

        toggle_view(&mut editor, "def shade").unwrap();
        assert!(editor.buffer_named_is_showing(&name));
        assert_eq!(shown_forms(), [("def shade".to_string(), true)]);
        // A later try of the same name refills the open view.
        try_forms(&mut editor, &file, "(def shade 2)").unwrap();
        assert_eq!(editor.read_buffer_text(&name).as_deref(), Some("(def shade 2)"));

        // Clicking again closes it; a tile closed another way is forgotten.
        toggle_view(&mut editor, "def shade").unwrap();
        assert!(!editor.buffer_named_is_showing(&name));
        assert_eq!(shown_forms(), [("def shade".to_string(), false)]);
        toggle_view(&mut editor, "def shade").unwrap();
        editor.close_tile_showing_buffer_named(&name);
        assert!(forget_closed_views(&editor));
        assert_eq!(shown_forms(), [("def shade".to_string(), false)]);

        // Revert closes what is open.
        toggle_view(&mut editor, "def shade").unwrap();
        revert(&mut editor, &file).unwrap();
        assert!(!editor.buffer_named_is_showing(&name));
        assert!(toggle_view(&mut editor, "def shade").is_err(), "nothing tried to show");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tried_forms_replace_by_name_and_skip_repeats() {
        let first = merged("", "(import eseq.mixer)\n(override eseq.mixer/a () 1)").unwrap();
        let second = merged(&first, "(import eseq.mixer)\n(override eseq.mixer/a () 2)\n(bind-key \"C-c m\" \"x\")").unwrap();
        assert_eq!(second, "(import eseq.mixer)\n\n(override eseq.mixer/a () 2)\n\n(bind-key \"C-c m\" \"x\")\n");
        assert_eq!(labels_of(&second), ["override eseq.mixer/a", "bind-key"]);
        let keys = merged("", "(bind-key \"C-c a\" \"x\")\n(bind-key \"C-c b\" \"y\")").unwrap();
        assert_eq!(labels_of(&keys), ["bind-key", "bind-key #2"], "each repeat is its own row");
        assert_eq!(forms_of(&keys)[1].1, "(bind-key \"C-c b\" \"y\")");
        assert_eq!(forms_of(&second)[0].1, "(override eseq.mixer/a () 2)", "a form shows as written");
        assert!(merged("", "(module x)").is_err(), "the module is user.try's");
        assert!(merged("", "(def").is_err());
    }
}
