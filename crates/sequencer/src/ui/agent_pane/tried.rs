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

/// One label per form of `source` but its imports.
fn labels_of(source: &str) -> Vec<String> {
    let forms = forms::top_level_forms(source).unwrap_or_default();
    forms.iter().filter(|form| form.head != IMPORT).map(label).collect()
}

/// What is being tried, for the pane: one label per form, imports left out.
pub(super) fn shown_labels() -> Vec<String> {
    labels_of(&tried())
}

/// Run one of the agent's live tool calls.
pub(super) fn serve(editor: &mut Editor, action: &LiveAction) -> Result<String, String> {
    match action {
        LiveAction::Try(code) => try_forms(editor, &try_file(), code),
        LiveAction::Keep => keep(editor, &try_file(), &user_init_path()),
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
    settle(editor);
    Ok(format!(
        "The app now shows: {}. The user can Keep it (into init.lisp) or Revert it from the pane; \
         tell them what to look at.",
        shown_labels().join(", ")
    ))
}

/// Take `user.try`'s overrides off and forget the tried forms.
fn clear(editor: &mut Editor, file: &Path) -> Vec<String> {
    let targets = editor.runtime_mut().remove_module_overrides(MODULE);
    set_tried(String::new());
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
    fn tried_forms_replace_by_name_and_skip_repeats() {
        let first = merged("", "(import eseq.mixer)\n(override eseq.mixer/a () 1)").unwrap();
        let second = merged(&first, "(import eseq.mixer)\n(override eseq.mixer/a () 2)\n(bind-key \"C-c m\" \"x\")").unwrap();
        assert_eq!(second, "(import eseq.mixer)\n\n(override eseq.mixer/a () 2)\n\n(bind-key \"C-c m\" \"x\")\n");
        assert_eq!(labels_of(&second), ["override eseq.mixer/a", "bind-key"]);
        assert!(merged("", "(module x)").is_err(), "the module is user.try's");
        assert!(merged("", "(def").is_err());
    }
}
