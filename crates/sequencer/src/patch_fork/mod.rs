/*!
Filesystem primitives behind "fork an instrument / effect"
(`docs/instrument-fork-spec.md`).

A fork is the existing *draft* authoring flow with a different seed: instead of
writing the starter template into the draft directory, we copy an existing
instrument (or effect) into it. Everything downstream — live preview on a
transient track, error gating, name-collision refusal, finalize — is reused.

Three pieces live here:

* [`fork_patch_files`] seeds a draft directory from a source directory. It
  copies the whole directory (`dsp.lisp`, `dsp.layout.json`, `ui.lisp`,
  `instrument.json`, asset dirs such as `waves/`) and *stages* the source's
  preset bank inside the draft as [`STAGED_PRESET_BANK_FILE`], because the bank
  carries the old engine name and has to be rewritten at finalize. Both on-disk
  forms are probed: the sibling `<name>.presets` and the in-directory
  [`PRESET_BANK_FILE`] (`<name>/.presets`, which is what
  `resolve_instrument_storage_path` produces for a trailing-slash engine name
  and therefore what folder instruments actually ship).
* [`rewrite_preset_bank_json`] repoints a bank's `engine_name` / `source_file`
  at the fork. Those two fields embed the *old* instrument name.
* [`materialize_forked_assets`] is the finalize-time other half: the existing
  `save-new-instrument` path only writes `dsp.lisp` plus the layout sidecar, so
  every other artifact the fork carried has to be copied out of the draft into
  the finalized directory, and the staged bank written back out as
  `<final_dir>/.presets` — the path the loader resolves *exactly* for the
  finalized `"<slug>/"` engine name, rather than the sibling form it can only
  reach through the ambiguous directory walk.

The layout sidecar is copied when it exists. The spec calls it mandatory, but
several shipped instruments (e.g. `core/triton`) have no authored sidecar at
all; refusing to fork those would be worse than reproducing the source's own
auto-materialized layout.
*/

use std::path::{Path, PathBuf};

/// Name the source's `<name>.presets` bank takes while it is staged inside a
/// draft directory. Hidden so the patcher/asset scans ignore it, and stripped
/// again by [`materialize_forked_assets`].
pub const STAGED_PRESET_BANK_FILE: &str = ".fork-staged.presets";

/// The bank file *inside* a folder-style patch dir. `instruments/flutefab/`
/// stores its presets at `instruments/flutefab/.presets`, which is exactly
/// what `resolve_instrument_storage_path("flutefab/", "presets")` builds.
pub const PRESET_BANK_FILE: &str = ".presets";

/// Files the finalize path writes itself; copying them out of the draft would
/// clobber the freshly compiled emission, its layout, or the run mode the
/// session is authoritative for (`save_instrument_run_mode`).
const FINALIZE_OWNED_FILES: &[&str] = &["dsp.lisp", "dsp.layout.json", "instrument.json"];

/// `instruments/core/triton` -> `instruments/core/triton.presets`.
///
/// Deliberately not `Path::with_extension`, which would eat a trailing
/// `.something` in a directory name.
#[must_use]
pub fn preset_bank_sibling_path(patch_dir: &Path) -> Option<PathBuf> {
    let file_name = patch_dir.file_name()?.to_str()?;
    let mut path = patch_dir.to_path_buf();
    path.set_file_name(format!("{file_name}.presets"));
    Some(path)
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target)
        .map_err(|error| format!("Failed to create '{}': {error}", target.display()))?;
    let entries = std::fs::read_dir(source)
        .map_err(|error| format!("Failed to read '{}': {error}", source.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("Failed to read '{}': {error}", source.display()))?;
        let name = entry.file_name();
        let from = entry.path();
        let to = target.join(&name);
        let file_type = entry
            .file_type()
            .map_err(|error| format!("Failed to stat '{}': {error}", from.display()))?;
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|error| {
                format!(
                    "Failed to copy '{}' to '{}': {error}",
                    from.display(),
                    to.display()
                )
            })?;
        }
    }
    Ok(())
}

/// Seed `draft_dir` with a copy of the instrument/effect directory
/// `source_dir`, staging its sibling preset bank inside the draft.
///
/// `draft_dir` is created if missing and is expected to be empty or to contain
/// only a placeholder `dsp.lisp` (the caller mints it via
/// `create_new_instrument_draft_dir`); existing entries with colliding names
/// are overwritten.
pub fn fork_patch_files(source_dir: &Path, draft_dir: &Path) -> Result<(), String> {
    if !source_dir.is_dir() {
        return Err(format!(
            "'{}' is not a folder-style patch directory",
            source_dir.display()
        ));
    }
    if !source_dir.join("dsp.lisp").is_file() {
        return Err(format!(
            "'{}' has no dsp.lisp to fork",
            source_dir.display()
        ));
    }
    copy_dir_recursive(source_dir, draft_dir)?;

    // Folder instruments keep the bank *inside* the directory
    // (`instruments/flutefab/.presets`); older ones keep it as a sibling. The
    // in-directory copy already rode along in `copy_dir_recursive`, but
    // `materialize_forked_assets` skips dot-files, so it still has to be
    // staged under the name finalize looks for.
    let bank_path = preset_bank_sibling_path(source_dir)
        .filter(|path| path.is_file())
        .or_else(|| Some(source_dir.join(PRESET_BANK_FILE)).filter(|path| path.is_file()));
    if let Some(bank_path) = bank_path {
        let staged = draft_dir.join(STAGED_PRESET_BANK_FILE);
        std::fs::copy(&bank_path, &staged).map_err(|error| {
            format!(
                "Failed to stage preset bank '{}': {error}",
                bank_path.display()
            )
        })?;
    }
    Ok(())
}

