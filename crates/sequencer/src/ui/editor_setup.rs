use crate::AppBackend;
use eseqlisp::backend::Backend;
use eseqlisp::{Editor, EditorConfig, Runtime};
use sequencer::app;

use super::constants::ui_entrypoint_path;
use super::custom_ui::reload_custom_instrument_ui;
use super::state_values::push_project_scratch_to_named_buffer;

pub(crate) const METAL_SEQ_TEXT_FONT_SIZE_PT: f64 = 13.0;
const STARTUP_GRID_LAYOUT_EXPR: &str = "(eseq.seq-layout/apply-fx-layout)";

pub(crate) fn create_editor_and_backend(
    runtime: Runtime,
    app: &app::App,
) -> Result<(Editor, AppBackend), Box<dyn std::error::Error>> {
    let mut editor = create_editor(runtime, app)?;
    let mut backend =
        AppBackend::new_with_size_and_font_size(1250, 850, METAL_SEQ_TEXT_FONT_SIZE_PT)
            .map_err(|_| "render backend creation failed")?;
    backend
        .initialize()
        .map_err(|_| "render backend init failed")?;
    configure_editor_for_backend(&mut editor, &mut backend);

    Ok((editor, backend))
}

pub(crate) fn create_editor(
    runtime: Runtime,
    app: &app::App,
) -> Result<Editor, Box<dyn std::error::Error>> {
    create_editor_with_user_init_path(runtime, app, sequencer::paths::user_init_path())
}

pub(crate) fn create_editor_with_user_init_path(
    mut runtime: Runtime,
    app: &app::App,
    user_init_path: Option<std::path::PathBuf>,
) -> Result<Editor, Box<dyn std::error::Error>> {
    let app_paths = sequencer::app_paths::app_paths();
    // `@/` paths are rooted at immutable factory content, independent of the
    // process working directory. Module imports use the tiered user → package
    // → factory search path instead of this raw-load root.
    runtime.set_load_root(app_paths.factory_root());
    let (module_load_roots, package_errors) = app_paths.module_load_roots();
    runtime.set_scoped_module_load_path(module_load_roots);
    // The checked-in UI modules contain intentional module-local bare names
    // and historical alias declarations. Authored instruments/effects/scripts
    // stay outside this exclusion and are always preflighted.
    runtime.exclude_module_alias_scan_root(app_paths.ui_dir());
    let (factory_init_source, factory_init_path) = read_eseqlisp_factory_init_source();
    let mut editor = Editor::new(
        runtime,
        EditorConfig {
            init_source: Some(factory_init_source),
            init_source_path: factory_init_path,
            vim_mode: true,
        },
    );

    // Invalid packages were excluded from the load path; surface each one the
    // same way a failed user init is surfaced instead of aborting boot.
    for message in package_errors {
        eprintln!("metal_seq: {message}");
        editor.handle_host_event(eseqlisp::HostEvent::Error(message));
    }

    reload_custom_instrument_ui(&mut editor);
    let ui_entrypoint = ui_entrypoint_path();
    // Execute the distro root directly rather than opening it as an authored
    // file. Transactional file evaluation deliberately rejects compatibility-
    // alias escape hatches used by a few event-time UI cycles; those modules
    // are still checked individually when edited or hot-reloaded.
    let grid_source = std::fs::read_to_string(&ui_entrypoint).map_err(|error| {
        format!(
            "failed to read UI entrypoint {}: {error}",
            ui_entrypoint.display()
        )
    })?;
    editor
        .runtime_mut()
        .eval_str(&grid_source)
        .map_err(|error| format!("failed to execute {}: {error:?}", ui_entrypoint.display()))?;
    editor.refresh_runtime_side_effects();
    reload_custom_instrument_ui(&mut editor);
    push_project_scratch_to_named_buffer(&mut editor, &app);
    load_user_init(&mut editor, user_init_path.as_deref());
    apply_startup_grid_layout(&mut editor)?;
    log_lisp_ui_load_diagnostics(&mut editor);
    Ok(editor)
}

