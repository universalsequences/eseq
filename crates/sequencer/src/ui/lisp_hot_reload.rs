mod discovery;

use std::path::{Path, PathBuf};
use std::time::Duration;

use eseqlisp::{Editor, ReloadReport};
use discovery::{DiscoveryRoots, DiscoveryWorker, ReloadBatch};

use super::custom_ui::{is_generated_custom_ui_source_path, reload_custom_instrument_ui};

const DEBOUNCE_WINDOW: Duration = Duration::from_millis(150);

pub(crate) struct LispHotReloadWatcher(DiscoveryWorker);

impl LispHotReloadWatcher {
    pub(crate) fn start(paths: Vec<PathBuf>) -> Option<Self> {
        if std::env::var("METAL_SEQ_DISABLE_LISP_HOT_RELOAD").ok()
            .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        {
            eprintln!("metal_seq: Lisp hot reload watcher disabled by environment");
            return None;
        }
        match DiscoveryWorker::start(paths, discovery_roots) {
            Ok(worker) => Some(Self(worker)),
            Err(error) => {
                eprintln!("metal_seq: cannot start Lisp discovery worker: {error}");
                None
            }
        }
    }

    pub(crate) fn set_watched_paths(&self, paths: Vec<PathBuf>) { self.0.set_sources(paths); }
    pub(crate) fn poll_ready_paths(&self) -> ReloadBatch { self.0.poll() }
}

fn discovery_roots() -> DiscoveryRoots {
    let paths = sequencer::app_paths::app_paths();
    DiscoveryRoots {
        custom: paths.instrument_dirs().into_iter().chain(paths.effect_dirs())
            .chain([paths.midi_fx_dir()]).collect(),
        packages: [paths.packages_dir(), paths.factory_packages_dir()].into_iter().collect(),
    }
}

pub(crate) fn watched_lisp_paths(editor: &Editor) -> Vec<PathBuf> {
    let mut paths = editor
        .runtime()
        .lisp_source_paths()
        .into_iter()
        .filter(|path| !is_generated_custom_ui_source_path(path))
        .collect::<Vec<_>>();
    // A valid init is already in the module graph. Add it explicitly as well
    // so a boot-time-erroring init remains watched and can recover live.
    if let Some(path) = sequencer::paths::user_init_path() {
        // Watch the parent even before init exists, so creation recovers live.
        paths.push(path);
    }
    paths.sort();
    paths.dedup();
    paths
}

pub(crate) fn process_lisp_hot_reload_paths(editor: &mut Editor, changes: ReloadBatch) -> bool {
    let paths: Vec<_> = changes.paths.into_iter().collect();
    eprintln!(
        "metal_seq: Lisp hot reload observed changes: {}",
        format_paths(&paths)
    );
    let mut reload_paths = Vec::new();
    let mut custom_ui_changed = false;
    for path in paths {
        if has_dirty_open_buffer(editor, &path) {
            eprintln!(
                "metal_seq: Lisp hot reload skipped dirty open buffer: {}",
                path.display()
            );
            editor.handle_host_event(eseqlisp::HostEvent::Status(format!(
                "Lisp hot reload skipped dirty open buffer: {}",
                path.display()
            )));
            continue;
        }
        let custom_ui = changes.custom_ui.contains(&path);
        // A removed custom source must rebuild dispatch without that definition.
        // Keep any open buffer text, just as a normal external deletion does.
        if let Err(error) = if custom_ui && !path.exists() { Ok(()) }
            else { refresh_clean_open_buffers(editor, &path) }
        {
            eprintln!(
                "metal_seq: Lisp hot reload skipped unreadable file: {} ({error})",
                path.display()
            );
            editor.handle_host_event(eseqlisp::HostEvent::Status(format!(
                "Lisp hot reload skipped unreadable file: {} ({error})",
                path.display()
            )));
            continue;
        }
        if custom_ui {
            custom_ui_changed = true;
        } else {
            reload_paths.push(path);
        }
    }

    let mut success = true;
    let normal_lisp_changed = !reload_paths.is_empty();
    if normal_lisp_changed {
        eprintln!(
            "metal_seq: Lisp hot reload evaluating: {}",
            format_paths(&reload_paths)
        );
        let overlays = editor.snapshot_file_backed_sources();
        let report = editor
            .runtime_mut()
            .reload_paths_transactional(reload_paths, overlays);
        success &= report.success;
        log_reload_report(&report);
        editor.process_lisp_reload_report(report);
    }

    if custom_ui_changed {
        eprintln!("metal_seq: Lisp hot reload rebuilding custom instrument/effect UI");
        let custom_success = reload_custom_instrument_ui(editor);
        success &= custom_success;
        if custom_success {
            editor.handle_host_event(eseqlisp::HostEvent::Status(
                "Custom instrument/effect UI hot reload succeeded".to_string(),
            ));
        } else {
            editor.handle_host_event(eseqlisp::HostEvent::Status(
                "Custom instrument/effect UI hot reload failed; kept previous definitions"
                    .to_string(),
            ));
        }
        editor.mark_needs_redraw();
    }

    if !normal_lisp_changed && !custom_ui_changed {
        eprintln!("metal_seq: Lisp hot reload has no eligible paths");
        return false;
    }
    success
}

