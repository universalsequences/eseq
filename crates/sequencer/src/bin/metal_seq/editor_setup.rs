use eseqlisp::backend::Backend;
use eseqlisp::metal_backend::MetalBackend;
use eseqlisp::{Editor, EditorConfig, Runtime};
use sequencer::ui;

use super::custom_ui::reload_custom_instrument_ui;
use super::state_values::push_project_scratch_to_named_buffer;

const METAL_SEQ_TEXT_FONT_SIZE_PT: f64 = 13.0;
const STARTUP_GRID_LAYOUT_EXPR: &str = "(seq-apply-fx-layout)";

pub(crate) fn create_editor_and_backend(
    runtime: Runtime,
    app: &ui::App,
) -> Result<(Editor, MetalBackend), Box<dyn std::error::Error>> {
    let (init_src, init_path) = read_eseqlisp_init_source();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init_src),
            init_source_path: init_path,
            vim_mode: true,
        },
    );

    reload_custom_instrument_ui(&mut editor);
    let _ = editor.open_or_create_file_buffer("metal-seq-grid.lisp");
    let grid_source = editor.active_buffer().text();
    let overlays = editor.snapshot_file_backed_sources();
    let report = editor.runtime_mut().eval_source_transactional(
        Some(std::path::PathBuf::from("metal-seq-grid.lisp")),
        &grid_source,
        overlays,
    );
    if !report.success {
        return Err(format!(
            "failed to load metal-seq-grid.lisp: {}",
            report.failure_message()
        )
        .into());
    }
    editor.process_lisp_reload_report(report);
    apply_startup_grid_layout(&mut editor)?;
    log_lisp_ui_load_diagnostics(&mut editor);
    reload_custom_instrument_ui(&mut editor);
    push_project_scratch_to_named_buffer(&mut editor, &app);

    let mut backend =
        MetalBackend::new_with_size_and_font_size(1250, 850, METAL_SEQ_TEXT_FONT_SIZE_PT)
            .map_err(|_| "Metal backend creation failed")?;
    backend
        .initialize()
        .map_err(|_| "Metal backend init failed")?;

    {
        let (cell_w, cell_h) = backend.cell_dimensions();
        if let Some((text_cell_w, text_cell_h)) = backend.sync_text_zoom(editor.text_zoom()) {
            editor.set_text_cell_dimensions(cell_w, cell_h, text_cell_w, text_cell_h);
        }
        if let Some(measurer) = backend.create_text_measurer() {
            editor.set_text_measurer(measurer, cell_w, cell_h);
        }
    }

    Ok((editor, backend))
}

pub(crate) fn apply_startup_grid_layout(
    editor: &mut Editor,
) -> Result<(), Box<dyn std::error::Error>> {
    editor
        .runtime_mut()
        .eval_str(STARTUP_GRID_LAYOUT_EXPR)
        .map_err(|err| format!("failed to apply startup grid layout: {err:?}"))?;
    editor.refresh_runtime_side_effects();
    Ok(())
}

fn read_eseqlisp_init_source() -> (String, Option<std::path::PathBuf>) {
    eseqlisp_init_candidates()
        .into_iter()
        .find_map(|path| {
            std::fs::read_to_string(&path)
                .ok()
                .map(|source| (source, Some(path)))
        })
        .unwrap_or_default()
}

fn eseqlisp_init_candidates() -> Vec<std::path::PathBuf> {
    sequencer::paths::eseqlisp_init_candidates()
}

fn log_lisp_ui_load_diagnostics(editor: &mut Editor) {
    if let Some(status) = editor.runtime_mut().take_status_message() {
        eprintln!("metal_seq: Lisp UI status during startup: {status}");
    }

    for name in [
        "*metal*",
        "*sequencer*",
        "*samples*",
        "*track*",
        "*fx*",
        "*piano-roll*",
        "*mixer*",
        "*transport*",
    ] {
        match editor.buffers.iter().find(|buffer| buffer.name == name) {
            Some(buffer) if buffer.widget_tree.is_some() => {}
            Some(_) => {
                eprintln!("metal_seq: Lisp UI buffer {name} exists but has no widget tree");
            }
            None => {
                eprintln!("metal_seq: Lisp UI buffer {name} was not created");
            }
        }
    }
}
