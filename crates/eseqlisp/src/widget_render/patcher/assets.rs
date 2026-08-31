use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::layout::LayoutNode;

use super::prop_str;

static ASSET_LIBRARY_ROOTS: OnceLock<[PathBuf; 2]> = OnceLock::new();

/// Installs the mutable and factory asset roots used by patcher `@file`
/// autocomplete. The sequencer owns application path discovery; eseqlisp only
/// consumes the resolved roots.
pub fn set_asset_library_roots(user: PathBuf, factory: PathBuf) {
    let _ = ASSET_LIBRARY_ROOTS.set([user, factory]);
}

pub(super) fn autocomplete_asset_paths_for_node(node: &LayoutNode) -> Vec<String> {
    let source_path = prop_str(&node.props, "path")
        .or_else(|| prop_str(&node.props, "file"))
        .map(PathBuf::from);
    let draft_root = source_path.as_deref().and_then(Path::parent);
    let library_roots = ASSET_LIBRARY_ROOTS
        .get()
        .map(|roots| roots.as_slice())
        .unwrap_or(&[]);
    collect_asset_path_spellings(draft_root, source_path.as_deref(), library_roots)
}

pub(super) fn collect_asset_path_spellings(
    draft_root: Option<&Path>,
    source_path: Option<&Path>,
    library_roots: &[PathBuf],
) -> Vec<String> {
    let mut spellings = BTreeSet::new();
    if let Some(root) = draft_root {
        collect_asset_paths_under(root, root, source_path, &mut spellings);
    }
    for root in library_roots {
        collect_asset_paths_under(root, root, None, &mut spellings);
    }
    spellings.into_iter().collect()
}

fn collect_asset_paths_under(
    root: &Path,
    directory: &Path,
    excluded_path: Option<&Path>,
    spellings: &mut BTreeSet<String>,
) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            collect_asset_paths_under(root, &path, excluded_path, spellings);
            continue;
        }
        if !file_type.is_file() || excluded_path.is_some_and(|excluded| excluded == path) {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let Some(spelling) = relative.to_str() else {
            continue;
        };
        let spelling = spelling.replace(std::path::MAIN_SEPARATOR, "/");
        if !spelling.is_empty()
            && !spelling.chars().any(|ch| ch == '"' || ch.is_control())
        {
            spellings.insert(spelling);
        }
    }
}
