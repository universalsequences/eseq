//! Instrument-owned allocation controls. Values live in the ordinary device
//! parameter store, so presets, history, projects and parameter locks agree.
use super::{EffectDescriptor, EffectSlotSnapshot};
use crate::scheduled_event::{ScheduledInstrumentParams, ScheduledInstrumentParamTarget};

/// The instrument's voice-mode role (Poly/Mono/Stereo/Unison = 0/1/2/3)
/// shapes each note's sound; Stereo and Unison stack their copies inside one
/// DGen voice, so they cost more per note but never more notes. The track's
/// Voices and Trigger settings are the only note limit and legato control.
/// Mono is the one exception: its DSP is a monophonic synth (stacked voices,
/// glide), so it forces one-note allocation whatever the track says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstrumentVoiceControls {
    mode: usize,
}

impl InstrumentVoiceControls {
    pub fn from_descriptor(desc: &EffectDescriptor) -> Option<Self> {
        let mode = desc.params.iter().position(|p|
            p.ui_metadata.as_ref().and_then(|ui| ui.role.as_deref()) == Some("voice-mode"))?;
        Some(Self { mode })
    }

    pub fn forces_mono(self, value: impl Fn(usize) -> f32) -> bool {
        let mode = value(self.mode);
        mode.is_finite() && mode.round() == 1.0
    }
}

impl EffectSlotSnapshot {
    /// Whether this instrument's current voice mode is Mono, reading
    /// scheduled (p-locked) values over the slot's stored ones.
    pub fn instrument_forces_mono(&self, params: &ScheduledInstrumentParams) -> bool {
        self.instrument_voice_controls.is_some_and(|controls| controls.forces_mono(|index| {
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
    fn only_mono_mode_overrides_the_tracks_allocation() {
        let controls = InstrumentVoiceControls { mode: 0 };
        for (mode, mono) in [(0.0, false), (1.0, true), (2.0, false), (3.0, false), (f32::NAN, false)] {
            assert_eq!(controls.forces_mono(|_| mode), mono, "mode {mode}");
        }
    }
}
