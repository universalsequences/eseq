use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eseqlisp::{Editor, ReloadReport};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::custom_ui::{
    custom_ui_source_paths, is_generated_custom_ui_source_path, reload_custom_instrument_ui,
};

const DEBOUNCE_WINDOW: Duration = Duration::from_millis(150);

#[derive(Debug)]
pub(crate) struct LispHotReloadWatcher {
    receiver: Receiver<PathBuf>,
    watcher: RecommendedWatcher,
    watched_dirs: BTreeSet<PathBuf>,
    watched_files: BTreeSet<PathBuf>,
    pending: BTreeSet<PathBuf>,
    last_event_at: Option<Instant>,
}

impl LispHotReloadWatcher {
    pub(crate) fn start(paths: impl IntoIterator<Item = PathBuf>) -> Option<Self> {
        if std::env::var("METAL_SEQ_DISABLE_LISP_HOT_RELOAD")
            .ok()
            .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        {
            eprintln!("metal_seq: Lisp hot reload watcher disabled by environment");
            return None;
        }

        let (tx, rx) = mpsc::channel();
        let watcher = RecommendedWatcher::new(
            move |result: notify::Result<Event>| match result {
                Ok(event) => {
                    if !is_reload_event(&event.kind) {
                        return;
                    }
                    for path in event.paths {
                        let _ = tx.send(path);
                    }
                }
                Err(error) => {
                    eprintln!("metal_seq: Lisp hot reload watcher error: {error}");
                }
            },
            Config::default(),
        )
        .ok()?;

        let mut watcher = Self {
            receiver: rx,
            watcher,
            watched_dirs: BTreeSet::new(),
            watched_files: BTreeSet::new(),
            pending: BTreeSet::new(),
            last_event_at: None,
        };
        watcher.set_watched_paths(paths);
        eprintln!(
            "metal_seq: Lisp hot reload watcher observing {} files in {} dirs",
            watcher.watched_files.len(),
            watcher.watched_dirs.len()
        );
        Some(watcher)
    }

    pub(crate) fn set_watched_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        let next_files = paths
            .into_iter()
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("lisp"))
            .map(|path| watch_path(&path))
            .collect::<BTreeSet<_>>();
        let desired_dirs = next_files
            .iter()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect::<BTreeSet<_>>();

        if next_files == self.watched_files && desired_dirs == self.watched_dirs {
            return;
        }

        let mut active_dirs = self.watched_dirs.clone();
        for dir in self.watched_dirs.difference(&desired_dirs) {
            if let Err(error) = self.watcher.unwatch(dir) {
                eprintln!(
                    "metal_seq: Lisp hot reload failed to unwatch {}: {error}",
                    dir.display()
                );
            }
            active_dirs.remove(dir);
        }
        for dir in desired_dirs.difference(&self.watched_dirs) {
            match self.watcher.watch(dir, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    active_dirs.insert(dir.clone());
                }
                Err(error) => {
                    eprintln!(
                        "metal_seq: Lisp hot reload failed to watch {}: {error}",
                        dir.display()
                    );
                }
            };
        }

        let active_files = next_files
            .into_iter()
            .filter(|path| {
                path.parent()
                    .is_some_and(|parent| active_dirs.contains(parent))
            })
            .collect::<BTreeSet<_>>();
        let changed =
            active_files.len() != self.watched_files.len() || active_dirs != self.watched_dirs;
        self.watched_files = active_files;
        self.watched_dirs = active_dirs;
        self.pending
            .retain(|path| self.watched_files.contains(path));
        if changed {
            eprintln!(
                "metal_seq: Lisp hot reload watcher now observing {} files in {} dirs",
                self.watched_files.len(),
                self.watched_dirs.len()
            );
        }
    }

    pub(crate) fn poll_ready_paths(&mut self) -> Vec<PathBuf> {
        while let Ok(path) = self.receiver.try_recv() {
            let path = watch_path(&path);
            if !self.watched_files.contains(&path) {
                continue;
            }
            self.pending.insert(path);
            self.last_event_at = Some(Instant::now());
        }
        if self.pending.is_empty()
            || self
                .last_event_at
                .is_some_and(|last| last.elapsed() < DEBOUNCE_WINDOW)
        {
            return Vec::new();
        }
        self.last_event_at = None;
        std::mem::take(&mut self.pending).into_iter().collect()
    }
}

fn is_reload_event(kind: &EventKind) -> bool {
    !matches!(kind, EventKind::Access(_))
}

pub(crate) fn watched_lisp_paths(editor: &Editor) -> Vec<PathBuf> {
    let mut paths = editor
        .runtime()
        .lisp_source_paths()
        .into_iter()
        .filter(|path| !is_generated_custom_ui_source_path(path))
        .collect::<Vec<_>>();
    paths.extend(custom_ui_source_paths());
    paths.sort();
    paths.dedup();
    paths
}

pub(crate) fn process_lisp_hot_reload_paths(editor: &mut Editor, paths: Vec<PathBuf>) -> bool {
    eprintln!(
        "metal_seq: Lisp hot reload observed changes: {}",
        format_paths(&paths)
    );
    let mut reload_paths = Vec::new();
    let custom_ui_paths = custom_ui_source_paths()
        .into_iter()
        .map(|path| watch_path(&path))
        .collect::<BTreeSet<_>>();
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
        if let Err(error) = refresh_clean_open_buffers(editor, &path) {
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
        if custom_ui_paths.contains(&watch_path(&path)) {
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
    let a = std::fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf());
    let b = std::fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf());
    a == b
}

fn watch_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| absolute_normalized_path(path))
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
            vec![child.clone()]
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
            vec![child.clone()]
        ));
        assert_eq!(
            editor.active_buffer().text(),
            r#"(def hot-label "unsaved")"#
        );
        assert!(editor.active_buffer().dirty);
    }

    #[test]
    fn watched_lisp_paths_include_custom_instrument_and_effect_ui_sources() {
        let editor = Editor::new(Runtime::new(), EditorConfig::default());
        let watched = watched_lisp_paths(&editor)
            .into_iter()
            .map(|path| watch_path(&path))
            .collect::<BTreeSet<_>>();

        for path in [
            PathBuf::from("instruments/bass/korg1/ui.lisp"),
            PathBuf::from("effects/sidechain/ui.lisp"),
        ] {
            assert!(
                watched.contains(&watch_path(&path)),
                "hot reload watch list should include {}",
                path.display()
            );
        }
    }
}
