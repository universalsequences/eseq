use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "remove-project-script-from-scratch",
    "open-script-source-tab",
    "new-script",
    "save-new-script",
    "cancel-new-script",
];

#[allow(clippy::too_many_lines)]
pub(super) fn handle(
    name: &str,
    payload: Value,
    _app: &mut app::App,
    mut editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {

    match name {
        "remove-project-script-from-scratch" => {
            let Some(source_path) = extract_string_from_payload(&payload, "path")
                .filter(|path| !path.trim().is_empty())
            else {
                editor.handle_host_event(HostEvent::Status(
                    "Error removing project script: missing path".to_string(),
                ));
                return;
            };
            remove_project_script_from_scratch(&mut editor, &source_path);
        }
        "open-script-source-tab" => {
            let Some(path_str) = extract_string_from_payload(&payload, "path")
                .filter(|path| !path.trim().is_empty())
            else {
                editor.handle_host_event(HostEvent::Status(
                    "Error opening script source: missing path".to_string(),
                ));
                return;
            };
            let label = extract_string_from_payload(&payload, "label")
                .filter(|label| !label.trim().is_empty())
                .unwrap_or_else(|| {
                    Path::new(&path_str)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Script")
                        .to_string()
                });
            let read_only = extract_bool_from_payload(&payload, "read-only");
            match register_script_source_tab(
                &mut editor,
                Path::new(&path_str),
                &label,
                &path_str,
            ) {
                Ok(buffer_name) => {
                    if read_only {
                        if let Some(buffer) = editor
                            .buffers
                            .iter_mut()
                            .find(|buffer| buffer.name == buffer_name)
                        {
                            buffer.read_only = true;
                        }
                    }
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Opened script source: {label}"
                    )));
                }
                Err(error) => {
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error opening script source: {error}"
                    )));
                }
            }
        }
        "new-script" => {
            if ctx.sessions.script_draft_session.is_some() {
                editor.handle_host_event(HostEvent::Status(
                    "Finish the current script draft before creating another"
                        .to_string(),
                ));
                return;
            }
            let path = match create_new_script_draft_path() {
                Ok(path) => path,
                Err(error) => {
                    editor.handle_host_event(HostEvent::Status(error));
                    return;
                }
            };
            if let Err(error) = std::fs::write(&path, NEW_SCRIPT_TEMPLATE) {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Failed to write starter script: {error}"
                )));
                return;
            }
            match register_script_source_tab(
                &mut editor,
                &path,
                NEW_SCRIPT_TAB_LABEL,
                "",
            ) {
                Ok(buffer_name) => {
                    ctx.sessions.script_draft_session = Some(ScriptDraftSession {
                        temp_path: path,
                        buffer_name,
                    });
                    let rt = editor.runtime_mut();
                    let _ = rt.eval_str(
                        r#"
                                    (set! sbrowser-script-save-mode "new-script")
                                    (set! sbrowser-script-name "")
                                    (set! sbrowser-tab "scripts")
                                    "#,
                    );
                    rt.run_reactive_cycle();
                    editor.refresh_runtime_side_effects();
                    let _ = refresh_sample_browser_buffer(&mut editor);
                    editor.handle_host_event(HostEvent::Status(
                        "Created script draft".to_string(),
                    ));
                }
                Err(error) => {
                    let _ = std::fs::remove_file(&path);
                    editor.handle_host_event(HostEvent::Status(format!(
                        "Error creating script draft: {error}"
                    )));
                }
            }
        }
        "save-new-script" => {
            let requested_name =
                extract_string_from_payload(&payload, "name").unwrap_or_default();
            let Some(filename) = script_file_name_from_input(&requested_name) else {
                editor.handle_host_event(HostEvent::Status(
                    "Enter a script name".to_string(),
                ));
                return;
            };
            let Some(session) = ctx.sessions.script_draft_session.clone() else {
                editor.handle_host_event(HostEvent::Status(
                    "No script draft is active".to_string(),
                ));
                return;
            };
            let root = script_root_dir();
            if let Err(error) = std::fs::create_dir_all(&root) {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Failed to create script directory '{}': {error}",
                    root.display()
                )));
                return;
            }
            let target = root.join(&filename);
            if target.exists() {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Script already exists: {filename}"
                )));
                return;
            }
            let Some(buffer_idx) = editor
                .buffers
                .iter()
                .position(|buffer| buffer.name == session.buffer_name)
            else {
                editor.handle_host_event(HostEvent::Status(
                    "Script draft buffer is no longer open".to_string(),
                ));
                return;
            };
            let display_label = filename.trim_end_matches(".lisp").to_string();
            let mut source = editor.buffers[buffer_idx].text();
            if source.trim() == NEW_SCRIPT_TEMPLATE.trim() {
                let escaped_label = escape_lisp_string(&display_label);
                source = format!(
                    "; ESeqLisp script\n; Source-only scripts can still appear as sequencer tabs.\n(seq-register-script-source-tab \"{escaped_label}\")\n\n"
                );
                editor.buffers[buffer_idx].set_text(&source);
            }
            let tmp_path = target.with_file_name(format!(
                ".{}.tmp",
                target
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("script.lisp")
            ));
            if let Err(error) = std::fs::write(&tmp_path, &source).and_then(|_| {
                std::fs::rename(&tmp_path, &target).or_else(|error| {
                    let _ = std::fs::remove_file(&tmp_path);
                    Err(error)
                })
            }) {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Failed to save script: {error}"
                )));
                return;
            }
            editor.buffers[buffer_idx].path = Some(target.clone());
            editor.buffers[buffer_idx].dirty = false;
            if let Some(parent) = session.temp_path.parent() {
                let _ = std::fs::remove_dir_all(parent);
            }

            let target_str = target.to_string_lossy().replace('\\', "/");
            let load_form = format!(
                "(seq-script-load-file \"{}\")",
                escape_lisp_string(&target_str)
            );
            if let Err(error) = editor.runtime_mut().eval_str(&load_form) {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Saved script but failed to load it: {error:?}"
                )));
            }
            if let Err(error) = register_script_source_tab(
                &mut editor,
                &target,
                &display_label,
                &target_str,
            ) {
                editor.handle_host_event(HostEvent::Status(format!(
                    "Saved script but failed to register source tab: {error}"
                )));
            }
            ctx.sessions.script_draft_session = None;
            let rt = editor.runtime_mut();
            let _ = rt.eval_str(
                r#"
                            (set! sbrowser-script-save-mode "")
                            (set! sbrowser-script-name "")
                            (set! sbrowser-tab "scripts")
                            "#,
            );
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            let _ = refresh_sample_browser_buffer(&mut editor);
            editor.handle_host_event(HostEvent::Status(format!(
                "Saved script: {display_label}"
            )));
        }
        "cancel-new-script" => {
            if let Some(session) = ctx.sessions.script_draft_session.take() {
                let unregister = format!(
                    "(seq-unregister-step-sequencer-tab \"{}\")",
                    escape_lisp_string(&session.buffer_name)
                );
                let _ = editor.runtime_mut().eval_str(&unregister);
                editor.refresh_runtime_side_effects();
                editor.remove_buffer_by_name(&session.buffer_name);
                if let Some(parent) = session.temp_path.parent() {
                    let _ = std::fs::remove_dir_all(parent);
                }
            }
            let rt = editor.runtime_mut();
            let _ = rt.eval_str(
                r#"
                            (set! sbrowser-script-save-mode "")
                            (set! sbrowser-script-name "")
                            (set! sbrowser-tab "scripts")
                            "#,
            );
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            let _ = refresh_sample_browser_buffer(&mut editor);
            editor.handle_host_event(HostEvent::Status(
                "Cancelled script draft".to_string(),
            ));
        }
        _ => {}
    }
}
