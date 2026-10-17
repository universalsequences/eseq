/*!
Mod input roles on the audio thread (docs/mod-input-roles-spec.md).

A track whose ext mod input carries the `trigger` role is sequenced by that
signal. The callback cannot see block N's mod input before it renders block
N, so detection runs after `render_chunk` and the triggers it finds fire in
the next block at the same frame offsets: one engine block of latency, with
sample-exact spacing between cable triggers.

A trigger replays the track's *latch*, the last scheduler trigger dispatched on
it, with `velocity` / `duration` / `note` inputs overriding those fields.
With nothing latched, it plays the track's defaults like a seedless neural
fire.
*/

#[allow(unused_imports)]
use super::*;

use crate::instruments::mod_trigger_lanes::{self, LaneBlock};
use crate::scheduled_event::ScheduledChordData;
use crate::sequencer::{ModInputRole, ModInputRoles, ModSteps, EXT_MOD_INPUT_COUNT};

/// Cable triggers detected in one block. A gate that toggles faster than
/// this many times per block is far past audio-rate trigger use.
pub(super) const MOD_TRIGGER_CAPACITY: usize = 64;
/// Schmitt thresholds: rise at 0.5, re-arm below 0.25.
const RISE_THRESHOLD: f32 = 0.5;
const REARM_THRESHOLD: f32 = 0.25;

/// One detected cable trigger, with its value roles sampled at the edge.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ModTriggerEvent {
    pub(super) track_idx: usize,
    pub(super) velocity: Option<f32>,
    pub(super) duration: Option<f32>,
    pub(super) note: Option<f32>,
}

pub(super) struct ModTriggerRuntime {
    /// This block's roles, read once per block from `TrackParams`.
    roles: [ModInputRoles; MAX_TRACKS],
    /// Schmitt state per input: true while above the rise threshold.
    high: [[bool; EXT_MOD_INPUT_COUNT]; MAX_TRACKS],
    /// Triggers detected after the last render, fired at the next block.
    pending: ArrayVec<(u32, ModTriggerEvent), MOD_TRIGGER_CAPACITY>,
    latches: [Option<Arc<ScheduledEvent>>; MAX_TRACKS],
}

impl Default for ModTriggerRuntime {
    fn default() -> Self {
        Self {
            roles: [ModInputRoles::default(); MAX_TRACKS],
            high: [[false; EXT_MOD_INPUT_COUNT]; MAX_TRACKS],
            pending: ArrayVec::new(),
            latches: std::array::from_fn(|_| None),
        }
    }
}

impl ModTriggerRuntime {
    /// Forget every latch: transport start/stop and pattern changes, so a
    /// cable trigger never replays a step from a pattern no longer playing.
    pub(super) fn clear_latches(&mut self) {
        for latch in &mut self.latches {
            *latch = None;
        }
    }

    /// Track delete: indices shift, so per-track state is dropped wholesale.
    pub(super) fn reset(&mut self) {
        self.clear_latches();
        self.pending.clear();
        self.high = [[false; EXT_MOD_INPUT_COUNT]; MAX_TRACKS];
    }

    pub(super) fn clear_pending(&mut self) {
        self.pending.clear();
    }

    /// Record a scheduler trigger as the track's latch. Returns whether the
    /// trigger should still sound: steps on a `latch`-mode track only latch.
    pub(super) fn note_scheduled_trigger(&mut self, event: &CallbackScheduledEvent) -> bool {
        let Some(track) = scheduled_trigger_track(event).filter(|track| *track < MAX_TRACKS) else {
            return true;
        };
        let roles = self.roles[track];
        // Auditions (step previews while editing) are not playback.
        if !roles.has_trigger() || event.event.audition_generation != 0 {
            return true;
        }
        self.latches[track] = Some(Arc::clone(&event.event));
        !(roles.steps == ModSteps::Latch
            && matches!(event.event.kind, ScheduledEventKind::ResolvedTrigger { .. }))
    }
}

