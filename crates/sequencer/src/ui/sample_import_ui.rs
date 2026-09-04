//! Sample import draft: the state behind the drag-and-drop import modal.
//!
//! Dropping files onto the window stages them here (`stage_drop`); the modal
//! in `content/ui/sample-import.lisp` reads and edits the draft through the
//! `seq-sample-import-*` natives below, and the `sample-import-commit` /
//! `sample-import-cancel` host commands (`host_commands/sample_import.rs`)
//! finish it. Everything runs on the UI thread, so the draft is a plain
//! thread-local rather than a shared lock.
//!
//! Tag vocabulary: `tag_candidates` holds every tag the sample DB already
//! knows plus every tag typed during this draft. Adding a tag that matches a
//! candidate case-insensitively adopts the candidate's spelling, so
//! "Hi-Hat" and "hi-hat" collapse onto whichever the library already uses.
//! The modal's autocomplete reads the same list through
//! `seq-sample-import-tag-suggestions`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use eseqlisp::vm::Value;
use eseqlisp::Runtime;
use sequencer::sample_db::SampleDb;
use sequencer::sample_import::{
    import_staged_samples, normalize_tags, stage_paths, ImportSummary, StagedSample,
    StagedSampleStatus,
};

use super::values::build_string_list;

const MAX_SUGGESTIONS: usize = 8;

#[derive(Debug, Clone)]
pub(crate) struct SampleImportDraft {
    staged: Vec<StagedSample>,
    /// Tree node path per staged sample: `<root>/<sub dirs>/<file name>`,
    /// where `<root>` is the dropped folder's name (or "Files" for loose
    /// files). Folder nodes are the proper prefixes of these paths.
    node_paths: Vec<String>,
    /// The selected tree node: a folder path (tags apply to everything
    /// under it) or a file path (tags apply to that sample). `None` selects
    /// the whole drop.
    selection: Option<String>,
    /// The nested `tree` widget items, built once per draft.
    tree: Value,
    /// Tags applied to every imported sample at commit time.
    batch_tags: Vec<String>,
    /// Known tag vocabulary, sorted case-insensitively.
    tag_candidates: Vec<String>,
    /// Folder names from the dropped roots, offered as one-click batch tags
    /// ("drop a folder of 808s, tag them all 808").
    suggested_tags: Vec<String>,
}

thread_local! {
    static DRAFT: RefCell<Option<SampleImportDraft>> = const { RefCell::new(None) };
}

impl SampleImportDraft {
    pub(crate) fn from_drop(paths: Vec<PathBuf>, db_path: &Path) -> Result<Self, String> {
        let db = SampleDb::open(db_path)
            .map_err(|error| format!("failed to open {}: {error}", db_path.display()))?;
        let staged = stage_paths(&paths, &db);
        let node_paths = staged
            .iter()
            .map(|sample| node_path_for(&sample.source_path, &paths))
            .collect();
        let tag_candidates = db
            .list_tags()
            .map_err(|error| format!("failed to list sample tags: {error}"))?;
        let suggested_tags = normalize_tags(
            &paths
                .iter()
                .filter(|path| path.is_dir())
                .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
                .map(str::to_string)
                .collect::<Vec<_>>(),
        );
        Ok(Self::new(staged, node_paths, tag_candidates, suggested_tags))
    }

    pub(crate) fn new(
        staged: Vec<StagedSample>,
        node_paths: Vec<String>,
        tag_candidates: Vec<String>,
        suggested_tags: Vec<String>,
    ) -> Self {
        debug_assert_eq!(staged.len(), node_paths.len());
        let tree = build_tree(&staged, &node_paths);
        let mut draft = Self {
            staged,
            node_paths,
            selection: None,
            tree,
            batch_tags: Vec::new(),
            tag_candidates: Vec::new(),
            suggested_tags,
        };
        draft.add_tag_candidates(&tag_candidates);
        draft
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.staged.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.staged.len()
    }

    #[cfg(test)]
    pub(crate) fn sample_tags(&self, index: usize) -> Vec<String> {
        self.staged[index].tags.clone()
    }

    pub(crate) fn commit(&self, db_path: &Path, sample_dir: &Path) -> Result<ImportSummary, String> {
        let mut db = SampleDb::open(db_path)
            .map_err(|error| format!("failed to open {}: {error}", db_path.display()))?;
        Ok(import_staged_samples(
            &self.staged,
            &self.batch_tags,
            sample_dir,
            &mut db,
        ))
    }

