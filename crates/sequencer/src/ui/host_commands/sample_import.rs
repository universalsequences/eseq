use crate::*;

/// Finishes the drag-and-drop sample import modal
/// (`content/ui/sample-import.lisp`). The draft itself lives in
/// `sample_import_ui`; these two commands need the editor for status text,
/// the browser refresh, and the modal close.
pub(super) const COMMANDS: &[&str] = &["sample-import-commit", "sample-import-cancel"];

pub(super) fn handle(
    name: &str,
    _payload: Value,
    _app: &mut app::App,
    editor: &mut Editor,
    _ctx: &mut LoopCtx<'_>,
) {
    match name {
        "sample-import-commit" => {
            let Some(draft) = take_draft() else {
                close_import_modal(editor);
                return;
            };
            let summary = draft.commit(
                &sequencer::app_paths::app_paths().sample_db_path(),
                &sequencer::app_paths::app_paths().samples_dir(),
            );
            match summary {
                Ok(summary) => {
                    close_import_modal(editor);
                    editor.show_transient_message(format!(
                        "Imported {} sample(s), skipped {} duplicate(s), {} failed",
                        summary.imported, summary.duplicates, summary.failed
                    ));
                    let _ = refresh_sample_browser_buffer(editor);
                }
                Err(error) => {
                    // The draft (titles, tags) is untouched by a failed
                    // commit: put it back and keep the modal open so the
                    // user can retry instead of re-staging everything.
                    install_draft(draft);
                    editor.show_transient_message(format!("Sample import failed: {error}"));
                    editor.mark_needs_redraw();
                }
            }
        }
        "sample-import-cancel" => {
            let _ = take_draft();
            close_import_modal(editor);
            editor.show_transient_message("Sample import canceled");
        }
        _ => {}
    }
}

fn close_import_modal(editor: &mut Editor) {
    let _ = editor
        .runtime_mut()
        .eval_str("(eseq.sample-import/close)");
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}
