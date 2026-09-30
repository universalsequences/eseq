//! Instrument-owned allocation controls. Values live in the ordinary device
//! parameter store, so presets, history, projects and parameter locks agree.
use super::{EffectDescriptor, EffectSlotSnapshot};
use crate::scheduled_event::{ScheduledInstrumentParams, ScheduledInstrumentParamTarget};
use crate::sequencer::MonoTrigger;

/// Roles form one explicit contract: voice-mode = Poly/Mono/Stereo/Unison
/// (0/1/2/3), voice-count = physical oscillator voices (1..32), legato = bool.
/// Stereo consumes two physical voices per note, Unison consumes four. Mono
/// always allocates one note; its DSP implements the four component voices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstrumentVoiceControls {
    mode: usize,
    count: usize,
    legato: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstrumentVoiceConfig {
    pub polyphonic: bool,
    pub max_polyphony: usize,
    pub mono_trigger: MonoTrigger,
}

impl InstrumentVoiceControls {
    pub fn from_descriptor(desc: &EffectDescriptor) -> Option<Self> {
        let role = |name: &str| desc.params.iter().position(|p|
            p.ui_metadata.as_ref().and_then(|ui| ui.role.as_deref()) == Some(name));
        Some(Self { mode: role("voice-mode")?, count: role("voice-count")?, legato: role("legato")? })
    }

    pub fn resolve(self, value: impl Fn(usize) -> f32) -> InstrumentVoiceConfig {
        let finite = |index, fallback| { let v = value(index); if v.is_finite() { v } else { fallback } };
        let mode = finite(self.mode, 0.0).round().clamp(0.0, 3.0) as usize;
        let physical = finite(self.count, 1.0).round().clamp(1.0, crate::audio::MAX_VOICES as f32) as usize;
        let copies = [1, 4, 2, 4][mode];
        InstrumentVoiceConfig {
            polyphonic: mode != 1,
            max_polyphony: if mode == 1 { 1 } else { (physical / copies).max(1) },
            mono_trigger: if mode == 1 && finite(self.legato, 0.0) > 0.5 { MonoTrigger::Legato } else { MonoTrigger::Retrig },
        }
    }
}

impl EffectSlotSnapshot {
    pub fn instrument_voice_config(&self, params: &ScheduledInstrumentParams) -> Option<InstrumentVoiceConfig> {
        Some(self.instrument_voice_controls?.resolve(|index| {
            let node_index = self.param_node_indices.get(index).copied();
            params.iter().rev().find(|p| p.target == ScheduledInstrumentParamTarget::Synth && Some(p.idx) == node_index.map(u64::from))
                .map(|p| p.value).unwrap_or_else(|| self.defaults.get(index).copied().unwrap_or(0.0))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_voice_budget_and_legato_follow_instrument_values() {
        let controls = InstrumentVoiceControls { mode: 0, count: 1, legato: 2 };
        for (mode, notes) in [(0.0,32),(1.0,1),(2.0,16),(3.0,8)] {
            let values = [mode,32.0,1.0];
            let config = controls.resolve(|i| values[i]);
            assert_eq!(config.max_polyphony, notes);
            assert_eq!(config.polyphonic, mode != 1.0);
            assert_eq!(config.mono_trigger, if mode == 1.0 { MonoTrigger::Legato } else { MonoTrigger::Retrig });
        }
        assert_eq!(controls.resolve(|i| [3.0,1.0,0.0][i]).max_polyphony,1);
        assert_eq!(controls.resolve(|_| f32::NAN).max_polyphony,1);
    }
}
