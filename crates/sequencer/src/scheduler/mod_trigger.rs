//! Scheduler-side pieces of a mod input trigger (docs/mod-input-roles-spec.md
//! §3-§4), exposed for the audio callback. Every function here runs on the
//! audio thread, so none of them may allocate.

use super::*;

/// What a mod input trigger plays on a track that has no latched scheduler
/// trigger yet: the track's defaults, exactly as a seedless neural fire
/// resolves them (no step p-locks, no chord, no effect params).
pub(crate) struct DefaultModTrigger {
    pub(crate) resolved: ResolvedStep,
    pub(crate) chord: ScheduledChordData,
    pub(crate) instrument_params: ScheduledInstrumentParams,
    pub(crate) sampler_params: ScheduledSamplerParams,
    pub(crate) voice_policy: scheduled_event::ScheduledVoicePolicy,
    pub(crate) instrument_fingerprint: u64,
}

pub(crate) fn default_mod_trigger(snapshot: &SequencerSnapshot, track_idx: usize) -> Option<DefaultModTrigger> {
    let track = snapshot.tracks.get(track_idx)?;
    let instrument_params = resolve_instrument_defaults(snapshot, track_idx);
    let mut sampler_params = resolve_sampler_params(snapshot, track_idx, usize::MAX);
    apply_sampler_instrument_param_overrides(snapshot, track_idx, &mut sampler_params, &instrument_params);
    Some(DefaultModTrigger {
        resolved: crate::generator::default_resolved(),
        chord: ScheduledChordData {
            live_origins: [None; MAX_VOICES],
            count: 0,
            notes: [0.0; MAX_VOICES],
            durations: [0.0; MAX_VOICES],
            delays: [0.0; MAX_VOICES],
            step_transpose: 0.0,
        },
        voice_policy: scheduled_event::ScheduledVoicePolicy::from_track_with_params(track, &instrument_params),
        instrument_fingerprint: instrument_sound_fingerprint(snapshot, track_idx, &instrument_params, &[]),
        instrument_params,
        sampler_params,
    })
}

/// Quantize a trigger's transpose (and its chord) to the track's fit-to-scale
/// setting, as the scheduler does before enqueueing a step.
pub(crate) fn fit_mod_trigger_to_scale(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    resolved: ResolvedStep,
    chord: ScheduledChordData,
) -> (ResolvedStep, ScheduledChordData) {
    apply_fit_to_scale_to_trigger(snapshot, track_idx, resolved, chord)
}