    fn ready_count(&self) -> usize {
        self.count_status(|status| matches!(status, StagedSampleStatus::Ready))
    }

    fn duplicate_count(&self) -> usize {
        self.count_status(|status| matches!(status, StagedSampleStatus::Duplicate))
    }

    fn failed_count(&self) -> usize {
        self.count_status(|status| matches!(status, StagedSampleStatus::Error(_)))
    }

    fn count_status(&self, pred: impl Fn(&StagedSampleStatus) -> bool) -> usize {
        self.staged
            .iter()
            .filter(|sample| pred(&sample.status))
            .count()
    }

    /// Indices under the selected node: everything for no selection, the
    /// subtree for a folder path, the one sample for a file path.
    fn selection_targets(&self) -> Vec<usize> {
        match &self.selection {
            None => (0..self.staged.len()).collect(),
            Some(node) => {
                let prefix = format!("{node}/");
                (0..self.staged.len())
                    .filter(|index| {
                        let path = &self.node_paths[*index];
                        path == node || path.starts_with(&prefix)
                    })
                    .collect()
            }
        }
    }

    fn selected_file_index(&self) -> Option<usize> {
        let node = self.selection.as_ref()?;
        self.node_paths.iter().position(|path| path == node)
    }

    fn select(&mut self, node: &str) {
        self.selection = (!node.is_empty()).then(|| node.to_string());
    }

    /// Resolve a typed tag against the vocabulary: trimmed, and spelled the
    /// way an existing candidate spells it when one matches ignoring case.
    fn canonical_tag(&self, tag: &str) -> Option<String> {
        let tag = tag.trim();
        if tag.is_empty() {
            return None;
        }
        Some(
            self.tag_candidates
                .iter()
                .find(|candidate| candidate.eq_ignore_ascii_case(tag))
                .cloned()
                .unwrap_or_else(|| tag.to_string()),
        )
    }

