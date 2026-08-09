//! The model the patcher's agentic bubbles (Cmd+K / Cmd+Shift+K) run on.
//!
//! Distinct from `store::ConversationStore`'s per-conversation model: a bubble
//! is not a conversation, it is a one-shot turn, so the choice has to live
//! somewhere process-global. `M-x choose-model` writes it; every bubble reads
//! it. Unset falls back to the historical behaviour in `agentic_bubble.rs`
//! (Gemini flash when a Gemini key is present).
//!
//! Persisted to `.eseq/prefs.json` under the workspace root — the same
//! directory the dgen dylib cache already uses — so the choice survives a
//! relaunch. Persistence is best-effort: a read or write failure degrades to
//! "no choice recorded" rather than failing the bubble.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use super::providers::{default_model_presets, AgentProviderKind};

#[derive(Debug, Default, Serialize, Deserialize)]
struct Prefs {
    /// Model id (e.g. "claude-opus-5"), matched against `default_model_presets`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agentic_model: Option<String>,
}

fn prefs_path() -> PathBuf {
    crate::paths::workspace_root()
        .join(".eseq")
        .join("prefs.json")
}

fn cell() -> &'static Mutex<Option<String>> {
    static CELL: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(load_from_disk()))
}

fn load_from_disk() -> Option<String> {
    let raw = std::fs::read_to_string(prefs_path()).ok()?;
    let prefs: Prefs = serde_json::from_str(&raw).ok()?;
    prefs
        .agentic_model
        .filter(|id| !id.trim().is_empty())
        .filter(|id| is_known_model(id))
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

fn is_known_model(id: &str) -> bool {
    default_model_presets().iter().any(|preset| preset.id == id)
}

/// The chosen model id, or `None` when the user has never picked one.
pub fn agentic_model() -> Option<String> {
    cell().lock().ok().and_then(|guard| guard.clone())
}

/// The provider that serves `agentic_model()`, resolved from the preset table.
pub fn agentic_provider() -> Option<AgentProviderKind> {
    let model = agentic_model()?;
    default_model_presets()
        .into_iter()
        .find(|preset| preset.id == model)
        .map(|preset| preset.provider)
}

/// Record a choice. An id absent from `default_model_presets` is rejected so a
/// typo in Lisp can't silently point every bubble at a nonexistent model.
pub fn set_agentic_model(model: &str) -> Result<(), String> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return Err("model id is empty".to_string());
    }
    if !is_known_model(trimmed) {
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

    #[test]
    fn every_preset_id_is_accepted_as_known() {
        for preset in default_model_presets() {
            assert!(is_known_model(&preset.id), "{} not known", preset.id);
        }
    }

    #[test]
    fn provider_resolves_from_the_preset_table() {
        // Pure lookup over the preset table — no global state touched, so this
        // stays independent of whatever the developer's prefs.json holds.
        let preset = default_model_presets()
            .into_iter()
            .next()
            .expect("preset table is non-empty");
        let resolved = default_model_presets()
            .into_iter()
            .find(|entry| entry.id == preset.id)
            .map(|entry| entry.provider);
        assert_eq!(resolved, Some(preset.provider));
    }
}
