use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "project-save-open",
    "project-new-request",
    "open-help",
    "open-url",
    "about-open",
];

/// The Save / Save As / About modals are mounted in the step-panel buffers,
/// and a modal only receives pointer input through the active tile.
pub(super) fn activate_dialog_tile(editor: &mut Editor) {
    if !editor.switch_active_tile_to_buffer_named("*arrangement*") {
        editor.switch_active_tile_to_buffer_named("*sequencer*");
    }
}

pub(super) fn lisp_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    let status = std::process::Command::new(program)
        .arg(url)
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Could not open {url} ({status})"))
    }
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let result = (|| -> Result<(), String> {
        match name {
            "project-save-open" => {
                let (mode, then) = match payload {
                    Value::Map(ref map) => (
                        map_string(map, "mode").unwrap_or_default(),
                        map_string(map, "then").unwrap_or_default(),
                    ),
                    _ => (String::new(), String::new()),
                };
                let current = app.current_project_name.clone().unwrap_or_default();
                if mode != "save-as" && !current.is_empty() {
                    let mut map = std::collections::HashMap::new();
                    map.insert(
                        "then".to_string(),
                        std::rc::Rc::new(std::cell::RefCell::new(Value::String(then))),
                    );
                    super::project::handle("save-project", Value::Map(map), app, editor, ctx);
                    return Ok(());
                }
                let title = if mode == "save-as" {
                    "Save project as"
                } else {
                    "Save project"
                };
                activate_dialog_tile(editor);
                editor
                    .runtime_mut()
                    .eval_str(&format!(
                        "(eseq.file-dialogs/open-save {} {} {})",
                        lisp_string(title),
                        lisp_string(&current),
                        lisp_string(&then)
                    ))
                    .map_err(|e| format!("{e:?}"))?;
            }
            "project-new-request" => {
                if !app.has_unsaved_changes() {
                    super::project::handle("new-project", Value::Nil, app, editor, ctx);
                    return Ok(());
                }
                activate_dialog_tile(editor);
                editor
                    .runtime_mut()
                    .eval_str("(eseq.file-dialogs/open-unsaved-prompt)")
                    .map_err(|e| format!("{e:?}"))?;
            }
            "open-help" => {
                // The File menu lives in the transport strip; open the manual
                // in the main panel tile rather than replacing the transport.
                activate_dialog_tile(editor);
                editor
                    .runtime_mut()
                    .eval_str("(eseq.manual/open-manual)")
                    .map_err(|e| format!("{e:?}"))?;
            }
            "open-url" => {
                let url = match payload {
                    Value::Map(ref map) => map_string(map, "url").unwrap_or_default(),
                    _ => String::new(),
                };
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return Err(format!("open-url: refusing non-http(s) target {url:?}"));
                }
                open_url(&url)?;
            }
            "about-open" => {
                activate_dialog_tile(editor);
                editor
                    .runtime_mut()
                    .eval_str(&format!(
                        "(eseq.file-dialogs/open-about {})",
                        lisp_string(env!("CARGO_PKG_VERSION"))
                    ))
                    .map_err(|e| format!("{e:?}"))?;
            }
            _ => {}
        }
        Ok(())
    })();
    if let Err(error) = result {
        editor.show_transient_message(error);
    }
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}
