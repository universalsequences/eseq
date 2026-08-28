use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "remove-project-script-from-scratch",
    "open-script-source-tab",
];

pub(super) fn handle(
    name: &str,
    payload: Value,
    _app: &mut app::App,
    editor: &mut Editor,
    _ctx: &mut LoopCtx<'_>,
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
            remove_project_script_from_scratch(editor, &source_path);
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
            match register_script_source_tab(editor, Path::new(&path_str), &label, &path_str) {
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
                Err(error) => editor.handle_host_event(HostEvent::Status(format!(
                    "Error opening script source: {error}"
                ))),
            }
        }
        _ => {}
    }
}
