use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::cmp::Reverse;
use std::collections::hash_map::DefaultHasher;
use std::collections::BinaryHeap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::audiograph::*;
use crate::delay;
use crate::effects::EffectSlotSnapshot;
use crate::gatepitch;
use crate::recorder::MasterRecorder;
use crate::sampler::{
    PARAM_ATTACK_SAMPLES, PARAM_END_POINT, PARAM_GATE_MODE, PARAM_GATE_SAMPLES, PARAM_PLAYHEAD,
    PARAM_RELEASE_SAMPLES, PARAM_SPEED, PARAM_START_POINT, PARAM_TRANSPOSE, PARAM_TRIGGER,
    PARAM_VELOCITY,
};
use crate::scheduled_event::{
    ScheduledEffectParam, ScheduledEvent, ScheduledEventKind, ScheduledEventQueue,
    ScheduledInstrumentParam, ScheduledInstrumentParamTarget, TimedEvent,
};
use crate::sequencer::{
    sync_beats, BusId, KeyboardTrigger, SequencerState, StepParam, SwingResolution, MAX_TRACKS,
};
use crate::ui::BusGateRuntimeState;
use crate::voice::{VoicePool, MAX_VOICES};

fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}

/// Per-track chop re-trigger state.
struct ChopTracker {
    /// How many chop triggers remain (excluding the initial trigger).
    remaining: u32,
    /// Samples countdown until next chop trigger.
    counter: f64,
    /// Samples between each chop trigger.
    interval: f64,
    /// The step whose params to re-use.
    step: usize,
    /// Gate length in samples for each chop subdivision.
    chop_gate: f32,
}

/// Pending gate-off events for custom instrument voices.
struct GateOffPending {
    lid: u64,
    countdown: f64,
}

/// Per-track gate-off queue for custom instruments.
struct GateOffTracker {
    pending: Vec<GateOffPending>,
}

impl GateOffTracker {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    /// Schedule a gate-off after `delay_samples` for the given voice LID.
    fn schedule(&mut self, lid: u64, delay_samples: f64) {
        // If there's already a pending gate-off for this LID, replace it
        for p in &mut self.pending {
            if p.lid == lid {
                p.countdown = delay_samples;
                return;
            }
        }
        self.pending.push(GateOffPending {
            lid,
            countdown: delay_samples,
        });
    }

    fn cancel(&mut self, lid: u64) {
        self.pending.retain(|p| p.lid != lid);
    }

    /// Advance all countdowns by nframes. Returns LIDs that expired.
    fn process(&mut self, nframes: usize) -> Vec<u64> {
        let mut expired = Vec::new();
        self.pending.retain_mut(|p| {
            p.countdown -= nframes as f64;
            if p.countdown <= 0.0 {
                expired.push(p.lid);
                false
            } else {
                true
            }
        });
        expired
    }
}

fn swing_delay_samples(
    sample_rate: f64,
    bpm: f64,
    swing_pct: f32,
    resolution: SwingResolution,
) -> f64 {
    let samples_per_quarter = sample_rate * 60.0 / bpm;
    let resolution_samples = resolution.step_beats() * samples_per_quarter;
    ((swing_pct as f64 / 100.0) - 0.5) * 2.0 * resolution_samples
}

fn cancel_gate_off_for_lid(gate_off_state: &mut [GateOffTracker], lid: u64) {
    for tracker in gate_off_state {
        tracker.cancel(lid);
    }
}

#[derive(Clone, Copy, Default)]
struct ActiveKeyboardNote {
    source_transpose: f32,
    logical_id: u64,
}

struct AudioCallbackData {
    lg: LiveGraphPtr,
    state: Arc<SequencerState>,
    num_channels: usize,
    chop_state: Vec<ChopTracker>,
    gate_off_state: Vec<GateOffTracker>,
    sample_rate: f64,
    last_bpm: u32,
    last_mod_reset_counter: u32,
    voice_pools: Vec<VoicePool>,
    custom_engine_pools: Vec<CustomEnginePool>,
    active_keyboard_notes: [[Option<ActiveKeyboardNote>; MAX_VOICES]; MAX_TRACKS],
    keyboard_rx: std::sync::mpsc::Receiver<KeyboardTrigger>,
    master_recorder: Arc<MasterRecorder>,
    accumulator_states: [crate::accumulator::AccumulatorRuntimeState; MAX_TRACKS],
    last_playing: bool,
    last_pattern: u32,
    last_num_tracks: usize,
    last_topology_epoch: u64,
    /// Per-track flag set on pattern switch/play-start; each track clears its own flag at step 0.
    pending_accum_reset: [bool; MAX_TRACKS],
    scheduled_events: Arc<ScheduledEventQueue<4096>>,
    rendered_samples: Arc<AtomicU64>,
    bus_gate_runtime: Arc<Mutex<Vec<BusGateRuntimeState>>>,
    bus_gate_playheads: Arc<Mutex<Vec<(BusId, usize)>>>,
    bus_gate_clocks: Vec<BusGateClock>,
    bus_gate_was_playing: bool,
    bus_gate_play_start_sample: u64,
    dropped_scheduled_events: u64,
    late_scheduled_events: u64,
    events_heap: BinaryHeap<Reverse<TimedEvent>>,
    event_seq: u64,
    trace_audio: bool,
    trace_callback_counter: u64,
    trace_render_probe_blocks: u32,
    trace_silent_active_callbacks: u32,
}

#[derive(Clone, Copy)]
struct BusGateClock {
    id: crate::sequencer::BusId,
    last_target: f32,
    last_step: Option<usize>,
}

struct CustomVoiceSlot {
    logical_id: u64,
    age: u64,
    active: bool,
    note: f32,
    assigned_track: Option<usize>,
    fingerprint: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CustomVoiceAllocation {
    voice_idx: usize,
    logical_id: u64,
    previous_track: Option<usize>,
    stole_active_voice: bool,
}

struct CustomEnginePool {
    voices: [CustomVoiceSlot; MAX_VOICES],
    num_voices: usize,
    enabled_voice_count: usize,
    age_counter: u64,
}

impl CustomEnginePool {
    fn new() -> Self {
        Self {
            voices: std::array::from_fn(|_| CustomVoiceSlot {
                logical_id: 0,
                age: 0,
                active: false,
                note: 0.0,
                assigned_track: None,
                fingerprint: 0,
            }),
            num_voices: 0,
            enabled_voice_count: 1,
            age_counter: 0,
        }
    }

    fn add_voice(&mut self, logical_id: u64) {
        if self.num_voices < MAX_VOICES {
            self.voices[self.num_voices] = CustomVoiceSlot {
                logical_id,
                age: 0,
                active: false,
                note: 0.0,
                assigned_track: None,
                fingerprint: 0,
            };
            self.num_voices += 1;
        }
    }

    fn reset(&mut self) {
        self.num_voices = 0;
        self.enabled_voice_count = 1;
        self.age_counter = 0;
        for voice in &mut self.voices {
            *voice = CustomVoiceSlot {
                logical_id: 0,
                age: 0,
                active: false,
                note: 0.0,
                assigned_track: None,
                fingerprint: 0,
            };
        }
    }

    fn allocate_voice(
        &mut self,
        track: usize,
        note: f32,
        polyphonic: bool,
    ) -> CustomVoiceAllocation {
        self.age_counter += 1;
        if !polyphonic {
            if let Some(idx) =
                (0..self.num_voices).find(|&i| self.voices[i].assigned_track == Some(track))
            {
                let slot = &mut self.voices[idx];
                let previous_track = slot.assigned_track;
                let stole_active_voice = slot.active;
                slot.age = self.age_counter;
                slot.active = true;
                slot.note = note;
                slot.assigned_track = Some(track);
                return CustomVoiceAllocation {
                    voice_idx: idx,
                    logical_id: slot.logical_id,
                    previous_track,
                    stole_active_voice,
                };
            }
        }

        let mut free_idx = None;
        let mut free_age = u64::MAX;
        let mut oldest_same_track = None;
        let mut oldest_same_track_age = u64::MAX;
        let mut oldest_idx = 0;
        let mut oldest_age = u64::MAX;

        for i in 0..self.num_voices {
            let voice = &self.voices[i];
            if !voice.active && voice.age < free_age {
                free_idx = Some(i);
                free_age = voice.age;
            }
            if voice.assigned_track == Some(track) && voice.age < oldest_same_track_age {
                oldest_same_track = Some(i);
                oldest_same_track_age = voice.age;
            }
            if voice.age < oldest_age {
                oldest_idx = i;
                oldest_age = voice.age;
            }
        }

        let idx = free_idx.or(oldest_same_track).unwrap_or(oldest_idx);
        let slot = &mut self.voices[idx];
        let previous_track = slot.assigned_track;
        let stole_active_voice = slot.active;
        slot.age = self.age_counter;
        slot.active = true;
        slot.note = note;
        slot.assigned_track = Some(track);
        CustomVoiceAllocation {
            voice_idx: idx,
            logical_id: slot.logical_id,
            previous_track,
            stole_active_voice,
        }
    }

    fn release_voice_by_logical_id(&mut self, logical_id: u64) {
        for i in 0..self.num_voices {
            if self.voices[i].logical_id == logical_id {
                self.voices[i].active = false;
                return;
            }
        }
    }

    fn note_voice_allocated(&mut self, engine_id: usize, voice_idx: usize) {
        let needed = (voice_idx + 1).min(MAX_VOICES).max(1);
        if needed > self.enabled_voice_count {
            self.enabled_voice_count = needed;
            crate::lisp_effect::set_dgen_engine_enabled_voices(engine_id, needed);
        }
    }

