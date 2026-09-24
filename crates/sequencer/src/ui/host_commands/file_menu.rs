use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "project-save-open",
    "project-new-request",
    "project-quit-confirmed",
    "app-quit-request",
    "master-recording-saved",
    "reveal-path",
    "open-help",
    "open-url",
    "about-open",
    "settings-open",
    "midi-refresh",
    "midi-set-enabled",
];

/// The Save / Save As / About modals are mounted in the step-panel buffers,
/// and a modal only receives pointer input through the active tile.
/// Make the tile that mounts the file dialogs (`eseq.file-dialogs/panel`)
/// the active one, so the modal about to open is both rendered and reachable
/// by the pointer. The panel is mounted by the two step-panel buffers only;
/// while a script sequencer tab is in front, the main tile shows the script's
/// own buffer and neither switch finds a tile. In that case the step panel is
/// flipped back to the Seq tab first — one click to return to the script
/// beats a modal that opens where nothing renders it.
pub(crate) fn activate_dialog_tile(editor: &mut Editor) {
    if editor.switch_active_tile_to_buffer_named("*arrangement*")
        || editor.switch_active_tile_to_buffer_named("*sequencer*")
    {
        return;
    }
    if editor
        .runtime_mut()
        .eval_str("(eseq.seq-step-tabs/seq-select-main-step-tab-by-index 1)")
        .is_ok()
    {
        editor.refresh_runtime_side_effects();
        editor.switch_active_tile_to_buffer_named("*sequencer*");
    }
}

/// Every quit route (window close button, Quit eseq, the editor's quit
/// command) lands on `editor.should_quit()`. With unsaved project changes the
/// quit is held back and the Save / Don't Save / Cancel prompt opens instead;
/// returns true when it did.
pub(crate) fn intercept_unsaved_quit(
    app: &app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) -> bool {
    if ctx.sessions.quit_confirmed || !app.has_unsaved_changes() {
        return false;
    }
    editor.clear_quit_request();
    activate_dialog_tile(editor);
    if let Err(error) = editor
        .runtime_mut()
        .eval_str("(eseq.file-dialogs/open-unsaved-quit-prompt)")
    {
        // Never trap the user in the app because the prompt failed to open.
        eprintln!("unsaved quit prompt failed: {error:?}");
        return false;
    }
    editor.runtime_mut().run_reactive_cycle();
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
    true
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

/// Label for the toast link that reveals a file, named for the platform's
/// file manager.
const REVEAL_LABEL: &str = if cfg!(target_os = "macos") {
    "Show in Finder"
} else if cfg!(target_os = "windows") {
    "Show in Explorer"
} else {
    "Open Folder"
};

/// Show `path` in the platform file manager: selected in Finder / Explorer,
/// or its folder opened elsewhere.
pub(super) fn reveal_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // Explorer exits non-zero even when it opened the window.
        let mut select = std::ffi::OsString::from("/select,");
        select.push(path);
        std::process::Command::new("explorer")
            .arg(select)
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    #[cfg(not(target_os = "windows"))]
    {
        let status = command.status().map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("Could not show {} ({status})", path.display()));
        }
        Ok(())
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
            "settings-open" => {
                activate_dialog_tile(editor);
                editor.runtime_mut().eval_str("(eseq.settings/open-settings)")
                    .map_err(|e| format!("{e:?}"))?;
                super::audio_settings::publish(editor, None);
                if let Some(commands) = &ctx.sessions.midi_commands {
                    commands.send(sequencer::midi_input::service::Command::Refresh)
                        .map_err(|e| e.to_string())?;
                }
            }
            "midi-refresh" | "midi-set-enabled" => {
                use sequencer::midi_input::service::Command;
                let command = if name == "midi-refresh" { Command::Refresh } else {
                    let Value::Map(ref map) = payload else { return Err("Missing MIDI device".into()); };
                    let id = map_string(map, "id").ok_or("Missing MIDI device ID")?;
                    let enabled = map.get("enabled").and_then(|v| match &*v.borrow() {
                        Value::Bool(value) => Some(*value), _ => None,
                    }).ok_or("Missing MIDI enabled state")?;
                    Command::SetEnabled { id, enabled }
                };
                ctx.sessions.midi_commands.as_ref().ok_or("MIDI service is unavailable")?
                    .send(command).map_err(|e| e.to_string())?;
            }
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
            // The WAV transport button finished a take: keep the toast up
            // until dismissed, with a link to the file.
            "master-recording-saved" => {
                let Value::String(path) = payload else {
                    return Err("master-recording-saved expects a path".to_string());
                };
                let file = std::path::Path::new(&path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                editor.show_sticky_toast(
                    format!("Saved recording {file}"),
                    eseqlisp::ToastKind::Success,
                    Some(eseqlisp::ToastAction {
                        label: REVEAL_LABEL.to_string(),
                        command: HostCommand::Custom {
                            name: "reveal-path".to_string(),
                            payload: Value::String(path),
                        },
                    }),
                );
            }
            "reveal-path" => {
                let Value::String(path) = payload else {
                    return Err("reveal-path expects a path".to_string());
                };
                reveal_in_file_manager(std::path::Path::new(&path))?;
            }
            // File > Quit eseq. Only requests the quit; `intercept_unsaved_quit`
            // decides whether the unsaved-changes prompt comes first.
            "app-quit-request" => editor.request_quit(),
            "project-quit-confirmed" => {
                ctx.sessions.quit_confirmed = true;
                editor.request_quit();
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
