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
use crate::effects::EffectSlotSnapshot;
use crate::gatepitch;
use crate::recorder::MasterRecorder;
use crate::sampler::{
    PARAM_ATTACK_SAMPLES, PARAM_ENABLED, PARAM_END_POINT, PARAM_GATE_MODE, PARAM_GATE_SAMPLES,
    PARAM_LOOP_MODE, PARAM_LOOP_XFADE_SAMPLES, PARAM_PLAYHEAD, PARAM_RELEASE_SAMPLES,
    PARAM_REVERSE, PARAM_SPEED, PARAM_SR_HZ, PARAM_START_POINT, PARAM_TRANSPOSE, PARAM_TRIGGER,
    PARAM_VELOCITY, PARAM_WARP_ENABLED, PARAM_WARP_MODE, PARAM_WARP_ONSET_TABLE_PTR_HI,
    PARAM_WARP_ONSET_TABLE_PTR_LO, PARAM_WARP_PROJECT_BPM, PARAM_WARP_RATIO, PARAM_WARP_SAMPLE_BPM,
};
use crate::scheduled_event::{
    resolved_chord_transpose, ScheduledEffectParam, ScheduledEvent, ScheduledEventKind,
    ScheduledEventQueue, ScheduledInstrumentParam, ScheduledInstrumentParamTarget,
    ScheduledInstrumentParams, ScheduledSamplerParams, TimedEvent,
};
use crate::sequencer::{
    sync_beats, BusId, CustomInstrumentRunMode, InstrumentType, KeyboardTrigger, SequencerState,
    StepParam, SwingResolution, MAX_TRACKS,
};
use crate::ui::BusGateRuntimeState;
use crate::voice::{VoicePool, MAX_VOICES};

pub const FALLBACK_SAMPLE_RATE: u32 = 44_100;
const CUSTOM_ENGINE_RELEASE_TAIL_SECONDS: f64 = 20.0;

unsafe fn push_param_span(lg: *mut LiveGraph, logical_id: u64, idx: u64, span: u32, value: f32) {
    for lane in 0..span.max(1) as u64 {
        params_push_wrapper(
            lg,
            ParamMsg {
                idx: idx + lane,
                logical_id,
                fvalue: value,
            },
        );
    }
}