    fn sync_enabled_voice_count(&mut self, engine_id: usize) {
        self.enabled_voice_count = crate::lisp_effect::get_dgen_engine_enabled_voices(engine_id);
    }

    fn invalidate_sound_cache(&mut self) {
        for i in 0..self.num_voices {
            self.voices[i].fingerprint = 0;
        }
    }
}

fn sync_sampler_voice_pool(state: &SequencerState, track: usize, pool: &mut VoicePool) {
    let desired_count = state.runtime.voice_counts[track].load(Ordering::Acquire) as usize;
    let desired_count = desired_count.min(MAX_VOICES);

    let mut needs_reset = pool.num_voices != desired_count;
    if !needs_reset {
        for v in 0..desired_count {
            let desired_lid = state.runtime.voice_lids[track][v].load(Ordering::Acquire);
            if pool.voices[v].logical_id != desired_lid {
                needs_reset = true;
                break;
            }
        }
    }

    if needs_reset {
        pool.reset();
        for v in 0..desired_count {
            let lid = state.runtime.voice_lids[track][v].load(Ordering::Acquire);
            if lid != 0 {
                pool.add_voice(lid, lid as i32);
            }
        }
    }
}

fn sync_custom_engine_pool(state: &SequencerState, engine_id: usize, pool: &mut CustomEnginePool) {
    let desired_count =
        state.runtime.engine_voice_counts[engine_id].load(Ordering::Acquire) as usize;
    let desired_count = desired_count.min(MAX_VOICES);

    let mut needs_reset = pool.num_voices != desired_count;
    if !needs_reset {
        for v in 0..desired_count {
            let desired_lid = state.runtime.engine_voice_lids[engine_id][v].load(Ordering::Acquire);
            if pool.voices[v].logical_id != desired_lid {
                needs_reset = true;
                break;
            }
        }
    }

    if needs_reset {
        pool.reset();
        crate::lisp_effect::reset_dgen_engine_enabled_voices(engine_id);
        for v in 0..desired_count {
            let lid = state.runtime.engine_voice_lids[engine_id][v].load(Ordering::Acquire);
            if lid != 0 {
                pool.add_voice(lid);
            }
        }
    } else {
        pool.sync_enabled_voice_count(engine_id);
    }
}

fn reset_audio_runtime_for_track_topology(data: &mut AudioCallbackData, num_tracks: usize) {
    // Topology edits can invalidate the per-track gate-off bookkeeping for
    // already-ringing custom voices. Explicitly send gate-off to every live
    // custom engine voice before resetting callback-local state so notes do
    // not hang after tracks are compacted.
    for engine_id in 0..MAX_TRACKS {
        let voice_count =
            data.state.runtime.engine_voice_counts[engine_id].load(Ordering::Acquire) as usize;
        for voice_idx in 0..voice_count.min(MAX_VOICES) {
            let lid =
                data.state.runtime.engine_voice_lids[engine_id][voice_idx].load(Ordering::Acquire);
            if lid != 0 {
                unsafe {
                    send_custom_note_off(data.lg.0, lid);
                }
            }
        }
    }

    for pool in &mut data.voice_pools {
        pool.reset();
    }
    for pool in &mut data.custom_engine_pools {
        pool.reset();
    }
    for tracker in &mut data.gate_off_state {
        tracker.pending.clear();
    }
    for chop in &mut data.chop_state {
        chop.remaining = 0;
        chop.counter = 0.0;
        chop.interval = 0.0;
        chop.step = 0;
        chop.chop_gate = 0.0;
    }
    data.active_keyboard_notes = [[None; MAX_VOICES]; MAX_TRACKS];
    data.pending_accum_reset = [true; MAX_TRACKS];
    data.scheduled_events.clear();
    data.events_heap.clear();
    data.event_seq = 0;
    data.last_num_tracks = num_tracks;
    data.last_topology_epoch = data.state.transport.topology_epoch.load(Ordering::Relaxed);
    data.last_playing = false;
    data.last_pattern = u32::MAX;

    for t in 0..num_tracks {
        sync_sampler_voice_pool(&data.state, t, &mut data.voice_pools[t]);
        if let Some(engine_id) = track_engine_id(&data.state, t) {
            sync_custom_engine_pool(
                &data.state,
                engine_id,
                &mut data.custom_engine_pools[engine_id],
            );
        }
    }
}

fn publish_active_voice_counts(data: &AudioCallbackData, num_tracks: usize) {
    for track in 0..MAX_TRACKS {
        let active = if track < num_tracks {
            let is_custom =
                data.state.runtime.instrument_type_flags[track].load(Ordering::Relaxed) == 1;
            if is_custom {
                track_engine_id(&data.state, track)
                    .map(|engine_id| {
                        let pool = &data.custom_engine_pools[engine_id];
                        pool.voices[..pool.num_voices]
                            .iter()
                            .filter(|voice| voice.active && voice.assigned_track == Some(track))
                            .count()
                    })
                    .unwrap_or(0)
            } else {
                let pool = &data.voice_pools[track];
                pool.voices[..pool.num_voices]
                    .iter()
                    .filter(|voice| voice.active)
                    .count()
            }
        } else {
            0
        };
        data.state.transport.active_voice_counts[track].store(active as u32, Ordering::Relaxed);
    }
}

/// Send a trigger to the sampler with the given per-step params, gate length, and explicit transpose.
unsafe fn send_trigger(
    lg: *mut LiveGraph,
    lid: u64,
    velocity: f32,
    speed: f32,
    gate_samples: f32,
    attack_samples: f32,
    release_samples: f32,
    gate_mode: f32,
    transpose: f32,
    start_point: f32,
    end_point: f32,
) {
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_VELOCITY,
            logical_id: lid,
            fvalue: velocity,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_SPEED,
            logical_id: lid,
            fvalue: speed,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_GATE_SAMPLES,
            logical_id: lid,
            fvalue: gate_samples,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_TRANSPOSE,
            logical_id: lid,
            fvalue: transpose,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_ATTACK_SAMPLES,
            logical_id: lid,
            fvalue: attack_samples,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_RELEASE_SAMPLES,
            logical_id: lid,
            fvalue: release_samples,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_GATE_MODE,
            logical_id: lid,
            fvalue: gate_mode,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_START_POINT,
            logical_id: lid,
            fvalue: start_point,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_END_POINT,
            logical_id: lid,
            fvalue: end_point,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_PLAYHEAD,
            logical_id: lid,
            fvalue: 0.0,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_TRIGGER,
            logical_id: lid,
            fvalue: 1.0,
        },
    );
}

/// Send a keyboard trigger directly to a voice (no step data lookup).
unsafe fn send_keyboard_trigger(
    lg: *mut LiveGraph,
    lid: u64,
    transpose: f32,
    velocity: f32,
    attack_samples: f32,
    release_samples: f32,
    gate_mode: f32,
    start_point: f32,
    end_point: f32,
) {
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_VELOCITY,
            logical_id: lid,
            fvalue: velocity,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_SPEED,
            logical_id: lid,
            fvalue: 1.0,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_GATE_SAMPLES,
            logical_id: lid,
            fvalue: f32::MAX,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_TRANSPOSE,
            logical_id: lid,
            fvalue: transpose,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_ATTACK_SAMPLES,
            logical_id: lid,
            fvalue: attack_samples,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_RELEASE_SAMPLES,
            logical_id: lid,
            fvalue: release_samples,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_GATE_MODE,
            logical_id: lid,
            fvalue: gate_mode,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_START_POINT,
            logical_id: lid,
            fvalue: start_point,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_END_POINT,
            logical_id: lid,
            fvalue: end_point,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_PLAYHEAD,
            logical_id: lid,
            fvalue: 0.0,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_TRIGGER,
            logical_id: lid,
            fvalue: 1.0,
        },
    );
}

/// Send a gate-on trigger to a GatePitch node with pitch in Hz and normalized velocity.
unsafe fn send_custom_trigger(
    lg: *mut LiveGraph,
    gatepitch_lid: u64,
    pitch_hz: f32,
    velocity: f32,
) {
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: gatepitch::PARAM_TRIGGER,
            logical_id: gatepitch_lid,
            fvalue: 1.0,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: gatepitch::PARAM_PITCH,
            logical_id: gatepitch_lid,
            fvalue: pitch_hz,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: gatepitch::PARAM_VELOCITY,
            logical_id: gatepitch_lid,
            fvalue: velocity,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: gatepitch::PARAM_GATE,
            logical_id: gatepitch_lid,
            fvalue: 1.0,
        },
    );
}

/// Send a gate-off to a GatePitch node.
unsafe fn send_custom_note_off(lg: *mut LiveGraph, gatepitch_lid: u64) {
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: gatepitch::PARAM_GATE,
            logical_id: gatepitch_lid,
            fvalue: 0.0,
        },
    );
}

fn custom_pitch_hz(transpose: f32, base_note_offset: f32) -> f32 {
    440.0 * 2f32.powf((transpose + base_note_offset) / 12.0)
}

fn resolve_live_keyboard_transpose(
    state: &SequencerState,
    accumulator_state: crate::accumulator::AccumulatorRuntimeState,
    track_idx: usize,
    raw_transpose: f32,
) -> f32 {
    let tp = &state.pattern.track_params[track_idx];
    let accum_idx = tp.get_accumulator_idx();
    let with_accumulator = match crate::accumulator::ACCUMULATOR_REGISTRY.get(accum_idx) {
        Some(def) if def.name == "TransposeRamp" => raw_transpose + accumulator_state.value,
        _ => raw_transpose,
    };
    let fts = tp.get_fts_scale();
    if fts > 0 {
        crate::scale::quantize_transpose(with_accumulator, fts)
    } else {
        with_accumulator
    }
}