    fn add_tag_candidates(&mut self, tags: &[String]) {
        for tag in tags {
            let tag = tag.trim();
            if tag.is_empty() {
                continue;
            }
            if !self
                .tag_candidates
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(tag))
            {
                self.tag_candidates.push(tag.to_string());
            }
        }
        self.tag_candidates.sort_by_key(|tag| tag.to_lowercase());
    }

    fn suggestions(&self, prefix: &str, exclude: &[String]) -> Vec<String> {
        let prefix = prefix.trim().to_lowercase();
        let excluded = |tag: &String| exclude.iter().any(|ex| ex.eq_ignore_ascii_case(tag));
        let mut out: Vec<String> = Vec::new();
        if prefix.is_empty() {
            return out;
        }
        for tag in self
            .tag_candidates
            .iter()
            .filter(|tag| tag.to_lowercase().starts_with(&prefix))
            .filter(|tag| !excluded(tag))
        {
            out.push(tag.clone());
            if out.len() >= MAX_SUGGESTIONS {
                return out;
            }
        }
        for tag in self
            .tag_candidates
            .iter()
            .filter(|tag| {
                let lower = tag.to_lowercase();
                lower.contains(&prefix) && !lower.starts_with(&prefix)
            })
            .filter(|tag| !excluded(tag))
        {
            out.push(tag.clone());
            if out.len() >= MAX_SUGGESTIONS {
                break;
            }
        }
        out
    }

    fn set_title(&mut self, index: usize, title: &str) {
        let title = title.trim();
        if title.is_empty() {
            return;
        }
        if let Some(sample) = self.staged.get_mut(index) {
            sample.title = title.to_string();
        }
    }

    fn add_tag_to(&mut self, indices: &[usize], tag: &str) -> bool {
        let Some(tag) = self.canonical_tag(tag) else {
            return false;
        };
        self.add_tag_candidates(std::slice::from_ref(&tag));
        for index in indices {
            if let Some(sample) = self.staged.get_mut(*index) {
                if !sample.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
                    sample.tags.push(tag.clone());
                }
            }
        }
        true
    }

    fn remove_tag_from(&mut self, indices: &[usize], tag: &str) {
        for index in indices {
            if let Some(sample) = self.staged.get_mut(*index) {
                sample.tags.retain(|t| !t.eq_ignore_ascii_case(tag));
            }
        }
    }

    fn add_batch_tag(&mut self, tag: &str) -> bool {
        let Some(tag) = self.canonical_tag(tag) else {
            return false;
        };
        self.add_tag_candidates(std::slice::from_ref(&tag));
        if !self.batch_tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
            self.batch_tags.push(tag);
        }
        true
    }

    fn remove_batch_tag(&mut self, tag: &str) {
        self.batch_tags.retain(|t| !t.eq_ignore_ascii_case(tag));
    }

    /// Tags shared by every bulk target (shown as removable chips in the
    /// bulk pane).
    fn common_tags(&self, indices: &[usize]) -> Vec<String> {
        let Some(first) = indices.first().and_then(|index| self.staged.get(*index)) else {
            return Vec::new();
        };
        first
            .tags
            .iter()
            .filter(|tag| {
                indices.iter().all(|index| {
                    self.staged
                        .get(*index)
                        .is_some_and(|sample| sample.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)))
                })
            })
            .cloned()
            .collect()
    }

    fn summary_value(&self) -> Value {
        map_value([
            ("total", number(self.staged.len())),
            ("ready", number(self.ready_count())),
            ("duplicates", number(self.duplicate_count())),
            ("failed", number(self.failed_count())),
            ("batch-tags", build_string_list(&self.batch_tags)),
            (
                "suggested-tags",
                build_string_list(
                    &self
                        .suggested_tags
                        .iter()
                        .filter(|tag| !self.batch_tags.iter().any(|b| b.eq_ignore_ascii_case(tag)))
                        .cloned()
                        .collect::<Vec<_>>(),
                ),
            ),
        ])
    }

    fn sample_value(&self, index: usize) -> Value {
        let sample = &self.staged[index];
        let (status, error) = match &sample.status {
            StagedSampleStatus::Ready => ("ready", String::new()),
            StagedSampleStatus::Duplicate => ("duplicate", String::new()),
            StagedSampleStatus::Error(error) => ("error", error.clone()),
        };
        let file = sample
            .source_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_string();
        map_value([
            ("index", number(index)),
            ("node", Value::String(self.node_paths[index].clone())),
            ("title", Value::String(sample.title.clone())),
            ("file", Value::String(file)),
            (
                "path",
                Value::String(sample.source_path.to_string_lossy().into_owned()),
            ),
            ("tags", build_string_list(&sample.tags)),
            ("status", Value::String(status.to_string())),
            ("error", Value::String(error)),
        ])
    }

    /// What the right-hand pane shows for the selected node: a count and
    /// shared tags for a folder (never a sample list; a thousand-file drop
    /// must not become a thousand widget rows), the sample itself for a file.
    fn selection_value(&self) -> Value {
        let targets = self.selection_targets();
        let file = self.selected_file_index();
        let label = match &self.selection {
            None => "Everything".to_string(),
            Some(node) => node.rsplit('/').next().unwrap_or(node).to_string(),
        };
        map_value([
            (
                "kind",
                Value::String(
                    match (&self.selection, file) {
                        (None, _) => "all",
                        (Some(_), Some(_)) => "file",
                        (Some(_), None) => "folder",
                    }
                    .to_string(),
                ),
            ),
            ("node", Value::String(self.selection.clone().unwrap_or_default())),
            ("label", Value::String(label)),
            ("count", number(targets.len())),
            ("tags", build_string_list(&self.common_tags(&targets))),
            ("file", file.map_or(Value::Nil, |index| self.sample_value(index))),
        ])
    }
}

/// Tree node path for one staged file: the dropped folder it came from
/// (by name), then its sub-directories, then the file name. Loose files
/// dropped on their own gather under "Files".
fn node_path_for(path: &Path, roots: &[PathBuf]) -> String {
    let file = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string();
    let root = roots
        .iter()
        .filter(|root| root.is_dir() && path.starts_with(root))
        .max_by_key(|root| root.components().count());
    let Some(root) = root else {
        return format!("Files/{file}");
    };
    let root_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Files");
    let mut segments = vec![root_name.to_string()];
    if let Some(parent) = path.strip_prefix(root).ok().and_then(|relative| relative.parent()) {
        segments.extend(
            parent
                .components()
                .filter_map(|component| component.as_os_str().to_str().map(str::to_string)),
        );
    }
    segments.push(file);
    segments.join("/")
}

