//! `save-custom-values`: persist the *customize* buffer's state as a managed
//! block in the user init file.
//!
//! The block holds one `(setopt name value)` per `defcustom` knob whose live
//! value differs from its declared default, one
//! `(disable-module-overrides mod)` per overriding module switched off, and a
//! `(set-override-entry-enabled …)` for any single entry switched off on its
//! own. It replaces any previous managed block and leaves the rest of
//! `init.lisp` untouched; when nothing is customized the block is removed.
//! Because the init file evaluates last at boot and hot-reloads on save, the
//! block is exactly what restores the customization next session.

use crate::*;
use eseqlisp::vm::format_lisp_value;

pub(super) const COMMANDS: &[&str] = &["save-custom-values", "customize-open"];

pub(super) const BLOCK_START: &str = ";; customize -- managed, edit via M-x customize";
pub(super) const BLOCK_END: &str = ";; end customize";

pub(super) fn handle(
    name: &str,
    _payload: Value,
    _app: &mut app::App,
    editor: &mut Editor,
    _ctx: &mut LoopCtx<'_>,
) {
    match name {
        "customize-open" => {
            // A modal only receives pointer input through the active tile.
            super::file_menu::activate_dialog_tile(editor);
            if let Err(error) = editor
                .runtime_mut()
                .eval_str("(eseq.customize/open-customize)")
            {
                editor.show_transient_message(format!("Customize: {error:?}"));
            }
            editor.runtime_mut().run_reactive_cycle();
            editor.refresh_runtime_side_effects();
        }
        "save-custom-values" => match save_custom_values(editor) {
            Ok(count) => {
                let _ = editor
                    .runtime_mut()
                    .eval_str("(set! eseq.customize/customize-dirty false)");
                editor.runtime_mut().run_reactive_cycle();
                editor.refresh_runtime_side_effects();
                let path = super::packages::user_init_path();
                editor.show_transient_message(if count == 0 {
                    format!(
                        "No customizations; managed block removed from {}",
                        path.display()
                    )
                } else {
                    format!("Saved {count} customizations to {}", path.display())
                });
            }
            Err(error) => editor.show_transient_message(format!("Save customizations: {error}")),
        },
        _ => {}
    }
    editor.mark_needs_redraw();
}

/// Rewrite the managed block; returns how many lines it now holds.
fn save_custom_values(editor: &mut Editor) -> Result<usize, String> {
    let lines = managed_block_lines(editor)?;
    let path = super::packages::user_init_path();

    // An open init buffer is the source of truth for its text (the user may
    // have unsaved edits); update it in place and let the ordinary save
    // path write it.
    if let Some(buffer) = editor
        .buffers
        .iter_mut()
        .find(|buffer| buffer.path.as_ref() == Some(&path))
    {
        let updated = source_with_managed_block(&buffer.text(), &lines);
        buffer.set_text(&updated);
        editor.mark_needs_redraw();
    }

    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("Could not read '{}': {error}", path.display())),
    };
    let updated = source_with_managed_block(&source, &lines);
    if updated == source {
        return Ok(lines.len());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create '{}': {error}", parent.display()))?;
    }
    super::packages::write_text_atomically(&path, &updated)?;
    Ok(lines.len())
}

fn eval_list(editor: &mut Editor, form: &str) -> Result<Vec<Rc<RefCell<Value>>>, String> {
    match editor.runtime_mut().eval_str(form) {
        Ok(Some(Value::List(items))) => Ok(items),
        Ok(other) => Err(format!("{form} returned {other:?}")),
        Err(error) => Err(format!("{form} failed: {error:?}")),
    }
}

fn map_field(map: &HashMap<String, Rc<RefCell<Value>>>, key: &str) -> Value {
    map.get(key)
        .map(|cell| cell.borrow().clone())
        .unwrap_or(Value::Nil)
}

fn map_string(map: &HashMap<String, Rc<RefCell<Value>>>, key: &str) -> Option<String> {
    match map_field(map, key) {
        Value::String(text) => Some(text),
        _ => None,
    }
}

/// The lines the managed block should hold right now, sorted for stable
/// diffs: knobs first, then module switches, then single entries.
fn managed_block_lines(editor: &mut Editor) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for row in eval_list(editor, "(custom-declarations)")? {
        let Value::Map(map) = &*row.borrow() else {
            continue;
        };
        let (Some(name), value, default) = (
            map_string(map, "name"),
            map_field(map, "value"),
            map_field(map, "default"),
        ) else {
            continue;
        };
        if value == default {
            continue;
        }
        match lisp_literal(&value) {
            Some(literal) => lines.push(format!("(setopt {name} {literal})")),
            None => {
                return Err(format!(
                    "{name} holds a value that cannot be written as Lisp source: {value:?}"
                ));
            }
        }
    }
    let mut disabled_modules = Vec::new();
    for module in eval_list(editor, "(disabled-override-modules)")? {
        if let Value::String(module) = &*module.borrow() {
            disabled_modules.push(module.clone());
        }
    }
    for module in &disabled_modules {
        lines.push(format!("(disable-module-overrides {module})"));
    }
    for row in eval_list(editor, "(override-declarations)")? {
        let Value::Map(map) = &*row.borrow() else {
            continue;
        };
        let (Some(target), Some(module)) = (map_string(map, "target"), map_string(map, "module"))
        else {
            continue;
        };
        if map_field(map, "enabled") == Value::Bool(false)
            && !disabled_modules.contains(&module)
            && !target.contains('"')
            && !module.contains('"')
        {
            lines.push(format!(
                "(set-override-entry-enabled \"{target}\" \"{module}\" false)"
            ));
        }
    }
    Ok(lines)
}