fn clear_active_keyboard_note_by_lid(
    active_notes: &mut [[Option<ActiveKeyboardNote>; MAX_VOICES]; MAX_TRACKS],
    logical_id: u64,
) {
    for track_notes in active_notes.iter_mut() {
        for slot in track_notes.iter_mut() {
            if slot.is_some_and(|note| note.logical_id == logical_id) {
                *slot = None;
            }
        }
    }
}

fn store_active_keyboard_note(
    active_notes: &mut [[Option<ActiveKeyboardNote>; MAX_VOICES]; MAX_TRACKS],
    track_idx: usize,
    source_transpose: f32,
    logical_id: u64,
) {
    clear_active_keyboard_note_by_lid(active_notes, logical_id);
    let track_notes = &mut active_notes[track_idx];
    if let Some(slot) = track_notes.iter_mut().find(|slot| {
        slot.is_some_and(|note| (note.source_transpose - source_transpose).abs() < 0.01)
    }) {
        *slot = Some(ActiveKeyboardNote {
            source_transpose,
            logical_id,
        });
        return;
    }
    if let Some(slot) = track_notes.iter_mut().find(|slot| slot.is_none()) {
        *slot = Some(ActiveKeyboardNote {
            source_transpose,
            logical_id,
        });
        return;
    }
    track_notes[0] = Some(ActiveKeyboardNote {
        source_transpose,
        logical_id,
    });
}

fn take_active_keyboard_note(
    active_notes: &mut [[Option<ActiveKeyboardNote>; MAX_VOICES]; MAX_TRACKS],
    track_idx: usize,
    source_transpose: f32,
) -> Option<ActiveKeyboardNote> {
    let track_notes = &mut active_notes[track_idx];
    for slot in track_notes.iter_mut() {
        if slot.is_some_and(|note| (note.source_transpose - source_transpose).abs() < 0.01) {
            return slot.take();
        }
    }
    None
}

fn resolved_chord_transpose(
    chord_transpose: f32,
    step_transpose: f32,
    resolved_transpose: f32,
) -> f32 {
    chord_transpose + (resolved_transpose - step_transpose)
}

fn track_engine_id(state: &SequencerState, track_idx: usize) -> Option<usize> {
    let engine_id = state.runtime.track_engine_ids[track_idx].load(Ordering::Relaxed);
    if engine_id == u32::MAX {
        None
    } else {
        Some(engine_id as usize)
    }
}

fn instrument_sound_fingerprint(
    state: &SequencerState,
    track_idx: usize,
    engine_id: usize,
    step: Option<usize>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    engine_id.hash(&mut hasher);
    state.pattern.instrument_base_note_offsets[track_idx]
        .load(Ordering::Relaxed)
        .hash(&mut hasher);

    let slot = &state.pattern.instrument_slots[track_idx];
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params {
        let value_bits = if let Some(step_idx) = step {
            slot.plocks
                .get(step_idx, param_idx)
                .unwrap_or_else(|| slot.defaults.get(param_idx))
                .to_bits()
        } else {
            slot.defaults.get(param_idx).to_bits()
        };
        value_bits.hash(&mut hasher);
    }

    hasher.finish()
}

unsafe fn dispatch_effect_chain_for_track(
    lg: *mut LiveGraph,
    effect_params: &[ScheduledEffectParam],
) {
    for param in effect_params {
        params_push_wrapper(
            lg,
            ParamMsg {
                idx: param.idx,
                logical_id: param.logical_id,
                fvalue: param.value,
            },
        );
    }
}

unsafe fn route_custom_voice_to_track(
    lg: *mut LiveGraph,
    state: &SequencerState,
    engine_id: usize,
    voice_idx: usize,
    track_idx: usize,
) {
    let num_tracks = state.active_track_count();
    for t in 0..num_tracks {
        let lid_l =
            state.runtime.engine_route_lids[engine_id][voice_idx][t].load(Ordering::Relaxed);
        let lid_r =
            state.runtime.engine_route_lids_r[engine_id][voice_idx][t].load(Ordering::Relaxed);
        if lid_l == 0 && lid_r == 0 {
            continue;
        }
        let value = if t == track_idx { 1.0 } else { 0.0 };
        if lid_l != 0 {
            params_push_wrapper(
                lg,
                ParamMsg {
                    idx: 0,
                    logical_id: lid_l,
                    fvalue: value,
                },
            );
        }
        if lid_r != 0 {
            params_push_wrapper(
                lg,
                ParamMsg {
                    idx: 0,
                    logical_id: lid_r,
                    fvalue: value,
                },
            );
        }
    }
}

/// Dispatch instrument param values (with p-lock support) to a selected synth node.
unsafe fn dispatch_instrument_params_to_voice(
    lg: *mut LiveGraph,
    synth_id: u64,
    modulator_id: u64,
    instrument_params: &[ScheduledInstrumentParam],
) {
    for param in instrument_params {
        let (logical_id, idx) = match param.target {
            ScheduledInstrumentParamTarget::Synth => (synth_id, param.idx),
            ScheduledInstrumentParamTarget::Modulator => (modulator_id, param.idx),
        };
        params_push_wrapper(
            lg,
            ParamMsg {
                idx,
                logical_id,
                fvalue: param.value,
            },
        );
    }
}

unsafe fn dispatch_instrument_defaults_to_voice(
    lg: *mut LiveGraph,
    state: &SequencerState,
    track_idx: usize,
    synth_id: u64,
    modulator_id: u64,
) {
    let slot = &state.pattern.instrument_slots[track_idx];
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params {
        let idx = slot.resolve_node_idx(param_idx);
        let is_mod_param = idx as u32 >= crate::voice_modulator::MOD_PARAM_BASE;
        let logical_id = if is_mod_param { modulator_id } else { synth_id };
        let resolved_idx = if is_mod_param {
            idx - crate::voice_modulator::MOD_PARAM_BASE as u64
        } else {
            idx
        };
        params_push_wrapper(
            lg,
            ParamMsg {
                idx: resolved_idx,
                logical_id,
                fvalue: slot.defaults.get(param_idx),
            },
        );
    }
}

fn dispatch_scheduled_step(
    data: &mut AudioCallbackData,
    track_idx: usize,
    step: usize,
    samples_per_step: f32,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: Vec<ScheduledInstrumentParam>,
    instrument_fingerprint: u64,
) {
    unsafe {
        dispatch_effect_chain_for_track(data.lg.0, &effect_params);
    }
    fire_resolved(
        data,
        track_idx,
        step,
        samples_per_step as f64,
        resolved,
        chord,
        instrument_params,
        instrument_fingerprint,
    );
}

fn dispatch_scheduled_event(data: &mut AudioCallbackData, event: ScheduledEvent) {
    match event.kind {
        ScheduledEventKind::ResolvedTrigger {
            track,
            step,
            samples_per_step,
            resolved,
            chord,
            effect_params,
            instrument_params,
            instrument_fingerprint,
        } => {
            dispatch_scheduled_step(
                data,
                track,
                step,
                samples_per_step,
                resolved,
                chord,
                effect_params,
                instrument_params,
                instrument_fingerprint,
            );
        }
    }
}

fn render_chunk(data: &mut AudioCallbackData, output: &mut [f32]) {
    if output.is_empty() {
        return;
    }
    let nframes = output.len() / data.num_channels;
    if nframes == 0 {
        return;
    }
    unsafe {
        process_next_block(data.lg.0, output.as_mut_ptr(), nframes as i32);
    }
}

fn bus_gate_state_at(sequence: &crate::ui::BusGateSequence, total_beats: f64) -> (f32, usize) {
    const EPS: f64 = 1e-9;
    let ns = sequence.num_steps.clamp(1, crate::sequencer::MAX_STEPS);
    let mut starts = [0.0f64; crate::sequencer::MAX_STEPS];
    let mut durations = [0.0f64; crate::sequencer::MAX_STEPS];
    let mut accum = 0.0f64;
    for step in 0..ns {
        let timebase = sequence.timebase_plocks[step].unwrap_or(sequence.timebase);
        let duration = timebase.step_beats(ns).max(EPS);
        let sync = sync_beats(sequence.syncs[step]);
        if sync > EPS {
            accum = ceil_to_grid(accum, sync);
        }
        starts[step] = accum;
        durations[step] = duration;
        accum += duration;
    }
    let sync0 = sync_beats(sequence.syncs[0]);
    if sync0 > EPS {
        accum = ceil_to_grid(accum, sync0).max(EPS);
    }
    if accum <= EPS {
        return (1.0, 0);
    }

    let pos = total_beats.rem_euclid(accum);
    let mut active_step = None;
    for idx in 0..ns {
        if pos + EPS >= starts[idx] && pos < starts[idx] + durations[idx] {
            active_step = Some(idx);
            break;
        }
    }
    let step = active_step.unwrap_or_else(|| {
        let idx = starts[..ns].partition_point(|&start| start <= pos);
        idx.saturating_sub(1).min(ns - 1)
    });
    if active_step.is_none() {
        return (0.0, step);
    }

    if !sequence.steps[step] {
        return (0.0, step);
    }
    let local = pos - starts[step];
    let gate_duration = durations[step] * sequence.durations[step].clamp(0.0, 1.0) as f64;
    if local <= gate_duration + EPS {
        (sequence.velocities[step].clamp(0.0, 1.0), step)
    } else {
        (0.0, step)
    }
}

fn bus_gate_target_at(sequence: &crate::ui::BusGateSequence, total_beats: f64) -> f32 {
    bus_gate_state_at(sequence, total_beats).0
}

fn ceil_to_grid(value: f64, grid: f64) -> f64 {
    let rem = value % grid;
    if rem > 1e-9 {
        value + (grid - rem)
    } else {
        value
    }
}