fn log_reload_report(report: &ReloadReport) {
    eprintln!(
        "metal_seq: Lisp hot reload {}",
        if report.success {
            "succeeded"
        } else {
            "failed"
        }
    );
    if let Some(path) = &report.requested_path {
        eprintln!("metal_seq:   requested: {}", path.display());
    }
    if let Some(path) = &report.evaluated_path {
        eprintln!("metal_seq:   evaluated: {}", path.display());
    }
    if !report.changed_symbols.is_empty() {
        eprintln!(
            "metal_seq:   changed symbols: {}",
            report.changed_symbols.join(", ")
        );
    }
    if !report.rerendered_roots.is_empty() {
        eprintln!(
            "metal_seq:   rerendered roots: {}",
            report.rerendered_roots.join(", ")
        );
    }
    for diagnostic in &report.diagnostics {
        eprintln!("metal_seq:   diagnostic: {diagnostic}");
    }
}

fn format_paths(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return "<none>".to_string();
    }
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn has_dirty_open_buffer(editor: &Editor, path: &Path) -> bool {
    editor.buffers.iter().any(|buffer| {
        buffer
            .path
            .as_ref()
            .is_some_and(|open| same_path(open, path))
            && buffer.dirty
    })
}

fn refresh_clean_open_buffers(editor: &mut Editor, path: &Path) -> std::io::Result<()> {
    let matching = editor
        .buffers
        .iter()
        .enumerate()
        .filter_map(|(idx, buffer)| {
            buffer
                .path
                .as_ref()
                .is_some_and(|open| same_path(open, path))
                .then_some(idx)
        })
        .collect::<Vec<_>>();
    if matching.is_empty() {
        return Ok(());
    }

    let text = std::fs::read_to_string(path)?;
    for idx in matching {
        refresh_clean_open_buffer(editor, idx, &text);
    }
    editor.mark_needs_redraw();
    Ok(())
}

fn refresh_clean_open_buffer(editor: &mut Editor, buffer_idx: usize, text: &str) {
    let cursor = editor.buffers[buffer_idx].cursor;
    let scroll_top = editor.buffers[buffer_idx].scroll_top;
    editor.buffers[buffer_idx].set_text(text);
    editor.buffers[buffer_idx].dirty = false;

    let row = cursor
        .0
        .min(editor.buffers[buffer_idx].lines.len().saturating_sub(1));
    let col = editor.buffers[buffer_idx]
        .lines
        .get(row)
        .map(|line| cursor.1.min(line.chars().count()))
        .unwrap_or(0);
    editor.buffers[buffer_idx].cursor = (row, col);
    editor.buffers[buffer_idx].scroll_top =
        scroll_top.min(editor.buffers[buffer_idx].lines.len().saturating_sub(1));
}

fn same_path(a: &Path, b: &Path) -> bool {
    watch_path(a) == watch_path(b)
}

fn watch_path(path: &Path) -> PathBuf {
    let absolute = absolute_normalized_path(path);
    // Canonicalize the existing prefix too: a deleted /var/... source must
    // still match its previously discovered /private/var/... identity on macOS.
    for ancestor in absolute.ancestors() {
        if let Ok(canonical) = std::fs::canonicalize(ancestor) {
            let suffix = absolute.strip_prefix(ancestor).unwrap();
            return if suffix.as_os_str().is_empty() { canonical } else { canonical.join(suffix) };
        }
    }
    absolute
}