/// Offscreen capture rendering rides the Metal backend's PNG path and is
/// macOS-only for now; the wgpu backend has no offscreen frame renderer yet.
#[cfg(target_os = "macos")]
pub(crate) fn create_capture_backend(
    editor: &mut Editor,
    width: u32,
    height: u32,
) -> Result<AppBackend, Box<dyn std::error::Error>> {
    let mut backend =
        AppBackend::new_capture_with_font_size(width, height, METAL_SEQ_TEXT_FONT_SIZE_PT)
            .map_err(|_| "Metal capture backend creation failed")?;
    backend
        .initialize()
        .map_err(|_| "Metal capture backend init failed")?;
    configure_editor_for_backend(editor, &mut backend);
    Ok(backend)
}

fn configure_editor_for_backend(editor: &mut Editor, backend: &mut AppBackend) {
    let (cell_w, cell_h) = backend.cell_dimensions();
    if let Some((text_cell_w, text_cell_h)) = backend.sync_text_zoom(editor.text_zoom()) {
        editor.set_text_cell_dimensions(cell_w, cell_h, text_cell_w, text_cell_h);
    }
    if let Some(measurer) = backend.create_text_measurer() {
        editor.set_text_measurer(measurer, cell_w, cell_h);
    }
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

fn read_eseqlisp_factory_init_source() -> (String, Option<std::path::PathBuf>) {
    eseqlisp_factory_init_candidates()
        .into_iter()
        .find_map(|path| {
            std::fs::read_to_string(&path)
                .ok()
                .map(|source| (source, Some(path)))
        })
        .unwrap_or_default()
}

fn eseqlisp_factory_init_candidates() -> Vec<std::path::PathBuf> {
    sequencer::paths::eseqlisp_init_candidates()
}

/// Evaluate the user tier only after every factory/content root. A failed
/// transaction is rolled back wholesale, surfaced in the status line and
/// `*lisp-reload*`, and never aborts application boot.
pub(crate) fn load_user_init(editor: &mut Editor, path: Option<&std::path::Path>) {
    let Some(path) = path else {
        return;
    };
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            let message = format!("User init {} could not be read: {error}", path.display());
            eprintln!("metal_seq: {message}");
            editor.handle_host_event(eseqlisp::HostEvent::Error(message));
            return;
        }
    };
    if source.trim().is_empty() {
        return;
    }
    let overlays = editor.snapshot_file_backed_sources();
    let report = editor.runtime_mut().eval_source_transactional(
        Some(path.to_path_buf()),
        &source,
        overlays,
    );
    if report.success {
        editor.refresh_runtime_side_effects();
    } else {
        eprintln!(
            "metal_seq: user init {} failed; continuing with factory behavior: {}",
            path.display(),
            report.failure_message()
        );
        editor.process_lisp_reload_report(report);
    }
}

fn log_lisp_ui_load_diagnostics(editor: &mut Editor) {
    if let Some(status) = editor.runtime_mut().take_status_message() {
        eprintln!("metal_seq: Lisp UI status during startup: {status}");
    }

    // "*metal*" (the legacy step grid) is intentionally absent: ui/main.lisp
    // no longer loads step-grid.lisp, so the buffer is never created.
    for name in [
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

#[cfg(test)]
mod tests {
    use super::load_user_init;
    use eseqlisp::vm::Value;
    use eseqlisp::{Editor, EditorConfig, Runtime};

    fn temp_init_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "metal-seq-user-init-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create init fixture dir");
        dir.join("init.lisp")
    }

    #[test]
    fn user_init_error_is_visible_and_boot_keeps_factory_behavior() {
        let path = temp_init_path("error");
        std::fs::write(
            &path,
            "(def factory-value () 99)\n(missing-init-function)",
        )
        .expect("write init fixture");
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor
            .runtime_mut()
            .eval_str("(def factory-value () 7)")
            .expect("factory def");

        load_user_init(&mut editor, Some(&path));

        assert_eq!(
            editor
                .runtime_mut()
                .eval_str("(factory-value)")
                .expect("factory still callable"),
            Some(Value::Number(7.0)),
            "the failed init transaction must roll back all partial changes"
        );
        assert!(
            editor.buffers.iter().any(|buffer| buffer.name == "*lisp-reload*"),
            "init diagnostics must be visible in the reload buffer"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_user_init_is_silent() {
        let path = temp_init_path("missing");
        let _ = std::fs::remove_file(&path);
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        load_user_init(&mut editor, Some(&path));
        assert!(editor.runtime_mut().take_status_message().is_none());
        assert!(!editor.buffers.iter().any(|buffer| buffer.name == "*lisp-reload*"));
    }
}