unsafe fn dispatch_bus_effect_params_at_step(
    lg: *mut LiveGraph,
    effect_slots: &[EffectSlotSnapshot],
    step: usize,
) {
    for slot in effect_slots {
        if slot.node_id == 0 {
            continue;
        }
        let num_params = slot.num_params as usize;
        for param_idx in 0..num_params {
            let idx = slot
                .param_node_indices
                .get(param_idx)
                .copied()
                .unwrap_or(param_idx as u32);
            if idx == u32::MAX || param_idx >= slot.defaults.len() {
                continue;
            }
            let value = slot
                .plocks
                .get(step)
                .and_then(|step_plocks| step_plocks.get(param_idx))
                .copied()
                .flatten()
                .unwrap_or(slot.defaults[param_idx]);
            if !value.is_finite() {
                continue;
            }
            crate::audiograph::params_push_wrapper(
                lg,
                crate::audiograph::ParamMsg {
                    idx: idx as u64,
                    logical_id: slot.node_id as u64,
                    fvalue: value,
                },
            );
        }
    }
}

fn sync_bus_gate_params(data: &mut AudioCallbackData, block_start_sample: u64) {
    let playing = data.state.transport.playing.load(Ordering::Relaxed);
    let bpm = data.state.transport.bpm.load(Ordering::Relaxed).max(1) as f64;
    if playing && !data.bus_gate_was_playing {
        data.bus_gate_play_start_sample = block_start_sample;
        for clock in &mut data.bus_gate_clocks {
            clock.last_target = f32::NAN;
            clock.last_step = None;
        }
    }
    if !playing && data.bus_gate_was_playing {
        for clock in &mut data.bus_gate_clocks {
            clock.last_target = f32::NAN;
            clock.last_step = None;
        }
    }
    data.bus_gate_was_playing = playing;

    let elapsed_samples = block_start_sample.saturating_sub(data.bus_gate_play_start_sample);
    let total_beats = elapsed_samples as f64 * bpm / (data.sample_rate * 60.0);
    let Ok(gates) = data.bus_gate_runtime.try_lock() else {
        return;
    };
    let gates = gates.clone();
    let mut playheads = Vec::with_capacity(gates.len());

    data.bus_gate_clocks
        .retain(|clock| gates.iter().any(|gate| gate.id == clock.id));

    for gate in gates {
        if gate.gate_id <= 0 {
            continue;
        }
        let (target, step) = if playing {
            bus_gate_state_at(&gate.sequence, total_beats)
        } else {
            (1.0, 0)
        };
        playheads.push((gate.id, step));
        let clock_idx = data
            .bus_gate_clocks
            .iter()
            .position(|clock| clock.id == gate.id)
            .unwrap_or_else(|| {
                data.bus_gate_clocks.push(BusGateClock {
                    id: gate.id,
                    last_target: f32::NAN,
                    last_step: None,
                });
                data.bus_gate_clocks.len() - 1
            });
        let clock = &mut data.bus_gate_clocks[clock_idx];
        if clock.last_step != Some(step) {
            clock.last_step = Some(step);
            unsafe {
                dispatch_bus_effect_params_at_step(data.lg.0, &gate.effect_slots, step);
            }
        }
        if (clock.last_target - target).abs() <= 0.0001 {
            continue;
        }
        clock.last_target = target;
        unsafe {
            crate::audiograph::params_push_wrapper(
                data.lg.0,
                crate::audiograph::ParamMsg {
                    idx: crate::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                    logical_id: gate.gate_id as u64,
                    fvalue: target,
                },
            );
        }
    }
    if let Ok(mut shared_playheads) = data.bus_gate_playheads.try_lock() {
        *shared_playheads = playheads;
    }
}

fn interleaved_peak(output: &[f32], num_channels: usize) -> (f32, f32) {
    let mut peak_l = 0.0f32;
    let mut peak_r = 0.0f32;
    if num_channels == 0 {
        return (peak_l, peak_r);
    }
    let nframes = output.len() / num_channels;
    for i in 0..nframes {
        let l = output[i * num_channels].abs();
        if l > peak_l {
            peak_l = l;
        }
        if num_channels > 1 {
            let r = output[i * num_channels + 1].abs();
            if r > peak_r {
                peak_r = r;
            }
        }
    }
    (peak_l, peak_r)
}

fn zero_output_frames(output: &mut [f32], start_frame: usize, num_channels: usize) {
    let start = start_frame.saturating_mul(num_channels);
    if start < output.len() {
        output[start..].fill(0.0);
    }
}