/// A value as eseqlisp source. eseqlisp strings have no escape sequences, so
/// a string containing a quote cannot be written; nested containers are
/// spelled with `list`/`dict` so they evaluate back to themselves.
pub(super) fn lisp_literal(value: &Value) -> Option<String> {
    match value {
        Value::Number(_) | Value::Bool(_) | Value::Nil | Value::Keyword(_) => {
            Some(format_lisp_value(value))
        }
        Value::String(text) => (!text.contains('"')).then(|| format!("\"{text}\"")),
        Value::List(items) => {
            let mut rendered = Vec::with_capacity(items.len());
            for item in items {
                rendered.push(lisp_literal(&item.borrow())?);
            }
            Some(format!("(list {})", rendered.join(" ")))
        }
        Value::Map(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            let mut rendered = Vec::with_capacity(entries.len());
            for (key, cell) in entries {
                rendered.push(format!(":{key} {}", lisp_literal(&cell.borrow())?));
            }
            Some(format!("(dict {})", rendered.join(" ")))
        }
        _ => None,
    }
}

/// `source` with its managed block replaced by `lines` (removed when empty).
pub(super) fn source_with_managed_block(source: &str, lines: &[String]) -> String {
    let mut kept = Vec::new();
    let mut inside = false;
    let mut after_block = false;
    let mut insert_at = None;
    for line in source.lines() {
        if after_block {
            // Also drop the blank lines that separated the old block from
            // what followed it; the rebuilt block brings its own.
            if line.trim().is_empty() {
                continue;
            }
            after_block = false;
        }
        if !inside && line.trim() == BLOCK_START {
            inside = true;
            // Drop the blank line the previous block was separated with.
            while kept
                .last()
                .is_some_and(|previous: &&str| previous.trim().is_empty())
            {
                kept.pop();
            }
            insert_at = Some(kept.len());
            continue;
        }
        if inside {
            if line.trim() == BLOCK_END {
                inside = false;
                after_block = true;
            }
            continue;
        }
        kept.push(line);
    }
    let mut body = kept.join("\n");
    if source.ends_with('\n') && !body.is_empty() {
        body.push('\n');
    }
    if lines.is_empty() {
        return body;
    }
    let block = std::iter::once(BLOCK_START.to_string())
        .chain(lines.iter().cloned())
        .chain(std::iter::once(BLOCK_END.to_string()))
        .collect::<Vec<_>>()
        .join("\n");
    match insert_at {
        Some(index) if index < kept.len() => {
            // The old block sat in the middle: keep its position.
            let mut rebuilt = kept[..index].join("\n");
            if !rebuilt.is_empty() {
                rebuilt.push_str("\n\n");
            }
            rebuilt.push_str(&block);
            rebuilt.push_str("\n\n");
            rebuilt.push_str(&kept[index..].join("\n"));
            if source.ends_with('\n') {
                rebuilt.push('\n');
            }
            rebuilt
        }
        _ => {
            let mut rebuilt = body.trim_end().to_string();
            if !rebuilt.is_empty() {
                rebuilt.push_str("\n\n");
            }
            rebuilt.push_str(&block);
            rebuilt.push('\n');
            rebuilt
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eseqlisp::{EditorConfig, Runtime};

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn managed_block_is_appended_replaced_and_removed_without_touching_the_rest() {
        let init = "(module user.init)\n\n(bind-key \"C-c k\" \"user.init/x\")\n";
        let first = source_with_managed_block(init, &lines(&["(setopt a.b/c 1)"]));
        assert_eq!(
            first,
            "(module user.init)\n\n(bind-key \"C-c k\" \"user.init/x\")\n\n\
             ;; customize -- managed, edit via M-x customize\n(setopt a.b/c 1)\n;; end customize\n"
        );
        let second = source_with_managed_block(
            &first,
            &lines(&[
                "(setopt a.b/c 2)",
                "(disable-module-overrides autechre.mixer)",
            ]),
        );
        assert_eq!(
            second,
            "(module user.init)\n\n(bind-key \"C-c k\" \"user.init/x\")\n\n\
             ;; customize -- managed, edit via M-x customize\n(setopt a.b/c 2)\n\
             (disable-module-overrides autechre.mixer)\n;; end customize\n"
        );
        assert_eq!(source_with_managed_block(&second, &[]), init);
        assert_eq!(source_with_managed_block("", &[]), "");
    }

    #[test]
    fn managed_block_keeps_its_place_when_user_code_follows_it() {
        let init = "(module user.init)\n\n;; customize -- managed, edit via M-x customize\n\
                    (setopt a.b/c 1)\n;; end customize\n\n(def later () 1)\n";
        let updated = source_with_managed_block(init, &lines(&["(setopt a.b/c 5)"]));
        assert_eq!(
            updated,
            "(module user.init)\n\n;; customize -- managed, edit via M-x customize\n\
             (setopt a.b/c 5)\n;; end customize\n\n(def later () 1)\n"
        );
    }

    #[test]
    fn lisp_literals_round_trip_and_refuse_unwritable_strings() {
        assert_eq!(lisp_literal(&Value::Number(9.0)).as_deref(), Some("9"));
        assert_eq!(lisp_literal(&Value::Number(0.25)).as_deref(), Some("0.25"));
        assert_eq!(lisp_literal(&Value::Bool(false)).as_deref(), Some("false"));
        assert_eq!(
            lisp_literal(&Value::Keyword("soft".into())).as_deref(),
            Some(":soft")
        );
        assert_eq!(
            lisp_literal(&Value::String("soft".into())).as_deref(),
            Some("\"soft\"")
        );
        assert_eq!(lisp_literal(&Value::String("a\"b".into())), None);
        let list = Value::List(vec![
            Rc::new(RefCell::new(Value::Number(1.0))),
            Rc::new(RefCell::new(Value::String("x".into()))),
        ]);
        assert_eq!(lisp_literal(&list).as_deref(), Some("(list 1 \"x\")"));
    }

    #[test]
    fn managed_block_lines_follow_live_knobs_and_disabled_overrides() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        let src = "(module t.pkg)\n\
                   (defcustom rows 8 :type :number :doc \"rows\")\n\
                   (defcustom label \"a\" :type :string :doc \"label\")\n\
                   (def value () 1)\n";
        let overlays = editor.snapshot_file_backed_sources();
        let report = editor.runtime_mut().eval_source_transactional(
            Some(std::env::temp_dir().join(format!("customize-save-{}.lisp", std::process::id()))),
            src,
            overlays,
        );
        assert!(report.success, "{:?}", report.diagnostics);
        assert_eq!(
            managed_block_lines(&mut editor).unwrap(),
            Vec::<String>::new()
        );
        editor
            .runtime_mut()
            .eval_str("(setopt t.pkg/rows 12)")
            .unwrap();
        // A knob set back to its default drops out.
        editor
            .runtime_mut()
            .eval_str("(setopt t.pkg/label \"b\")")
            .unwrap();
        editor
            .runtime_mut()
            .eval_str("(setopt t.pkg/label \"a\")")
            .unwrap();
        let overlays = editor.snapshot_file_backed_sources();
        let report = editor.runtime_mut().eval_source_transactional(
            Some(
                std::env::temp_dir()
                    .join(format!("customize-save-user-{}.lisp", std::process::id())),
            ),
            "(module t.user)\n(override t.pkg/value () 2)",
            overlays,
        );
        assert!(report.success, "{:?}", report.diagnostics);
        editor
            .runtime_mut()
            .eval_str("(disable-module-overrides t.user)")
            .unwrap();
        assert_eq!(
            managed_block_lines(&mut editor).unwrap(),
            lines(&[
                "(setopt t.pkg/rows 12)",
                "(disable-module-overrides t.user)"
            ])
        );
        // Replaying the block restores the same state in a fresh runtime.
        let mut fresh = Editor::new(Runtime::new(), EditorConfig::default());
        let overlays = fresh.snapshot_file_backed_sources();
        let report = fresh.runtime_mut().eval_source_transactional(
            Some(
                std::env::temp_dir().join(format!("customize-save-2-{}.lisp", std::process::id())),
            ),
            src,
            overlays,
        );
        assert!(report.success, "{:?}", report.diagnostics);
        let block = source_with_managed_block(
            "(module user.init)\n",
            &managed_block_lines(&mut editor).unwrap(),
        );
        fresh.runtime_mut().eval_str(&block).unwrap();
        assert_eq!(
            fresh.runtime_mut().eval_str("t.pkg/rows").unwrap(),
            Some(Value::Number(12.0))
        );
        let overlays = fresh.snapshot_file_backed_sources();
        let report = fresh.runtime_mut().eval_source_transactional(
            Some(
                std::env::temp_dir()
                    .join(format!("customize-save-user-2-{}.lisp", std::process::id())),
            ),
            "(module t.user)\n(override t.pkg/value () 2)",
            overlays,
        );
        assert!(report.success, "{:?}", report.diagnostics);
        assert_eq!(
            fresh.runtime_mut().eval_str("(t.pkg/value)").unwrap(),
            Some(Value::Number(1.0)),
            "a package disabled in the managed block stays disabled when it loads afterwards"
        );
    }
}
