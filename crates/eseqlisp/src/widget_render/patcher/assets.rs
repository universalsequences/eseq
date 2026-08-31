use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

use crate::layout::LayoutNode;

use super::prop_str;

struct AssetRoots {
    fallback_base: PathBuf,
    libraries: [PathBuf; 2],
}

static ASSET_ROOTS: OnceLock<AssetRoots> = OnceLock::new();

/// Installs the content fallback base and mutable/factory asset libraries used
/// by DGenLisp asset resolution. Patcher autocomplete lists only the libraries;
/// host metadata reads also use the content base when no draft path exists.
/// This lets generated custom-UI buffers resolve fully content-relative paths
/// even though they no longer have the original UI file as their active path.
pub fn set_asset_roots(fallback_base: PathBuf, user: PathBuf, factory: PathBuf) {
    let _ = ASSET_ROOTS.set(AssetRoots {
        fallback_base,
        libraries: [user, factory],
    });
}

/// Resolves the same relative asset spelling used by DGenLisp: draft-local (or
/// the configured content base), then the user asset library, then the factory
/// asset library. Absolute paths are deliberately excluded from host UI reads.
pub(crate) fn resolve_asset_reference(reference: &str, draft_root: Option<&Path>) -> Option<PathBuf> {
    let roots = ASSET_ROOTS.get();
    resolve_asset_reference_with_fallback_roots(
        reference,
        draft_root,
        roots.map(|roots| roots.fallback_base.as_path()),
        roots.map(|roots| roots.libraries[0].as_path()),
        roots.map(|roots| roots.libraries[1].as_path()),
    )
}

pub(crate) fn resolve_asset_reference_with_fallback_roots(
    reference: &str,
    draft_root: Option<&Path>,
    fallback_root: Option<&Path>,
    user_root: Option<&Path>,
    factory_root: Option<&Path>,
) -> Option<PathBuf> {
    let reference = Path::new(reference);
    if reference.is_absolute() {
        return None;
    }

    for root in [draft_root, fallback_root].into_iter().flatten() {
        let candidate = root.join(reference);
        if candidate.is_file() {
            return Some(candidate.canonicalize().unwrap_or(candidate));
        }
    }

    for root in [user_root, factory_root].into_iter().flatten() {
        let candidate = root.join(reference);
        if !candidate.is_file() {
            continue;
        }
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let canonical_candidate = candidate.canonicalize().unwrap_or(candidate);
        if canonical_candidate.starts_with(&canonical_root) {
            return Some(canonical_candidate);
        }
    }
    None
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PatcherAssetSidebarEntry {
    pub reference: String,
    pub source_path: PathBuf,
    pub tier: &'static str,
}

/// Lists tensor JSON assets available to the active patch. Draft references are
/// relative to the patch directory; shared references are relative to their
/// library root, matching host-side `@file` resolution.
pub fn asset_sidebar_entries(source_path: Option<&Path>) -> Vec<PatcherAssetSidebarEntry> {
    let draft_root = source_path.and_then(Path::parent);
    let roots = ASSET_ROOTS.get();
    collect_asset_sidebar_entries(
        draft_root,
        roots.map(|roots| roots.libraries[0].as_path()),
        roots.map(|roots| roots.libraries[1].as_path()),
    )
}

pub(super) fn autocomplete_asset_paths_for_node(node: &LayoutNode) -> Vec<String> {
    let source_path = prop_str(&node.props, "path")
        .or_else(|| prop_str(&node.props, "file"))
        .map(PathBuf::from);
    let draft_root = source_path.as_deref().and_then(Path::parent);
    let library_roots = ASSET_ROOTS
        .get()
        .map(|roots| roots.libraries.as_slice())
        .unwrap_or(&[]);
    collect_asset_path_spellings(draft_root, source_path.as_deref(), library_roots)
}

pub(super) fn collect_asset_sidebar_entries(
    draft_root: Option<&Path>,
    user_root: Option<&Path>,
    factory_root: Option<&Path>,
) -> Vec<PatcherAssetSidebarEntry> {
    let mut entries = Vec::new();
    if let Some(root) = draft_root {
        collect_tensor_assets_under(root, root, "Draft", &mut entries);
    }
    if let Some(root) = user_root {
        collect_tensor_assets_under(root, root, "User", &mut entries);
    }
    if let Some(root) = factory_root {
        collect_tensor_assets_under(root, root, "Factory", &mut entries);
    }
    entries.sort_by(|left, right| {
        let tier_order = |tier| match tier {
            "Draft" => 0,
            "User" => 1,
            _ => 2,
        };
        tier_order(left.tier)
            .cmp(&tier_order(right.tier))
            .then_with(|| left.reference.cmp(&right.reference))
    });
    entries
}

fn collect_tensor_assets_under(
    root: &Path,
    directory: &Path,
    tier: &'static str,
    entries: &mut Vec<PatcherAssetSidebarEntry>,
) {
    let Ok(directory_entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in directory_entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            collect_tensor_assets_under(root, &path, tier, entries);
            continue;
        }
        if !file_type.is_file()
            || path.extension().and_then(|extension| extension.to_str()) != Some("json")
            || read_tensor_asset_shape(&path).is_none()
        {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let Some(reference) = relative.to_str() else {
            continue;
        };
        let reference = reference.replace(std::path::MAIN_SEPARATOR, "/");
        if reference.is_empty()
            || reference
                .chars()
                .any(|character| character == '"' || character.is_control())
        {
            continue;
        }
        entries.push(PatcherAssetSidebarEntry {
            reference,
            source_path: path,
            tier,
        });
    }
}

#[derive(Deserialize)]
struct TensorAssetHeader {
    shape: Vec<u64>,
}

pub(super) fn read_tensor_asset_shape(path: &Path) -> Option<Vec<u64>> {
    let header: TensorAssetHeader = serde_json::from_reader(File::open(path).ok()?).ok()?;
    (!header.shape.is_empty() && header.shape.iter().all(|dimension| *dimension > 0))
        .then_some(header.shape)
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