/// Fire a resolved step trigger for a track (handles gate, chop setup, envelope params).
/// Uses voice pool allocation for polyphonic playback.
fn fire_resolved(
    data: &mut AudioCallbackData,
    track_idx: usize,
    step: usize,
    samples_per_step: f64,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    instrument_params: Vec<ScheduledInstrumentParam>,
    instrument_fingerprint: u64,
) {
    let tp = &data.state.pattern.track_params[track_idx];
    let is_custom =
        data.state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed) == 1;
    let sampler_lid = data.state.runtime.sampler_lids[track_idx].load(Ordering::Acquire);
    if !is_custom && sampler_lid == 0 {
        return;
    }

    let chop = resolved.chop.round() as u32;
    let chop = chop.max(1);

    let total_gate = (resolved.duration as f64 * samples_per_step) as f32;
    let chop_gate = total_gate / chop as f32;

    // Envelope and sampler params from instrument slot (params 0-3)
    let inst_slot = &data.state.pattern.instrument_slots[track_idx];
    let attack_ms = inst_slot
        .plocks
        .get(step, 0)
        .unwrap_or_else(|| inst_slot.defaults.get(0));
    let release_ms = inst_slot
        .plocks
        .get(step, 1)
        .unwrap_or_else(|| inst_slot.defaults.get(1));
    let attack_samples = attack_ms * data.sample_rate as f32 / 1000.0;
    let release_samples = release_ms * data.sample_rate as f32 / 1000.0;
    let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
    let start_point = inst_slot
        .plocks
        .get(step, 2)
        .unwrap_or_else(|| inst_slot.defaults.get(2));
    let end_point = inst_slot
        .plocks
        .get(step, 3)
        .unwrap_or_else(|| inst_slot.defaults.get(3));
    let velocity = resolved.velocity;
    let base_note_offset = f32::from_bits(
        data.state.pattern.instrument_base_note_offsets[track_idx].load(Ordering::Relaxed),
    );
    let step_transpose = chord.step_transpose;
    let pan_lid = data.state.runtime.pan_lids[track_idx].load(Ordering::Acquire);
    if pan_lid != 0 {
        let effective_pan = (tp.get_pan() + resolved.pan).clamp(-1.0, 1.0);
        unsafe {
            crate::audiograph::params_push_wrapper(
                data.lg.0,
                crate::audiograph::ParamMsg {
                    idx: crate::stereo_panner::STEREO_PANNER_PARAM_PAN,
                    logical_id: pan_lid,
                    fvalue: effective_pan,
                },
            );
        }
    }

    // Fit to Scale: quantize the final transpose to the nearest scale degree.
    // Keep the pre-FTS value so chord notes can be individually quantized.
    let fts = tp.get_fts_scale();
    let pre_fts_transpose = resolved.transpose;
    let resolved = if fts > 0 {
        crate::accumulator::ResolvedStep {
            transpose: crate::scale::quantize_transpose(resolved.transpose, fts),
            ..resolved
        }
    } else {
        resolved
    };

    // Sync polyphonic setting from track params
    let track_polyphonic = tp.is_polyphonic();
    data.voice_pools[track_idx].polyphonic = track_polyphonic;
    let engine_id = if is_custom {
        track_engine_id(&data.state, track_idx)
    } else {
        None
    };

    // Check chord data: if chord has notes, trigger each note on its own voice
    let chord_count = chord.count;
    if chord_count > 0 {
        for n in 0..chord_count {
            let note_duration = chord.durations[n].max(0.0);
            let note_total_gate = if note_duration > 0.0 {
                (note_duration as f64 * samples_per_step) as f32
            } else {
                total_gate
            };
            let note_chop_gate = note_total_gate / chop as f32;
            // Apply accumulator offset using pre-FTS transpose, then FTS-quantize each note.
            let raw = resolved_chord_transpose(chord.notes[n], step_transpose, pre_fts_transpose);
            let transpose = if fts > 0 {
                crate::scale::quantize_transpose(raw, fts)
            } else {
                raw
            };
            if is_custom {
                let Some(engine_id) = engine_id else {
                    continue;
                };
                let allocation = data.custom_engine_pools[engine_id].allocate_voice(
                    track_idx,
                    transpose,
                    track_polyphonic,
                );
                let voice_idx = allocation.voice_idx;
                data.custom_engine_pools[engine_id].note_voice_allocated(engine_id, voice_idx);
                let lid = allocation.logical_id;
                let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                    .load(Ordering::Relaxed);
                let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id]
                    [voice_idx]
                    .load(Ordering::Relaxed);
                if lid == 0 || synth_id == 0 || modulator_id == 0 {
                    continue;
                }
                if data.trace_audio {
                    let enabled = data.custom_engine_pools[engine_id].enabled_voice_count;
                    eprintln!(
                        "audio-trace: scheduled custom note-on track={track_idx} engine={engine_id} voice={voice_idx} lid={lid} synth={synth_id} mod={modulator_id} chord_note={n} enabled_voices={enabled} poly={track_polyphonic} stolen={}",
                        allocation.stole_active_voice,
                    );
                    data.trace_render_probe_blocks = data.trace_render_probe_blocks.max(12);
                }
                let pitch_hz = custom_pitch_hz(transpose, base_note_offset);
                cancel_gate_off_for_lid(&mut data.gate_off_state, lid);
                if allocation.stole_active_voice || !track_polyphonic {
                    unsafe {
                        send_custom_note_off(data.lg.0, lid);
                        route_custom_voice_to_track(
                            data.lg.0,
                            &data.state,
                            engine_id,
                            voice_idx,
                            track_idx,
                        );
                        if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                            != instrument_fingerprint
                        {
                            dispatch_instrument_params_to_voice(
                                data.lg.0,
                                synth_id as u64,
                                modulator_id as u64,
                                &instrument_params,
                            );
                        }
                    }
                } else {
                    unsafe {
                        route_custom_voice_to_track(
                            data.lg.0,
                            &data.state,
                            engine_id,
                            voice_idx,
                            track_idx,
                        );
                        if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                            != instrument_fingerprint
                        {
                            dispatch_instrument_params_to_voice(
                                data.lg.0,
                                synth_id as u64,
                                modulator_id as u64,
                                &instrument_params,
                            );
                        }
                    }
                }
                data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint =
                    instrument_fingerprint;
                unsafe {
                    send_custom_trigger(data.lg.0, lid, pitch_hz, velocity);
                }
                if gate_mode > 0.5 {
                    data.gate_off_state[track_idx].schedule(lid, note_total_gate as f64);
                }
            } else {
                let voice = data.voice_pools[track_idx].allocate_voice(transpose);
                let voice_lid = voice.logical_id;
                let lid = if voice_lid != 0 {
                    voice_lid
                } else {
                    sampler_lid
                };
                unsafe {
                    send_trigger(
                        data.lg.0,
                        lid,
                        velocity,
                        resolved.speed,
                        note_chop_gate,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        transpose,
                        start_point,
                        end_point,
                    );
                }
            }
        }
    } else {
        // Single-note mode: use resolved transpose
        let transpose = resolved.transpose;
        if is_custom {
            let Some(engine_id) = engine_id else {
                return;
            };
            let allocation = data.custom_engine_pools[engine_id].allocate_voice(
                track_idx,
                transpose,
                track_polyphonic,
            );
            let voice_idx = allocation.voice_idx;
            data.custom_engine_pools[engine_id].note_voice_allocated(engine_id, voice_idx);
            let lid = allocation.logical_id;
            let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                .load(Ordering::Relaxed);
            let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id][voice_idx]
                .load(Ordering::Relaxed);
            if lid == 0 || synth_id == 0 || modulator_id == 0 {
                return;
            }
            if data.trace_audio {
                let enabled = data.custom_engine_pools[engine_id].enabled_voice_count;
                eprintln!(
                    "audio-trace: scheduled custom note-on track={track_idx} engine={engine_id} voice={voice_idx} lid={lid} synth={synth_id} mod={modulator_id} enabled_voices={enabled} poly={track_polyphonic} stolen={}",
                    allocation.stole_active_voice,
                );
                data.trace_render_probe_blocks = data.trace_render_probe_blocks.max(12);
            }
            let pitch_hz = custom_pitch_hz(transpose, base_note_offset);
            cancel_gate_off_for_lid(&mut data.gate_off_state, lid);
            if allocation.stole_active_voice || !track_polyphonic {
                unsafe {
                    send_custom_note_off(data.lg.0, lid);
                    route_custom_voice_to_track(
                        data.lg.0,
                        &data.state,
                        engine_id,
                        voice_idx,
                        track_idx,
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != instrument_fingerprint
                    {
                        dispatch_instrument_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            modulator_id as u64,
                            &instrument_params,
                        );
                    }
                }
            } else {
                unsafe {
                    route_custom_voice_to_track(
                        data.lg.0,
                        &data.state,
                        engine_id,
                        voice_idx,
                        track_idx,
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != instrument_fingerprint
                    {
                        dispatch_instrument_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            modulator_id as u64,
                            &instrument_params,
                        );
                    }
                }
            }
            data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint =
                instrument_fingerprint;
            unsafe {
                send_custom_trigger(data.lg.0, lid, pitch_hz, velocity);
            }
            if gate_mode > 0.5 {
                data.gate_off_state[track_idx].schedule(lid, total_gate as f64);
            }
        } else {
            let voice = data.voice_pools[track_idx].allocate_voice(transpose);
            let voice_lid = voice.logical_id;
            let lid = if voice_lid != 0 {
                voice_lid
            } else {
                sampler_lid
            };
            unsafe {
                send_trigger(
                    data.lg.0,
                    lid,
                    velocity,
                    resolved.speed,
                    chop_gate,
                    attack_samples,
                    release_samples,
                    gate_mode,
                    transpose,
                    start_point,
                    end_point,
                );
            }
        }
    }

    // Update send gain (reverb send amount from track-level param)
    let send_lid = data.state.runtime.send_lids[track_idx].load(Ordering::Acquire);
    if send_lid != 0 {
        unsafe {
            params_push_wrapper(
                data.lg.0,
                ParamMsg {
                    idx: 0,
                    logical_id: send_lid,
                    fvalue: tp.get_send(),
                },
            );
        }
    }

    data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);

    // Setup chop re-triggers (sampler only — custom instruments handle gate duration internally)
    if !is_custom && chop > 1 {
        data.chop_state[track_idx] = ChopTracker {
            remaining: chop - 1,
            counter: samples_per_step / chop as f64,
            interval: samples_per_step / chop as f64,
            step,
            chop_gate,
        };
    } else {
        data.chop_state[track_idx].remaining = 0;
    }
}