unsafe fn dispatch_voice_modulator_transport(lg: *mut LiveGraph, modulator_id: u64, bpm: f32) {
    if modulator_id == 0 {
        return;
    }
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::voice_modulator::PARAM_BPM as u64,
            logical_id: modulator_id,
            fvalue: bpm.clamp(20.0, 400.0),
        },
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OutputDeviceConfig {
    sample_rate: u32,
    channels: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OutputFormatRange {
    channels: u16,
    min_sample_rate: u32,
    max_sample_rate: u32,
    supports_f32: bool,
}

impl OutputFormatRange {
    fn supports_sample_rate(self, sample_rate: u32) -> bool {
        self.min_sample_rate <= sample_rate && sample_rate <= self.max_sample_rate
    }
}

fn select_output_channels(
    sample_rate: u32,
    default_channels: u16,
    ranges: impl IntoIterator<Item = OutputFormatRange>,
) -> Option<u16> {
    ranges
        .into_iter()
        .filter(|range| range.supports_sample_rate(sample_rate))
        .filter(|range| range.supports_f32)
        .map(|range| range.channels)
        .min_by_key(|&channels| {
            let preference = if channels == default_channels {
                0
            } else if channels == 2 {
                1
            } else {
                2
            };
            (preference, channels)
        })
}

fn select_output_config(
    default_sample_rate: u32,
    default_channels: u16,
    ranges: impl IntoIterator<Item = OutputFormatRange>,
) -> Option<OutputDeviceConfig> {
    let ranges: Vec<OutputFormatRange> = ranges.into_iter().collect();
    if let Some(channels) =
        select_output_channels(default_sample_rate, default_channels, ranges.clone())
    {
        return Some(OutputDeviceConfig {
            sample_rate: default_sample_rate,
            channels,
        });
    }

    if default_sample_rate == FALLBACK_SAMPLE_RATE {
        return None;
    }

    select_output_channels(FALLBACK_SAMPLE_RATE, default_channels, ranges).map(|channels| {
        OutputDeviceConfig {
            sample_rate: FALLBACK_SAMPLE_RATE,
            channels,
        }
    })
}

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
    host_clock_was_playing: bool,
    host_clock_play_start_sample: u64,
    free_patch_transport_routes: [FreePatchTransportRouteState; MAX_TRACKS],
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct FreePatchTransportRouteState {
    valid: bool,
    engine_id: usize,
    route_hash: u64,
    open: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FreePatchTransportRouteTarget {
    engine_id: usize,
    route_hash: u64,
    open: bool,
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
    release_started_sample: Option<u64>,
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
                release_started_sample: None,
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
                release_started_sample: None,
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
                release_started_sample: None,
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
        max_polyphony: usize,
    ) -> CustomVoiceAllocation {
        self.age_counter += 1;
        let max_polyphony = max_polyphony.clamp(1, MAX_VOICES);
        if !polyphonic {
            if let Some(idx) =
                (0..self.num_voices).find(|&i| self.voices[i].assigned_track == Some(track))
            {
                let slot = &mut self.voices[idx];
                let previous_track = slot.assigned_track;
                let stole_active_voice = slot.active;
                slot.age = self.age_counter;
                slot.active = true;
                slot.release_started_sample = None;
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

        let mut active_same_note_idx = None;
        let mut releasing_same_note_idx = None;
        let mut releasing_same_note_age = u64::MAX;
        let mut idle_same_track_idx = None;
        let mut idle_same_track_age = u64::MAX;
        let mut releasing_same_track_idx = None;
        let mut releasing_same_track_age = u64::MAX;
        let mut unassigned_idle_idx = None;
        let mut unassigned_idle_age = u64::MAX;
        let mut oldest_same_track = None;
        let mut oldest_same_track_age = u64::MAX;
        let mut assigned_same_track_count = 0usize;
        let mut idle_other_track_idx = None;
        let mut idle_other_track_age = u64::MAX;
        let mut releasing_other_track_idx = None;
        let mut releasing_other_track_age = u64::MAX;
        let mut oldest_idx = 0;
        let mut oldest_age = u64::MAX;

        for i in 0..self.num_voices {
            let voice = &self.voices[i];
            if !voice.active {
                let is_releasing = voice.release_started_sample.is_some();
                match voice.assigned_track {
                    Some(assigned) if assigned == track => {
                        if is_releasing {
                            if (voice.note - note).abs() < 0.01
                                && voice.age < releasing_same_note_age
                            {
                                releasing_same_note_idx = Some(i);
                                releasing_same_note_age = voice.age;
                            }
                            if voice.age < releasing_same_track_age {
                                releasing_same_track_idx = Some(i);
                                releasing_same_track_age = voice.age;
                            }
                        } else if voice.age < idle_same_track_age {
                            idle_same_track_idx = Some(i);
                            idle_same_track_age = voice.age;
                        }
                    }
                    Some(_) => {
                        if is_releasing {
                            if voice.age < releasing_other_track_age {
                                releasing_other_track_idx = Some(i);
                                releasing_other_track_age = voice.age;
                            }
                        } else if voice.age < idle_other_track_age {
                            idle_other_track_idx = Some(i);
                            idle_other_track_age = voice.age;
                        }
                    }
                    None => {
                        if !is_releasing && voice.age < unassigned_idle_age {
                            unassigned_idle_idx = Some(i);
                            unassigned_idle_age = voice.age;
                        }
                    }
                }
            }
            if voice.active
                && voice.assigned_track == Some(track)
                && (voice.note - note).abs() < 0.01
            {
                active_same_note_idx = Some(i);
            }
            if voice.assigned_track == Some(track) {
                assigned_same_track_count += 1;
                if voice.age < oldest_same_track_age {
                    oldest_same_track = Some(i);
                    oldest_same_track_age = voice.age;
                }
            }
            if voice.age < oldest_age {
                oldest_idx = i;
                oldest_age = voice.age;
            }
        }

        let idx = if assigned_same_track_count >= max_polyphony {
            active_same_note_idx
                .or(releasing_same_note_idx)
                .or(idle_same_track_idx)
                .or(releasing_same_track_idx)
                .or(oldest_same_track)
                .unwrap_or(oldest_idx)
        } else {
            active_same_note_idx
                .or(releasing_same_note_idx)
                .or(idle_same_track_idx)
                .or(unassigned_idle_idx)
                .or(idle_other_track_idx)
                .or(releasing_same_track_idx)
                .or(oldest_same_track)
                .or(releasing_other_track_idx)
                .unwrap_or(oldest_idx)
        };
        let slot = &mut self.voices[idx];
        let previous_track = slot.assigned_track;
        let stole_active_voice = slot.active;
        slot.age = self.age_counter;
        slot.active = true;
        slot.release_started_sample = None;
        slot.note = note;
        slot.assigned_track = Some(track);
        CustomVoiceAllocation {
            voice_idx: idx,
            logical_id: slot.logical_id,
            previous_track,
            stole_active_voice,
        }
    }

    fn allocate_free_patch_voice(
        &mut self,
        track: usize,
        note: f32,
    ) -> Option<CustomVoiceAllocation> {
        if self.num_voices == 0 {
            return None;
        }
        self.age_counter += 1;
        let slot = &mut self.voices[0];
        let previous_track = slot.assigned_track;
        let stole_active_voice = slot.active;
        slot.age = self.age_counter;
        slot.active = true;
        slot.release_started_sample = None;
        slot.note = note;
        slot.assigned_track = Some(track);
        Some(CustomVoiceAllocation {
            voice_idx: 0,
            logical_id: slot.logical_id,
            previous_track,
            stole_active_voice,
        })
    }

    fn release_voice_by_logical_id(&mut self, logical_id: u64, release_sample: u64) {
        for i in 0..self.num_voices {
            if self.voices[i].logical_id == logical_id {
                self.voices[i].active = false;
                self.voices[i].release_started_sample = Some(release_sample);
                return;
            }
        }
    }

    fn release_free_patch_voice_by_logical_id(&mut self, logical_id: u64) {
        for i in 0..self.num_voices {
            if self.voices[i].logical_id == logical_id {
                self.voices[i].active = false;
                self.voices[i].release_started_sample = None;
                return;
            }
        }
    }

    fn note_voice_allocated(&mut self, engine_id: usize, voice_idx: usize) {
        let needed = (voice_idx + 1).min(MAX_VOICES).max(1);
        if needed > self.enabled_voice_count {
            self.enabled_voice_count = needed;
            crate::lisp_host::set_dgen_engine_enabled_voices(engine_id, needed);
        }
    }

    fn sync_enabled_voice_count(&mut self, engine_id: usize) {
        self.enabled_voice_count = crate::lisp_host::get_dgen_engine_enabled_voices(engine_id);
    }

    fn shrink_released_voices(
        &mut self,
        engine_id: usize,
        current_sample: u64,
        release_tail_samples: u64,
    ) {
        let mut highest_retained_idx = 0usize;
        for i in 0..self.num_voices {
            let voice = &mut self.voices[i];
            if let Some(release_started_sample) = voice.release_started_sample {
                if current_sample.saturating_sub(release_started_sample) >= release_tail_samples {
                    voice.release_started_sample = None;
                }
            }
            if voice.active || voice.release_started_sample.is_some() {
                highest_retained_idx = highest_retained_idx.max(i);
            }
        }

        let needed = (highest_retained_idx + 1).clamp(1, MAX_VOICES);
        if needed < self.enabled_voice_count {
            self.enabled_voice_count = needed;
            crate::lisp_host::set_dgen_engine_enabled_voices(engine_id, needed);
        }
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
            let desired_node_id = state.runtime.synth_node_ids[track][v].load(Ordering::Acquire);
            let desired_gatepitch_id =
                state.runtime.sampler_gatepitch_node_ids[track][v].load(Ordering::Acquire);
            let desired_modulator_id =
                state.runtime.sampler_modulator_node_ids[track][v].load(Ordering::Acquire);
            if pool.voices[v].logical_id != desired_lid
                || pool.voices[v].node_id as u32 != desired_node_id
                || pool.voices[v].gatepitch_id as u32 != desired_gatepitch_id
                || pool.voices[v].modulator_id as u32 != desired_modulator_id
            {
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
                let node_id = state.runtime.synth_node_ids[track][v].load(Ordering::Acquire) as i32;
                let gatepitch_id = state.runtime.sampler_gatepitch_node_ids[track][v]
                    .load(Ordering::Acquire) as i32;
                let modulator_id = state.runtime.sampler_modulator_node_ids[track][v]
                    .load(Ordering::Acquire) as i32;
                pool.add_modulated_voice(lid, node_id, gatepitch_id, modulator_id);
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
        crate::lisp_host::reset_dgen_engine_enabled_voices(engine_id);
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

fn free_patch_route_lids_hash(
    state: &SequencerState,
    engine_id: usize,
    num_tracks: usize,
) -> Option<u64> {
    if engine_id >= state.runtime.engine_route_lids.len() {
        return None;
    }

    let mut hasher = DefaultHasher::new();
    engine_id.hash(&mut hasher);
    num_tracks.hash(&mut hasher);
    for track in 0..num_tracks.min(MAX_TRACKS) {
        state.runtime.engine_route_lids[engine_id][0][track]
            .load(Ordering::Acquire)
            .hash(&mut hasher);
        state.runtime.engine_route_lids_r[engine_id][0][track]
            .load(Ordering::Acquire)
            .hash(&mut hasher);
        for input in 0..crate::sequencer::EXT_MOD_INPUT_COUNT {
            state.runtime.engine_ext_route_lids[engine_id][0][track][input]
                .load(Ordering::Acquire)
                .hash(&mut hasher);
        }
    }
    Some(hasher.finish())
}

fn free_patch_transport_route_target(
    state: &SequencerState,
    track: usize,
    num_tracks: usize,
    playing: bool,
) -> Option<FreePatchTransportRouteTarget> {
    if track >= num_tracks || track >= MAX_TRACKS {
        return None;
    }
    if InstrumentType::from_runtime_flag(
        state.runtime.instrument_type_flags[track].load(Ordering::Acquire),
    ) != InstrumentType::Custom
    {
        return None;
    }
    if track_custom_run_mode(state, track) != CustomInstrumentRunMode::FreePatch {
        return None;
    }
    let engine_id = track_engine_id(state, track)?;
    let route_hash = free_patch_route_lids_hash(state, engine_id, num_tracks)?;
    Some(FreePatchTransportRouteTarget {
        engine_id,
        route_hash,
        open: playing,
    })
}

fn free_patch_transport_route_cache_is_fresh(
    cached: FreePatchTransportRouteState,
    target: FreePatchTransportRouteTarget,
) -> bool {
    cached.valid
        && cached.engine_id == target.engine_id
        && cached.route_hash == target.route_hash
        && cached.open == target.open
        && target.open
}

unsafe fn set_free_patch_transport_route(
    lg: *mut LiveGraph,
    state: &SequencerState,
    engine_id: usize,
    track: usize,
    num_tracks: usize,
    open: bool,
) {
    if engine_id >= state.runtime.engine_route_lids.len() {
        return;
    }

    for route_track in 0..num_tracks.min(MAX_TRACKS) {
        let value = if open && route_track == track {
            1.0
        } else {
            0.0
        };
        let lid_l =
            state.runtime.engine_route_lids[engine_id][0][route_track].load(Ordering::Acquire);
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

        let lid_r =
            state.runtime.engine_route_lids_r[engine_id][0][route_track].load(Ordering::Acquire);
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

        for input in 0..crate::sequencer::EXT_MOD_INPUT_COUNT {
            let ext_lid = state.runtime.engine_ext_route_lids[engine_id][0][route_track][input]
                .load(Ordering::Acquire);
            if ext_lid != 0 {
                params_push_wrapper(
                    lg,
                    ParamMsg {
                        idx: 0,
                        logical_id: ext_lid,
                        fvalue: value,
                    },
                );
            }
        }
    }
}

fn sync_free_patch_transport_routes(data: &mut AudioCallbackData, num_tracks: usize) {
    let playing = data.state.transport.playing.load(Ordering::Acquire);
    for track in 0..MAX_TRACKS {
        let Some(target) =
            free_patch_transport_route_target(&data.state, track, num_tracks, playing)
        else {
            data.free_patch_transport_routes[track].valid = false;
            continue;
        };

        let cached = data.free_patch_transport_routes[track];
        if free_patch_transport_route_cache_is_fresh(cached, target) {
            continue;
        }

        unsafe {
            set_free_patch_transport_route(
                data.lg.0,
                &data.state,
                target.engine_id,
                track,
                num_tracks,
                target.open,
            );
        }
        data.free_patch_transport_routes[track] = FreePatchTransportRouteState {
            valid: true,
            engine_id: target.engine_id,
            route_hash: target.route_hash,
            open: target.open,
        };
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
    for (engine_id, pool) in data.custom_engine_pools.iter_mut().enumerate() {
        pool.reset();
        crate::lisp_host::reset_dgen_engine_enabled_voices(engine_id);
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
    data.host_clock_was_playing = false;
    data.host_clock_play_start_sample = 0;
    data.free_patch_transport_routes = [FreePatchTransportRouteState::default(); MAX_TRACKS];
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
            let is_custom = InstrumentType::from_runtime_flag(
                data.state.runtime.instrument_type_flags[track].load(Ordering::Relaxed),
            ) == InstrumentType::Custom;
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
    enabled: f32,
    reverse: f32,
    loop_mode: f32,
    loop_xfade_samples: f32,
    sr_hz: f32,
    warp_enabled: f32,
    warp_mode: f32,
    warp_ratio: f32,
    warp_sample_bpm: f32,
    warp_project_bpm: f32,
    warp_ptr_lo: f32,
    warp_ptr_hi: f32,
) {
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_ENABLED,
            logical_id: lid,
            fvalue: enabled,
        },
    );
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
            idx: PARAM_REVERSE,
            logical_id: lid,
            fvalue: reverse,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_LOOP_MODE,
            logical_id: lid,
            fvalue: loop_mode,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_LOOP_XFADE_SAMPLES,
            logical_id: lid,
            fvalue: loop_xfade_samples,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_SR_HZ,
            logical_id: lid,
            fvalue: sr_hz,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_WARP_ENABLED,
            logical_id: lid,
            fvalue: warp_enabled,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_WARP_MODE,
            logical_id: lid,
            fvalue: warp_mode,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_WARP_RATIO,
            logical_id: lid,
            fvalue: warp_ratio,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_WARP_SAMPLE_BPM,
            logical_id: lid,
            fvalue: warp_sample_bpm,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_WARP_PROJECT_BPM,
            logical_id: lid,
            fvalue: warp_project_bpm,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_WARP_ONSET_TABLE_PTR_LO,
            logical_id: lid,
            fvalue: warp_ptr_lo,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_WARP_ONSET_TABLE_PTR_HI,
            logical_id: lid,
            fvalue: warp_ptr_hi,
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
    enabled: f32,
    reverse: f32,
    loop_mode: f32,
    loop_xfade_samples: f32,
    sr_hz: f32,
    warp_enabled: f32,
    warp_mode: f32,
    warp_ratio: f32,
    warp_sample_bpm: f32,
    warp_project_bpm: f32,
    warp_ptr_lo: f32,
    warp_ptr_hi: f32,
) {
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_ENABLED,
            logical_id: lid,
            fvalue: enabled,
        },
    );
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
            idx: PARAM_REVERSE,
            logical_id: lid,
            fvalue: reverse,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_LOOP_MODE,
            logical_id: lid,
            fvalue: loop_mode,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_LOOP_XFADE_SAMPLES,
            logical_id: lid,
            fvalue: loop_xfade_samples,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_SR_HZ,
            logical_id: lid,
            fvalue: sr_hz,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_WARP_ENABLED,
            logical_id: lid,
            fvalue: warp_enabled,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_WARP_MODE,
            logical_id: lid,
            fvalue: warp_mode,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_WARP_RATIO,
            logical_id: lid,
            fvalue: warp_ratio,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_WARP_SAMPLE_BPM,
            logical_id: lid,
            fvalue: warp_sample_bpm,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_WARP_PROJECT_BPM,
            logical_id: lid,
            fvalue: warp_project_bpm,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_WARP_ONSET_TABLE_PTR_LO,
            logical_id: lid,
            fvalue: warp_ptr_lo,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_WARP_ONSET_TABLE_PTR_HI,
            logical_id: lid,
            fvalue: warp_ptr_hi,
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

unsafe fn set_modulator_gate(lg: *mut LiveGraph, modulator_lid: u64, gate: f32) {
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::track_modulator::PARAM_GATE,
            logical_id: modulator_lid,
            fvalue: gate.clamp(0.0, 1.0),
        },
    );
}

unsafe fn trigger_modulator_pulse(
    lg: *mut LiveGraph,
    modulator_lid: u64,
    pulse_samples: f32,
    pulse_level: f32,
) {
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::track_modulator::PARAM_PULSE_SAMPLES,
            logical_id: modulator_lid,
            fvalue: pulse_samples.max(1.0),
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::track_modulator::PARAM_PULSE_LEVEL,
            logical_id: modulator_lid,
            fvalue: pulse_level.clamp(0.0, 1.0),
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::track_modulator::PARAM_TRIGGER,
            logical_id: modulator_lid,
            fvalue: 1.0,
        },
    );
}

unsafe fn dispatch_modulator_params(
    lg: *mut LiveGraph,
    modulator_lid: u64,
    instrument_params: &[ScheduledInstrumentParam],
) {
    for param in instrument_params {
        if param.target != ScheduledInstrumentParamTarget::Synth {
            continue;
        }
        push_param_span(lg, modulator_lid, param.idx, param.span, param.value);
    }
}

fn custom_pitch_hz(transpose: f32, base_note_offset: f32) -> f32 {
    440.0 * 2f32.powf((transpose + base_note_offset) / 12.0)
}

fn track_accepts_scheduled_trigger(state: &SequencerState, track_idx: usize) -> bool {
    let Some(track_params) = state.pattern.track_params.get(track_idx) else {
        return false;
    };
    if track_params.is_muted() {
        return false;
    }
    let has_solo = state
        .pattern
        .track_params
        .iter()
        .take(state.active_track_count())
        .any(|params| params.is_solo());
    !has_solo || track_params.is_solo()
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

fn track_engine_id(state: &SequencerState, track_idx: usize) -> Option<usize> {
    let engine_id = state.runtime.track_engine_ids[track_idx].load(Ordering::Relaxed);
    if engine_id == u32::MAX {
        None
    } else {
        Some(engine_id as usize)
    }
}

fn track_custom_run_mode(state: &SequencerState, track_idx: usize) -> CustomInstrumentRunMode {
    CustomInstrumentRunMode::from_runtime_flag(
        state.runtime.instrument_run_mode_flags[track_idx].load(Ordering::Relaxed),
    )
}

fn sampler_warp_runtime(
    state: &SequencerState,
    track_idx: usize,
    warp_enabled: f32,
    warp_mode: f32,
    sample_bpm: f32,
) -> (f32, f32, f32, f32, f32, f32, f32) {
    let project_bpm = state.transport.bpm.load(Ordering::Relaxed).max(1) as f32;
    let sample_bpm = sample_bpm.clamp(20.0, 400.0);
    if warp_enabled <= 0.5 || warp_mode.round() != 0.0 {
        return (0.0, warp_mode, 1.0, sample_bpm, project_bpm, 0.0, 0.0);
    }
    let status = state.runtime.sampler_analysis_status[track_idx].load(Ordering::Acquire);
    if status != 2 {
        return (0.0, warp_mode, 1.0, sample_bpm, project_bpm, 0.0, 0.0);
    }
    let ptr_lo_bits = state.runtime.sampler_onset_ptr_lo[track_idx].load(Ordering::Acquire);
    let ptr_hi_bits = state.runtime.sampler_onset_ptr_hi[track_idx].load(Ordering::Acquire);
    if ptr_lo_bits == 0 && ptr_hi_bits == 0 {
        return (0.0, warp_mode, 1.0, sample_bpm, project_bpm, 0.0, 0.0);
    }
    (
        1.0,
        warp_mode,
        project_bpm / sample_bpm,
        sample_bpm,
        project_bpm,
        f32::from_bits(ptr_lo_bits),
        f32::from_bits(ptr_hi_bits),
    )
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
    let mut sorted_params = effect_params.iter().collect::<Vec<_>>();
    sorted_params.sort_by_key(|param| (param.logical_id, param.idx));
    for param in sorted_params {
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
        for input in 0..crate::sequencer::EXT_MOD_INPUT_COUNT {
            let ext_lid = state.runtime.engine_ext_route_lids[engine_id][voice_idx][t][input]
                .load(Ordering::Relaxed);
            if ext_lid != 0 {
                params_push_wrapper(
                    lg,
                    ParamMsg {
                        idx: 0,
                        logical_id: ext_lid,
                        fvalue: value,
                    },
                );
            }
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
        push_param_span(lg, logical_id, idx, param.span, param.value);
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
    let mut param_indices = (0..num_params).collect::<Vec<_>>();
    param_indices.sort_by_key(|param_idx| slot.resolve_node_idx(*param_idx));
    for param_idx in param_indices {
        let idx = slot.resolve_node_idx(param_idx);
        let is_mod_param = idx as u32 >= crate::voice_modulator::MOD_PARAM_BASE;
        let logical_id = if is_mod_param { modulator_id } else { synth_id };
        let resolved_idx = if is_mod_param {
            idx - crate::voice_modulator::MOD_PARAM_BASE as u64
        } else {
            idx
        };
        push_param_span(
            lg,
            logical_id,
            resolved_idx,
            slot.resolve_node_span(param_idx),
            slot.defaults.get(param_idx),
        );
    }
}

unsafe fn dispatch_sampler_modulator_params_to_voice(
    lg: *mut LiveGraph,
    modulator_id: u64,
    instrument_params: &[ScheduledInstrumentParam],
) {
    if modulator_id == 0 {
        return;
    }
    for param in instrument_params {
        if param.target != ScheduledInstrumentParamTarget::Modulator {
            continue;
        }
        push_param_span(lg, modulator_id, param.idx, param.span, param.value);
    }
}

unsafe fn dispatch_sampler_extra_params_to_voice(
    lg: *mut LiveGraph,
    sampler_lid: u64,
    instrument_params: &[ScheduledInstrumentParam],
) {
    for param in instrument_params {
        if param.target != ScheduledInstrumentParamTarget::Synth {
            continue;
        }
        if param.idx < crate::sampler::PARAM_SCRUB_OFFSET {
            continue;
        }
        push_param_span(lg, sampler_lid, param.idx, param.span, param.value);
    }
}

fn sampler_live_param_value(idx: u64, value: f32, sample_rate: f64) -> f32 {
    if idx == PARAM_ATTACK_SAMPLES
        || idx == PARAM_RELEASE_SAMPLES
        || idx == PARAM_LOOP_XFADE_SAMPLES
    {
        // Sampler p-lock values for these UI params are stored in ms; the DSP
        // node consumes samples.
        value * sample_rate as f32 / 1000.0
    } else {
        value
    }
}

unsafe fn dispatch_sampler_live_params_to_voice(
    lg: *mut LiveGraph,
    sampler_lid: u64,
    modulator_id: u64,
    instrument_params: &[ScheduledInstrumentParam],
    sample_rate: f64,
) {
    for param in instrument_params {
        match param.target {
            ScheduledInstrumentParamTarget::Synth => {
                push_param_span(
                    lg,
                    sampler_lid,
                    param.idx,
                    param.span,
                    sampler_live_param_value(param.idx, param.value, sample_rate),
                );
            }
            ScheduledInstrumentParamTarget::Modulator => {
                if modulator_id != 0 {
                    push_param_span(lg, modulator_id, param.idx, param.span, param.value);
                }
            }
        }
    }
}

fn dispatch_instrument_params_to_active_voices(
    data: &mut AudioCallbackData,
    track_idx: usize,
    instrument_params: &[ScheduledInstrumentParam],
) {
    if instrument_params.is_empty() {
        return;
    }
    if let Some(engine_id) = track_engine_id(&data.state, track_idx) {
        let pool = &mut data.custom_engine_pools[engine_id];
        let free_patch =
            track_custom_run_mode(&data.state, track_idx) == CustomInstrumentRunMode::FreePatch;
        for voice_idx in 0..pool.num_voices {
            let targets_voice = if free_patch {
                voice_idx == 0
            } else {
                pool.voices[voice_idx].active
                    && pool.voices[voice_idx].assigned_track == Some(track_idx)
            };
            if !targets_voice {
                continue;
            }
            let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                .load(Ordering::Relaxed);
            let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id][voice_idx]
                .load(Ordering::Relaxed);
            if synth_id == 0 {
                continue;
            }
            unsafe {
                dispatch_instrument_params_to_voice(
                    data.lg.0,
                    synth_id as u64,
                    modulator_id as u64,
                    instrument_params,
                );
            }
            // Force re-resolve on the next trigger because live p-locks can
            // diverge the active voice from descriptor defaults.
            pool.voices[voice_idx].fingerprint = 0;
        }
    } else {
        let pool = &data.voice_pools[track_idx];
        for voice in pool.voices[..pool.num_voices]
            .iter()
            .filter(|voice| voice.active && voice.logical_id != 0)
        {
            unsafe {
                dispatch_sampler_live_params_to_voice(
                    data.lg.0,
                    voice.logical_id,
                    voice.modulator_id as u64,
                    instrument_params,
                    data.sample_rate,
                );
            }
        }
    }
}

unsafe fn dispatch_sampler_modulator_defaults_to_voice(
    lg: *mut LiveGraph,
    state: &SequencerState,
    track_idx: usize,
    modulator_id: u64,
) {
    if modulator_id == 0 {
        return;
    }
    let slot = &state.pattern.instrument_slots[track_idx];
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params {
        let idx = slot.resolve_node_idx(param_idx);
        if (idx as u32) < crate::voice_modulator::MOD_PARAM_BASE {
            continue;
        }
        params_push_wrapper(
            lg,
            ParamMsg {
                idx: idx - crate::voice_modulator::MOD_PARAM_BASE as u64,
                logical_id: modulator_id,
                fvalue: slot.defaults.get(param_idx),
            },
        );
    }
}

unsafe fn dispatch_sampler_extra_defaults_to_voice(
    lg: *mut LiveGraph,
    state: &SequencerState,
    track_idx: usize,
    sampler_lid: u64,
) {
    let slot = &state.pattern.instrument_slots[track_idx];
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params {
        let idx = slot.resolve_node_idx(param_idx);
        if idx < crate::sampler::PARAM_SCRUB_OFFSET
            || idx as u32 >= crate::voice_modulator::MOD_PARAM_BASE
        {
            continue;
        }
        params_push_wrapper(
            lg,
            ParamMsg {
                idx,
                logical_id: sampler_lid,
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
    instrument_params: ScheduledInstrumentParams,
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
        None,
    );
}

fn dispatch_scheduled_network_step(
    data: &mut AudioCallbackData,
    track_idx: usize,
    samples_per_step: f32,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    sampler_params: ScheduledSamplerParams,
    instrument_fingerprint: u64,
) {
    unsafe {
        dispatch_effect_chain_for_track(data.lg.0, &effect_params);
    }
    fire_resolved(
        data,
        track_idx,
        0,
        samples_per_step as f64,
        resolved,
        chord,
        instrument_params,
        instrument_fingerprint,
        Some(sampler_params),
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
        ScheduledEventKind::InstrumentParams {
            track,
            instrument_params,
        } => {
            dispatch_instrument_params_to_active_voices(data, track, &instrument_params);
        }
        ScheduledEventKind::EffectParams { effect_params, .. } => unsafe {
            dispatch_effect_chain_for_track(data.lg.0, &effect_params);
        },
        ScheduledEventKind::NetworkTrigger {
            track,
            samples_per_step,
            resolved,
            chord,
            effect_params,
            instrument_params,
            sampler_params,
            instrument_fingerprint,
            ..
        } => {
            dispatch_scheduled_network_step(
                data,
                track,
                samples_per_step,
                resolved,
                chord,
                effect_params,
                instrument_params,
                sampler_params,
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
    publish_sampler_modulator_activity(data);
    unsafe {
        data.lg
            .process_next_block(output.as_mut_ptr(), nframes as i32);
    }
}

fn publish_sampler_modulator_activity(data: &AudioCallbackData) {
    for (track_idx, pool) in data.voice_pools.iter().enumerate().take(MAX_TRACKS) {
        let mut mask = 0u64;
        for voice_idx in 0..pool.num_voices.min(MAX_VOICES) {
            if pool.voices[voice_idx].active {
                mask |= 1u64 << voice_idx;
            }
        }
        crate::voice_modulator::set_sampler_active_mask(track_idx, mask);
    }
}

fn bus_gate_state_at(
    sequence: &crate::sequencer::BusGateSequence,
    total_beats: f64,
) -> (f32, usize) {
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

fn bus_gate_target_at(sequence: &crate::sequencer::BusGateSequence, total_beats: f64) -> f32 {
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
        let mut param_indices = (0..num_params).collect::<Vec<_>>();
        param_indices.sort_by_key(|param_idx| {
            slot.param_node_indices
                .get(*param_idx)
                .copied()
                .unwrap_or(*param_idx as u32)
        });
        for param_idx in param_indices {
            let idx = slot
                .param_node_indices
                .get(param_idx)
                .copied()
                .unwrap_or(param_idx as u32);
            if idx == u32::MAX || param_idx >= slot.defaults.len() {
                continue;
            }
            let (logical_id, idx) = if idx >= crate::voice_modulator::MOD_PARAM_BASE {
                if slot.modulator_node_id == 0 {
                    continue;
                }
                (
                    slot.modulator_node_id as u64,
                    (idx - crate::voice_modulator::MOD_PARAM_BASE) as u64,
                )
            } else {
                (slot.node_id as u64, idx as u64)
            };
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
                    idx,
                    logical_id,
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

fn sync_instrument_host_clock_params(data: &mut AudioCallbackData, block_start_sample: u64) {
    let playing = data.state.transport.playing.load(Ordering::Relaxed);
    if playing && !data.host_clock_was_playing {
        data.host_clock_play_start_sample = block_start_sample;
    }
    if !playing && data.host_clock_was_playing {
        data.host_clock_play_start_sample = block_start_sample;
    }
    data.host_clock_was_playing = playing;

    let (phase, increment) = if playing {
        let bpm = data.state.transport.bpm.load(Ordering::Relaxed).max(1) as f64;
        let samples_per_bar = data.sample_rate * 240.0 / bpm;
        let elapsed_samples = block_start_sample.saturating_sub(data.host_clock_play_start_sample);
        (
            (elapsed_samples as f64 / samples_per_bar).fract() as f32,
            (1.0 / samples_per_bar) as f32,
        )
    } else {
        (0.0, 0.0)
    };

    for engine_id in 0..data.state.runtime.engine_voice_counts.len() {
        let voice_count =
            data.state.runtime.engine_voice_counts[engine_id].load(Ordering::Acquire) as usize;
        for voice_idx in 0..voice_count.min(MAX_VOICES) {
            let lid =
                data.state.runtime.engine_voice_lids[engine_id][voice_idx].load(Ordering::Acquire);
            if lid == 0 {
                continue;
            }
            unsafe {
                params_push_wrapper(
                    data.lg.0,
                    ParamMsg {
                        idx: gatepitch::PARAM_CLOCK_PHASE,
                        logical_id: lid,
                        fvalue: phase,
                    },
                );
                params_push_wrapper(
                    data.lg.0,
                    ParamMsg {
                        idx: gatepitch::PARAM_CLOCK_INC,
                        logical_id: lid,
                        fvalue: increment,
                    },
                );
            }
        }
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
    instrument_params: ScheduledInstrumentParams,
    instrument_fingerprint: u64,
    scheduled_sampler_params: Option<ScheduledSamplerParams>,
) {
    if !track_accepts_scheduled_trigger(&data.state, track_idx) {
        return;
    }
    let tp = &data.state.pattern.track_params[track_idx];
    let instrument_type = InstrumentType::from_runtime_flag(
        data.state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed),
    );
    let is_custom = instrument_type == InstrumentType::Custom;
    let is_modulator = instrument_type == InstrumentType::Modulator;
    let sampler_lid = data.state.runtime.sampler_lids[track_idx].load(Ordering::Acquire);
    if !is_custom && !is_modulator && sampler_lid == 0 {
        return;
    }

    let chop = resolved.chop.round() as u32;
    let chop = chop.max(1);

    let total_gate = (resolved.duration as f64 * samples_per_step) as f32;
    let chop_gate = total_gate / chop as f32;

    let fallback_sampler_params = || {
        let inst_slot = &data.state.pattern.instrument_slots[track_idx];
        ScheduledSamplerParams {
            attack_ms: inst_slot
                .plocks
                .get(step, 0)
                .unwrap_or_else(|| inst_slot.defaults.get(0)),
            release_ms: inst_slot
                .plocks
                .get(step, 1)
                .unwrap_or_else(|| inst_slot.defaults.get(1)),
            start_point: inst_slot
                .plocks
                .get(step, 2)
                .unwrap_or_else(|| inst_slot.defaults.get(2)),
            end_point: inst_slot
                .plocks
                .get(step, 3)
                .unwrap_or_else(|| inst_slot.defaults.get(3)),
            instrument_enabled: inst_slot
                .plocks
                .get(step, 4)
                .unwrap_or_else(|| inst_slot.defaults.get(4)),
            reverse: inst_slot
                .plocks
                .get(step, 5)
                .unwrap_or_else(|| inst_slot.defaults.get(5)),
            loop_mode: inst_slot
                .plocks
                .get(step, 6)
                .unwrap_or_else(|| inst_slot.defaults.get(6)),
            loop_xfade_ms: inst_slot
                .plocks
                .get(step, 7)
                .unwrap_or_else(|| inst_slot.defaults.get(7)),
            sr_hz: inst_slot
                .plocks
                .get(step, 8)
                .unwrap_or_else(|| inst_slot.defaults.get(8)),
            warp_enabled: inst_slot
                .plocks
                .get(step, 9)
                .unwrap_or_else(|| inst_slot.defaults.get(9)),
            warp_mode: inst_slot
                .plocks
                .get(step, 10)
                .unwrap_or_else(|| inst_slot.defaults.get(10)),
            sample_bpm: inst_slot
                .plocks
                .get(step, 11)
                .unwrap_or_else(|| inst_slot.defaults.get(11)),
            playback_speed: inst_slot
                .plocks
                .get(step, 12)
                .unwrap_or_else(|| inst_slot.defaults.get(12)),
            scrub: inst_slot
                .plocks
                .get(step, 13)
                .unwrap_or_else(|| inst_slot.defaults.get(13)),
        }
    };
    let sampler_params = scheduled_sampler_params.unwrap_or_else(fallback_sampler_params);
    let attack_ms = sampler_params.attack_ms;
    let release_ms = sampler_params.release_ms;
    let attack_samples = attack_ms * data.sample_rate as f32 / 1000.0;
    let release_samples = release_ms * data.sample_rate as f32 / 1000.0;
    let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
    let start_point = sampler_params.start_point;
    let end_point = sampler_params.end_point;
    let instrument_enabled = sampler_params.instrument_enabled;
    let reverse = sampler_params.reverse;
    let loop_mode = sampler_params.loop_mode;
    let loop_xfade_samples = sampler_params.loop_xfade_ms * data.sample_rate as f32 / 1000.0;
    let sr_hz = sampler_params.sr_hz;
    let warp_enabled = sampler_params.warp_enabled;
    let warp_mode = sampler_params.warp_mode;
    let sample_bpm = sampler_params.sample_bpm;
    let playback_speed = sampler_params.playback_speed;
    let scrub = sampler_params.scrub;
    let (
        warp_enabled,
        warp_mode,
        warp_ratio,
        warp_sample_bpm,
        warp_project_bpm,
        warp_ptr_lo,
        warp_ptr_hi,
    ) = sampler_warp_runtime(&data.state, track_idx, warp_enabled, warp_mode, sample_bpm);
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

    if is_modulator {
        let lid = data.state.runtime.modulator_lids[track_idx].load(Ordering::Acquire);
        if lid == 0 {
            return;
        }
        unsafe {
            dispatch_modulator_params(data.lg.0, lid, &instrument_params);
            trigger_modulator_pulse(data.lg.0, lid, chop_gate, resolved.velocity);
        }
        if chop > 1 {
            data.chop_state[track_idx] = ChopTracker {
                remaining: chop - 1,
                counter: chop_gate as f64,
                interval: chop_gate as f64,
                step,
                chop_gate,
            };
        } else {
            data.chop_state[track_idx].remaining = 0;
        }
        data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
        return;
    }

    // Sync polyphonic setting from track params
    let track_polyphonic = tp.is_polyphonic();
    let track_max_polyphony = tp.get_max_polyphony();
    data.voice_pools[track_idx].polyphonic = track_polyphonic;
    let engine_id = if is_custom {
        track_engine_id(&data.state, track_idx)
    } else {
        None
    };
    let free_patch = is_custom
        && track_custom_run_mode(&data.state, track_idx) == CustomInstrumentRunMode::FreePatch;

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
            let transpose =
                resolved_chord_transpose(chord.notes[n], step_transpose, resolved.transpose);
            if is_custom {
                let Some(engine_id) = engine_id else {
                    continue;
                };
                let allocation = if free_patch {
                    let Some(allocation) = data.custom_engine_pools[engine_id]
                        .allocate_free_patch_voice(track_idx, transpose)
                    else {
                        continue;
                    };
                    allocation
                } else {
                    data.custom_engine_pools[engine_id].allocate_voice(
                        track_idx,
                        transpose,
                        track_polyphonic,
                        track_max_polyphony,
                    )
                };
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
                if allocation.stole_active_voice || !track_polyphonic || free_patch {
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
                let voice =
                    data.voice_pools[track_idx].allocate_voice_retriggering_same_note(transpose);
                let voice_lid = voice.logical_id;
                let lid = if voice_lid != 0 {
                    voice_lid
                } else {
                    sampler_lid
                };
                if voice.modulator_id > 0 {
                    unsafe {
                        dispatch_sampler_modulator_params_to_voice(
                            data.lg.0,
                            voice.modulator_id as u64,
                            &instrument_params,
                        );
                        send_custom_trigger(
                            data.lg.0,
                            voice.gatepitch_id as u64,
                            custom_pitch_hz(transpose + base_note_offset, 0.0),
                            velocity,
                        );
                    }
                }
                unsafe {
                    dispatch_sampler_extra_params_to_voice(data.lg.0, lid, &instrument_params);
                    send_trigger(
                        data.lg.0,
                        lid,
                        velocity,
                        resolved.speed * playback_speed,
                        note_chop_gate,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        transpose + base_note_offset,
                        start_point,
                        end_point,
                        instrument_enabled,
                        reverse,
                        loop_mode,
                        loop_xfade_samples,
                        sr_hz,
                        warp_enabled,
                        warp_mode,
                        warp_ratio,
                        warp_sample_bpm,
                        warp_project_bpm,
                        warp_ptr_lo,
                        warp_ptr_hi,
                    );
                    params_push_wrapper(
                        data.lg.0,
                        ParamMsg {
                            idx: crate::sampler::PARAM_SCRUB_OFFSET,
                            logical_id: lid,
                            fvalue: scrub,
                        },
                    );
                }
                if gate_mode > 0.5 {
                    data.gate_off_state[track_idx].schedule(lid, note_total_gate as f64);
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
            let allocation = if free_patch {
                let Some(allocation) = data.custom_engine_pools[engine_id]
                    .allocate_free_patch_voice(track_idx, transpose)
                else {
                    return;
                };
                allocation
            } else {
                data.custom_engine_pools[engine_id].allocate_voice(
                    track_idx,
                    transpose,
                    track_polyphonic,
                    track_max_polyphony,
                )
            };
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
            if allocation.stole_active_voice || !track_polyphonic || free_patch {
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
            let voice =
                data.voice_pools[track_idx].allocate_voice_retriggering_same_note(transpose);
            let voice_lid = voice.logical_id;
            let lid = if voice_lid != 0 {
                voice_lid
            } else {
                sampler_lid
            };
            if voice.modulator_id > 0 {
                unsafe {
                    dispatch_sampler_modulator_params_to_voice(
                        data.lg.0,
                        voice.modulator_id as u64,
                        &instrument_params,
                    );
                    send_custom_trigger(
                        data.lg.0,
                        voice.gatepitch_id as u64,
                        custom_pitch_hz(transpose + base_note_offset, 0.0),
                        velocity,
                    );
                }
            }
            unsafe {
                dispatch_sampler_extra_params_to_voice(data.lg.0, lid, &instrument_params);
                send_trigger(
                    data.lg.0,
                    lid,
                    velocity,
                    resolved.speed * playback_speed,
                    chop_gate,
                    attack_samples,
                    release_samples,
                    gate_mode,
                    transpose + base_note_offset,
                    start_point,
                    end_point,
                    instrument_enabled,
                    reverse,
                    loop_mode,
                    loop_xfade_samples,
                    sr_hz,
                    warp_enabled,
                    warp_mode,
                    warp_ratio,
                    warp_sample_bpm,
                    warp_project_bpm,
                    warp_ptr_lo,
                    warp_ptr_hi,
                );
                params_push_wrapper(
                    data.lg.0,
                    ParamMsg {
                        idx: crate::sampler::PARAM_SCRUB_OFFSET,
                        logical_id: lid,
                        fvalue: scrub,
                    },
                );
            }
            if gate_mode > 0.5 {
                data.gate_off_state[track_idx].schedule(lid, total_gate as f64);
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
    sync_instrument_host_clock_params(data, block_start_sample);

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
    sync_free_patch_transport_routes(data, num_tracks);

    // Process keyboard triggers
    let mut processed_keyboard_trigger = false;
    while let Ok(kt) = data.keyboard_rx.try_recv() {
        processed_keyboard_trigger = true;
        if kt.track >= num_tracks {
            continue;
        }
        let is_custom =
            data.state.runtime.instrument_type_flags[kt.track].load(Ordering::Relaxed) == 1;
        let track_polyphonic = data.state.pattern.track_params[kt.track].is_polyphonic();
        let track_max_polyphony = data.state.pattern.track_params[kt.track].get_max_polyphony();
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
                if track_custom_run_mode(&data.state, kt.track)
                    == CustomInstrumentRunMode::FreePatch
                {
                    pool.release_free_patch_voice_by_logical_id(active_note.logical_id);
                } else {
                    pool.release_voice_by_logical_id(active_note.logical_id, block_end_sample);
                }
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
                            let gatepitch_id = pool
                                .voices
                                .iter()
                                .find(|voice| voice.logical_id == active_note.logical_id)
                                .map(|voice| voice.gatepitch_id)
                                .unwrap_or(0);
                            if gatepitch_id > 0 {
                                send_custom_note_off(data.lg.0, gatepitch_id as u64);
                            }
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
                let free_patch = track_custom_run_mode(&data.state, kt.track)
                    == CustomInstrumentRunMode::FreePatch;
                let allocation = if free_patch {
                    let Some(allocation) = data.custom_engine_pools[engine_id]
                        .allocate_free_patch_voice(kt.track, resolved_transpose)
                    else {
                        continue;
                    };
                    allocation
                } else {
                    data.custom_engine_pools[engine_id].allocate_voice(
                        kt.track,
                        resolved_transpose,
                        track_polyphonic,
                        track_max_polyphony,
                    )
                };
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
                if allocation.stole_active_voice || !track_polyphonic || free_patch {
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
                let voice = data.voice_pools[kt.track]
                    .allocate_voice_retriggering_same_note(resolved_transpose);
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
                let kb_enabled = kb_inst_slot.defaults.get(4);
                let kb_reverse = kb_inst_slot.defaults.get(5);
                let kb_loop_mode = kb_inst_slot.defaults.get(6);
                let kb_loop_xfade_samples =
                    kb_inst_slot.defaults.get(7) * data.sample_rate as f32 / 1000.0;
                let kb_sr_hz = kb_inst_slot.defaults.get(8);
                let kb_playback_speed = kb_inst_slot.defaults.get(12);
                let (
                    kb_warp_enabled,
                    kb_warp_mode,
                    kb_warp_ratio,
                    kb_warp_sample_bpm,
                    kb_warp_project_bpm,
                    kb_warp_ptr_lo,
                    kb_warp_ptr_hi,
                ) = sampler_warp_runtime(
                    &data.state,
                    kt.track,
                    kb_inst_slot.defaults.get(9),
                    kb_inst_slot.defaults.get(10),
                    kb_inst_slot.defaults.get(11),
                );
                if voice.modulator_id > 0 {
                    unsafe {
                        dispatch_sampler_modulator_defaults_to_voice(
                            data.lg.0,
                            &data.state,
                            kt.track,
                            voice.modulator_id as u64,
                        );
                        send_custom_trigger(
                            data.lg.0,
                            voice.gatepitch_id as u64,
                            custom_pitch_hz(resolved_transpose + base_note_offset, 0.0),
                            kt.velocity,
                        );
                    }
                }
                unsafe {
                    send_keyboard_trigger(
                        data.lg.0,
                        voice_lid,
                        resolved_transpose + base_note_offset,
                        kt.velocity,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        kb_start,
                        kb_end,
                        kb_enabled,
                        kb_reverse,
                        kb_loop_mode,
                        kb_loop_xfade_samples,
                        kb_sr_hz,
                        kb_warp_enabled,
                        kb_warp_mode,
                        kb_warp_ratio,
                        kb_warp_sample_bpm,
                        kb_warp_project_bpm,
                        kb_warp_ptr_lo,
                        kb_warp_ptr_hi,
                    );
                    params_push_wrapper(
                        data.lg.0,
                        ParamMsg {
                            idx: crate::sampler::PARAM_SPEED,
                            logical_id: voice_lid,
                            fvalue: kb_playback_speed,
                        },
                    );
                    dispatch_sampler_extra_defaults_to_voice(
                        data.lg.0,
                        &data.state,
                        kt.track,
                        voice_lid,
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
    if processed_keyboard_trigger {
        sync_free_patch_transport_routes(data, num_tracks);
    }

    // Schedule accumulator reset on play-start or pattern change; consumed at next step 0.
    {
        let playing = data.state.transport.playing.load(Ordering::Relaxed);
        let pattern = data.state.current_scene_index() as u32;
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

    // Push BPM to per-voice modulators when it changes. Track Filter/Delay
    // inserts are descriptor-managed on the control side.
    let bpm = data.state.transport.bpm.load(Ordering::Relaxed);
    if bpm != data.last_bpm {
        data.last_bpm = bpm;
        let bpm_f = bpm as f32;
        for engine in &data.state.runtime.engine_modulator_node_ids {
            for node in engine {
                let logical_id = node.load(Ordering::Relaxed);
                if logical_id != 0 {
                    unsafe {
                        dispatch_voice_modulator_transport(data.lg.0, logical_id as u64, bpm_f);
                    }
                }
            }
        }
        for pool in &data.voice_pools {
            for voice in pool.voices.iter().take(pool.num_voices) {
                if voice.modulator_id > 0 {
                    unsafe {
                        dispatch_voice_modulator_transport(
                            data.lg.0,
                            voice.modulator_id as u64,
                            bpm_f,
                        );
                    }
                }
                if voice.logical_id != 0 {
                    unsafe {
                        params_push_wrapper(
                            data.lg.0,
                            ParamMsg {
                                idx: PARAM_WARP_PROJECT_BPM,
                                logical_id: voice.logical_id,
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
            if InstrumentType::from_runtime_flag(
                data.state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed),
            ) == InstrumentType::Modulator
            {
                let lid = data.state.runtime.modulator_lids[track_idx].load(Ordering::Acquire);
                let slot = &data.state.pattern.instrument_slots[track_idx];
                while cs.counter <= 0.0 && cs.remaining > 0 {
                    if lid != 0 {
                        let rise = slot
                            .plocks
                            .get(cs.step, 0)
                            .unwrap_or_else(|| slot.defaults.get(0));
                        let fall = slot
                            .plocks
                            .get(cs.step, 1)
                            .unwrap_or_else(|| slot.defaults.get(1));
                        unsafe {
                            params_push_wrapper(
                                data.lg.0,
                                ParamMsg {
                                    idx: crate::track_modulator::PARAM_RISE_MS,
                                    logical_id: lid,
                                    fvalue: rise,
                                },
                            );
                            params_push_wrapper(
                                data.lg.0,
                                ParamMsg {
                                    idx: crate::track_modulator::PARAM_FALL_MS,
                                    logical_id: lid,
                                    fvalue: fall,
                                },
                            );
                            let velocity = data.state.pattern.step_data[track_idx]
                                .get(cs.step, StepParam::Velocity);
                            trigger_modulator_pulse(data.lg.0, lid, cs.chop_gate, velocity);
                        }
                        data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
                    }
                    cs.remaining -= 1;
                    cs.counter += cs.interval;
                }
                continue;
            }
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
            let chop_reverse = chop_inst_slot
                .plocks
                .get(cs.step, 5)
                .unwrap_or_else(|| chop_inst_slot.defaults.get(5));
            let chop_loop_mode = chop_inst_slot
                .plocks
                .get(cs.step, 6)
                .unwrap_or_else(|| chop_inst_slot.defaults.get(6));
            let chop_loop_xfade_samples = chop_inst_slot
                .plocks
                .get(cs.step, 7)
                .unwrap_or_else(|| chop_inst_slot.defaults.get(7))
                * data.sample_rate as f32
                / 1000.0;
            let chop_sr_hz = chop_inst_slot
                .plocks
                .get(cs.step, 8)
                .unwrap_or_else(|| chop_inst_slot.defaults.get(8));
            let chop_warp_enabled = chop_inst_slot
                .plocks
                .get(cs.step, 9)
                .unwrap_or_else(|| chop_inst_slot.defaults.get(9));
            let chop_warp_mode = chop_inst_slot
                .plocks
                .get(cs.step, 10)
                .unwrap_or_else(|| chop_inst_slot.defaults.get(10));
            let chop_sample_bpm = chop_inst_slot
                .plocks
                .get(cs.step, 11)
                .unwrap_or_else(|| chop_inst_slot.defaults.get(11));
            let chop_playback_speed = chop_inst_slot
                .plocks
                .get(cs.step, 12)
                .unwrap_or_else(|| chop_inst_slot.defaults.get(12));
            let (
                chop_warp_enabled,
                chop_warp_mode,
                chop_warp_ratio,
                chop_warp_sample_bpm,
                chop_warp_project_bpm,
                chop_warp_ptr_lo,
                chop_warp_ptr_hi,
            ) = sampler_warp_runtime(
                &data.state,
                track_idx,
                chop_warp_enabled,
                chop_warp_mode,
                chop_sample_bpm,
            );
            let chop_base_note_offset = f32::from_bits(
                data.state.pattern.instrument_base_note_offsets[track_idx].load(Ordering::Relaxed),
            );
            let sd = &data.state.pattern.step_data[track_idx];
            while cs.counter <= 0.0 && cs.remaining > 0 {
                // Allocate a voice for the chop re-trigger
                let transpose = sd.get(cs.step, StepParam::Transpose);
                let voice =
                    data.voice_pools[track_idx].allocate_voice_retriggering_same_note(transpose);
                let voice_lid = voice.logical_id;
                let sampler_lid =
                    data.state.runtime.sampler_lids[track_idx].load(Ordering::Acquire);
                let lid = if voice_lid != 0 {
                    voice_lid
                } else {
                    sampler_lid
                };
                if voice.modulator_id > 0 {
                    unsafe {
                        dispatch_sampler_modulator_defaults_to_voice(
                            data.lg.0,
                            &data.state,
                            track_idx,
                            voice.modulator_id as u64,
                        );
                        send_custom_trigger(
                            data.lg.0,
                            voice.gatepitch_id as u64,
                            custom_pitch_hz(transpose + chop_base_note_offset, 0.0),
                            sd.get(cs.step, StepParam::Velocity),
                        );
                    }
                }
                unsafe {
                    dispatch_sampler_extra_defaults_to_voice(
                        data.lg.0,
                        &data.state,
                        track_idx,
                        lid,
                    );
                    send_trigger(
                        data.lg.0,
                        lid,
                        sd.get(cs.step, StepParam::Velocity),
                        sd.get(cs.step, StepParam::Speed) * chop_playback_speed,
                        cs.chop_gate,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        transpose + chop_base_note_offset,
                        chop_start,
                        chop_end,
                        chop_inst_slot
                            .plocks
                            .get(cs.step, 4)
                            .unwrap_or_else(|| chop_inst_slot.defaults.get(4)),
                        chop_reverse,
                        chop_loop_mode,
                        chop_loop_xfade_samples,
                        chop_sr_hz,
                        chop_warp_enabled,
                        chop_warp_mode,
                        chop_warp_ratio,
                        chop_warp_sample_bpm,
                        chop_warp_project_bpm,
                        chop_warp_ptr_lo,
                        chop_warp_ptr_hi,
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
            if InstrumentType::from_runtime_flag(
                data.state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed),
            ) == InstrumentType::Modulator
            {
                unsafe {
                    set_modulator_gate(data.lg.0, lid, 0.0);
                }
            } else if let Some(engine_id) = track_engine_id(&data.state, track_idx) {
                if track_custom_run_mode(&data.state, track_idx)
                    == CustomInstrumentRunMode::FreePatch
                {
                    data.custom_engine_pools[engine_id].release_free_patch_voice_by_logical_id(lid);
                } else {
                    data.custom_engine_pools[engine_id]
                        .release_voice_by_logical_id(lid, block_end_sample);
                }
                unsafe {
                    send_custom_note_off(data.lg.0, lid);
                }
            } else {
                if let Some(voice) = data.voice_pools[track_idx].voices
                    [..data.voice_pools[track_idx].num_voices]
                    .iter()
                    .find(|voice| voice.logical_id == lid)
                {
                    if voice.gatepitch_id > 0 {
                        unsafe {
                            send_custom_note_off(data.lg.0, voice.gatepitch_id as u64);
                        }
                    }
                }
                data.voice_pools[track_idx].release_voice_by_logical_id(lid);
            }
        }
    }
    let custom_release_tail_samples =
        (CUSTOM_ENGINE_RELEASE_TAIL_SECONDS * data.sample_rate).round() as u64;
    for engine_id in 0..MAX_TRACKS {
        if data.state.runtime.engine_voice_counts[engine_id].load(Ordering::Acquire) == 0 {
            continue;
        }
        data.custom_engine_pools[engine_id].shrink_released_voices(
            engine_id,
            block_end_sample,
            custom_release_tail_samples,
        );
    }

    let mut rendered_frames = 0usize;
    let mut zero_chunk_spins = 0usize;
    const RENDER_CHUNK_ALIGNMENT: u64 = 4;
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

        let dispatch_horizon =
            (current_sample + (RENDER_CHUNK_ALIGNMENT - 1)).min(block_end_sample);
        while let Some(std::cmp::Reverse(event)) = data.events_heap.peek() {
            if event.event.pattern_epoch != current_pattern_epoch {
                let _ = data.events_heap.pop();
                continue;
            }
            if event.sample_time > dispatch_horizon {
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
        let mut chunk_frames = next_sample.saturating_sub(current_sample);
        chunk_frames -= chunk_frames % RENDER_CHUNK_ALIGNMENT;
        let chunk_frames = chunk_frames as usize;
        if chunk_frames == 0 {
            if block_end_sample.saturating_sub(current_sample) < RENDER_CHUNK_ALIGNMENT {
                zero_output_frames(output, rendered_frames, data.num_channels);
                break;
            }
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
            let mod_stats = crate::voice_modulator::take_process_stats();
            if mod_stats.calls > 0 {
                eprintln!(
                    "audio-trace: modulator-stats calls={} rendered={} disabled_custom={} disabled_sampler={} all_slots_off={} unbound_rendered={} rendered_frames={} disabled_frames={} all_slots_off_frames={}",
                    mod_stats.calls,
                    mod_stats.rendered_calls,
                    mod_stats.disabled_custom_skips,
                    mod_stats.disabled_sampler_skips,
                    mod_stats.all_slots_off_calls,
                    mod_stats.unbound_rendered_calls,
                    mod_stats.rendered_frames,
                    mod_stats.disabled_frames,
                    mod_stats.all_slots_off_frames,
                );
                for stats in mod_stats.engines {
                    eprintln!(
                        "audio-trace: modulator-engine engine={} enabled={} calls={} rendered={} disabled={} rendered_frames={} disabled_frames={}",
                        stats.engine_id,
                        stats.enabled_voices,
                        stats.calls,
                        stats.rendered_calls,
                        stats.disabled_skips,
                        stats.rendered_frames,
                        stats.disabled_frames,
                    );
                }
                for stats in mod_stats.sampler_tracks {
                    eprintln!(
                        "audio-trace: modulator-sampler track={} active_mask=0x{:03x} calls={} rendered={} disabled={} rendered_frames={} disabled_frames={}",
                        stats.track_idx,
                        stats.active_mask,
                        stats.calls,
                        stats.rendered_calls,
                        stats.disabled_skips,
                        stats.rendered_frames,
                        stats.disabled_frames,
                    );
                }
            }
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
    crate::voice_modulator::set_process_stats_enabled(trace_audio);
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
        host_clock_was_playing: false,
        host_clock_play_start_sample: 0,
        free_patch_transport_routes: [FreePatchTransportRouteState::default(); MAX_TRACKS],
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
        events_heap: BinaryHeap::with_capacity(4096),
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

/// Query the default output device, preserving the system sample rate when possible.
pub fn query_device_config() -> Result<(u32, u16), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No output device available")?;
    let default_config = device
        .default_output_config()
        .map_err(|e| format!("Failed to get default config: {e}"))?;
    let ranges: Vec<OutputFormatRange> = device
        .supported_output_configs()
        .map_err(|e| format!("Failed to query supported output configs: {e}"))?
        .map(|range| OutputFormatRange {
            channels: range.channels(),
            min_sample_rate: range.min_sample_rate().0,
            max_sample_rate: range.max_sample_rate().0,
            supports_f32: range.sample_format() == cpal::SampleFormat::F32,
        })
        .collect();
    let selected = select_output_config(
        default_config.sample_rate().0,
        default_config.channels(),
        ranges,
    )
    .ok_or_else(|| {
        let device_name = device
            .name()
            .unwrap_or_else(|_| "default output device".to_string());
        format!(
            "{device_name} does not support f32 output at either {} Hz or its default {} Hz rate",
            FALLBACK_SAMPLE_RATE,
            default_config.sample_rate().0
        )
    })?;

    Ok((selected.sample_rate, selected.channels))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    use super::{
        bus_gate_target_at, free_patch_transport_route_cache_is_fresh,
        free_patch_transport_route_target, instrument_sound_fingerprint,
        resolve_live_keyboard_transpose, resolved_chord_transpose, sampler_warp_runtime,
        select_output_channels, select_output_config, swing_delay_samples,
        track_accepts_scheduled_trigger, CustomEnginePool, FreePatchTransportRouteState,
        FreePatchTransportRouteTarget, GateOffTracker, OutputDeviceConfig, OutputFormatRange,
        FALLBACK_SAMPLE_RATE,
    };
    use crate::accumulator::AccumulatorRuntimeState;
    use crate::analysis::{pack_ptr, OnsetTableShared};
    use crate::sequencer::{
        CustomInstrumentRunMode, InstrumentType, SequencerState, SwingResolution,
    };

    #[test]
    fn output_config_prefers_system_default_sample_rate_over_44100() {
        let ranges = [
            OutputFormatRange {
                channels: 2,
                min_sample_rate: 48_000,
                max_sample_rate: 48_000,
                supports_f32: true,
            },
            OutputFormatRange {
                channels: 4,
                min_sample_rate: 44_100,
                max_sample_rate: 96_000,
                supports_f32: true,
            },
            OutputFormatRange {
                channels: 2,
                min_sample_rate: 44_100,
                max_sample_rate: 44_100,
                supports_f32: true,
            },
        ];

        assert_eq!(
            select_output_config(48_000, 2, ranges),
            Some(OutputDeviceConfig {
                sample_rate: 48_000,
                channels: 2,
            })
        );
    }

    #[test]
    fn output_config_keeps_default_channels_at_selected_rate() {
        let ranges = [
            OutputFormatRange {
                channels: 6,
                min_sample_rate: 48_000,
                max_sample_rate: 96_000,
                supports_f32: true,
            },
            OutputFormatRange {
                channels: 2,
                min_sample_rate: 48_000,
                max_sample_rate: 96_000,
                supports_f32: true,
            },
            OutputFormatRange {
                channels: 1,
                min_sample_rate: 44_100,
                max_sample_rate: 44_100,
                supports_f32: true,
            },
        ];

        assert_eq!(select_output_channels(48_000, 6, ranges), Some(6));
    }

    #[test]
    fn output_config_prefers_stereo_when_default_channels_do_not_support_selected_rate() {
        let ranges = [
            OutputFormatRange {
                channels: 6,
                min_sample_rate: 44_100,
                max_sample_rate: 44_100,
                supports_f32: true,
            },
            OutputFormatRange {
                channels: 1,
                min_sample_rate: 48_000,
                max_sample_rate: 48_000,
                supports_f32: true,
            },
            OutputFormatRange {
                channels: 2,
                min_sample_rate: 48_000,
                max_sample_rate: 96_000,
                supports_f32: true,
            },
        ];

        assert_eq!(select_output_channels(48_000, 6, ranges), Some(2));
    }

    #[test]
    fn output_config_uses_default_sample_rate_without_44100() {
        let ranges = [
            OutputFormatRange {
                channels: 2,
                min_sample_rate: 48_000,
                max_sample_rate: 48_000,
                supports_f32: true,
            },
            OutputFormatRange {
                channels: 2,
                min_sample_rate: 88_200,
                max_sample_rate: 96_000,
                supports_f32: true,
            },
        ];

        assert_eq!(
            select_output_config(48_000, 2, ranges),
            Some(OutputDeviceConfig {
                sample_rate: 48_000,
                channels: 2,
            })
        );
    }

    #[test]
    fn output_config_uses_default_sample_rate_when_44100_lacks_f32() {
        let ranges = [
            OutputFormatRange {
                channels: 2,
                min_sample_rate: 44_100,
                max_sample_rate: 44_100,
                supports_f32: false,
            },
            OutputFormatRange {
                channels: 2,
                min_sample_rate: 48_000,
                max_sample_rate: 48_000,
                supports_f32: true,
            },
        ];

        assert_eq!(
            select_output_config(48_000, 2, ranges),
            Some(OutputDeviceConfig {
                sample_rate: 48_000,
                channels: 2,
            })
        );
    }

    #[test]
    fn output_config_falls_back_to_44100_when_default_rate_lacks_f32() {
        let ranges = [
            OutputFormatRange {
                channels: 2,
                min_sample_rate: 48_000,
                max_sample_rate: 48_000,
                supports_f32: false,
            },
            OutputFormatRange {
                channels: 2,
                min_sample_rate: FALLBACK_SAMPLE_RATE,
                max_sample_rate: FALLBACK_SAMPLE_RATE,
                supports_f32: true,
            },
        ];

        assert_eq!(
            select_output_config(48_000, 2, ranges),
            Some(OutputDeviceConfig {
                sample_rate: FALLBACK_SAMPLE_RATE,
                channels: 2,
            })
        );
    }

    #[test]
    fn output_config_rejects_when_default_and_fallback_rates_lack_f32_support() {
        let ranges = [
            OutputFormatRange {
                channels: 2,
                min_sample_rate: 44_100,
                max_sample_rate: 44_100,
                supports_f32: false,
            },
            OutputFormatRange {
                channels: 2,
                min_sample_rate: 48_000,
                max_sample_rate: 48_000,
                supports_f32: false,
            },
        ];

        assert_eq!(select_output_config(48_000, 2, ranges), None);
    }

    #[test]
    fn sampler_warp_ratio_speeds_source_when_project_bpm_is_higher() {
        let state = SequencerState::new(1, Vec::new());
        state.transport.bpm.store(160, Ordering::Relaxed);
        state.runtime.sampler_analysis_status[0].store(2, Ordering::Relaxed);
        let table = OnsetTableShared {
            onsets_frames: vec![0, 22_050],
            sample_len_frames: 44_100,
        };
        let (lo, hi) = pack_ptr(&table as *const OnsetTableShared);
        state.runtime.sampler_onset_ptr_lo[0].store(lo.to_bits(), Ordering::Relaxed);
        state.runtime.sampler_onset_ptr_hi[0].store(hi.to_bits(), Ordering::Relaxed);

        let (_, _, ratio, sample_bpm, project_bpm, _, _) =
            sampler_warp_runtime(&state, 0, 1.0, 0.0, 120.0);

        assert!((ratio - (160.0 / 120.0)).abs() < 0.0001);
        assert!((sample_bpm - 120.0).abs() < 0.0001);
        assert!((project_bpm - 160.0).abs() < 0.0001);
    }

    #[test]
    fn custom_engine_pool_reuses_inactive_same_track_voices_before_expanding() {
        let mut pool = CustomEnginePool::new();
        for lid in 1..=4 {
            pool.add_voice(lid);
        }

        let a = pool.allocate_voice(0, 0.0, true, 6);
        let b = pool.allocate_voice(0, 4.0, true, 6);
        assert_eq!(a.logical_id, 1);
        assert_eq!(b.logical_id, 2);
        assert!(!a.stole_active_voice);
        assert!(!b.stole_active_voice);

        pool.release_voice_by_logical_id(a.logical_id, 0);
        pool.release_voice_by_logical_id(b.logical_id, 0);
        pool.shrink_released_voices(0, 1_000, 1_000);

        let c = pool.allocate_voice(0, 7.0, true, 6);
        let d = pool.allocate_voice(0, 11.0, true, 6);
        assert_eq!(c.logical_id, 1);
        assert_eq!(d.logical_id, 2);
        assert!(!c.stole_active_voice);
        assert!(!d.stole_active_voice);
    }

    #[test]
    fn custom_engine_pool_uses_unassigned_voice_for_new_same_track_overlap() {
        let mut pool = CustomEnginePool::new();
        for lid in 1..=4 {
            pool.add_voice(lid);
        }

        let a = pool.allocate_voice(0, 0.0, true, 6);
        let b = pool.allocate_voice(0, 4.0, true, 6);

        assert_eq!(a.logical_id, 1);
        assert_eq!(b.logical_id, 2);
        assert!(!a.stole_active_voice);
        assert!(!b.stole_active_voice);
    }

    #[test]
    fn custom_engine_pool_enforces_track_polyphony_cap_as_segment() {
        let mut pool = CustomEnginePool::new();
        for lid in 1..=6 {
            pool.add_voice(lid);
        }

        let a0 = pool.allocate_voice(0, 0.0, true, 2);
        let a1 = pool.allocate_voice(0, 4.0, true, 2);
        let b0 = pool.allocate_voice(1, 0.0, true, 2);
        let b1 = pool.allocate_voice(1, 4.0, true, 2);

        assert_eq!(a0.logical_id, 1);
        assert_eq!(a1.logical_id, 2);
        assert_eq!(b0.logical_id, 3);
        assert_eq!(b1.logical_id, 4);

        let capped = pool.allocate_voice(0, 7.0, true, 2);
        assert!(capped.stole_active_voice);
        assert_eq!(capped.previous_track, Some(0));
        assert!(matches!(capped.logical_id, 1 | 2));

        let track_one_count = pool.voices[..pool.num_voices]
            .iter()
            .filter(|voice| voice.assigned_track == Some(1))
            .count();
        assert_eq!(track_one_count, 2);
    }

    #[test]
    fn custom_engine_pool_reuses_releasing_same_note_before_expanding() {
        let mut pool = CustomEnginePool::new();
        for lid in 1..=6 {
            pool.add_voice(lid);
        }

        let low = pool.allocate_voice(0, 0.0, true, 6);
        let low_lid = low.logical_id;
        pool.release_voice_by_logical_id(low_lid, 1_000);

        let retriggered = pool.allocate_voice(0, 0.0, true, 6);

        assert_eq!(low_lid, 1);
        assert_eq!(retriggered.logical_id, low_lid);
        assert!(!retriggered.stole_active_voice);
        assert_eq!(pool.voices[0].release_started_sample, None);
    }

    #[test]
    fn custom_engine_pool_uses_unassigned_voice_for_different_note_before_cap() {
        let mut pool = CustomEnginePool::new();
        for lid in 1..=6 {
            pool.add_voice(lid);
        }

        let low = pool.allocate_voice(0, -24.0, true, 6);
        let low_lid = low.logical_id;
        pool.release_voice_by_logical_id(low_lid, 1_000);

        let mid = pool.allocate_voice(0, 0.0, true, 6);
        let high = pool.allocate_voice(0, 7.0, true, 6);

        assert_eq!(low_lid, 1);
        assert_eq!(mid.logical_id, 2);
        assert_eq!(high.logical_id, 3);
        assert!(!mid.stole_active_voice);
        assert!(!high.stole_active_voice);
        assert_eq!(pool.voices[0].release_started_sample, Some(1_000));
    }

    #[test]
    fn custom_engine_pool_shrinks_only_after_release_tail_expires() {
        let mut pool = CustomEnginePool::new();
        for lid in 1..=4 {
            pool.add_voice(lid);
        }

        let low = pool.allocate_voice(0, 0.0, true, 6);
        let high = pool.allocate_voice(0, 7.0, true, 6);
        let low_lid = low.logical_id;
        let high_lid = high.logical_id;
        pool.enabled_voice_count = 2;

        pool.release_voice_by_logical_id(high_lid, 1_000);
        pool.shrink_released_voices(0, 1_999, 1_000);
        assert_eq!(pool.enabled_voice_count, 2);

        pool.shrink_released_voices(0, 2_000, 1_000);
        assert_eq!(pool.enabled_voice_count, 1);
        assert!(pool.voices[0].active);
        assert_eq!(pool.voices[0].logical_id, low_lid);
    }

    #[test]
    fn custom_engine_pool_steals_same_tracks_active_voice_first() {
        let mut pool = CustomEnginePool::new();
        for lid in 1..=2 {
            pool.add_voice(lid);
        }

        let first = pool.allocate_voice(0, 0.0, true, 6);
        let second = pool.allocate_voice(1, 4.0, true, 6);
        assert_eq!(first.logical_id, 1);
        assert_eq!(second.logical_id, 2);

        let stolen = pool.allocate_voice(1, 7.0, true, 6);
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

        let first = pool.allocate_voice(3, 0.0, false, 6);
        let reused = pool.allocate_voice(3, 12.0, false, 6);

        assert_eq!(reused.logical_id, first.logical_id);
        assert!(reused.stole_active_voice);
        assert_eq!(reused.previous_track, Some(3));
    }

    #[test]
    fn custom_engine_pool_free_patch_always_targets_voice_zero_without_release_tail() {
        let mut pool = CustomEnginePool::new();
        for lid in 10..=12 {
            pool.add_voice(lid);
        }

        let first = pool.allocate_free_patch_voice(2, 0.0).unwrap();
        let second = pool.allocate_free_patch_voice(2, 7.0).unwrap();

        assert_eq!(first.voice_idx, 0);
        assert_eq!(first.logical_id, 10);
        assert_eq!(second.voice_idx, 0);
        assert_eq!(second.logical_id, 10);
        assert!(second.stole_active_voice);

        pool.release_free_patch_voice_by_logical_id(second.logical_id);

        assert!(!pool.voices[0].active);
        assert_eq!(pool.voices[0].assigned_track, Some(2));
        assert_eq!(pool.voices[0].release_started_sample, None);
    }

    #[test]
    fn free_patch_transport_route_target_only_matches_custom_free_patch_tracks() {
        let state = SequencerState::new(3, Vec::new());
        state.runtime.instrument_type_flags[0]
            .store(InstrumentType::Custom.runtime_flag(), Ordering::Relaxed);
        state.runtime.instrument_run_mode_flags[0].store(
            CustomInstrumentRunMode::Instrument.runtime_flag(),
            Ordering::Relaxed,
        );
        state.runtime.track_engine_ids[0].store(10, Ordering::Relaxed);

        state.runtime.instrument_type_flags[1]
            .store(InstrumentType::Custom.runtime_flag(), Ordering::Relaxed);
        state.runtime.instrument_run_mode_flags[1].store(
            CustomInstrumentRunMode::FreePatch.runtime_flag(),
            Ordering::Relaxed,
        );
        state.runtime.track_engine_ids[1].store(11, Ordering::Relaxed);

        state.runtime.instrument_type_flags[2]
            .store(InstrumentType::Sampler.runtime_flag(), Ordering::Relaxed);
        state.runtime.instrument_run_mode_flags[2].store(
            CustomInstrumentRunMode::FreePatch.runtime_flag(),
            Ordering::Relaxed,
        );
        state.runtime.track_engine_ids[2].store(12, Ordering::Relaxed);

        assert_eq!(free_patch_transport_route_target(&state, 0, 3, true), None);
        assert_eq!(free_patch_transport_route_target(&state, 2, 3, true), None);

        let target = free_patch_transport_route_target(&state, 1, 3, true)
            .expect("custom free-patch track should produce a route target");
        assert_eq!(target.engine_id, 11);
        assert!(target.open);

        let stopped_target = free_patch_transport_route_target(&state, 1, 3, false)
            .expect("custom free-patch track should still be tracked while stopped");
        assert_eq!(stopped_target.engine_id, 11);
        assert!(!stopped_target.open);
        assert_eq!(stopped_target.route_hash, target.route_hash);
    }

    #[test]
    fn free_patch_transport_route_target_hash_changes_when_route_nodes_change() {
        let state = SequencerState::new(2, Vec::new());
        state.runtime.instrument_type_flags[0]
            .store(InstrumentType::Custom.runtime_flag(), Ordering::Relaxed);
        state.runtime.instrument_run_mode_flags[0].store(
            CustomInstrumentRunMode::FreePatch.runtime_flag(),
            Ordering::Relaxed,
        );
        state.runtime.track_engine_ids[0].store(4, Ordering::Relaxed);
        state.runtime.engine_route_lids[4][0][0].store(100, Ordering::Relaxed);
        state.runtime.engine_route_lids_r[4][0][0].store(101, Ordering::Relaxed);

        let before = free_patch_transport_route_target(&state, 0, 2, false)
            .expect("route target should exist")
            .route_hash;

        state.runtime.engine_route_lids[4][0][0].store(200, Ordering::Relaxed);
        let after = free_patch_transport_route_target(&state, 0, 2, false)
            .expect("route target should exist after route-node change")
            .route_hash;

        assert_ne!(before, after);
    }

    #[test]
    fn free_patch_transport_route_cache_does_not_suppress_stopped_mute() {
        let cached = FreePatchTransportRouteState {
            valid: true,
            engine_id: 3,
            route_hash: 99,
            open: false,
        };
        let stopped_target = FreePatchTransportRouteTarget {
            engine_id: 3,
            route_hash: 99,
            open: false,
        };
        let playing_target = FreePatchTransportRouteTarget {
            open: true,
            ..stopped_target
        };

        assert!(!free_patch_transport_route_cache_is_fresh(
            cached,
            stopped_target
        ));
        assert!(free_patch_transport_route_cache_is_fresh(
            FreePatchTransportRouteState {
                open: true,
                ..cached
            },
            playing_target
        ));
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
        let sequence = crate::sequencer::BusGateSequence::default();
        assert_eq!(bus_gate_target_at(&sequence, 0.0), 1.0);
        assert_eq!(bus_gate_target_at(&sequence, 0.24), 1.0);
        assert_eq!(bus_gate_target_at(&sequence, 0.25), 1.0);
        assert_eq!(bus_gate_target_at(&sequence, 1.99), 1.0);
    }

    #[test]
    fn bus_gate_sequence_follows_step_activity_and_duration() {
        let mut sequence = crate::sequencer::BusGateSequence::default();
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
        let mut sequence = crate::sequencer::BusGateSequence::default();
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
        state.pattern.instrument_slots[0].set_plock(3, 1, 0.9);
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

    #[test]
    fn scheduled_triggers_respect_track_mute() {
        let state = SequencerState::new(2, Vec::new());
        state.pattern.track_params[1].set_mute(true);

        assert!(track_accepts_scheduled_trigger(&state, 0));
        assert!(!track_accepts_scheduled_trigger(&state, 1));
    }

    #[test]
    fn scheduled_triggers_respect_solo_mutes() {
        let state = SequencerState::new(3, Vec::new());
        state.pattern.track_params[0].set_solo(true);

        assert!(track_accepts_scheduled_trigger(&state, 0));
        assert!(!track_accepts_scheduled_trigger(&state, 1));
        assert!(!track_accepts_scheduled_trigger(&state, 2));
    }
}