/// `duration` role: `2^(8v − 4)` steps, so 0.5 is one step and the range
/// spans 1/16 to 16 steps.
pub(super) fn mod_duration_steps(value: f32) -> f32 {
    (8.0 * value.clamp(0.0, 1.0) - 4.0).exp2()
}

/// `note` role: ±24 semitones around the root, 0.5 = root.
pub(super) fn mod_note_transpose(value: f32) -> f32 {
    (48.0 * value.clamp(0.0, 1.0) - 24.0).round()
}

/// Before the block's events are collected: refresh roles, arm the mod input
/// clips of tracks with a `trigger` input, and turn the triggers detected
/// after the previous render into this block's events.
pub(super) fn begin_mod_trigger_block(data: &mut AudioCallbackData, num_tracks: usize, playing: bool) {
    for track in 0..MAX_TRACKS {
        let roles = if track < num_tracks {
            data.state.pattern.track_params[track].mod_roles()
        } else {
            ModInputRoles::default()
        };
        let runtime = &mut data.mod_triggers;
        if !roles.has_trigger() {
            runtime.high[track] = [false; EXT_MOD_INPUT_COUNT];
            runtime.latches[track] = None;
        }
        runtime.roles[track] = roles;
        mod_trigger_lanes::begin_block(track, roles.has_trigger());
    }
    if !playing {
        data.mod_triggers.pending.clear();
        return;
    }
    let pending = std::mem::take(&mut data.mod_triggers.pending);
    for (frame_offset, event) in pending {
        if event.track_idx >= num_tracks {
            continue;
        }
        let seq = data.event_seq;
        data.event_seq = data.event_seq.wrapping_add(1);
        try_push_block_event(data, frame_offset, seq, BlockEventKind::ModTrigger(event));
    }
}

/// After the graph rendered: find this block's rising edges on every armed
/// `trigger` input and sample the value roles at each edge.
pub(super) fn detect_mod_triggers(data: &mut AudioCallbackData, num_tracks: usize, nframes: usize, playing: bool) {
    let runtime = &mut data.mod_triggers;
    for track in 0..num_tracks.min(MAX_TRACKS) {
        let roles = runtime.roles[track];
        if !roles.has_trigger() {
            continue;
        }
        let lane = |role: ModInputRole| {
            roles.input_for(role).and_then(|input| LaneBlock::read(track, input))
        };
        let (velocity, duration, note) =
            (lane(ModInputRole::Velocity), lane(ModInputRole::Duration), lane(ModInputRole::Note));
        for input in 0..EXT_MOD_INPUT_COUNT {
            if roles.roles[input] != ModInputRole::Trigger {
                continue;
            }
            let Some(trigger) = LaneBlock::read(track, input) else {
                continue;
            };
            let mut high = runtime.high[track][input];
            for frame in 0..nframes {
                let value = trigger.value(frame);
                if high {
                    high = value >= REARM_THRESHOLD;
                    continue;
                }
                if value < RISE_THRESHOLD {
                    continue;
                }
                high = true;
                if !playing {
                    continue;
                }
                let event = ModTriggerEvent {
                    track_idx: track,
                    velocity: velocity.map(|lane| lane.value(frame)),
                    duration: duration.map(|lane| lane.value(frame)),
                    note: note.map(|lane| lane.value(frame)),
                };
                if runtime.pending.try_push((frame as u32, event)).is_err() {
                    data.dropped_scheduled_events = data.dropped_scheduled_events.saturating_add(1);
                }
            }
            runtime.high[track][input] = high;
        }
    }
}

