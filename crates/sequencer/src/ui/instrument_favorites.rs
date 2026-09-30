//! Favorited saved instruments for the browser's Instruments tab.
//!
//! Keyed by the canonical tier-qualified id (`factory:Synths/Digi Syn`,
//! `user:MF DOOM Kicks/Boom-Bap Kick 51`, `pkg:…`), never by a row's raw
//! `:name`: Factory and Library rows carry bare names with a trailing `/`
//! for folder instruments, while Engines rows carry the qualified id the
//! project stored. [`canonical_instrument_id`] folds both onto one key so a
//! heart set in one section shows in every section holding that instrument.
//!
//! Persisted as `favorites.json` under `AppPaths::user_data_root` (`.local/`
//! in development, Application Support when installed). It is a separate file
//! rather than a key in `prefs.json` because some `prefs.json` writers
//! re-serialize only the fields they know. Persistence is best-effort: a
//! missing or unreadable file reads as no favorites, and a failed write keeps
//! the in-memory set so the heart still toggles for this session.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use sequencer::app_paths::ContentTier;
use serde::{Deserialize, Serialize};

/// The key favorites are stored under. `section_tier` is the tier of the
/// tree section a bare name came from (Factory or Library); with none, a bare
/// name resolves the way loading it would (factory wins over user).
///
/// A versioned factory instrument (docs/instrument-versioning-spec.md) keys
/// on its lineage, `factory:<top folder>`, whatever release or old name the
/// id spells: a heart survives new releases and renames, and a favorite saved
/// as `factory:Synths/Digi Drift` lands on the Digi Syn row once its manifest
/// lists Digi Drift as a release.
pub(crate) fn canonical_instrument_id(section_tier: Option<&ContentTier>, name: &str) -> String {
    let trimmed = name.trim_end_matches('/');
    let versioned = match (ContentTier::parse_id(name), section_tier) {
        (Ok(Some((ContentTier::Factory, _))), _) | (Ok(None), None) => {
            sequencer::lisp_host::instrument_lineage_id(name)
        }
        (Ok(None), Some(ContentTier::Factory)) => {
            sequencer::lisp_host::instrument_lineage_id(&ContentTier::Factory.qualify(trimmed))
        }
        _ => None,
    };
    if let Some(lineage) = versioned {
        return lineage;
    }
    match ContentTier::parse_id(name) {
        Ok(Some((tier, path))) => tier.qualify(path),
        Ok(None) => match section_tier {
            Some(tier) => tier.qualify(trimmed),
            None => sequencer::lisp_host::qualify_instrument_id(name)
                .unwrap_or_else(|_| trimmed.to_string()),
        },
        Err(_) => trimmed.to_string(),
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FavoritesFile {
    #[serde(default)]
    instruments: BTreeSet<String>,
}

fn favorites_path() -> PathBuf {
    sequencer::app_paths::app_paths().user_data_root().join("favorites.json")
}

fn cell() -> &'static Mutex<BTreeSet<String>> {
    static CELL: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(canonicalize_loaded(load_from(&favorites_path()))))
}

fn load_from(path: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<FavoritesFile>(&raw).ok())
        .map(|file| file.instruments)
        .unwrap_or_default()
}

/// Entries written before ids were canonical were raw row names; fold them
/// onto their canonical key once at load.
fn canonicalize_loaded(instruments: BTreeSet<String>) -> BTreeSet<String> {
    instruments
        .iter()
        .map(|name| canonical_instrument_id(None, name))
        .collect()
}

fn save_to(path: &Path, instruments: &BTreeSet<String>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(&FavoritesFile { instruments: instruments.clone() })
        .map_err(|error| format!("could not encode favorites: {error}"))?;
    std::fs::write(path, json).map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn persist(instruments: &BTreeSet<String>) {
    if let Err(error) = save_to(&favorites_path(), instruments) {
        eprintln!("[favorites] {error}");
    }
}

/// Snapshot of every favorited instrument id.
pub(crate) fn favorite_instruments() -> BTreeSet<String> {
    cell().lock().map(|set| set.clone()).unwrap_or_default()
}

/// Whether the instrument with canonical id `id` is a favorite.
pub(crate) fn is_favorite_instrument(id: &str) -> bool {
    let id = canonical_instrument_id(None, id);
    cell().lock().map(|set| set.contains(&id)).unwrap_or(false)
}

/// Flip the favorite state of the instrument with canonical id `id` (a
/// row's `:favorite-id`) and return the new state.
pub(crate) fn toggle_favorite_instrument(id: &str) -> bool {
    let name = canonical_instrument_id(None, id);
    let name = name.as_str();
    let Ok(mut set) = cell().lock() else {
        return false;
    };
    let now_favorite = if set.remove(name) {
        false
    } else {
        set.insert(name.to_string());
        true
    };
    persist(&set);
    now_favorite
}

/// Carry a favorite across a move: moving a Library instrument into a folder
/// changes its id, and the heart should move with it.
/// Moves only happen in the user tier, so both names are Library names.
pub(crate) fn rename_favorite_instrument(old: &str, new: &str) {
    let old = canonical_instrument_id(Some(&ContentTier::User), old);
    let new = canonical_instrument_id(Some(&ContentTier::User), new);
    let (old, new) = (old.as_str(), new.as_str());
    let Ok(mut set) = cell().lock() else {
        return;
    };
    if old != new && set.remove(old) {
        set.insert(new.to_string());
        persist(&set);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorites_file_round_trips_and_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("favorites.json");
        assert!(load_from(&path).is_empty());

        let set: BTreeSet<String> =
            ["MF DOOM Kicks/Boom-Bap Kick 51".to_string(), "pkg:a.b/lead".to_string()].into();
        save_to(&path, &set).unwrap();
        assert_eq!(load_from(&path), set);
    }

    #[test]
    fn section_names_and_engine_ids_share_one_canonical_key() {
        let library = canonical_instrument_id(Some(&ContentTier::User), "MF DOOM Kicks/Boom-Bap Kick 51/");
        let engine = canonical_instrument_id(None, "user:MF DOOM Kicks/Boom-Bap Kick 51");
        assert_eq!(library, "user:MF DOOM Kicks/Boom-Bap Kick 51");
        assert_eq!(library, engine);
        assert_eq!(
            canonical_instrument_id(Some(&ContentTier::Factory), "Synths/Digi Drift/"),
            canonical_instrument_id(None, "factory:Synths/Digi Drift/"),
        );
        // A package row's name is already qualified; the section tier is moot.
        assert_eq!(
            canonical_instrument_id(Some(&ContentTier::Package("a.b".into())), "pkg:a.b/lead/"),
            "pkg:a.b/lead"
        );
    }

    #[test]
    fn unreadable_favorites_file_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("favorites.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(load_from(&path).is_empty());
    }
}