/// Seed `draft_dir` from a *resolved source path* — either a folder-style
/// `.../<name>/dsp.lisp` or a legacy flat `.../<name>.lisp`.
///
/// Flat instruments (`instruments/strings/concert-harp.lisp`) have no directory
/// to copy; their layout sidecar and preset bank are name-adjacent files, and
/// the fork is normalized into the folder layout every new draft uses.
pub fn fork_patch_source(source_dsp: &Path, draft_dir: &Path) -> Result<(), String> {
    if source_dsp.file_name().and_then(|name| name.to_str()) == Some("dsp.lisp") {
        let Some(parent) = source_dsp.parent() else {
            return Err(format!(
                "'{}' has no parent directory",
                source_dsp.display()
            ));
        };
        return fork_patch_files(parent, draft_dir);
    }
    if !source_dsp.is_file() {
        return Err(format!("'{}' does not exist", source_dsp.display()));
    }
    std::fs::create_dir_all(draft_dir)
        .map_err(|error| format!("Failed to create '{}': {error}", draft_dir.display()))?;
    std::fs::copy(source_dsp, draft_dir.join("dsp.lisp"))
        .map_err(|error| format!("Failed to copy '{}': {error}", source_dsp.display()))?;
    let stem = source_dsp.with_extension("");
    let layout = source_dsp.with_extension("layout.json");
    if layout.is_file() {
        std::fs::copy(&layout, draft_dir.join("dsp.layout.json"))
            .map_err(|error| format!("Failed to copy '{}': {error}", layout.display()))?;
    }
    let metadata = source_dsp.with_extension("instrument.json");
    if metadata.is_file() {
        std::fs::copy(&metadata, draft_dir.join("instrument.json"))
            .map_err(|error| format!("Failed to copy '{}': {error}", metadata.display()))?;
    }
    if let Some(bank) = preset_bank_sibling_path(&stem) {
        if bank.is_file() {
            std::fs::copy(&bank, draft_dir.join(STAGED_PRESET_BANK_FILE)).map_err(|error| {
                format!("Failed to stage preset bank '{}': {error}", bank.display())
            })?;
        }
    }
    Ok(())
}

/// Repoint a preset bank at `new_engine_name` (`"my-fork/"` — the same
/// trailing-slash form `save_instrument_presets` writes).
///
/// Only `engine_name` and `source_file` change; presets key by parameter
/// *name*, so the bank is valid for a fork by construction.
pub fn rewrite_preset_bank_json(bank_json: &str, new_engine_name: &str) -> Result<String, String> {
    let mut bank: serde_json::Value = serde_json::from_str(bank_json)
        .map_err(|error| format!("Failed to parse preset bank: {error}"))?;
    let serde_json::Value::Object(ref mut map) = bank else {
        return Err("Preset bank is not a JSON object".to_string());
    };
    map.insert(
        "engine_name".to_string(),
        serde_json::Value::String(new_engine_name.to_string()),
    );
    map.insert(
        "source_file".to_string(),
        serde_json::Value::String(format!("instruments/{new_engine_name}.lisp")),
    );
    serde_json::to_string_pretty(&bank)
        .map_err(|error| format!("Failed to serialize preset bank: {error}"))
}

/// Finalize half of a fork: copy everything the draft carried beyond
/// `dsp.lisp` / `dsp.layout.json` into `final_dir`, and write the staged
/// preset bank back out as `final_dir/.presets`, repointed at
/// `final_engine_name`.
///
/// The in-directory form is deliberate: finalized forks are folder-style, so
/// their engine name is `"<slug>/"` and `resolve_instrument_storage_path`
/// resolves the bank to `instruments/<slug>/.presets` without touching the
/// directory walk.
///
/// A no-op for drafts that were never forked (nothing but `dsp.lisp` and its
/// sidecar in them), so the finalize path can call it unconditionally.
pub fn materialize_forked_assets(
    draft_dir: &Path,
    final_dir: &Path,
    final_engine_name: &str,
) -> Result<(), String> {
    let entries = std::fs::read_dir(draft_dir)
        .map_err(|error| format!("Failed to read '{}': {error}", draft_dir.display()))?;
    let mut staged_bank: Option<PathBuf> = None;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("Failed to read '{}': {error}", draft_dir.display()))?;
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if name_str == STAGED_PRESET_BANK_FILE {
            staged_bank = Some(entry.path());
            continue;
        }
        if FINALIZE_OWNED_FILES.contains(&name_str) || name_str.starts_with('.') {
            continue;
        }
        let from = entry.path();
        let to = final_dir.join(&name);
        let file_type = entry
            .file_type()
            .map_err(|error| format!("Failed to stat '{}': {error}", from.display()))?;
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("Failed to create '{}': {error}", parent.display()))?;
            }
            std::fs::copy(&from, &to).map_err(|error| {
                format!(
                    "Failed to copy '{}' to '{}': {error}",
                    from.display(),
                    to.display()
                )
            })?;
        }
    }

    let Some(staged_bank) = staged_bank else {
        return Ok(());
    };
    let target_bank = final_dir.join(PRESET_BANK_FILE);
    let raw = std::fs::read_to_string(&staged_bank).map_err(|error| {
        format!(
            "Failed to read staged preset bank '{}': {error}",
            staged_bank.display()
        )
    })?;
    let rewritten = rewrite_preset_bank_json(&raw, final_engine_name)?;
    if let Some(parent) = target_bank.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create '{}': {error}", parent.display()))?;
    }
    std::fs::write(&target_bank, rewritten).map_err(|error| {
        format!(
            "Failed to write preset bank '{}': {error}",
            target_bank.display()
        )
    })
}

#[cfg(test)]
mod tests;