fn apply_mod_overrides(
    snapshot: &SequencerSnapshot,
    event: ModTriggerEvent,
    mut resolved: crate::accumulator::ResolvedStep,
    mut chord: ScheduledChordData,
) -> (crate::accumulator::ResolvedStep, ScheduledChordData) {
    if let Some(value) = event.velocity {
        resolved.velocity = value.clamp(0.0, 1.0);
    }
    if let Some(value) = event.duration {
        let steps = mod_duration_steps(value);
        resolved.duration = steps;
        for duration in &mut chord.durations[..chord.count.min(MAX_VOICES)] {
            if *duration > 0.0 {
                *duration = steps;
            }
        }
    }
    if let Some(value) = event.note {
        resolved.transpose = mod_note_transpose(value);
        return crate::scheduler::fit_mod_trigger_to_scale(snapshot, event.track_idx, resolved, chord);
    }
    (resolved, chord)
}

/// Fire one cable trigger: the track's latched scheduler trigger with the
/// value roles applied, or its defaults when nothing is latched.
pub(super) fn dispatch_mod_trigger(data: &mut AudioCallbackData, frame_offset: u32, event: ModTriggerEvent) {
    let track = event.track_idx;
    if track >= data.state.active_track_count() {
        return;
    }
    let snapshot = Arc::clone(&data.scheduler_snapshot);
    let latch = data.mod_triggers.latches[track].clone();
    match latch.as_deref().map(|latched| &latched.kind) {
        Some(ScheduledEventKind::ResolvedTrigger {
            step,
            samples_per_step,
            resolved,
            chord,
            effect_params,
            instrument_params,
            instrument_tensor_params,
            sampler_params,
            voice_policy,
            instrument_fingerprint,
            rack_macro_values,
            rack_slot_params,
            ..
        }) => {
            let (resolved, chord) = apply_mod_overrides(&snapshot, event, *resolved, *chord);
            dispatch_scheduled_step(
                data,
                frame_offset,
                track,
                *step,
                *samples_per_step,
                resolved,
                chord,
                effect_params,
                instrument_params,
                instrument_tensor_params,
                *sampler_params,
                *voice_policy,
                *instrument_fingerprint,
                *rack_macro_values,
                rack_slot_params,
            );
        }
        Some(ScheduledEventKind::NetworkTrigger {
            seed,
            samples_per_step,
            resolved,
            chord,
            effect_params,
            instrument_params,
            instrument_tensor_params,
            sampler_params,
            voice_policy,
            instrument_fingerprint,
            rack_macro_values,
            rack_slot_params,
            ..
        }) => {
            let (resolved, chord) = apply_mod_overrides(&snapshot, event, *resolved, *chord);
            dispatch_scheduled_network_step(
                data,
                frame_offset,
                track,
                seed.map(|(_, step)| step),
                *samples_per_step,
                resolved,
                chord,
                effect_params,
                instrument_params,
                instrument_tensor_params,
                *sampler_params,
                *voice_policy,
                *instrument_fingerprint,
                *rack_macro_values,
                rack_slot_params,
            );
        }
        _ => {
            let Some(defaults) = crate::scheduler::default_mod_trigger(&snapshot, track) else {
                return;
            };
            let params = &snapshot.tracks[track].params;
            let samples_per_step = params.timebase.samples_per_step(
                data.sample_rate,
                snapshot.transport.bpm.max(1) as f64,
                params.num_steps,
            ) as f32;
            let (resolved, chord) = apply_mod_overrides(&snapshot, event, defaults.resolved, defaults.chord);
            dispatch_scheduled_network_step(
                data,
                frame_offset,
                track,
                None,
                samples_per_step,
                resolved,
                chord,
                &[],
                &defaults.instrument_params,
                &ScheduledInstrumentTensorParams::new(),
                defaults.sampler_params,
                defaults.voice_policy,
                defaults.instrument_fingerprint,
                [None; crate::sequencer::RACK_MACRO_COUNT],
                &ScheduledRackSlotParams::new(),
            );
        }
    }
}

#[cfg(test)]
#[path = "mod_trigger_tests.rs"]
mod tests;
