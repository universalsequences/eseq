use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use crate::buffer::BufferTextStyle;
use crate::runtime::Runtime;
use crate::theme;
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
        "(define-mode name :read-only bool :on-enter fn-name :on-key fn-name)",
        "Register a named major mode.",
        |args, ctx| {
            let Some(Value::String(name)) = args.first() else {
                return Err("define-mode expects a name string".to_string());
            };
            let mut read_only = false;
            let mut on_enter: Option<String> = None;
            let mut on_key: Option<String> = None;
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
                    Some(Value::Keyword(k)) if k == "on-key" => {
                        if let Some(Value::String(fn_name)) = args.get(i + 1) {
                            on_key = Some(fn_name.clone());
                        }
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
            ctx.define_mode(name.clone(), read_only, on_enter, on_key);
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
        "set-buffer-mode-for",
        "(set-buffer-mode-for buffer-name mode-name)",
        "Set a named mode on a specific buffer (by name).",
        |args, ctx| {
            let (Some(Value::String(buf_name)), Some(Value::String(mode_name))) =
                (args.first(), args.get(1))
            else {
                return Err("set-buffer-mode-for expects (buffer-name mode-name)".to_string());
            };
            ctx.set_buffer_mode_for(buf_name.clone(), mode_name.clone());
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
        "set-buffer-styles",
        "(set-buffer-styles styles)",
        "Set buffer-local text style spans from a list of maps.",
        |args, ctx| {
            let Some(Value::List(items)) = args.first() else {
                return Err("set-buffer-styles expects a list".to_string());
            };
            let mut styles = Vec::with_capacity(items.len());
            for item in items {
                let Value::Map(map) = &*item.borrow() else {
                    return Err("set-buffer-styles items must be maps".to_string());
                };
                styles.push(parse_buffer_text_style(map)?);
            }
            ctx.set_buffer_styles(styles);
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
                let info = describe_directory_entry(&entry.path(), &metadata, &name);
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
                map.insert(
                    "display".to_string(),
                    Rc::new(RefCell::new(Value::String(info.display))),
                );
                map.insert(
                    "permissions".to_string(),
                    Rc::new(RefCell::new(Value::String(info.permissions))),
                );
                map.insert(
                    "links".to_string(),
                    Rc::new(RefCell::new(Value::Number(info.links as f64))),
                );
                map.insert(
                    "owner".to_string(),
                    Rc::new(RefCell::new(Value::String(info.owner))),
                );
                map.insert(
                    "group".to_string(),
                    Rc::new(RefCell::new(Value::String(info.group))),
                );
                map.insert(
                    "modified".to_string(),
                    Rc::new(RefCell::new(Value::String(info.modified))),
                );
                result.push(Rc::new(RefCell::new(Value::Map(map))));
            }
            result.sort_by(|a, b| {
                let a_name = match &*a.borrow() {
                    Value::Map(map) => map
                        .get("name")
                        .and_then(|v| match &*v.borrow() {
                            Value::String(s) => Some(s.to_ascii_lowercase()),
                            _ => None,
                        })
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                let b_name = match &*b.borrow() {
                    Value::Map(map) => map
                        .get("name")
                        .and_then(|v| match &*v.borrow() {
                            Value::String(s) => Some(s.to_ascii_lowercase()),
                            _ => None,
                        })
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                a_name.cmp(&b_name)
            });
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
        "render-widget-to-buffer",
        "(render-widget-to-buffer buffer-name tree)",
        "Render a widget tree in a named buffer's overlay without switching to it.",
        |args, ctx| {
            println!("render widget to buffer called");
            let mut args = args.into_iter();
            println!("args {:?}", args);
            let Some(Value::String(name)) = args.next() else {
                println!("case 1");
                return Err("render-widget-to-buffer expects a buffer name string".to_string());
            };
            let Some(tree) = args.next() else {
                println!("case 2");
                return Err("render-widget-to-buffer expects a widget tree value".to_string());
            };
            ctx.render_widget_to_buffer(name, tree);
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
        "set-layout",
        "(set-layout spec)",
        "Set the window layout declaratively. Spec is a nested list:\n\
         (:rows ratio (:cols ratio \"buf-a\" ratio \"buf-b\" ...) ratio \"buf-c\" ...)\n\
         :rows splits horizontally (top/bottom), :cols splits vertically (left/right).\n\
         Each pane is preceded by its ratio (fraction of parent space).",
        |args, ctx| {
            use crate::runtime::LayoutSpec;

            fn parse_spec(val: &Value) -> Result<LayoutSpec, String> {
                match val {
                    Value::String(s) => Ok(LayoutSpec::Buffer { name: s.clone(), hide_status: false, borderless: false, min_width: None, min_height: None, max_width: None, max_height: None }),
                    Value::List(items) if !items.is_empty() && matches!(&*items[0].borrow(), Value::Keyword(k) if k == "buf") => {
                        // (:buf "name" :hide-status true :min-width 20 :min-height 10)
                        let name = items.get(1)
                            .and_then(|v| match &*v.borrow() {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            })
                            .ok_or("(:buf ...) requires a buffer name string")?;
                        let mut hide_status = false;
                        let mut borderless = false;
                        let mut min_width: Option<f32> = None;
                        let mut min_height: Option<f32> = None;
                        let mut max_width: Option<f32> = None;
                        let mut max_height: Option<f32> = None;
                        let mut i = 2;
                        while i < items.len() {
                            if let Value::Keyword(k) = &*items[i].borrow() {
                                let key = k.clone();
                                i += 1;
                                if i < items.len() {
                                    let v = items[i].borrow();
                                    match key.as_str() {
                                        "hide-status" => match &*v {
                                            Value::Bool(b) => hide_status = *b,
                                            Value::Symbol(s) if s == "true" => hide_status = true,
                                            Value::Symbol(s) if s == "false" => hide_status = false,
                                            _ => {}
                                        },
                                        "borderless" => match &*v {
                                            Value::Bool(b) => borderless = *b,
                                            Value::Symbol(s) if s == "true" => borderless = true,
                                            Value::Symbol(s) if s == "false" => borderless = false,
                                            _ => {}
                                        },
                                        "min-width" => if let Value::Number(n) = &*v { min_width = Some(*n as f32) },
                                        "min-height" => if let Value::Number(n) = &*v { min_height = Some(*n as f32) },
                                        "max-width" => if let Value::Number(n) = &*v { max_width = Some(*n as f32) },
                                        "max-height" => if let Value::Number(n) = &*v { max_height = Some(*n as f32) },
                                        _ => {}
                                    }
                                }
                            }
                            i += 1;
                        }
                        Ok(LayoutSpec::Buffer { name, hide_status, borderless, min_width, min_height, max_width, max_height })
                    }
                    Value::List(items) => {
                        if items.is_empty() {
                            return Err("empty layout spec".into());
                        }
                        let first = items[0].borrow();
                        let dir = match &*first {
                            Value::Keyword(k) if k == "rows" => "rows",
                            Value::Keyword(k) if k == "cols" => "cols",
                            _ => return Err("layout list must start with :rows or :cols".into()),
                        };
                        // Parse alternating ratio/spec pairs: :rows 0.7 spec1 0.3 spec2
                        let mut panes = Vec::new();
                        let mut i = 1;
                        while i < items.len() {
                            let ratio_val = items[i].borrow();
                            let ratio = match &*ratio_val {
                                Value::Number(n) => *n as f32,
                                _ => return Err(format!("expected ratio number at position {i}")),
                            };
                            drop(ratio_val);
                            i += 1;
                            if i >= items.len() {
                                return Err("expected spec after ratio".into());
                            }
                            let spec = parse_spec(&items[i].borrow())?;
                            panes.push((ratio, spec));
                            i += 1;
                        }
                        if panes.is_empty() {
                            return Err("no panes in layout spec".into());
                        }
                        Ok(match dir {
                            "rows" => LayoutSpec::Rows(panes),
                            _ => LayoutSpec::Cols(panes),
                        })
                    }
                    _ => Err("layout spec must be a string or list".into()),
                }
            }

            let spec_val = args.first().ok_or("set-layout: expected spec argument")?;
            let spec = parse_spec(spec_val).map_err(|e| format!("set-layout: {e}"))?;
            ctx.set_layout(spec);
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
        "string-contains?",
        "(string-contains? s needle)",
        "Return true if string contains needle.",
        |args, _ctx| {
            let (Some(Value::String(s)), Some(Value::String(needle))) = (args.first(), args.get(1))
            else {
                return Err("string-contains? expects two strings".to_string());
            };
            Ok(Value::Bool(s.contains(needle.as_str())))
        },
    );

    runtime.register_native_with_docs(
        "string-downcase",
        "(string-downcase s)",
        "Return a lowercase copy of the string.",
        |args, _ctx| {
            let Some(Value::String(s)) = args.first() else {
                return Err("string-downcase expects a string".to_string());
            };
            Ok(Value::String(s.to_ascii_lowercase()))
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

struct DirectoryEntryInfo {
    display: String,
    permissions: String,
    links: u64,
    owner: String,
    group: String,
    modified: String,
}

fn describe_directory_entry(
    path: &Path,
    metadata: &std::fs::Metadata,
    name: &str,
) -> DirectoryEntryInfo {
    #[cfg(unix)]
    {
        if let Some(info) = describe_directory_entry_unix(path, metadata, name) {
            return info;
        }
    }

    let permissions = if metadata.permissions().readonly() {
        "-r--r--r--"
    } else if metadata.is_dir() {
        "drwxr-xr-x"
    } else {
        "-rw-r--r--"
    };
    let display_name = if metadata.is_dir() {
        format!("{name}/")
    } else {
        name.to_string()
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|time| time.as_secs().to_string())
        .unwrap_or_else(|| "-".to_string());
    let display = format!(
        "{permissions} {:>2} {:>6} {modified:>12} {display_name}",
        1,
        metadata.len()
    );
    DirectoryEntryInfo {
        display,
        permissions: permissions.to_string(),
        links: 1,
        owner: "-".to_string(),
        group: "-".to_string(),
        modified,
    }
}

#[cfg(unix)]
fn describe_directory_entry_unix(
    path: &Path,
    metadata: &std::fs::Metadata,
    name: &str,
) -> Option<DirectoryEntryInfo> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let display_name = if metadata.is_dir() {
        format!("{name}/")
    } else {
        name.to_string()
    };

    let stat_output = Command::new("stat")
        .args(["-f", "%Sp\t%l\t%Su\t%Sg\t%z\t%Sm", "-t", "%b %e %H:%M"])
        .arg(path)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok());

    if let Some(output) = stat_output {
        let line = output.trim_end();
        let mut parts = line.split('\t');
        let permissions = parts.next()?.to_string();
        let links = parts.next()?.parse::<u64>().ok()?;
        let owner = parts.next()?.to_string();
        let group = parts.next()?.to_string();
        let size = parts.next()?.parse::<u64>().ok()?;
        let modified = parts.next()?.to_string();
        let size_text = format_size(size);
        let display = format!(
            "{permissions} {links:>2} {size_text:>6} {modified:>12} {display_name}"
        );
        return Some(DirectoryEntryInfo {
            display,
            permissions,
            links,
            owner,
            group,
            modified,
        });
    }

    let mode = metadata.permissions().mode();
    let file_type = if metadata.is_dir() { 'd' } else { '-' };
    let permissions = format!(
        "{}{}{}{}{}{}{}{}{}{}",
        file_type,
        bit(mode, 0o400, 'r'),
        bit(mode, 0o200, 'w'),
        bit(mode, 0o100, 'x'),
        bit(mode, 0o040, 'r'),
        bit(mode, 0o020, 'w'),
        bit(mode, 0o010, 'x'),
        bit(mode, 0o004, 'r'),
        bit(mode, 0o002, 'w'),
        bit(mode, 0o001, 'x')
    );
    let links = metadata.nlink();
    let owner = metadata.uid().to_string();
    let group = metadata.gid().to_string();
    let modified = metadata.mtime().to_string();
    let size_text = format_size(metadata.size());
    let display = format!(
        "{permissions} {links:>2} {size_text:>6} {modified:>12} {display_name}",
    );
    Some(DirectoryEntryInfo {
        display,
        permissions,
        links,
        owner,
        group,
        modified,
    })
}

#[cfg(unix)]
fn bit(mode: u32, flag: u32, ch: char) -> char {
    if mode & flag != 0 { ch } else { '-' }
}

fn format_size(size: u64) -> String {
    const UNITS: [&str; 5] = ["", "K", "M", "G", "T"];
    let mut value = size as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        size.to_string()
    } else if value >= 10.0 {
        format!("{value:.0}{}", UNITS[unit])
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

fn parse_buffer_text_style(
    map: &HashMap<String, Rc<RefCell<Value>>>,
) -> Result<BufferTextStyle, String> {
    Ok(BufferTextStyle {
        line: map.get("line").and_then(|v| match &*v.borrow() {
            Value::Number(n) if *n >= 0.0 => Some(*n as usize),
            _ => None,
        }),
        current_line: map
            .get("current-line")
            .is_some_and(|v| matches!(&*v.borrow(), Value::Bool(true))),
        start: map.get("start").and_then(|v| match &*v.borrow() {
            Value::Number(n) if *n >= 0.0 => Some(*n as usize),
            _ => None,
        }),
        end: map.get("end").and_then(|v| match &*v.borrow() {
            Value::Number(n) if *n >= 0.0 => Some(*n as usize),
            _ => None,
        }),
        full_line: map
            .get("full-line")
            .is_some_and(|v| matches!(&*v.borrow(), Value::Bool(true))),
        fg: map.get("fg").map(parse_style_color).transpose()?,
        bg: map.get("bg").map(parse_style_color).transpose()?,
        bold: map
            .get("bold")
            .is_some_and(|v| matches!(&*v.borrow(), Value::Bool(true))),
    })
}

fn parse_style_color(value: &Rc<RefCell<Value>>) -> Result<crate::backend::Color, String> {
    match &*value.borrow() {
        Value::Keyword(name) | Value::String(name) => {
            theme::named_color(name).ok_or_else(|| format!("Unknown style color '{name}'"))
        }
        _ => Err("Style colors must be keywords or strings".to_string()),
    }
}