fn audio_callback(data: &mut AudioCallbackData, output: &mut [f32]) {
    let callback_start = Instant::now();
    let nframes = output.len() / data.num_channels;
    data.trace_callback_counter = data.trace_callback_counter.wrapping_add(1);
    let num_tracks = data.state.active_track_count();
    let topology_epoch = data.state.transport.topology_epoch.load(Ordering::Relaxed);
    if num_tracks != data.last_num_tracks || topology_epoch != data.last_topology_epoch {
        if data.trace_audio {
            eprintln!(
                "audio-trace: topology reset tracks {}->{} epoch {}->{} rendered_samples={}",
                data.last_num_tracks,
                num_tracks,
                data.last_topology_epoch,
                topology_epoch,
                data.rendered_samples.load(Ordering::Acquire),
            );
            data.trace_render_probe_blocks = data.trace_render_probe_blocks.max(12);
        }
        reset_audio_runtime_for_track_topology(data, num_tracks);
    }
    if data.state.topology_edit_in_flight() {
        data.scheduled_events.clear();
        data.events_heap.clear();
        data.event_seq = 0;
    }
    let block_start_sample = data.rendered_samples.load(Ordering::Acquire);
    let block_end_sample = block_start_sample + nframes as u64;
    sync_bus_gate_params(data, block_start_sample);

    // Sync voice pools against current runtime bindings. Project loads can
    // replace tracks in-place, so growth-only sync leaves dead logical IDs.
    for t in 0..num_tracks {
        sync_sampler_voice_pool(&data.state, t, &mut data.voice_pools[t]);

        if let Some(engine_id) = track_engine_id(&data.state, t) {
            sync_custom_engine_pool(
                &data.state,
                engine_id,
                &mut data.custom_engine_pools[engine_id],
            );
        }
    }

    // Process keyboard triggers
    while let Ok(kt) = data.keyboard_rx.try_recv() {
        if kt.track >= num_tracks {
            continue;
        }
        let is_custom =
            data.state.runtime.instrument_type_flags[kt.track].load(Ordering::Relaxed) == 1;
        let track_polyphonic = data.state.pattern.track_params[kt.track].is_polyphonic();
        data.voice_pools[kt.track].polyphonic = track_polyphonic;
        let base_note_offset = f32::from_bits(
            data.state.pattern.instrument_base_note_offsets[kt.track].load(Ordering::Relaxed),
        );

        if kt.note_off {
            // Note-off: find the voice playing this note and stop it
            if is_custom {
                let Some(active_note) = take_active_keyboard_note(
                    &mut data.active_keyboard_notes,
                    kt.track,
                    kt.transpose,
                ) else {
                    continue;
                };
                let Some(engine_id) = track_engine_id(&data.state, kt.track) else {
                    continue;
                };
                let pool = &mut data.custom_engine_pools[engine_id];
                pool.release_voice_by_logical_id(active_note.logical_id);
                if active_note.logical_id != 0 {
                    unsafe {
                        send_custom_note_off(data.lg.0, active_note.logical_id);
                    }
                }
            } else {
                if let Some(active_note) = take_active_keyboard_note(
                    &mut data.active_keyboard_notes,
                    kt.track,
                    kt.transpose,
                ) {
                    let pool = &mut data.voice_pools[kt.track];
                    pool.release_voice_by_logical_id(active_note.logical_id);
                    if active_note.logical_id != 0 {
                        unsafe {
                            params_push_wrapper(
                                data.lg.0,
                                ParamMsg {
                                    idx: PARAM_GATE_SAMPLES,
                                    logical_id: active_note.logical_id,
                                    fvalue: 0.0,
                                },
                            );
                        }
                    }
                }
            }
        } else {
            // Note-on: allocate voice and trigger
            let resolved_transpose = resolve_live_keyboard_transpose(
                &data.state,
                data.accumulator_states[kt.track],
                kt.track,
                kt.transpose,
            );
            if is_custom {
                let Some(engine_id) = track_engine_id(&data.state, kt.track) else {
                    continue;
                };
                let allocation = data.custom_engine_pools[engine_id].allocate_voice(
                    kt.track,
                    resolved_transpose,
                    track_polyphonic,
                );
                let voice_idx = allocation.voice_idx;
                data.custom_engine_pools[engine_id].note_voice_allocated(engine_id, voice_idx);
                let voice_lid = allocation.logical_id;
                let fingerprint =
                    instrument_sound_fingerprint(&data.state, kt.track, engine_id, None);
                let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                    .load(Ordering::Relaxed);
                let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id]
                    [voice_idx]
                    .load(Ordering::Relaxed);
                if voice_lid == 0 || synth_id == 0 || modulator_id == 0 {
                    continue;
                }
                if data.trace_audio {
                    let enabled = data.custom_engine_pools[engine_id].enabled_voice_count;
                    eprintln!(
                        "audio-trace: keyboard custom note-on track={} engine={engine_id} voice={voice_idx} lid={voice_lid} synth={synth_id} mod={modulator_id} enabled_voices={enabled} poly={track_polyphonic} stolen={}",
                        kt.track,
                        allocation.stole_active_voice,
                    );
                    data.trace_render_probe_blocks = data.trace_render_probe_blocks.max(12);
                }
                let pitch_hz = custom_pitch_hz(resolved_transpose, base_note_offset);
                cancel_gate_off_for_lid(&mut data.gate_off_state, voice_lid);
                unsafe {
                    route_custom_voice_to_track(
                        data.lg.0,
                        &data.state,
                        engine_id,
                        voice_idx,
                        kt.track,
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != fingerprint
                    {
                        dispatch_instrument_defaults_to_voice(
                            data.lg.0,
                            &data.state,
                            kt.track,
                            synth_id as u64,
                            modulator_id as u64,
                        );
                    }
                }
                data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint = fingerprint;
                if allocation.stole_active_voice || !track_polyphonic {
                    unsafe {
                        send_custom_note_off(data.lg.0, voice_lid);
                    }
                }
                unsafe {
                    send_custom_trigger(data.lg.0, voice_lid, pitch_hz, kt.velocity);
                }
                store_active_keyboard_note(
                    &mut data.active_keyboard_notes,
                    kt.track,
                    kt.transpose,
                    voice_lid,
                );
            } else {
                let voice = data.voice_pools[kt.track].allocate_voice(resolved_transpose);
                let voice_lid = voice.logical_id;
                if voice_lid == 0 {
                    continue;
                }
                let tp = &data.state.pattern.track_params[kt.track];
                let kb_inst_slot = &data.state.pattern.instrument_slots[kt.track];
                let attack_samples =
                    kb_inst_slot.defaults.get(0) * data.sample_rate as f32 / 1000.0;
                let release_samples =
                    kb_inst_slot.defaults.get(1) * data.sample_rate as f32 / 1000.0;
                let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
                let kb_start = kb_inst_slot.defaults.get(2);
                let kb_end = kb_inst_slot.defaults.get(3);
                unsafe {
                    send_keyboard_trigger(
                        data.lg.0,
                        voice_lid,
                        resolved_transpose,
                        kt.velocity,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        kb_start,
                        kb_end,
                    );
                }
                store_active_keyboard_note(
                    &mut data.active_keyboard_notes,
                    kt.track,
                    kt.transpose,
                    voice_lid,
                );
            }
            data.state.transport.trigger_flash[kt.track].store(255, Ordering::Relaxed);
        }
    }

    // Schedule accumulator reset on play-start or pattern change; consumed at next step 0.
    {
        let playing = data.state.transport.playing.load(Ordering::Relaxed);
        let pattern = data.state.pattern.current_pattern.load(Ordering::Relaxed);
        if (!data.last_playing && playing) || data.last_pattern != pattern {
            // Pattern changes and fresh playback should always reapply custom instrument params
            // even if a voice slot is being reused from an older sound state.
            for pool in &mut data.custom_engine_pools {
                pool.invalidate_sound_cache();
            }
            data.pending_accum_reset = [true; MAX_TRACKS];
        }
        if !playing && data.last_playing {
            data.scheduled_events.clear();
            data.events_heap.clear();
        }
        data.last_playing = playing;
        data.last_pattern = pattern;
    }

    // Push BPM to all delay nodes only when it changes
    let bpm = data.state.transport.bpm.load(Ordering::Relaxed);
    if bpm != data.last_bpm {
        data.last_bpm = bpm;
        let bpm_f = bpm as f32;
        for i in 0..num_tracks {
            let delay_lid = data.state.runtime.delay_lids[i].load(Ordering::Acquire);
            if delay_lid != 0 {
                unsafe {
                    params_push_wrapper(
                        data.lg.0,
                        ParamMsg {
                            idx: delay::DELAY_PARAM_BPM,
                            logical_id: delay_lid,
                            fvalue: bpm_f,
                        },
                    );
                }
            }
        }
        for engine in &data.state.runtime.engine_modulator_node_ids {
            for node in engine {
                let logical_id = node.load(Ordering::Relaxed);
                if logical_id != 0 {
                    unsafe {
                        params_push_wrapper(
                            data.lg.0,
                            ParamMsg {
                                idx: crate::voice_modulator::PARAM_BPM as u64,
                                logical_id: logical_id as u64,
                                fvalue: bpm_f,
                            },
                        );
                    }
                }
            }
        }
    }

    let mod_reset_counter = data
        .state
        .transport
        .mod_reset_counter
        .load(Ordering::Relaxed);
    if mod_reset_counter != data.last_mod_reset_counter {
        data.last_mod_reset_counter = mod_reset_counter;
        for engine in &data.state.runtime.engine_modulator_node_ids {
            for node in engine {
                let logical_id = node.load(Ordering::Relaxed);
                if logical_id != 0 {
                    unsafe {
                        params_push_wrapper(
                            data.lg.0,
                            ParamMsg {
                                idx: crate::voice_modulator::PARAM_RESET_COUNTER as u64,
                                logical_id: logical_id as u64,
                                fvalue: mod_reset_counter as f32,
                            },
                        );
                    }
                }
            }
        }
    }

    // Process pending chop re-triggers (voice-aware)
    for track_idx in 0..num_tracks {
        let cs = &mut data.chop_state[track_idx];
        if cs.remaining > 0 {
            cs.counter -= nframes as f64;
            let tp = &data.state.pattern.track_params[track_idx];
            let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
            let chop_inst_slot = &data.state.pattern.instrument_slots[track_idx];
            let attack_samples = chop_inst_slot
                .plocks
                .get(cs.step, 0)
                .unwrap_or_else(|| chop_inst_slot.defaults.get(0))
                * data.sample_rate as f32
                / 1000.0;
            let release_samples = chop_inst_slot
                .plocks
                .get(cs.step, 1)
                .unwrap_or_else(|| chop_inst_slot.defaults.get(1))
                * data.sample_rate as f32
                / 1000.0;
            let chop_start = chop_inst_slot
                .plocks
                .get(cs.step, 2)
                .unwrap_or_else(|| chop_inst_slot.defaults.get(2));
            let chop_end = chop_inst_slot
                .plocks
                .get(cs.step, 3)
                .unwrap_or_else(|| chop_inst_slot.defaults.get(3));
            let sd = &data.state.pattern.step_data[track_idx];
            while cs.counter <= 0.0 && cs.remaining > 0 {
                // Allocate a voice for the chop re-trigger
                let transpose = sd.get(cs.step, StepParam::Transpose);
                let voice = data.voice_pools[track_idx].allocate_voice(transpose);
                let voice_lid = voice.logical_id;
                let sampler_lid =
                    data.state.runtime.sampler_lids[track_idx].load(Ordering::Acquire);
                let lid = if voice_lid != 0 {
                    voice_lid
                } else {
                    sampler_lid
                };
                unsafe {
                    send_trigger(
                        data.lg.0,
                        lid,
                        sd.get(cs.step, StepParam::Velocity),
                        sd.get(cs.step, StepParam::Speed),
                        cs.chop_gate,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        transpose,
                        chop_start,
                        chop_end,
                    );
                }
                data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
                cs.remaining -= 1;
                cs.counter += cs.interval;
            }
        }
    }

    // Process pending gate-off events for custom instruments
    for track_idx in 0..num_tracks {
        let expired = data.gate_off_state[track_idx].process(nframes);
        for lid in expired {
            if let Some(engine_id) = track_engine_id(&data.state, track_idx) {
                data.custom_engine_pools[engine_id].release_voice_by_logical_id(lid);
            } else {
                data.voice_pools[track_idx].release_voice_by_logical_id(lid);
            }
            unsafe {
                send_custom_note_off(data.lg.0, lid);
            }
        }
    }

    let mut rendered_frames = 0usize;
    let mut zero_chunk_spins = 0usize;
    while rendered_frames < nframes {
        let current_sample = block_start_sample + rendered_frames as u64;
        let current_pattern_epoch = data.state.transport.pattern_epoch.load(Ordering::Relaxed);

        // drain queue and add to binary heap (to sort events)
        while let Some(event) = data.scheduled_events.pop() {
            if event.pattern_epoch != current_pattern_epoch {
                continue;
            }
            data.events_heap.push(std::cmp::Reverse(TimedEvent {
                seq: data.event_seq,
                sample_time: event.sample_time,
                event,
            }));
            data.event_seq += 1;
        }

        while let Some(std::cmp::Reverse(event)) = data.events_heap.peek() {
            if event.event.pattern_epoch != current_pattern_epoch {
                let _ = data.events_heap.pop();
                continue;
            }
            if event.sample_time > current_sample {
                break;
            }
            let event = data.events_heap.pop().unwrap().0;
            if event.sample_time < current_sample {
                data.late_scheduled_events += 1;
            }
            dispatch_scheduled_event(data, event.event);
        }

        let next_sample = data
            .events_heap
            .peek()
            .map(|rev| rev.0.sample_time.min(block_end_sample))
            .unwrap_or(block_end_sample);
        let chunk_frames = (next_sample.saturating_sub(current_sample)) as usize;
        if chunk_frames == 0 {
            zero_chunk_spins += 1;
            if zero_chunk_spins >= 32 {
                let next_event_sample = data.events_heap.peek().map(|rev| rev.0.sample_time);
                eprintln!(
                    "audio: zero-chunk livelock guard tripped; rendered_frames={rendered_frames} nframes={nframes} current_sample={current_sample} next_event_sample={next_event_sample:?} heap_len={} late_events={}",
                    data.events_heap.len(),
                    data.late_scheduled_events,
                );
                zero_output_frames(output, rendered_frames, data.num_channels);
                break;
            }
            continue;
        }
        zero_chunk_spins = 0;

        let start = rendered_frames * data.num_channels;
        let end = (rendered_frames + chunk_frames) * data.num_channels;
        let probe_render = data.trace_audio && data.trace_render_probe_blocks > 0;
        if probe_render {
            eprintln!(
                "audio-trace: render-start callback={} chunk_frames={chunk_frames} rendered_frames={rendered_frames} tracks={num_tracks} heap_len={} rendered_samples={current_sample}",
                data.trace_callback_counter,
                data.events_heap.len(),
            );
        }
        let render_start = Instant::now();
        render_chunk(data, &mut output[start..end]);
        let render_elapsed = render_start.elapsed();
        if probe_render {
            let (chunk_peak_l, chunk_peak_r) =
                interleaved_peak(&output[start..end], data.num_channels);
            eprintln!(
                "audio-trace: render-done callback={} chunk_frames={chunk_frames} elapsed_us={} peak_l={chunk_peak_l:.6} peak_r={chunk_peak_r:.6}",
                data.trace_callback_counter,
                render_elapsed.as_micros(),
            );
            data.trace_render_probe_blocks -= 1;
        }
        if render_elapsed.as_millis() >= 10 {
            eprintln!(
                "audio: slow render_chunk; chunk_frames={chunk_frames} rendered_frames={rendered_frames} elapsed_ms={} heap_len={} current_sample={current_sample}",
                render_elapsed.as_millis(),
                data.events_heap.len(),
            );
        }
        rendered_frames += chunk_frames;
    }
    data.rendered_samples
        .store(block_end_sample, Ordering::Release);

    data.master_recorder.capture(output);

    // Scan interleaved output for peak levels
    let (peak_l, peak_r) = interleaved_peak(output, data.num_channels);
    data.state
        .transport
        .peak_l
        .store(peak_l.to_bits(), Ordering::Relaxed);
    data.state
        .transport
        .peak_r
        .store(peak_r.to_bits(), Ordering::Relaxed);

    if data.trace_audio {
        let active_custom_voices: usize = data
            .custom_engine_pools
            .iter()
            .map(|pool| {
                pool.voices
                    .iter()
                    .take(pool.num_voices)
                    .filter(|v| v.active)
                    .count()
            })
            .sum();
        let active_sampler_voices: usize = data
            .voice_pools
            .iter()
            .map(|pool| {
                pool.voices
                    .iter()
                    .take(pool.num_voices)
                    .filter(|v| v.active)
                    .count()
            })
            .sum();
        let active_voices = active_custom_voices + active_sampler_voices;
        if active_voices > 0 && peak_l <= 0.000001 && peak_r <= 0.000001 {
            data.trace_silent_active_callbacks =
                data.trace_silent_active_callbacks.saturating_add(1);
            if data.trace_silent_active_callbacks == 16
                || data.trace_silent_active_callbacks % 128 == 0
            {
                eprintln!(
                    "audio-trace: silent while voices active callbacks={} streak={} tracks={num_tracks} custom_active={active_custom_voices} sampler_active={active_sampler_voices} rendered_samples={} topology_epoch={} playing={} heap_len={} late_events={} dropped_events={}",
                    data.trace_callback_counter,
                    data.trace_silent_active_callbacks,
                    data.rendered_samples.load(Ordering::Acquire),
                    topology_epoch,
                    data.state.transport.playing.load(Ordering::Relaxed),
                    data.events_heap.len(),
                    data.late_scheduled_events,
                    data.dropped_scheduled_events,
                );
            }
        } else {
            data.trace_silent_active_callbacks = 0;
        }

        let sample_rate = data.sample_rate.max(1.0) as u64;
        let callbacks_per_second = (sample_rate / nframes.max(1) as u64).max(1);
        if data.trace_callback_counter % callbacks_per_second == 0 {
            eprintln!(
                "audio-trace: heartbeat callbacks={} rendered_samples={} tracks={num_tracks} active_custom={active_custom_voices} active_sampler={active_sampler_voices} peak_l={peak_l:.6} peak_r={peak_r:.6} topology_epoch={} cpu_load_pct={:.1}",
                data.trace_callback_counter,
                data.rendered_samples.load(Ordering::Acquire),
                topology_epoch,
                f32::from_bits(data.state.transport.cpu_load_pct.load(Ordering::Relaxed)),
            );
        }
    }

    publish_active_voice_counts(data, num_tracks);

    if nframes > 0 {
        let elapsed_secs = callback_start.elapsed().as_secs_f32();
        let block_budget_secs = nframes as f32 / data.sample_rate as f32;
        let raw_load_pct = if block_budget_secs > 0.0 {
            (elapsed_secs / block_budget_secs) * 100.0
        } else {
            0.0
        };
        let prev_load_pct =
            f32::from_bits(data.state.transport.cpu_load_pct.load(Ordering::Relaxed));
        let smoothed_load_pct = if prev_load_pct <= 0.0 {
            raw_load_pct
        } else {
            prev_load_pct * 0.97 + raw_load_pct * 0.03
        };
        data.state
            .transport
            .cpu_load_pct
            .store(smoothed_load_pct.to_bits(), Ordering::Relaxed);
    }
}