#[derive(Default)]
struct TreeFolder {
    folders: Vec<(String, TreeFolder)>,
    files: Vec<(String, usize)>,
    count: usize,
}

impl TreeFolder {
    fn insert(&mut self, segments: &[String], index: usize) {
        self.count += 1;
        match segments {
            [] => {}
            [file] => self.files.push((file.clone(), index)),
            [dir, rest @ ..] => {
                let position = match self.folders.iter().position(|(name, _)| name == dir) {
                    Some(position) => position,
                    None => {
                        self.folders.push((dir.clone(), TreeFolder::default()));
                        self.folders.len() - 1
                    }
                };
                self.folders[position].1.insert(rest, index);
            }
        }
    }

    fn items(&self, prefix: &str, staged: &[StagedSample]) -> Value {
        let mut items = Vec::new();
        for (name, folder) in &self.folders {
            let path = join_node(prefix, name);
            items.push(Rc::new(RefCell::new(map_value([
                ("label", Value::String(name.clone())),
                ("path", Value::String(path.clone())),
                ("icon", Value::String("folder".to_string())),
                ("detail", Value::String(folder.count.to_string())),
                ("draggable", Value::Bool(false)),
                ("drop-target", Value::Bool(false)),
                ("children", folder.items(&path, staged)),
            ]))));
        }
        for (name, index) in &self.files {
            let mut item = vec![
                ("label", Value::String(name.clone())),
                ("path", Value::String(join_node(prefix, name))),
                ("icon", Value::String("waveform".to_string())),
                ("draggable", Value::Bool(false)),
                ("drop-target", Value::Bool(false)),
            ];
            // Only non-ready files carry a detail: the tree prints whatever
            // the key holds, so ready files must not have one at all.
            match &staged[*index].status {
                StagedSampleStatus::Ready => {}
                StagedSampleStatus::Duplicate => {
                    item.push(("detail", Value::String("dup".to_string())));
                }
                StagedSampleStatus::Error(_) => {
                    item.push(("detail", Value::String("failed".to_string())));
                }
            }
            items.push(Rc::new(RefCell::new(map_from_pairs(item))));
        }
        Value::List(items)
    }
}

fn join_node(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

fn build_tree(staged: &[StagedSample], node_paths: &[String]) -> Value {
    let mut root = TreeFolder::default();
    for (index, path) in node_paths.iter().enumerate() {
        let segments: Vec<String> = path.split('/').map(str::to_string).collect();
        root.insert(&segments, index);
    }
    root.items("", staged)
}

fn number(value: usize) -> Value {
    Value::Number(value as f64)
}

fn map_value<const N: usize>(entries: [(&str, Value); N]) -> Value {
    map_from_pairs(entries.into_iter().collect())
}

fn map_from_pairs(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
            .collect::<HashMap<_, _>>(),
    )
}

// ── Draft slot ─────────────────────────────────────────────────────────

pub(crate) fn install_draft(draft: SampleImportDraft) {
    DRAFT.with(|slot| *slot.borrow_mut() = Some(draft));
}

pub(crate) fn take_draft() -> Option<SampleImportDraft> {
    DRAFT.with(|slot| slot.borrow_mut().take())
}

pub(crate) fn draft_is_open() -> bool {
    DRAFT.with(|slot| slot.borrow().is_some())
}

fn with_draft<T>(f: impl FnOnce(&SampleImportDraft) -> T) -> Option<T> {
    DRAFT.with(|slot| slot.borrow().as_ref().map(f))
}

fn with_draft_mut<T>(f: impl FnOnce(&mut SampleImportDraft) -> T) -> Option<T> {
    DRAFT.with(|slot| slot.borrow_mut().as_mut().map(f))
}

/// Opens the import modal over whichever step-panel buffer the main tile is
/// showing. A modal only receives pointer input through the *active* tile,
/// so the tile showing the mount must become active before the panel opens
/// (the sequencer and arrangement buffers both mount
/// `eseq.sample-import/panel`; they never share a tile).
pub(crate) fn open_sample_import_modal(editor: &mut eseqlisp::Editor) {
    if !editor.switch_active_tile_to_buffer_named("*sequencer*") {
        editor.switch_active_tile_to_buffer_named("*arrangement*");
    }
    let _ = editor.runtime_mut().eval_str("(eseq.sample-import/open)");
    editor.refresh_runtime_side_effects();
    editor.mark_needs_redraw();
}

