use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::runtime::Runtime;
use crate::vm::{Value, format_lisp_value};

pub(super) fn register_editor_natives(runtime: &mut Runtime) {
    runtime.register_native_with_docs(
        "bind-key",
        "(bind-key key handler)",
        "Bind a key chord string to a Lisp function.",
        |args, ctx| {
            let (Some(Value::String(key)), Some(Value::String(handler))) =
                (args.first(), args.get(1))
            else {
                return Err("bind-key expects (string string)".to_string());
            };
            ctx.bind_key(key.clone(), handler.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "status",
        "(status message)",
        "Show a message in the minibuffer.",
        |args, ctx| {
            let Some(Value::String(message)) = args.first() else {
                return Err("status expects a string".to_string());
            };
            ctx.set_status(message.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "s-expression-at-cursor",
        "(s-expression-at-cursor)",
        "Return the current s-expression as a string.",
        |_args, ctx| {
            Ok(ctx
                .current_sexp()
                .map(Value::String)
                .unwrap_or(Value::String(String::new())))
        },
    );

    runtime.register_native_with_docs(
        "current-buffer-text",
        "(current-buffer-text)",
        "Return the active buffer contents.",
        |_args, ctx| Ok(Value::String(ctx.current_buffer_text())),
    );

    runtime.register_native_with_docs(
        "current-buffer-name",
        "(current-buffer-name)",
        "Return the active buffer name.",
        |_args, ctx| Ok(Value::String(ctx.current_buffer_name())),
    );

    runtime.register_native_with_docs(
        "current-buffer-path",
        "(current-buffer-path)",
        "Return the active buffer path or false.",
        |_args, ctx| {
            Ok(match ctx.current_buffer_path() {
                Some(path) => Value::String(path.display().to_string()),
                None => Value::Bool(false),
            })
        },
    );

    runtime.register_native_with_docs(
        "host-command",
        "(host-command name payload)",
        "Send a command to the host application.",
        |args, ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Err("host-command expects a command name".to_string());
            };
            let payload = args.get(1).cloned().unwrap_or(Value::Bool(true));
            let buffer_id = ctx.current_buffer_id();
            let path = ctx.current_buffer_path();
            let source = ctx.current_buffer_text();

            match name.as_str() {
                "compile-instrument" => {
                    ctx.enqueue_command(crate::host::HostCommand::CompileInstrument {
                        source,
                        suggested_name: extract_suggested_name(&payload),
                        buffer_id: buffer_id.unwrap_or(0),
                        path,
                    });
                }
                "compile-effect" => {
                    ctx.enqueue_command(crate::host::HostCommand::CompileEffect {
                        source,
                        suggested_name: extract_suggested_name(&payload),
                        buffer_id: buffer_id.unwrap_or(0),
                        path,
                    });
                }
                _ => {
                    ctx.enqueue_command(crate::host::HostCommand::Custom {
                        name: name.clone(),
                        payload,
                    });
                }
            }
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "load-buffer",
        "(load-buffer)",
        "Load the current buffer from its path, discarding unsaved changes.",
        |_args, ctx| {
            ctx.request_load();
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "save-buffer",
        "(save-buffer)",
        "Save the current buffer.",
        |_args, ctx| {
            ctx.request_save();
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "save-buffer-as",
        "(save-buffer-as path)",
        "Save the current buffer to a new path.",
        |args, ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("save-buffer-as expects a path string".to_string());
            };
            ctx.request_save_as(path.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "eval-selection-or-sexp",
        "(eval-selection-or-sexp)",
        "Return the selected form or current s-expression as source.",
        |_args, ctx| {
            Ok(ctx
                .current_sexp()
                .map(Value::String)
                .unwrap_or(Value::Bool(false)))
        },
    );

    runtime.register_native_with_docs(
        "eval-buffer",
        "(eval-buffer)",
        "Return the whole buffer as source for evaluation.",
        |_args, ctx| Ok(Value::String(ctx.current_buffer_text())),
    );

    runtime.register_native_with_docs(
        "set-read-only",
        "(set-read-only bool)",
        "Set the current buffer's read-only state.",
        |args, ctx| {
            let read_only = match args.first() {
                Some(Value::Bool(b)) => *b,
                Some(Value::Nil) => false,
                _ => true,
            };
            ctx.set_read_only(read_only);
            Ok(Value::Bool(read_only))
        },
    );

    runtime.register_native_with_docs(
        "toggle-read-only",
        "(toggle-read-only)",
        "Toggle the current buffer's read-only state.",
        |_args, ctx| {
            let new_val = !ctx.current_buffer_read_only();
            ctx.set_read_only(new_val);
            Ok(Value::Bool(new_val))
        },
    );

    runtime.register_native_with_docs(
        "buffer-read-only?",
        "(buffer-read-only?)",
        "Return whether the current buffer is read-only.",
        |_args, ctx| Ok(Value::Bool(ctx.current_buffer_read_only())),
    );

    runtime.register_native_with_docs(
        "define-mode",
        "(define-mode name :read-only bool :on-enter fn-name)",
        "Register a named major mode.",
        |args, ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Err("define-mode expects a name string".to_string());
            };
            let mut read_only = false;
            let mut on_enter: Option<String> = None;
            let mut i = 1;
            while i < args.len() {
                match args.get(i) {
                    Some(Value::Keyword(k)) if k == "read-only" => {
                        read_only = matches!(args.get(i + 1), Some(Value::Bool(true)));
                        i += 2;
                    }
                    Some(Value::Keyword(k)) if k == "on-enter" => {
                        if let Some(Value::String(fn_name)) = args.get(i + 1) {
                            on_enter = Some(fn_name.clone());
                        }
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
            ctx.define_mode(name.clone(), read_only, on_enter);
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "mode-bind-key",
        "(mode-bind-key mode-name key handler)",
        "Add a keybinding to a registered mode.",
        |args, ctx| {
            let (Some(Value::String(mode)), Some(Value::String(key)), Some(Value::String(handler))) =
                (args.first(), args.get(1), args.get(2))
            else {
                return Err("mode-bind-key expects (string string string)".to_string());
            };
            ctx.mode_bind_key(mode.clone(), key.clone(), handler.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "set-buffer-mode",
        "(set-buffer-mode name)",
        "Activate a named mode on the current buffer.",
        |args, ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Err("set-buffer-mode expects a name string".to_string());
            };
            ctx.set_buffer_mode(name.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "current-buffer-mode",
        "(current-buffer-mode)",
        "Return the current buffer's mode name.",
        |_args, ctx| Ok(Value::String(ctx.current_buffer_mode())),
    );

    runtime.register_native_with_docs(
        "create-buffer",
        "(create-buffer name)",
        "Create a new scratch buffer and switch to it.",
        |args, ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Err("create-buffer expects a name string".to_string());
            };
            ctx.create_buffer(name.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "switch-to-buffer",
        "(switch-to-buffer name)",
        "Switch to a buffer by name.",
        |args, ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Err("switch-to-buffer expects a name string".to_string());
            };
            ctx.switch_to_buffer(name.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "buffer-list",
        "(buffer-list)",
        "Return a list of buffer name strings.",
        |_args, ctx| {
            let names = ctx.buffer_names();
            Ok(Value::List(
                names
                    .into_iter()
                    .map(|n| Rc::new(RefCell::new(Value::String(n))))
                    .collect(),
            ))
        },
    );

    runtime.register_native_with_docs(
        "set-buffer-text",
        "(set-buffer-text text)",
        "Replace the active buffer's contents.",
        |args, ctx| {
            let Some(Value::String(text)) = args.first() else {
                return Err("set-buffer-text expects a string".to_string());
            };
            ctx.set_buffer_text(text.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "set-buffer-lines",
        "(set-buffer-lines lines)",
        "Set the buffer contents from a list of line strings.",
        |args, ctx| {
            let Some(Value::List(items)) = args.first() else {
                return Err("set-buffer-lines expects a list".to_string());
            };
            let lines: Vec<String> = items
                .iter()
                .map(|item| match &*item.borrow() {
                    Value::String(s) => s.clone(),
                    other => format_lisp_value(other),
                })
                .collect();
            ctx.set_buffer_lines(lines);
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "goto-line",
        "(goto-line n)",
        "Move cursor to line n (1-indexed).",
        |args, ctx| {
            let Some(Value::Number(n)) = args.first() else {
                return Err("goto-line expects a number".to_string());
            };
            ctx.goto_line(*n as usize);
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "current-line-number",
        "(current-line-number)",
        "Return the current cursor line number (1-indexed).",
        |_args, ctx| Ok(Value::Number(ctx.current_line_number() as f64)),
    );

    runtime.register_native_with_docs(
        "current-line-text",
        "(current-line-text)",
        "Return the text of the current line.",
        |_args, ctx| Ok(Value::String(ctx.current_line_text())),
    );

    // ── Filesystem utilities ─────────────────────────────────────────────────

    runtime.register_native_with_docs(
        "list-directory",
        "(list-directory path)",
        "List directory entries as maps with :name, :directory, and :size keys.",
        |args, _ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("list-directory expects a path string".to_string());
            };
            let entries = std::fs::read_dir(path).map_err(|e| format!("list-directory: {e}"))?;
            let mut result = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|e| format!("list-directory: {e}"))?;
                let metadata = entry
                    .metadata()
                    .map_err(|e| format!("list-directory: {e}"))?;
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = metadata.is_dir();
                let size = metadata.len();
                let mut map = HashMap::new();
                map.insert(
                    "name".to_string(),
                    Rc::new(RefCell::new(Value::String(name))),
                );
                map.insert(
                    "directory".to_string(),
                    Rc::new(RefCell::new(Value::Bool(is_dir))),
                );
                map.insert(
                    "size".to_string(),
                    Rc::new(RefCell::new(Value::Number(size as f64))),
                );
                result.push(Rc::new(RefCell::new(Value::Map(map))));
            }
            Ok(Value::List(result))
        },
    );

    runtime.register_native_with_docs(
        "current-directory",
        "(current-directory)",
        "Return the current working directory as a string.",
        |_args, _ctx| {
            let cwd = std::env::current_dir().map_err(|e| format!("current-directory: {e}"))?;
            Ok(Value::String(cwd.display().to_string()))
        },
    );

    runtime.register_native_with_docs(
        "path-join",
        "(path-join a b)",
        "Join two path components.",
        |args, _ctx| {
            let (Some(Value::String(a)), Some(Value::String(b))) = (args.first(), args.get(1))
            else {
                return Err("path-join expects two strings".to_string());
            };
            let result = PathBuf::from(a).join(b);
            Ok(Value::String(result.display().to_string()))
        },
    );

    runtime.register_native_with_docs(
        "path-parent",
        "(path-parent path)",
        "Return the parent directory of a path, or nil.",
        |args, _ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("path-parent expects a string".to_string());
            };
            Ok(match PathBuf::from(path).parent() {
                Some(parent) => Value::String(parent.display().to_string()),
                None => Value::Nil,
            })
        },
    );

    runtime.register_native_with_docs(
        "path-filename",
        "(path-filename path)",
        "Return the filename component of a path, or nil.",
        |args, _ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("path-filename expects a string".to_string());
            };
            Ok(match PathBuf::from(path).file_name() {
                Some(name) => Value::String(name.to_string_lossy().to_string()),
                None => Value::Nil,
            })
        },
    );

    runtime.register_native_with_docs(
        "file-exists?",
        "(file-exists? path)",
        "Return true if a file or directory exists at path.",
        |args, _ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("file-exists? expects a string".to_string());
            };
            Ok(Value::Bool(Path::new(path).exists()))
        },
    );

    runtime.register_native_with_docs(
        "directory?",
        "(directory? path)",
        "Return true if path is a directory.",
        |args, _ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("directory? expects a string".to_string());
            };
            Ok(Value::Bool(Path::new(path).is_dir()))
        },
    );

    runtime.register_native_with_docs(
        "read-file-to-string",
        "(read-file-to-string path)",
        "Read a file's contents as a string.",
        |args, _ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("read-file-to-string expects a string".to_string());
            };
            let contents =
                std::fs::read_to_string(path).map_err(|e| format!("read-file-to-string: {e}"))?;
            Ok(Value::String(contents))
        },
    );

    runtime.register_native_with_docs(
        "render-widget",
        "(render-widget tree)",
        "Render a widget tree in the current buffer's overlay.",
        |args, ctx| {
            let Some(tree) = args.into_iter().next() else {
                return Err("render-widget expects a widget tree value".to_string());
            };
            ctx.render_widget(tree);
            Ok(Value::Nil)
        },
    );

    runtime.register_native_with_docs(
        "create-scratch",
        "(create-scratch name text)",
        "Create a scratch buffer with the given name and text. Does not switch to it.",
        |args, ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Err("create-scratch expects (name text)".to_string());
            };
            let text = match args.get(1) {
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            ctx.create_scratch(name.clone(), text);
            Ok(Value::Bool(true))
        },
    );

    // ── Tiling / window management ──────────────────────────────────────────

    runtime.register_native_with_docs(
        "split-window-right",
        "(split-window-right &optional buffer-name)",
        "Split the current window vertically. Optionally show a specific buffer in the new tile.",
        |args, ctx| {
            let name = match args.first() {
                Some(Value::String(s)) => Some(s.clone()),
                _ => None,
            };
            ctx.split_window_right(name);
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "split-window-below",
        "(split-window-below &optional buffer-name)",
        "Split the current window horizontally. Optionally show a specific buffer in the new tile.",
        |args, ctx| {
            let name = match args.first() {
                Some(Value::String(s)) => Some(s.clone()),
                _ => None,
            };
            ctx.split_window_below(name);
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "delete-window",
        "(delete-window)",
        "Close the current tile (C-x 0). The buffer is not deleted.",
        |_args, ctx| {
            ctx.delete_window();
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "delete-other-windows",
        "(delete-other-windows)",
        "Close all tiles except the current one (C-x 1).",
        |_args, ctx| {
            ctx.delete_other_windows();
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "other-window",
        "(other-window)",
        "Cycle focus to the next tile (C-x o).",
        |_args, ctx| {
            ctx.other_window();
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "set-window-buffer",
        "(set-window-buffer name)",
        "Show a different buffer in the current tile.",
        |args, ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Err("set-window-buffer expects a buffer name string".to_string());
            };
            ctx.set_window_buffer(name.clone());
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "window-hide-status",
        "(window-hide-status)",
        "Toggle the status bar for the current tile.",
        |_args, ctx| {
            ctx.window_hide_status();
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "resize-window",
        "(resize-window delta)",
        "Adjust the parent split ratio by delta (e.g. 0.05 or -0.05).",
        |args, ctx| {
            let Some(Value::Number(delta)) = args.first() else {
                return Err("resize-window expects a number".to_string());
            };
            ctx.resize_window(*delta);
            Ok(Value::Bool(true))
        },
    );

    runtime.register_native_with_docs(
        "open-file",
        "(open-file path)",
        "Open a file into a new file-backed buffer and switch to it.",
        |args, ctx| {
            let Some(Value::String(path)) = args.first() else {
                return Err("open-file expects a path string".to_string());
            };
            ctx.open_file(path.clone());
            Ok(Value::Bool(true))
        },
    );

    // ── String utilities ─────────────────────────────────────────────────────

    runtime.register_native_with_docs(
        "substring",
        "(substring s start [end])",
        "Extract a substring by character index.",
        |args, _ctx| {
            let Some(Value::String(s)) = args.first() else {
                return Err("substring expects a string".to_string());
            };
            let Some(Value::Number(start)) = args.get(1) else {
                return Err("substring expects a start index".to_string());
            };
            let start = (*start as usize).min(s.len());
            let end = match args.get(2) {
                Some(Value::Number(e)) => (*e as usize).min(s.len()),
                _ => s.len(),
            };
            Ok(Value::String(s.get(start..end).unwrap_or("").to_string()))
        },
    );

    runtime.register_native_with_docs(
        "string-starts-with?",
        "(string-starts-with? s prefix)",
        "Return true if string starts with prefix.",
        |args, _ctx| {
            let (Some(Value::String(s)), Some(Value::String(prefix))) = (args.first(), args.get(1))
            else {
                return Err("string-starts-with? expects two strings".to_string());
            };
            Ok(Value::Bool(s.starts_with(prefix.as_str())))
        },
    );

    runtime.register_native_with_docs(
        "string-ends-with?",
        "(string-ends-with? s suffix)",
        "Return true if string ends with suffix.",
        |args, _ctx| {
            let (Some(Value::String(s)), Some(Value::String(suffix))) = (args.first(), args.get(1))
            else {
                return Err("string-ends-with? expects two strings".to_string());
            };
            Ok(Value::Bool(s.ends_with(suffix.as_str())))
        },
    );

    runtime.register_native_with_docs(
        "cycle-view-mode",
        "(cycle-view-mode)",
        "Toggle the primary view mode between ui and text. Returns the new mode name.",
        |_args, ctx| {
            ctx.cycle_view_mode();
            Ok(Value::String("cycled".to_string()))
        },
    );

    runtime.register_native_with_docs(
        "set-view-mode",
        "(set-view-mode mode)",
        "Set the view mode to \"ui\", \"text\", or \"both\". Returns the requested mode.",
        |args, ctx| {
            let Some(Value::String(mode)) = args.first() else {
                return Err("set-view-mode expects a mode string".to_string());
            };
            ctx.set_view_mode(mode.clone());
            Ok(Value::String(mode.clone()))
        },
    );

    runtime.register_native_with_docs(
        "view-mode",
        "(view-mode)",
        "Return the current view mode: \"both\", \"ui\", or \"text\".",
        |_args, ctx| Ok(Value::String(ctx.current_view_mode())),
    );

    runtime.register_native_with_docs(
        "string-trim",
        "(string-trim s)",
        "Remove leading and trailing whitespace.",
        |args, _ctx| {
            let Some(Value::String(s)) = args.first() else {
                return Err("string-trim expects a string".to_string());
            };
            Ok(Value::String(s.trim().to_string()))
        },
    );

    runtime.register_native_with_docs(
        "apply-theme",
        "(apply-theme map)",
        "Apply a theme from a map of field name → color entries. Only fields present in the map are updated.",
        |args, ctx| {
            let Some(Value::Map(_)) = args.first() else {
                return Err("apply-theme expects a map".to_string());
            };
            ctx.apply_theme(args.into_iter().next().unwrap());
            Ok(Value::Bool(true))
        },
    );
}

pub(super) fn extract_suggested_name(payload: &Value) -> Option<String> {
    let Value::Map(map) = payload else {
        return None;
    };
    let value = map.get("suggested-name").or_else(|| map.get("name"))?;
    match &*value.borrow() {
        Value::String(name) if !name.is_empty() => Some(name.clone()),
        _ => None,
    }
}
