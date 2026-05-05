use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use eseqlisp::backend::Backend;
use eseqlisp::metal_backend::MetalBackend;
use eseqlisp::{Editor, EditorConfig, Runtime};
use sequencer::ui;

use super::custom_ui::reload_custom_instrument_ui;
use super::state_values::push_project_scratch_to_named_buffer;

pub(crate) fn create_editor_and_backend(
    runtime: Runtime,
    app: &ui::App,
) -> Result<(Editor, MetalBackend), Box<dyn std::error::Error>> {
    let init_src = std::fs::read_to_string("init.lisp")
        .or_else(|_| std::fs::read_to_string("../eseqlisp/init.lisp"))
        .unwrap_or_default();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(init_src),
            vim_mode: true,
        },
    );

    reload_custom_instrument_ui(&mut editor);
    let _ = editor.open_or_create_file_buffer("metal-seq-grid.lisp");
    editor.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    editor.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    reload_custom_instrument_ui(&mut editor);
    push_project_scratch_to_named_buffer(&mut editor, &app);

    let mut backend =
        MetalBackend::new_with_size(1100, 700).map_err(|_| "Metal backend creation failed")?;
    backend
        .initialize()
        .map_err(|_| "Metal backend init failed")?;

    {
        let (cell_w, cell_h) = backend.cell_dimensions();
        if let Some(measurer) = backend.create_text_measurer() {
            editor.set_text_measurer(measurer, cell_w, cell_h);
        }
    }

    Ok((editor, backend))
}