// ── Natives ────────────────────────────────────────────────────────────

fn arg_string(args: &[Value], index: usize) -> String {
    match args.get(index) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Keyword(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn arg_index(args: &[Value], index: usize) -> Option<usize> {
    match args.get(index) {
        Some(Value::Number(n)) if *n >= 0.0 => Some(*n as usize),
        _ => None,
    }
}

fn arg_string_list(args: &[Value], index: usize) -> Vec<String> {
    match args.get(index) {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match &*item.borrow() {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Registers the `seq-sample-import-*` natives the import modal reads and
/// edits the draft through. Mutating natives return `true` when the draft
/// changed so the Lisp side can bump its render generation.
pub(crate) fn register_sample_import_natives(runtime: &mut Runtime) {
    // Script entry point (capture fixtures, automation): stage a list of
    // files/folders exactly as a drop would, replacing any current draft.
    // Returns the staged count; the caller opens the modal.
    runtime.register_native("seq-sample-import-stage", |args, _ctx| {
        let paths: Vec<PathBuf> = arg_string_list(&args, 0)
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let draft = SampleImportDraft::from_drop(
            paths,
            &sequencer::app_paths::app_paths().sample_db_path(),
        )?;
        let count = draft.len();
        install_draft(draft);
        Ok(number(count))
    });
    runtime.register_native("seq-sample-import-open?", |_args, _ctx| {
        Ok(Value::Bool(draft_is_open()))
    });
    runtime.register_native("seq-sample-import-summary", |_args, _ctx| {
        Ok(with_draft(SampleImportDraft::summary_value).unwrap_or(Value::Nil))
    });
    runtime.register_native("seq-sample-import-tree", |_args, _ctx| {
        Ok(with_draft(|draft| draft.tree.clone()).unwrap_or_else(|| Value::List(vec![])))
    });
    runtime.register_native("seq-sample-import-selection", |_args, _ctx| {
        Ok(with_draft(SampleImportDraft::selection_value).unwrap_or(Value::Nil))
    });
    runtime.register_native("seq-sample-import-select", |args, _ctx| {
        let node = arg_string(&args, 0);
        with_draft_mut(|draft| draft.select(&node));
        Ok(Value::Bool(true))
    });
    runtime.register_native("seq-sample-import-tag-suggestions", |args, _ctx| {
        let prefix = arg_string(&args, 0);
        let exclude = arg_string_list(&args, 1);
        Ok(with_draft(|draft| build_string_list(&draft.suggestions(&prefix, &exclude)))
            .unwrap_or_else(|| Value::List(vec![])))
    });
    runtime.register_native("seq-sample-import-set-title", |args, _ctx| {
        let Some(index) = arg_index(&args, 0) else {
            return Ok(Value::Bool(false));
        };
        let title = arg_string(&args, 1);
        with_draft_mut(|draft| draft.set_title(index, &title));
        Ok(Value::Bool(true))
    });
    runtime.register_native("seq-sample-import-add-tag", |args, _ctx| {
        let Some(index) = arg_index(&args, 0) else {
            return Ok(Value::Bool(false));
        };
        let tag = arg_string(&args, 1);
        Ok(Value::Bool(
            with_draft_mut(|draft| draft.add_tag_to(&[index], &tag)).unwrap_or(false),
        ))
    });
    runtime.register_native("seq-sample-import-remove-tag", |args, _ctx| {
        let Some(index) = arg_index(&args, 0) else {
            return Ok(Value::Bool(false));
        };
        let tag = arg_string(&args, 1);
        with_draft_mut(|draft| draft.remove_tag_from(&[index], &tag));
        Ok(Value::Bool(true))
    });
    // Selection: every sample under the selected tree node.
    runtime.register_native("seq-sample-import-add-selection-tag", |args, _ctx| {
        let tag = arg_string(&args, 0);
        Ok(Value::Bool(
            with_draft_mut(|draft| {
                let targets = draft.selection_targets();
                draft.add_tag_to(&targets, &tag)
            })
            .unwrap_or(false),
        ))
    });
    runtime.register_native("seq-sample-import-remove-selection-tag", |args, _ctx| {
        let tag = arg_string(&args, 0);
        with_draft_mut(|draft| {
            let targets = draft.selection_targets();
            draft.remove_tag_from(&targets, &tag);
        });
        Ok(Value::Bool(true))
    });
    // Batch: tags every imported sample receives at commit time.
    runtime.register_native("seq-sample-import-add-batch-tag", |args, _ctx| {
        let tag = arg_string(&args, 0);
        Ok(Value::Bool(
            with_draft_mut(|draft| draft.add_batch_tag(&tag)).unwrap_or(false),
        ))
    });
    runtime.register_native("seq-sample-import-remove-batch-tag", |args, _ctx| {
        let tag = arg_string(&args, 0);
        with_draft_mut(|draft| draft.remove_batch_tag(&tag));
        Ok(Value::Bool(true))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_sample(path: &str, title: &str) -> StagedSample {
        StagedSample {
            source_path: PathBuf::from(path),
            hash: Some(format!("hash-{title}")),
            title: title.to_string(),
            tags: Vec::new(),
            status: StagedSampleStatus::Ready,
        }
    }

    /// Three files: two in `808s/claps`, one in `808s/kicks`.
    fn draft() -> SampleImportDraft {
        SampleImportDraft::new(
            vec![
                ready_sample("/drop/808s/kicks/kick.wav", "Kick"),
                ready_sample("/drop/808s/claps/clap.wav", "Clap"),
                ready_sample("/drop/808s/claps/clap2.wav", "Clap 2"),
            ],
            vec![
                "808s/kicks/kick.wav".to_string(),
                "808s/claps/clap.wav".to_string(),
                "808s/claps/clap2.wav".to_string(),
            ],
            vec!["hi-hat".to_string(), "snare".to_string(), "clap".to_string()],
            vec!["808s".to_string()],
        )
    }

    fn map_field(value: &Value, key: &str) -> Value {
        let Value::Map(map) = value else {
            panic!("expected a map, got {value:?}");
        };
        map[key].borrow().clone()
    }

    fn list_len(value: &Value) -> usize {
        match value {
            Value::List(items) => items.len(),
            other => panic!("expected a list, got {other:?}"),
        }
    }

    #[test]
    fn typed_tag_adopts_existing_candidate_spelling() {
        let mut draft = draft();
        assert!(draft.add_tag_to(&[1], "CLAP"));
        assert_eq!(draft.staged[1].tags, vec!["clap"]);
        assert!(draft.add_tag_to(&[1], "clap"));
        assert_eq!(draft.staged[1].tags, vec!["clap"]);
        assert!(!draft.add_tag_to(&[1], "   "));
    }

    #[test]
    fn new_tags_join_the_vocabulary_for_later_suggestions() {
        let mut draft = draft();
        assert!(draft.add_batch_tag("808"));
        assert_eq!(draft.suggestions("80", &[]), vec!["808"]);
        assert_eq!(draft.batch_tags, vec!["808"]);
        draft.remove_batch_tag("808");
        assert!(draft.batch_tags.is_empty());
    }

    #[test]
    fn suggestions_prefer_prefix_matches_and_skip_excluded_tags() {
        let mut draft = draft();
        draft.add_tag_candidates(&["snare-roll".to_string(), "brush-snare".to_string()]);
        assert_eq!(
            draft.suggestions("sn", &[]),
            vec!["snare", "snare-roll", "brush-snare"]
        );
        assert_eq!(
            draft.suggestions("sn", &["snare".to_string()]),
            vec!["snare-roll", "brush-snare"]
        );
        assert!(draft.suggestions("", &[]).is_empty());
    }

    #[test]
    fn selection_targets_follow_the_tree_node() {
        let mut draft = draft();
        assert_eq!(draft.selection_targets(), vec![0, 1, 2]);
        draft.select("808s/claps");
        assert_eq!(draft.selection_targets(), vec![1, 2]);
        assert_eq!(draft.selected_file_index(), None);
        draft.select("808s/claps/clap.wav");
        assert_eq!(draft.selection_targets(), vec![1]);
        assert_eq!(draft.selected_file_index(), Some(1));
        // A folder whose name prefixes another must not capture it.
        draft.select("808s/clap");
        assert!(draft.selection_targets().is_empty());
        draft.select("");
        assert_eq!(draft.selection_targets(), vec![0, 1, 2]);
    }

    #[test]
    fn folder_selection_tags_the_subtree_and_reports_shared_tags() {
        let mut draft = draft();
        draft.select("808s/claps");
        let targets = draft.selection_targets();
        assert!(draft.add_tag_to(&targets, "perc"));
        assert!(draft.staged[0].tags.is_empty());
        assert_eq!(draft.staged[1].tags, vec!["perc"]);
        assert_eq!(draft.staged[2].tags, vec!["perc"]);
        let selection = draft.selection_value();
        assert_eq!(map_field(&selection, "kind"), Value::String("folder".to_string()));
        assert_eq!(map_field(&selection, "label"), Value::String("claps".to_string()));
        assert_eq!(list_len(&map_field(&selection, "tags")), 1);
        assert!(matches!(map_field(&selection, "count"), Value::Number(n) if n == 2.0));
        draft.remove_tag_from(&targets, "PERC");
        assert!(draft.staged[1].tags.is_empty());
    }

    #[test]
    fn tree_nests_folders_then_files_with_counts() {
        let draft = draft();
        let Value::List(roots) = &draft.tree else {
            panic!("tree must be a list");
        };
        assert_eq!(roots.len(), 1);
        let root = roots[0].borrow().clone();
        assert_eq!(map_field(&root, "label"), Value::String("808s".to_string()));
        assert_eq!(map_field(&root, "detail"), Value::String("3".to_string()));
        let children = map_field(&root, "children");
        assert_eq!(list_len(&children), 2);
        let Value::List(children) = children else { unreachable!() };
        let kicks = children[0].borrow().clone();
        assert_eq!(map_field(&kicks, "path"), Value::String("808s/kicks".to_string()));
        let Value::List(kick_files) = map_field(&kicks, "children") else {
            panic!("folder children");
        };
        let kick = kick_files[0].borrow().clone();
        assert_eq!(
            map_field(&kick, "path"),
            Value::String("808s/kicks/kick.wav".to_string())
        );
        assert_eq!(map_field(&kick, "icon"), Value::String("waveform".to_string()));
    }

    #[test]
    fn node_paths_start_at_the_dropped_folder_name() {
        let roots = vec![PathBuf::from("/nonexistent/drop/808s")];
        // Roots that do not exist on disk are not directories, so the file
        // gathers under "Files"; the on-disk case is covered by staging a
        // real folder in the capture fixture.
        assert_eq!(
            node_path_for(Path::new("/nonexistent/drop/808s/kick.wav"), &roots),
            "Files/kick.wav"
        );
        let dir = std::env::temp_dir().join(format!("eseq-import-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("claps")).unwrap();
        let file = dir.join("claps").join("clap.wav");
        let root_name = dir.file_name().unwrap().to_str().unwrap().to_string();
        assert_eq!(
            node_path_for(&file, std::slice::from_ref(&dir)),
            format!("{root_name}/claps/clap.wav")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn summary_hides_suggested_tags_already_in_the_batch() {
        let mut draft = draft();
        assert_eq!(list_len(&map_field(&draft.summary_value(), "suggested-tags")), 1);
        draft.add_batch_tag("808s");
        assert_eq!(list_len(&map_field(&draft.summary_value(), "suggested-tags")), 0);
        assert!(matches!(
            map_field(&draft.summary_value(), "ready"),
            Value::Number(n) if n == 3.0
        ));
    }

    #[test]
    fn natives_edit_the_installed_draft() {
        let mut runtime = Runtime::new();
        register_sample_import_natives(&mut runtime);
        install_draft(draft());
        assert_eq!(
            runtime.eval_str("(seq-sample-import-open?)").unwrap(),
            Some(Value::Bool(true))
        );
        runtime
            .eval_str("(seq-sample-import-select \"808s/claps\")")
            .unwrap();
        runtime
            .eval_str("(seq-sample-import-add-selection-tag \"CLAP\")")
            .unwrap();
        runtime
            .eval_str("(seq-sample-import-set-title 0 \"Big Kick\")")
            .unwrap();
        let draft = take_draft().expect("draft installed");
        assert!(draft.staged[0].tags.is_empty());
        assert_eq!(draft.staged[1].tags, vec!["clap"]);
        assert_eq!(draft.staged[2].tags, vec!["clap"]);
        assert_eq!(draft.staged[0].title, "Big Kick");
        assert!(!draft_is_open());
        assert_eq!(
            runtime.eval_str("(seq-sample-import-summary)").unwrap(),
            Some(Value::Nil)
        );
    }
}
