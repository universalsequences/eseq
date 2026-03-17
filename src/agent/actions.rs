use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentInstrumentParamSchema {
    pub name: String,
    pub group: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub current_value: Option<f32>,
    pub unit: Option<String>,
    pub enum_labels: Vec<String>,
    pub scaling: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentInstrumentPresetSchema {
    pub instrument_name: String,
    pub source_file: Option<String>,
    pub base_note_offset: f32,
    pub existing_presets: Vec<String>,
    pub params: Vec<AgentInstrumentParamSchema>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentInstrumentPresetDraft {
    pub name: String,
    pub base_note_offset: Option<f32>,
    pub params: BTreeMap<String, f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentSessionContext {
    pub has_tracks: bool,
    pub current_track_name: Option<String>,
    pub current_track_index: Option<usize>,
    pub can_apply_effect_to_current_track: bool,
    pub current_effect_name: Option<String>,
    pub current_effect_source: Option<String>,
    pub current_effect_slot: Option<usize>,
    pub can_update_current_effect: bool,
    pub current_instrument_name: Option<String>,
    pub current_instrument_source: Option<String>,
    pub can_update_current_instrument: bool,
    pub current_instrument_preset_schema: Option<AgentInstrumentPresetSchema>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentAppAction {
    CreateInstrumentTrack { name: String, source: String },
    ApplyEffectToCurrentTrack { name: String, source: String },
    UpdateCurrentEffect { name: String, source: String },
    UpdateCurrentInstrument { name: String, source: String },
    SaveCurrentInstrumentPresets {
        instrument_name: String,
        presets: Vec<AgentInstrumentPresetDraft>,
    },
}

pub fn normalize_patch_name(raw: &str, fallback: &str) -> String {
    let normalized = raw
        .trim()
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' => ch.to_ascii_lowercase(),
            ' ' | '-' | '_' => '-',
            _ => '-',
        })
        .collect::<String>();

    let mut collapsed = String::with_capacity(normalized.len());
    let mut last_was_dash = false;
    for ch in normalized.chars() {
        if ch == '-' {
            if !last_was_dash {
                collapsed.push(ch);
            }
            last_was_dash = true;
        } else {
            collapsed.push(ch);
            last_was_dash = false;
        }
    }

    let trimmed = collapsed.trim_matches('-');
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_patch_name;

    #[test]
    fn normalize_patch_name_slugifies_input() {
        assert_eq!(
            normalize_patch_name(" FM Bass / 01 ", "fallback"),
            "fm-bass-01"
        );
    }

    #[test]
    fn normalize_patch_name_uses_fallback_when_empty() {
        assert_eq!(normalize_patch_name("///", "fallback"), "fallback");
    }
}
