//! The model the patcher's agentic bubbles (Cmd+K / Cmd+Shift+K) run on.
//!
//! Distinct from `store::ConversationStore`'s per-conversation model: a bubble
//! is not a conversation, it is a one-shot turn, so the choice has to live
//! somewhere process-global. `M-x choose-model` writes it; every bubble reads
//! it. Unset falls back to the historical behaviour in `agentic_bubble.rs`
//! (Gemini flash when a Gemini key is present).
//!
//! Persisted through `AppPaths`: `.eseq/prefs.json` in development and
//! Application Support in an installed app. Persistence is best-effort: a read
//! or write failure degrades to "no choice recorded" rather than failing the
//! bubble.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use super::models::AgentModelCatalog;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Prefs {
    /// Model id, validated against the runtime catalog when selected or used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agentic_model: Option<String>,
}

fn prefs_path() -> PathBuf {
    crate::app_paths::app_paths().preferences_path()
}

fn cell() -> &'static Mutex<Option<String>> {
    static CELL: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(load_from_disk()))
}

fn load_from_disk() -> Option<String> {
    let raw = std::fs::read_to_string(prefs_path()).ok()?;
    let prefs: Prefs = serde_json::from_str(&raw).ok()?;
    prefs.agentic_model.filter(|id| !id.trim().is_empty())
}

fn save_to_disk(model: Option<&str>) {
    let path = prefs_path();
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!(
                "[model-choice] could not create {}: {error}",
                parent.display()
            );
            return;
        }
    }
    // Read-modify-write so an unrelated future pref in the same file survives.
    let mut prefs: Prefs = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    prefs.agentic_model = model.map(str::to_string);
    match serde_json::to_string_pretty(&prefs) {
        Ok(json) => {
            if let Err(error) = std::fs::write(&path, json) {
                eprintln!("[model-choice] could not write {}: {error}", path.display());
            }
        }
        Err(error) => eprintln!("[model-choice] could not encode prefs: {error}"),
    }
}

/// The chosen model id, or `None` when the user has never picked one.
pub fn agentic_model() -> Option<String> {
    cell().lock().ok().and_then(|guard| guard.clone())
}

/// Record a choice. Reject ids absent from the current catalog before changing
/// preferences. A removed saved choice is retained so a new request can report
/// it explicitly instead of silently switching models or providers.
pub fn set_agentic_model(model: &str) -> Result<(), String> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return Err("model id is empty".to_string());
    }
    if AgentModelCatalog::load()?.model(trimmed).is_none() {
        return Err(format!("unknown model {trimmed}"));
    }
    if let Ok(mut guard) = cell().lock() {
        *guard = Some(trimmed.to_string());
    }
    save_to_disk(Some(trimmed));
    Ok(())
}

/// Clear the choice, restoring the built-in default-provider behaviour.
pub fn clear_agentic_model() {
    if let Ok(mut guard) = cell().lock() {
        *guard = None;
    }
    save_to_disk(None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_models() {
        assert!(set_agentic_model("not-a-real-model").is_err());
        assert!(set_agentic_model("  ").is_err());
    }
}
