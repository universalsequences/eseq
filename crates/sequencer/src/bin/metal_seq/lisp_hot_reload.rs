use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime};

use eseqlisp::{Editor, ReloadReport};

const SCAN_INTERVAL: Duration = Duration::from_millis(300);
const DEBOUNCE_WINDOW: Duration = Duration::from_millis(150);

#[derive(Debug)]
pub(crate) struct LispHotReloadWatcher {
    receiver: Receiver<PathBuf>,
    pending: BTreeSet<PathBuf>,
    last_event_at: Option<Instant>,
}

impl LispHotReloadWatcher {
    pub(crate) fn start(root: impl Into<PathBuf>) -> Option<Self> {
        if std::env::var("METAL_SEQ_DISABLE_LISP_HOT_RELOAD")
            .ok()
            .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        {
            eprintln!("metal_seq: Lisp hot reload watcher disabled by environment");
            return None;
        }

        let root = root.into();
        eprintln!(
            "metal_seq: Lisp hot reload watcher scanning {}",
            root.display()
        );
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("lisp-hot-reload-watch".to_string())
            .spawn(move || {
                let mut known = snapshot_lisp_files(&root);
                loop {
                    std::thread::sleep(SCAN_INTERVAL);
                    let next = snapshot_lisp_files(&root);
                    for (path, modified) in &next {
                        if known.get(path) != Some(modified) {
                            let _ = tx.send(path.clone());
                        }
                    }
                    known = next;
                }
            })
            .ok()?;

        Some(Self {
            receiver: rx,
            pending: BTreeSet::new(),
            last_event_at: None,
        })
    }

    pub(crate) fn poll_ready_paths(&mut self) -> Vec<PathBuf> {
        while let Ok(path) = self.receiver.try_recv() {
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

pub(crate) fn process_lisp_hot_reload_paths(editor: &mut Editor, paths: Vec<PathBuf>) -> bool {
    eprintln!(
        "metal_seq: Lisp hot reload observed changes: {}",
        format_paths(&paths)
    );
    let mut reload_paths = Vec::new();
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
        reload_paths.push(path);
    }
    if reload_paths.is_empty() {
        eprintln!("metal_seq: Lisp hot reload has no eligible paths");
        return false;
    }
    eprintln!(
        "metal_seq: Lisp hot reload evaluating: {}",
        format_paths(&reload_paths)
    );
    let overlays = editor.snapshot_file_backed_sources();
    let report = editor
        .runtime_mut()
        .reload_paths_transactional(reload_paths, overlays);
    let success = report.success;
    log_reload_report(&report);
    editor.process_lisp_reload_report(report);
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

fn snapshot_lisp_files(root: &Path) -> BTreeMap<PathBuf, Option<SystemTime>> {
    let mut out = BTreeMap::new();
    collect_lisp_files(root, &mut out);
    out
}

fn collect_lisp_files(path: &Path, out: &mut BTreeMap<PathBuf, Option<SystemTime>>) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name.starts_with('.') || matches!(name, "target" | "audiograph") {
            continue;
        }
        if path.is_dir() {
            collect_lisp_files(&path, out);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) == Some("lisp") {
            let canonical = std::fs::canonicalize(&path).unwrap_or(path);
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok();
            out.insert(canonical, modified);
        }
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let a = std::fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf());
    let b = std::fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf());
    a == b
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
}