/// Build a cpal output stream that drives the audiograph.
pub fn build_output_stream(
    lg: *mut LiveGraph,
    state: Arc<SequencerState>,
    sample_rate: u32,
    num_channels: usize,
    block_size: usize,
    master_recorder: Arc<MasterRecorder>,
    keyboard_rx: std::sync::mpsc::Receiver<KeyboardTrigger>,
    bus_gate_runtime: Arc<Mutex<Vec<BusGateRuntimeState>>>,
    bus_gate_playheads: Arc<Mutex<Vec<(BusId, usize)>>>,
) -> Result<Stream, String> {
    let chop_state = (0..MAX_TRACKS)
        .map(|_| ChopTracker {
            remaining: 0,
            counter: 0.0,
            interval: 0.0,
            step: 0,
            chop_gate: 0.0,
        })
        .collect();

    // Initialize voice pools from state
    let mut voice_pools: Vec<VoicePool> = (0..MAX_TRACKS).map(|_| VoicePool::new()).collect();
    let mut custom_engine_pools: Vec<CustomEnginePool> =
        (0..MAX_TRACKS).map(|_| CustomEnginePool::new()).collect();

    // Pre-populate voice pools for any existing tracks
    let num_tracks = state.active_track_count();
    for t in 0..num_tracks {
        sync_sampler_voice_pool(&state, t, &mut voice_pools[t]);

        if let Some(engine_id) = track_engine_id(&state, t) {
            sync_custom_engine_pool(&state, engine_id, &mut custom_engine_pools[engine_id]);
        }
    }

    let gate_off_state = (0..MAX_TRACKS).map(|_| GateOffTracker::new()).collect();
    let scheduled_events = Arc::new(ScheduledEventQueue::new());
    let rendered_samples = Arc::new(AtomicU64::new(0));
    let (audio_keyboard_tx, audio_keyboard_rx) = std::sync::mpsc::channel();
    let (live_keyboard_tx, live_keyboard_rx) = std::sync::mpsc::channel();
    {
        let state_for_keyboard_router = Arc::clone(&state);
        let _ = std::thread::Builder::new()
            .name("keyboard-midi-fx-router".to_string())
            .spawn(move || {
                while let Ok(trigger) = keyboard_rx.recv() {
                    if trigger.note_off {
                        let _ = live_keyboard_tx.send(trigger);
                        let _ = audio_keyboard_tx.send(trigger);
                        continue;
                    }
                    let use_midi_fx = trigger.track
                        < state_for_keyboard_router.active_track_count()
                        && !state_for_keyboard_router.pattern.track_params[trigger.track]
                            .midi_fx_chain()
                            .is_empty();
                    if use_midi_fx {
                        let _ = live_keyboard_tx.send(trigger);
                    } else {
                        let _ = audio_keyboard_tx.send(trigger);
                    }
                }
            });
    }
    let initial_topology_epoch = state.transport.topology_epoch.load(Ordering::Relaxed);
    let trace_audio = env_flag("TINYSEQ_AUDIO_TRACE", false);
    if trace_audio {
        eprintln!("audio-trace: enabled");
    }

    let mut cb_data = AudioCallbackData {
        lg: LiveGraphPtr(lg),
        state,
        num_channels,
        chop_state,
        gate_off_state,
        sample_rate: sample_rate as f64,
        last_bpm: 0,
        last_mod_reset_counter: 0,
        voice_pools,
        custom_engine_pools,
        active_keyboard_notes: [[None; MAX_VOICES]; MAX_TRACKS],
        keyboard_rx: audio_keyboard_rx,
        master_recorder,
        accumulator_states: [crate::accumulator::AccumulatorRuntimeState::default(); MAX_TRACKS],
        last_playing: false,
        last_pattern: u32::MAX,
        last_num_tracks: num_tracks,
        last_topology_epoch: initial_topology_epoch,
        pending_accum_reset: [false; MAX_TRACKS],
        scheduled_events: Arc::clone(&scheduled_events),
        rendered_samples: Arc::clone(&rendered_samples),
        bus_gate_runtime,
        bus_gate_playheads,
        bus_gate_clocks: Vec::new(),
        bus_gate_was_playing: false,
        bus_gate_play_start_sample: 0,
        dropped_scheduled_events: 0,
        late_scheduled_events: 0,
        events_heap: BinaryHeap::new(),
        event_seq: 0,
        trace_audio,
        trace_callback_counter: 0,
        trace_render_probe_blocks: 0,
        trace_silent_active_callbacks: 0,
    };
    crate::scheduler::spawn_scheduler_thread(
        Arc::clone(&cb_data.state),
        sample_rate,
        block_size,
        rendered_samples,
        scheduled_events,
        live_keyboard_rx,
    );

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No output device available")?;

    let config = cpal::StreamConfig {
        channels: num_channels as u16,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Fixed(block_size as u32),
    };

    let stream = device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                audio_callback(&mut cb_data, data);
            },
            |err| eprintln!("Audio stream error: {err}"),
            None,
        )
        .map_err(|e| format!("Failed to build output stream: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("Failed to play stream: {e}"))?;

    Ok(stream)
}

/// Query the default output device for sample rate and channel count.
pub fn query_device_config() -> Result<(u32, u16), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No output device available")?;
    let config = device
        .default_output_config()
        .map_err(|e| format!("Failed to get default config: {e}"))?;
    Ok((config.sample_rate().0, config.channels()))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    use super::{
        bus_gate_target_at, instrument_sound_fingerprint, resolve_live_keyboard_transpose,
        resolved_chord_transpose, swing_delay_samples, CustomEnginePool, GateOffTracker,
    };
    use crate::accumulator::AccumulatorRuntimeState;
    use crate::sequencer::{SequencerState, SwingResolution};

    #[test]
    fn custom_engine_pool_prefers_inactive_voices_before_stealing() {
        let mut pool = CustomEnginePool::new();
        for lid in 1..=4 {
            pool.add_voice(lid);
        }

        let a = pool.allocate_voice(0, 0.0, true);
        let b = pool.allocate_voice(0, 4.0, true);
        assert_eq!(a.logical_id, 1);
        assert_eq!(b.logical_id, 2);
        assert!(!a.stole_active_voice);
        assert!(!b.stole_active_voice);

        pool.release_voice_by_logical_id(a.logical_id);
        pool.release_voice_by_logical_id(b.logical_id);

        let c = pool.allocate_voice(0, 7.0, true);
        let d = pool.allocate_voice(0, 11.0, true);
        assert_eq!(c.logical_id, 3);
        assert_eq!(d.logical_id, 4);
        assert!(!c.stole_active_voice);
        assert!(!d.stole_active_voice);
    }

    #[test]
    fn custom_engine_pool_steals_same_tracks_active_voice_first() {
        let mut pool = CustomEnginePool::new();
        for lid in 1..=2 {
            pool.add_voice(lid);
        }

        let first = pool.allocate_voice(0, 0.0, true);
        let second = pool.allocate_voice(1, 4.0, true);
        assert_eq!(first.logical_id, 1);
        assert_eq!(second.logical_id, 2);

        let stolen = pool.allocate_voice(1, 7.0, true);
        assert!(stolen.stole_active_voice);
        assert_eq!(stolen.previous_track, Some(1));
        assert_eq!(stolen.logical_id, 2);
    }

    #[test]
    fn custom_engine_pool_mono_reuses_same_tracks_voice_and_marks_active_steal() {
        let mut pool = CustomEnginePool::new();
        for lid in 1..=2 {
            pool.add_voice(lid);
        }

        let first = pool.allocate_voice(3, 0.0, false);
        let reused = pool.allocate_voice(3, 12.0, false);

        assert_eq!(reused.logical_id, first.logical_id);
        assert!(reused.stole_active_voice);
        assert_eq!(reused.previous_track, Some(3));
    }

    #[test]
    fn gate_off_tracker_cancel_removes_matching_pending_lids() {
        let mut tracker = GateOffTracker::new();
        tracker.schedule(10, 100.0);
        tracker.schedule(20, 100.0);
        tracker.cancel(10);

        let expired = tracker.process(200);
        assert_eq!(expired, vec![20]);
    }

    #[test]
    fn swing_delay_uses_full_pair_offset() {
        let delay = swing_delay_samples(48_000.0, 120.0, 75.0, SwingResolution::Sixteenth);
        assert_eq!(delay, 3_000.0);

        let straight = swing_delay_samples(48_000.0, 120.0, 50.0, SwingResolution::Sixteenth);
        assert_eq!(straight, 0.0);
    }

    #[test]
    fn bus_gate_default_sequence_stays_open_across_steps() {
        let sequence = crate::ui::BusGateSequence::default();
        assert_eq!(bus_gate_target_at(&sequence, 0.0), 1.0);
        assert_eq!(bus_gate_target_at(&sequence, 0.24), 1.0);
        assert_eq!(bus_gate_target_at(&sequence, 0.25), 1.0);
        assert_eq!(bus_gate_target_at(&sequence, 1.99), 1.0);
    }

    #[test]
    fn bus_gate_sequence_follows_step_activity_and_duration() {
        let mut sequence = crate::ui::BusGateSequence::default();
        sequence.num_steps = 4;
        sequence.timebase = crate::sequencer::Timebase::Quarter;
        sequence.steps = [false; crate::sequencer::MAX_STEPS];
        sequence.steps[1] = true;
        sequence.velocities[1] = 0.5;
        sequence.durations[1] = 0.5;

        assert_eq!(bus_gate_target_at(&sequence, 0.25), 0.0);
        assert_eq!(bus_gate_target_at(&sequence, 1.10), 0.5);
        assert_eq!(bus_gate_target_at(&sequence, 1.60), 0.0);
        assert_eq!(bus_gate_target_at(&sequence, 2.10), 0.0);
    }

    #[test]
    fn bus_gate_sync_steps_snap_boundaries_to_grid() {
        let mut sequence = crate::ui::BusGateSequence::default();
        sequence.num_steps = 4;
        sequence.timebase = crate::sequencer::Timebase::Sixteenth;
        sequence.steps = [false; crate::sequencer::MAX_STEPS];
        sequence.steps[1] = true;
        sequence.syncs[1] = 3.0; // 1/4

        assert_eq!(bus_gate_target_at(&sequence, 0.50), 0.0);
        assert_eq!(bus_gate_target_at(&sequence, 1.01), 1.0);
        assert_eq!(bus_gate_target_at(&sequence, 1.30), 0.0);
    }

    #[test]
    fn sound_fingerprint_changes_when_step_sound_changes() {
        let state = Arc::new(SequencerState::new(1, Vec::new()));
        state.runtime.track_engine_ids[0].store(2, Ordering::Relaxed);
        state.pattern.instrument_slots[0]
            .num_params
            .store(2, Ordering::Relaxed);
        state.pattern.instrument_slots[0].defaults.set(0, 0.2);
        state.pattern.instrument_slots[0].defaults.set(1, 0.4);

        let base = instrument_sound_fingerprint(&state, 0, 2, Some(3));
        state.pattern.instrument_slots[0].plocks.set(3, 1, 0.9);
        let changed = instrument_sound_fingerprint(&state, 0, 2, Some(3));

        assert_ne!(base, changed);
    }

    #[test]
    fn sound_fingerprint_changes_when_base_note_changes() {
        let state = Arc::new(SequencerState::new(1, Vec::new()));
        state.runtime.track_engine_ids[0].store(5, Ordering::Relaxed);
        state.pattern.instrument_slots[0]
            .num_params
            .store(1, Ordering::Relaxed);
        state.pattern.instrument_slots[0].defaults.set(0, 0.5);

        let base = instrument_sound_fingerprint(&state, 0, 5, None);
        state.pattern.instrument_base_note_offsets[0].store(12.0f32.to_bits(), Ordering::Relaxed);
        let changed = instrument_sound_fingerprint(&state, 0, 5, None);

        assert_ne!(base, changed);
    }

    #[test]
    fn invalidating_custom_voice_cache_clears_all_fingerprints() {
        let mut pool = CustomEnginePool::new();
        pool.add_voice(10);
        pool.add_voice(20);
        pool.voices[0].fingerprint = 123;
        pool.voices[1].fingerprint = 456;

        pool.invalidate_sound_cache();

        assert_eq!(pool.voices[0].fingerprint, 0);
        assert_eq!(pool.voices[1].fingerprint, 0);
    }

    #[test]
    fn resolved_chord_transpose_applies_accumulator_offset() {
        assert_eq!(resolved_chord_transpose(7.0, 0.0, 5.0), 12.0);
        assert_eq!(resolved_chord_transpose(7.0, 2.0, 8.0), 13.0);
    }

    #[test]
    fn live_keyboard_transpose_applies_current_transpose_ramp_state() {
        let state = SequencerState::new(1, Vec::new());
        state.pattern.track_params[0].set_accumulator_idx(1);

        let resolved = resolve_live_keyboard_transpose(
            &state,
            AccumulatorRuntimeState {
                value: 5.0,
                reversed: false,
            },
            0,
            2.0,
        );

        assert_eq!(resolved, 7.0);
    }

    #[test]
    fn live_keyboard_transpose_quantizes_after_transpose_ramp_offset() {
        let state = SequencerState::new(1, Vec::new());
        state.pattern.track_params[0].set_accumulator_idx(1);
        state.pattern.track_params[0].set_fts_scale(1);

        let resolved = resolve_live_keyboard_transpose(
            &state,
            AccumulatorRuntimeState {
                value: 1.0,
                reversed: false,
            },
            0,
            2.6,
        );

        assert_eq!(resolved, 4.0);
    }
}