fn absolute_normalized_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    normalize_path(&absolute)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use eseqlisp::vm::Value;
    use eseqlisp::{EditorConfig, Runtime};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn widget_label_text(value: &Value) -> Option<String> {
        match value {
            Value::Map(map) => {
                if map.get("type").is_some_and(|value| {
                    matches!(&*value.borrow(), Value::Keyword(kind) | Value::String(kind) if kind == "label")
                }) {
                    if let Some(text) = map.get("text") {
                        if let Value::String(text) = &*text.borrow() {
                            return Some(text.clone());
                        }
                    }
                }
                map.get("children")
                    .and_then(|children| match &*children.borrow() {
                        Value::List(children) => children
                            .iter()
                            .find_map(|child| widget_label_text(&child.borrow())),
                        _ => None,
                    })
            }
            _ => None,
        }
    }

    fn hot_buffer_label(editor: &Editor, name: &str) -> Option<String> {
        editor
            .buffers
            .iter()
            .find(|buffer| buffer.name == name)
            .and_then(|buffer| buffer.widget_tree.as_ref())
            .and_then(widget_label_text)
    }

    #[test]
    fn watcher_refreshes_clean_open_buffer_before_reloading_disk_change() {
        let dir = temp_dir("metal-seq-hot-reload-clean-open");
        let root = dir.join("root.lisp");
        let child = dir.join("child.lisp");
        std::fs::write(
            &root,
            r#"(load "child.lisp")
(effect-buffer "*hot-watch*" (label hot-label))"#,
        )
        .unwrap();
        std::fs::write(&child, r#"(def hot-label "disk")"#).unwrap();

        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_or_create_file_buffer(&root).unwrap();
        editor.open_or_create_file_buffer(&child).unwrap();

        let root_source = std::fs::read_to_string(&root).unwrap();
        let overlays = editor.snapshot_file_backed_sources();
        let report = editor.runtime_mut().eval_source_transactional(
            Some(root.clone()),
            &root_source,
            overlays,
        );
        assert!(
            report.success,
            "initial reload failed: {:?}",
            report.diagnostics
        );
        editor.process_lisp_reload_report(report);
        assert_eq!(
            hot_buffer_label(&editor, "*hot-watch*").as_deref(),
            Some("disk")
        );

        let changed_child_source = r#"(def hot-label "external")"#;
        std::fs::write(&child, changed_child_source).unwrap();
        let child_idx = editor
            .buffers
            .iter()
            .position(|buffer| buffer.path.as_ref() == Some(&child))
            .unwrap();
        assert_eq!(
            editor.buffers[child_idx].text(),
            r#"(def hot-label "disk")"#
        );
        assert!(!editor.buffers[child_idx].dirty);

        assert!(process_lisp_hot_reload_paths(
            &mut editor,
            ReloadBatch { paths: [child.clone()].into_iter().map(|path| watch_path(&path)).collect(), ..Default::default() }
        ));

        let child_idx = editor
            .buffers
            .iter()
            .position(|buffer| buffer.path.as_ref() == Some(&child))
            .unwrap();
        assert_eq!(editor.buffers[child_idx].text(), changed_child_source);
        assert!(!editor.buffers[child_idx].dirty);
        assert_eq!(
            hot_buffer_label(&editor, "*hot-watch*").as_deref(),
            Some("external")
        );
    }

    #[test]
    fn watcher_does_not_overwrite_dirty_open_buffer() {
        let dir = temp_dir("metal-seq-hot-reload-dirty-open");
        let child = dir.join("child.lisp");
        std::fs::write(&child, r#"(def hot-label "disk")"#).unwrap();

        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_or_create_file_buffer(&child).unwrap();
        editor
            .active_buffer_mut()
            .set_text(r#"(def hot-label "unsaved")"#);
        editor.active_buffer_mut().dirty = true;

        std::fs::write(&child, r#"(def hot-label "external")"#).unwrap();

        assert!(!process_lisp_hot_reload_paths(
            &mut editor,
            ReloadBatch { paths: [child.clone()].into_iter().map(|path| watch_path(&path)).collect(), ..Default::default() }
        ));
        assert_eq!(
            editor.active_buffer().text(),
            r#"(def hot-label "unsaved")"#
        );
        assert!(editor.active_buffer().dirty);
    }

    #[test]
    fn deleted_custom_ui_is_not_resurrected_by_a_clean_open_buffer() {
        let root = sequencer::app_paths::app_paths().user_instruments_dir();
        std::fs::create_dir_all(&root).unwrap();
        let dir = tempfile::Builder::new().prefix("hot-reload-removal-").tempdir_in(root).unwrap();
        let ui = dir.path().join("ui.lisp");
        std::fs::write(dir.path().join("dsp.lisp"), "(out 0)").unwrap();
        std::fs::write(&ui, "(defsynth-ui (label 1))").unwrap();
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.runtime_mut().eval_str(
            "(def eseq.effects.custom-ui-sections/custom-ui-selected-section-for-current-scope () 0)"
        ).unwrap();
        editor.open_or_create_file_buffer(&ui).unwrap();
        assert!(reload_custom_instrument_ui(&mut editor));
        let name = dir.path().file_name().unwrap().to_str().unwrap();
        let expression = format!("(custom-instrument-synth-ui (dict :name \"user:{name}\"))");
        assert!(matches!(editor.runtime_mut().eval_str(&expression).unwrap(), Some(Value::Map(_))));
        std::fs::remove_file(&ui).unwrap();
        let path = watch_path(&ui);
        assert!(process_lisp_hot_reload_paths(&mut editor, ReloadBatch {
            paths: [path.clone()].into_iter().collect(), custom_ui: [path].into_iter().collect(),
        }));
        assert!(matches!(editor.runtime_mut().eval_str(&expression).unwrap(), Some(Value::Bool(false))));
        assert_eq!(editor.active_buffer().text(), "(defsynth-ui (label 1))", "keep the user's open text");
        editor.active_buffer_mut().dirty = true;
        assert!(reload_custom_instrument_ui(&mut editor));
        assert!(matches!(editor.runtime_mut().eval_str(&expression).unwrap(), Some(Value::Map(_))),
            "explicit evaluation still supports an unsaved custom UI overlay");
    }

    #[test]
    fn watched_lisp_paths_include_successful_external_user_init() {
        let dir = temp_dir("metal-seq-hot-reload-user-init");
        let init = dir.join("init.lisp");
        std::fs::write(&init, "(def user-init-probe () 1)").expect("write init");
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        let report = editor.runtime_mut().eval_source_transactional(
            Some(init.clone()),
            &std::fs::read_to_string(&init).expect("read init"),
            Vec::new(),
        );
        assert!(report.success, "init eval failed: {:?}", report.diagnostics);

        let watched = watched_lisp_paths(&editor)
            .into_iter()
            .map(|path| watch_path(&path))
            .collect::<BTreeSet<_>>();
        assert!(watched.contains(&watch_path(&init)));
    }

    #[test]
    #[cfg(unix)]
    fn deleted_source_keeps_dirty_buffer_identity_through_symlinked_parent() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        let alias = temp.path().join("alias");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let source = real.join("source.lisp");
        std::fs::write(&source, "(def value 1)").unwrap();
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_or_create_file_buffer(alias.join("source.lisp")).unwrap();
        editor.active_buffer_mut().dirty = true;
        let identity = watch_path(&source);
        std::fs::remove_file(&source).unwrap();
        assert_eq!(watch_path(&source), identity);
        assert!(has_dirty_open_buffer(&editor, &identity));
    }

    #[test]
    fn discovery_roots_cover_custom_instrument_and_effect_ui_sources() {
        let roots = discovery_roots();
        for path in [
            sequencer::app_paths::app_paths()
                .instruments_dir()
                .join("Drums/Digi Hat/ui.lisp"),
            sequencer::app_paths::app_paths()
                .effects_dir()
                .join("sidechain/ui.lisp"),
        ] {
            assert!(
                roots.custom.iter().any(|root| path.starts_with(root)),
                "hot reload discovery roots should cover {}",
                path.display()
            );
        }
    }
}
