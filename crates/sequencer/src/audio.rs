use arrayvec::ArrayVec;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::audiograph::*;
use crate::effects::{EffectSlotSnapshot, EffectSlotState, MAX_SLOT_PARAMS};
use crate::gatepitch;
use crate::recorder::MasterRecorder;
use crate::sampler::{
    PARAM_ATTACK_SAMPLES, PARAM_LOOP_XFADE_SAMPLES, PARAM_RELEASE_SAMPLES, PARAM_WARP_PROJECT_BPM,
    SAMPLER_EVENT_AUX_ATTACK_SAMPLES, SAMPLER_EVENT_AUX_ENABLED, SAMPLER_EVENT_AUX_END_POINT,
    SAMPLER_EVENT_AUX_GATE_MODE, SAMPLER_EVENT_AUX_GATE_SAMPLES, SAMPLER_EVENT_AUX_LOOP_MODE,
    SAMPLER_EVENT_AUX_LOOP_XFADE_SAMPLES, SAMPLER_EVENT_AUX_NOTE_ON_COUNT,
    SAMPLER_EVENT_AUX_RELEASE_SAMPLES, SAMPLER_EVENT_AUX_REVERSE, SAMPLER_EVENT_AUX_SCRUB_OFFSET,
    SAMPLER_EVENT_AUX_SPEED, SAMPLER_EVENT_AUX_SR_HZ, SAMPLER_EVENT_AUX_START_POINT,
    SAMPLER_EVENT_AUX_TRANSPOSE, SAMPLER_EVENT_AUX_VELOCITY, SAMPLER_EVENT_AUX_WARP_ENABLED,
    SAMPLER_EVENT_AUX_WARP_MODE, SAMPLER_EVENT_AUX_WARP_PRESERVE,
    SAMPLER_EVENT_AUX_WARP_PROJECT_BPM, SAMPLER_EVENT_AUX_WARP_PTR_HI,
    SAMPLER_EVENT_AUX_WARP_PTR_LO, SAMPLER_EVENT_AUX_WARP_RATIO, SAMPLER_EVENT_AUX_WARP_SAMPLE_BPM,
    SAMPLER_EVENT_AUX_WARP_SEG_ENVELOPE, SAMPLER_EVENT_AUX_WARP_SEG_LOOP_MODE,
};
use crate::scheduled_event::{
    resolved_chord_transpose, ScheduledEffectParam, ScheduledEvent, ScheduledEventKind,
    ScheduledEventQueue, ScheduledInstrumentParam, ScheduledInstrumentParamTarget,
    ScheduledInstrumentParams, ScheduledInstrumentTensorParam, ScheduledInstrumentTensorParams,
    ScheduledSamplerParams,
};
use crate::sequencer::{
    rack_slot_pool_index, sync_beats, BusId, CustomInstrumentRunMode, InstrumentType,
    KeyboardTrigger, RackRouting, RackSlotParam, RackSlotSnapshot, RackTrackSnapshot,
    SequencerSnapshot, SequencerState, StepParam, SwingResolution, MAX_INSTRUMENT_ENGINES,
    MAX_RACK_SLOTS, MAX_SAMPLER_POOLS, MAX_TRACKS,
};
use crate::ui::BusGateRuntimeState;
use crate::voice::{VoicePool, MAX_VOICES};

pub const FALLBACK_SAMPLE_RATE: u32 = 44_100;
const CUSTOM_ENGINE_RELEASE_TAIL_SECONDS: f64 = 20.0;
const SCHEDULED_EVENT_QUEUE_CAPACITY: usize = 4096;
const SCHEDULED_COUNTDOWN_CAPACITY: usize =
    SCHEDULED_EVENT_QUEUE_CAPACITY + MAX_TRACKS * MAX_VOICES * 2 + MAX_TRACKS;
const SCHEDULED_BLOCK_SCRATCH_CAPACITY: usize =
    SCHEDULED_EVENT_QUEUE_CAPACITY + MAX_TRACKS * MAX_VOICES * 2 + MAX_TRACKS;

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

fn next_block_event_sequence(data: &mut AudioCallbackData) -> u32 {
    next_event_sequence_from(&mut data.event_seq)
}

fn next_event_sequence_from(event_seq: &mut u64) -> u32 {
    let seq = *event_seq as u32;
    *event_seq = event_seq.wrapping_add(1);
    seq
}

unsafe fn push_graph_block_event(
    lg: *mut LiveGraph,
    logical_id: u64,
    frame_offset: u32,
    sequence: u32,
    kind: u32,
    aux: &[f32],
) -> bool {
    let mut event = GraphBlockEvent {
        logical_id,
        frame_offset,
        sequence,
        kind,
        aux_count: aux.len().min(GBE_AUX_CAP) as u32,
        aux: [0.0; GBE_AUX_CAP],
    };
    let aux_count = event.aux_count as usize;
    event.aux[..aux_count].copy_from_slice(&aux[..aux_count]);
    push_block_event(lg, event)
}

#[derive(Clone, Copy, Debug)]
struct HostTransportClock {
    bar_phase: f32,
    bar_phase_increment: f32,
}

unsafe fn dispatch_voice_modulator_bpm(lg: *mut LiveGraph, modulator_id: u64, bpm: f32) {
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

unsafe fn dispatch_voice_modulator_transport_clock(
    lg: *mut LiveGraph,
    modulator_id: u64,
    clock: HostTransportClock,
) {
    if modulator_id == 0 {
        return;
    }
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::voice_modulator::PARAM_TRANSPORT_BAR_PHASE as u64,
            logical_id: modulator_id,
            fvalue: clock.bar_phase,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::voice_modulator::PARAM_TRANSPORT_BAR_PHASE_INC as u64,
            logical_id: modulator_id,
            fvalue: clock.bar_phase_increment,
        },
    );
}

unsafe fn dispatch_transport_phase(
    lg: *mut LiveGraph,
    logical_id: u64,
    param_idx: u32,
    beat_phase: f32,
) {
    if logical_id == 0 {
        return;
    }
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: param_idx as u64,
            logical_id,
            fvalue: beat_phase,
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

#[derive(Clone, Copy, Debug)]
enum GateOffTarget {
    Custom { engine_id: usize, free_patch: bool },
    Sampler { gatepitch_id: i32 },
}

#[derive(Clone, Copy, Debug)]
struct GateOffEvent {
    track_idx: usize,
    logical_id: u64,
    target: GateOffTarget,
}

#[derive(Clone, Copy, Debug)]
struct ChopEvent {
    track_idx: usize,
    step: usize,
    chop_gate: f32,
}

#[derive(Debug)]
enum CountdownEventKind {
    Scheduled(ScheduledEvent),
    GateOff(GateOffEvent),
    Chop(ChopEvent),
}

#[derive(Debug)]
struct CountdownEvent {
    remaining_samples: f64,
    period_samples: f64,
    repeats: u32,
    pattern_epoch: u64,
    seq: u64,
    kind: CountdownEventKind,
}

#[derive(Debug)]
enum BlockEventKind {
    Scheduled(ScheduledEvent),
    GateOff(GateOffEvent),
    Chop(ChopEvent),
}

#[derive(Debug)]
struct BlockEvent {
    frame_offset: u32,
    seq: u64,
    kind: BlockEventKind,
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

fn cancel_gate_off_for_lid(
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    lid: u64,
) {
    countdown_events.retain(|event| {
        !matches!(
            event.kind,
            CountdownEventKind::GateOff(GateOffEvent { logical_id, .. }) if logical_id == lid
        )
    });
    block_events.retain(|event| {
        !matches!(
            event.kind,
            BlockEventKind::GateOff(GateOffEvent { logical_id, .. }) if logical_id == lid
        )
    });
}

fn cancel_chops_for_track(
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    track_idx: usize,
) {
    countdown_events.retain(|event| {
        !matches!(
            event.kind,
            CountdownEventKind::Chop(ChopEvent { track_idx: event_track, .. }) if event_track == track_idx
        )
    });
    block_events.retain(|event| {
        !matches!(
            event.kind,
            BlockEventKind::Chop(ChopEvent { track_idx: event_track, .. }) if event_track == track_idx
        )
    });
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveKeyboardVoiceTarget {
    Sampler { pool_id: usize },
    Custom { engine_id: usize, free_patch: bool },
}

impl Default for ActiveKeyboardVoiceTarget {
    fn default() -> Self {
        Self::Sampler { pool_id: 0 }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ActiveKeyboardVoice {
    logical_id: u64,
    gatepitch_id: i32,
    target: ActiveKeyboardVoiceTarget,
}

#[derive(Clone, Copy, Debug)]
struct ActiveKeyboardNote {
    source_transpose: f32,
    midi_note: Option<u8>,
    voice_count: u8,
    voices: [ActiveKeyboardVoice; MAX_RACK_SLOTS],
}

impl ActiveKeyboardNote {
    fn new(
        source_transpose: f32,
        midi_note: Option<u8>,
        voices: &[ActiveKeyboardVoice],
    ) -> Option<Self> {
        if voices.is_empty() {
            return None;
        }
        let mut note = Self {
            source_transpose,
            midi_note,
            voice_count: 0,
            voices: [ActiveKeyboardVoice::default(); MAX_RACK_SLOTS],
        };
        for voice in voices.iter().take(MAX_RACK_SLOTS) {
            note.voices[note.voice_count as usize] = *voice;
            note.voice_count += 1;
        }
        Some(note)
    }

    fn voices(&self) -> &[ActiveKeyboardVoice] {
        &self.voices[..self.voice_count as usize]
    }

    fn remove_voice_by_lid(&mut self, logical_id: u64) -> bool {
        if logical_id == 0 {
            return false;
        }
        let Some(pos) = self
            .voices()
            .iter()
            .position(|voice| voice.logical_id == logical_id)
        else {
            return false;
        };
        let voice_count = self.voice_count as usize;
        for idx in pos..voice_count.saturating_sub(1) {
            self.voices[idx] = self.voices[idx + 1];
        }
        self.voice_count -= 1;
        true
    }
}

struct AudioCallbackData {
    lg: LiveGraphPtr,
    state: Arc<SequencerState>,
    num_channels: usize,
    sample_rate: f64,
    last_bpm: u32,
    last_mod_reset_counter: u32,
    voice_pools: Vec<VoicePool>,
    custom_engine_pools: Vec<CustomEnginePool>,
    scheduler_snapshot: Arc<SequencerSnapshot>,
    scheduler_snapshot_version: u64,
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
    scheduled_events: Arc<ScheduledEventQueue<SCHEDULED_EVENT_QUEUE_CAPACITY>>,
    countdown_events: Vec<CountdownEvent>,
    block_events: Vec<BlockEvent>,
    block_events_need_sort: bool,
    current_callback_nframes: usize,
    rendered_samples: Arc<AtomicU64>,
    bus_gate_runtime: Arc<Mutex<Vec<BusGateRuntimeState>>>,
    bus_gate_playheads: Arc<Mutex<Vec<(BusId, usize)>>>,
    bus_gate_clocks: Vec<BusGateClock>,
    bus_gate_was_playing: bool,
    bus_gate_play_start_sample: u64,
    dropped_scheduled_events: u64,
    late_scheduled_events: u64,
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

fn sync_rack_voice_pools(data: &mut AudioCallbackData, num_tracks: usize) {
    // Iterate the snapshot by reference instead of cloning each RackTrackSnapshot
    // (and its nested EffectSlotSnapshot/Vec fields). This runs every audio
    // callback, so a deep clone here was a real-time-thread heap-allocation
    // storm unrelated to voice count or polyphony.
    let num_tracks = num_tracks.min(data.scheduler_snapshot.tracks.len());
    for track_idx in 0..num_tracks {
        let Some(rack) = data.scheduler_snapshot.tracks[track_idx]
            .rack_track
            .as_ref()
        else {
            continue;
        };
        for (slot_idx, slot) in rack.slots.iter().enumerate() {
            match slot.instrument_type {
                InstrumentType::Sampler => {
                    let Some(pool_id) = rack_slot_pool_index(track_idx, slot_idx) else {
                        continue;
                    };
                    if pool_id < data.voice_pools.len() {
                        sync_sampler_voice_pool(
                            &data.state,
                            pool_id,
                            &mut data.voice_pools[pool_id],
                        );
                    }
                }
                InstrumentType::Custom => {
                    let Some(engine_id) = slot.track_sound_state.engine_id else {
                        continue;
                    };
                    if engine_id < data.custom_engine_pools.len() {
                        sync_custom_engine_pool(
                            &data.state,
                            engine_id,
                            &mut data.custom_engine_pools[engine_id],
                        );
                    }
                }
                InstrumentType::Modulator | InstrumentType::Rack => {}
            }
        }
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
    for engine_id in 0..data.state.runtime.engine_voice_counts.len() {
        let voice_count =
            data.state.runtime.engine_voice_counts[engine_id].load(Ordering::Acquire) as usize;
        for voice_idx in 0..voice_count.min(MAX_VOICES) {
            let lid =
                data.state.runtime.engine_voice_lids[engine_id][voice_idx].load(Ordering::Acquire);
            if lid != 0 {
                let seq = next_block_event_sequence(data);
                unsafe {
                    send_custom_note_off(data.lg.0, lid, 0, seq);
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
    data.active_keyboard_notes = [[None; MAX_VOICES]; MAX_TRACKS];
    data.pending_accum_reset = [true; MAX_TRACKS];
    data.scheduled_events.clear();
    clear_countdown_events(data);
    data.event_seq = 0;
    data.last_num_tracks = num_tracks;
    data.last_topology_epoch = data.state.transport.topology_epoch.load(Ordering::Relaxed);
    data.last_playing = false;
    data.host_clock_was_playing = false;
    data.host_clock_play_start_sample = data.rendered_samples.load(Ordering::Acquire);
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
    sync_rack_voice_pools(data, num_tracks);
}

fn publish_active_voice_counts(data: &AudioCallbackData, num_tracks: usize) {
    for track in 0..MAX_TRACKS {
        let active = if track < num_tracks {
            match InstrumentType::from_runtime_flag(
                data.state.runtime.instrument_type_flags[track].load(Ordering::Relaxed),
            ) {
                InstrumentType::Custom => track_engine_id(&data.state, track)
                    .map(|engine_id| {
                        let pool = &data.custom_engine_pools[engine_id];
                        pool.voices[..pool.num_voices]
                            .iter()
                            .filter(|voice| voice.active && voice.assigned_track == Some(track))
                            .count()
                    })
                    .unwrap_or(0),
                InstrumentType::Rack => data
                    .scheduler_snapshot
                    .tracks
                    .get(track)
                    .and_then(|track| track.rack_track.as_ref())
                    .map(|rack| {
                        rack.slots
                            .iter()
                            .enumerate()
                            .map(|(slot_idx, slot)| match slot.instrument_type {
                                InstrumentType::Sampler => rack_slot_pool_index(track, slot_idx)
                                    .and_then(|pool_id| data.voice_pools.get(pool_id))
                                    .map(|pool| {
                                        pool.voices[..pool.num_voices]
                                            .iter()
                                            .filter(|voice| voice.active)
                                            .count()
                                    })
                                    .unwrap_or(0),
                                InstrumentType::Custom => slot
                                    .track_sound_state
                                    .engine_id
                                    .and_then(|engine_id| data.custom_engine_pools.get(engine_id))
                                    .map(|pool| {
                                        pool.voices[..pool.num_voices]
                                            .iter()
                                            .filter(|voice| {
                                                voice.active && voice.assigned_track == Some(track)
                                            })
                                            .count()
                                    })
                                    .unwrap_or(0),
                                InstrumentType::Modulator | InstrumentType::Rack => 0,
                            })
                            .sum()
                    })
                    .unwrap_or(0),
                InstrumentType::Sampler | InstrumentType::Modulator => {
                    let pool = &data.voice_pools[track];
                    pool.voices[..pool.num_voices]
                        .iter()
                        .filter(|voice| voice.active)
                        .count()
                }
            }
        } else {
            0
        };
        data.state.transport.active_voice_counts[track].store(active as u32, Ordering::Relaxed);
    }
}

fn release_rack_slot_active_voices(
    data: &mut AudioCallbackData,
    track_idx: usize,
    slot_idx: usize,
    slot: &RackSlotSnapshot,
    release_sample: u64,
    frame_offset: u32,
) {
    let note_offs = collect_rack_slot_active_voice_releases(
        &mut data.voice_pools,
        &mut data.custom_engine_pools,
        &mut data.countdown_events,
        &mut data.block_events,
        track_idx,
        slot_idx,
        slot,
        release_sample,
    );
    dispatch_rack_slot_note_offs(data, frame_offset, note_offs);
}

fn release_rack_active_voices(
    data: &mut AudioCallbackData,
    track_idx: usize,
    release_sample: u64,
    frame_offset: u32,
) {
    let Some(rack) = data
        .scheduler_snapshot
        .tracks
        .get(track_idx)
        .and_then(|track| track.rack_track.clone())
    else {
        return;
    };
    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        release_rack_slot_active_voices(
            data,
            track_idx,
            slot_idx,
            slot,
            release_sample,
            frame_offset,
        );
    }
}

fn release_track_active_voices(
    data: &mut AudioCallbackData,
    track_idx: usize,
    release_sample: u64,
    frame_offset: u32,
) {
    if track_idx >= MAX_TRACKS || track_idx >= data.state.active_track_count() {
        return;
    }
    cancel_chops_for_track(
        &mut data.countdown_events,
        &mut data.block_events,
        track_idx,
    );
    data.active_keyboard_notes[track_idx] = [None; MAX_VOICES];

    let instrument_type = InstrumentType::from_runtime_flag(
        data.state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed),
    );
    if instrument_type == InstrumentType::Modulator {
        let lid = data.state.runtime.modulator_lids[track_idx].load(Ordering::Acquire);
        if lid != 0 {
            unsafe {
                set_modulator_gate(data.lg.0, lid, 0.0);
            }
        }
        return;
    }
    if instrument_type == InstrumentType::Rack {
        release_rack_active_voices(data, track_idx, release_sample, frame_offset);
        return;
    }

    if let Some(engine_id) = track_engine_id(&data.state, track_idx) {
        let free_patch =
            track_custom_run_mode(&data.state, track_idx) == CustomInstrumentRunMode::FreePatch;
        let lids: Vec<u64> = data.custom_engine_pools[engine_id].voices
            [..data.custom_engine_pools[engine_id].num_voices]
            .iter()
            .filter(|voice| voice.active && voice.assigned_track == Some(track_idx))
            .map(|voice| voice.logical_id)
            .collect();
        for lid in lids {
            if free_patch {
                data.custom_engine_pools[engine_id].release_free_patch_voice_by_logical_id(lid);
            } else {
                data.custom_engine_pools[engine_id]
                    .release_voice_by_logical_id(lid, release_sample);
            }
            cancel_gate_off_for_lid(&mut data.countdown_events, &mut data.block_events, lid);
            let seq = next_block_event_sequence(data);
            unsafe {
                send_custom_note_off(data.lg.0, lid, frame_offset, seq);
            }
        }
        return;
    }

    let active: Vec<(u64, i32)> = data.voice_pools[track_idx].voices
        [..data.voice_pools[track_idx].num_voices]
        .iter()
        .filter(|voice| voice.active && voice.logical_id != 0)
        .map(|voice| (voice.logical_id, voice.gatepitch_id))
        .collect();
    for (lid, gatepitch_id) in active {
        data.voice_pools[track_idx].release_voice_by_logical_id(lid);
        cancel_gate_off_for_lid(&mut data.countdown_events, &mut data.block_events, lid);
        let gatepitch_seq = next_block_event_sequence(data);
        let sampler_seq = next_block_event_sequence(data);
        unsafe {
            if gatepitch_id > 0 {
                send_custom_note_off(data.lg.0, gatepitch_id as u64, frame_offset, gatepitch_seq);
            }
            send_sampler_note_off(data.lg.0, lid, frame_offset, sampler_seq);
        }
    }
}

fn enforce_mute_group_for_winning_track(
    data: &mut AudioCallbackData,
    winning_track: usize,
    release_sample: u64,
    frame_offset: u32,
) {
    if winning_track >= data.state.active_track_count() {
        return;
    }
    let group = data.state.pattern.track_params[winning_track].get_mute_group();
    if group == 0 {
        return;
    }
    let num_tracks = data.state.active_track_count().min(MAX_TRACKS);
    for track_idx in 0..num_tracks {
        if track_idx == winning_track {
            continue;
        }
        if data.state.pattern.track_params[track_idx].get_mute_group() == group {
            release_track_active_voices(data, track_idx, release_sample, frame_offset);
        }
    }
}

/// Send a trigger to the sampler with the given per-step params, gate length, and explicit transpose.
unsafe fn send_trigger(
    lg: *mut LiveGraph,
    lid: u64,
    frame_offset: u32,
    sequence: u32,
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
    warp_preserve: f32,
    warp_seg_loop_mode: f32,
    warp_seg_envelope: f32,
    scrub_offset: f32,
) {
    let mut aux = [0.0f32; SAMPLER_EVENT_AUX_NOTE_ON_COUNT];
    aux[SAMPLER_EVENT_AUX_ENABLED] = enabled;
    aux[SAMPLER_EVENT_AUX_VELOCITY] = velocity;
    aux[SAMPLER_EVENT_AUX_SPEED] = speed;
    aux[SAMPLER_EVENT_AUX_GATE_SAMPLES] = gate_samples;
    aux[SAMPLER_EVENT_AUX_TRANSPOSE] = transpose;
    aux[SAMPLER_EVENT_AUX_ATTACK_SAMPLES] = attack_samples;
    aux[SAMPLER_EVENT_AUX_RELEASE_SAMPLES] = release_samples;
    aux[SAMPLER_EVENT_AUX_GATE_MODE] = gate_mode;
    aux[SAMPLER_EVENT_AUX_START_POINT] = start_point;
    aux[SAMPLER_EVENT_AUX_END_POINT] = end_point;
    aux[SAMPLER_EVENT_AUX_REVERSE] = reverse;
    aux[SAMPLER_EVENT_AUX_LOOP_MODE] = loop_mode;
    aux[SAMPLER_EVENT_AUX_LOOP_XFADE_SAMPLES] = loop_xfade_samples;
    aux[SAMPLER_EVENT_AUX_SR_HZ] = sr_hz;
    aux[SAMPLER_EVENT_AUX_WARP_ENABLED] = warp_enabled;
    aux[SAMPLER_EVENT_AUX_WARP_MODE] = warp_mode;
    aux[SAMPLER_EVENT_AUX_WARP_RATIO] = warp_ratio;
    aux[SAMPLER_EVENT_AUX_WARP_SAMPLE_BPM] = warp_sample_bpm;
    aux[SAMPLER_EVENT_AUX_WARP_PROJECT_BPM] = warp_project_bpm;
    aux[SAMPLER_EVENT_AUX_WARP_PTR_LO] = warp_ptr_lo;
    aux[SAMPLER_EVENT_AUX_WARP_PTR_HI] = warp_ptr_hi;
    aux[SAMPLER_EVENT_AUX_SCRUB_OFFSET] = scrub_offset;
    aux[SAMPLER_EVENT_AUX_WARP_PRESERVE] = warp_preserve;
    aux[SAMPLER_EVENT_AUX_WARP_SEG_LOOP_MODE] = warp_seg_loop_mode;
    aux[SAMPLER_EVENT_AUX_WARP_SEG_ENVELOPE] = warp_seg_envelope;
    push_graph_block_event(lg, lid, frame_offset, sequence, GBE_NOTE_ON, &aux);
}

/// Send a keyboard trigger directly to a voice (no step data lookup).
unsafe fn send_keyboard_trigger(
    lg: *mut LiveGraph,
    lid: u64,
    frame_offset: u32,
    sequence: u32,
    transpose: f32,
    velocity: f32,
    speed: f32,
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
    warp_preserve: f32,
    warp_seg_loop_mode: f32,
    warp_seg_envelope: f32,
    scrub_offset: f32,
) {
    send_trigger(
        lg,
        lid,
        frame_offset,
        sequence,
        velocity,
        speed,
        f32::MAX,
        attack_samples,
        release_samples,
        gate_mode,
        transpose,
        start_point,
        end_point,
        enabled,
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
        warp_preserve,
        warp_seg_loop_mode,
        warp_seg_envelope,
        scrub_offset,
    );
}

/// Send a gate-on trigger to a GatePitch node with pitch in Hz and normalized velocity.
unsafe fn send_custom_trigger(
    lg: *mut LiveGraph,
    gatepitch_lid: u64,
    frame_offset: u32,
    sequence: u32,
    pitch_hz: f32,
    velocity: f32,
) {
    push_graph_block_event(
        lg,
        gatepitch_lid,
        frame_offset,
        sequence,
        GBE_NOTE_ON,
        &[pitch_hz, velocity],
    );
}

/// Send a gate-off to a GatePitch node.
unsafe fn send_custom_note_off(
    lg: *mut LiveGraph,
    gatepitch_lid: u64,
    frame_offset: u32,
    sequence: u32,
) {
    push_graph_block_event(lg, gatepitch_lid, frame_offset, sequence, GBE_GATE_OFF, &[]);
}

unsafe fn send_sampler_note_off(
    lg: *mut LiveGraph,
    sampler_lid: u64,
    frame_offset: u32,
    sequence: u32,
) {
    push_graph_block_event(lg, sampler_lid, frame_offset, sequence, GBE_GATE_OFF, &[]);
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
    frame_offset: u32,
    sequence: u32,
    pulse_samples: f32,
    pulse_level: f32,
) {
    push_graph_block_event(
        lg,
        modulator_lid,
        frame_offset,
        sequence,
        GBE_PULSE,
        &[pulse_samples.max(1.0), pulse_level.clamp(0.0, 1.0)],
    );
}

fn schedule_gate_off_event(
    data: &mut AudioCallbackData,
    track_idx: usize,
    logical_id: u64,
    source_frame_offset: u32,
    delay_samples: f64,
    target: GateOffTarget,
) {
    cancel_gate_off_for_lid(
        &mut data.countdown_events,
        &mut data.block_events,
        logical_id,
    );
    let event_offset = source_frame_offset as f64 + delay_samples.max(0.0);
    schedule_countdown_or_block_event(
        data,
        event_offset,
        0.0,
        1,
        data.state.transport.pattern_epoch.load(Ordering::Relaxed),
        CountdownEventKind::GateOff(GateOffEvent {
            track_idx,
            logical_id,
            target,
        }),
    );
}

fn schedule_chop_events(
    data: &mut AudioCallbackData,
    track_idx: usize,
    source_frame_offset: u32,
    first_delay_samples: f64,
    interval_samples: f64,
    repeats: u32,
    step: usize,
    chop_gate: f32,
) {
    cancel_chops_for_track(
        &mut data.countdown_events,
        &mut data.block_events,
        track_idx,
    );
    if repeats == 0 {
        return;
    }
    schedule_countdown_or_block_event(
        data,
        source_frame_offset as f64 + first_delay_samples.max(0.0),
        interval_samples.max(1.0),
        repeats,
        data.state.transport.pattern_epoch.load(Ordering::Relaxed),
        CountdownEventKind::Chop(ChopEvent {
            track_idx,
            step,
            chop_gate,
        }),
    );
}

fn dispatch_gate_off_event(
    data: &mut AudioCallbackData,
    event: GateOffEvent,
    frame_offset: u32,
    block_start_sample: u64,
) {
    match event.target {
        GateOffTarget::Custom {
            engine_id,
            free_patch,
        } => {
            if engine_id >= data.custom_engine_pools.len() {
                return;
            }
            if free_patch {
                data.custom_engine_pools[engine_id]
                    .release_free_patch_voice_by_logical_id(event.logical_id);
            } else {
                data.custom_engine_pools[engine_id].release_voice_by_logical_id(
                    event.logical_id,
                    block_start_sample + frame_offset as u64,
                );
            }
            let seq = next_block_event_sequence(data);
            unsafe {
                send_custom_note_off(data.lg.0, event.logical_id, frame_offset, seq);
            }
        }
        GateOffTarget::Sampler { gatepitch_id } => {
            if gatepitch_id > 0 {
                let seq = next_block_event_sequence(data);
                unsafe {
                    send_custom_note_off(data.lg.0, gatepitch_id as u64, frame_offset, seq);
                }
            }
            if event.track_idx >= data.voice_pools.len() {
                return;
            }
            data.voice_pools[event.track_idx].release_voice_by_logical_id(event.logical_id);
            let seq = next_block_event_sequence(data);
            unsafe {
                send_sampler_note_off(data.lg.0, event.logical_id, frame_offset, seq);
            }
        }
    }
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

fn custom_pitch_midi_note(transpose: f32, base_note_offset: f32) -> u8 {
    (transpose + base_note_offset + 60.0)
        .round()
        .clamp(0.0, 127.0) as u8
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
    if logical_id == 0 {
        return;
    }
    for track_notes in active_notes.iter_mut() {
        for slot in track_notes.iter_mut() {
            if let Some(note) = slot.as_mut() {
                note.remove_voice_by_lid(logical_id);
                if note.voice_count == 0 {
                    *slot = None;
                }
            }
        }
    }
}

fn store_active_keyboard_note(
    active_notes: &mut [[Option<ActiveKeyboardNote>; MAX_VOICES]; MAX_TRACKS],
    track_idx: usize,
    source_transpose: f32,
    midi_note: Option<u8>,
    voices: &[ActiveKeyboardVoice],
) {
    let Some(note) = ActiveKeyboardNote::new(source_transpose, midi_note, voices) else {
        return;
    };
    for voice in voices {
        clear_active_keyboard_note_by_lid(active_notes, voice.logical_id);
    }
    let track_notes = &mut active_notes[track_idx];
    if let Some(slot) = track_notes.iter_mut().find(|slot| {
        slot.is_some_and(|note| (note.source_transpose - source_transpose).abs() < 0.01)
    }) {
        *slot = Some(note);
        return;
    }
    if let Some(slot) = track_notes.iter_mut().find(|slot| slot.is_none()) {
        *slot = Some(note);
        return;
    }
    track_notes[0] = Some(note);
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

fn release_active_keyboard_voice(
    data: &mut AudioCallbackData,
    voice: ActiveKeyboardVoice,
    frame_offset: u32,
    block_end_sample: u64,
) {
    if voice.logical_id == 0 {
        return;
    }
    match voice.target {
        ActiveKeyboardVoiceTarget::Sampler { pool_id } => {
            if let Some(pool) = data.voice_pools.get_mut(pool_id) {
                pool.release_voice_by_logical_id(voice.logical_id);
            }
            let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
            let sampler_seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                if voice.gatepitch_id > 0 {
                    send_custom_note_off(
                        data.lg.0,
                        voice.gatepitch_id as u64,
                        frame_offset,
                        gatepitch_seq,
                    );
                }
                send_sampler_note_off(data.lg.0, voice.logical_id, frame_offset, sampler_seq);
            }
        }
        ActiveKeyboardVoiceTarget::Custom {
            engine_id,
            free_patch,
        } => {
            if let Some(pool) = data.custom_engine_pools.get_mut(engine_id) {
                if free_patch {
                    pool.release_free_patch_voice_by_logical_id(voice.logical_id);
                } else {
                    pool.release_voice_by_logical_id(voice.logical_id, block_end_sample);
                }
            }
            let seq = next_block_event_sequence(data);
            unsafe {
                send_custom_note_off(data.lg.0, voice.logical_id, frame_offset, seq);
            }
        }
    }
}

fn release_active_keyboard_note(
    data: &mut AudioCallbackData,
    note: ActiveKeyboardNote,
    frame_offset: u32,
    block_end_sample: u64,
) {
    for voice in note.voices() {
        release_active_keyboard_voice(data, *voice, frame_offset, block_end_sample);
    }
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
    if warp_enabled <= 0.5 {
        return (0.0, warp_mode, 1.0, sample_bpm, project_bpm, 0.0, 0.0);
    }
    let ratio = (project_bpm / sample_bpm).clamp(0.01, 32.0);
    if warp_mode.round() != 0.0 {
        // Non-onset warp modes (re-pitch family) need no analysis or onset table.
        return (1.0, warp_mode, ratio, sample_bpm, project_bpm, 0.0, 0.0);
    }
    // Beats runs off the beat grid (bpm-only); the onset table is attached
    // when analysis is ready so Preserve=Transients can snap to it, but its
    // absence no longer disables warp.
    let status = state.runtime.sampler_analysis_status[track_idx].load(Ordering::Acquire);
    let (ptr_lo, ptr_hi) = if status == 2 {
        (
            f32::from_bits(state.runtime.sampler_onset_ptr_lo[track_idx].load(Ordering::Acquire)),
            f32::from_bits(state.runtime.sampler_onset_ptr_hi[track_idx].load(Ordering::Acquire)),
        )
    } else {
        (0.0, 0.0)
    };
    (
        1.0,
        warp_mode,
        ratio,
        sample_bpm,
        project_bpm,
        ptr_lo,
        ptr_hi,
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
    let num_tensors = slot.tensor_params.num_params();
    for tensor_idx in 0..num_tensors {
        if let Some(cell_offset) = slot.tensor_params.tensor_cell_offset(tensor_idx) {
            cell_offset.hash(&mut hasher);
        }
        if let Some(values) = step
            .and_then(|step_idx| slot.tensor_params.plock_values(step_idx, tensor_idx))
            .or_else(|| slot.tensor_params.default_values(tensor_idx))
        {
            for value in values {
                value.to_bits().hash(&mut hasher);
            }
        }
    }

    hasher.finish()
}

fn slot_param_identity(
    node_id: u32,
    modulator_node_id: u32,
    raw_idx: u32,
) -> Option<crate::neural::ParamNodeId> {
    if raw_idx == u32::MAX {
        return None;
    }
    if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
        if modulator_node_id == 0 {
            return None;
        }
        Some(crate::neural::ParamNodeId {
            logical_id: modulator_node_id as u64,
            node_param_idx: raw_idx - crate::voice_modulator::MOD_PARAM_BASE,
        })
    } else {
        if node_id == 0 {
            return None;
        }
        Some(crate::neural::ParamNodeId {
            logical_id: node_id as u64,
            node_param_idx: raw_idx,
        })
    }
}

fn plock_identity_matches(
    plock_ids: &[Vec<Option<crate::neural::ParamNodeId>>],
    step_idx: usize,
    param_idx: usize,
    expected: Option<crate::neural::ParamNodeId>,
) -> bool {
    let Some(expected) = expected else {
        return false;
    };
    plock_ids
        .get(step_idx)
        .and_then(|step| step.get(param_idx))
        .copied()
        .flatten()
        == Some(expected)
}

fn resolved_slot_param_value(
    slot: &EffectSlotSnapshot,
    step_idx: usize,
    param_idx: usize,
    default: f32,
) -> f32 {
    let default_value = slot.defaults.get(param_idx).copied().unwrap_or(default);
    let Some(plock) = slot
        .plocks
        .get(step_idx)
        .and_then(|step| step.get(param_idx))
        .copied()
        .flatten()
    else {
        return default_value;
    };
    let raw_idx = slot
        .param_node_indices
        .get(param_idx)
        .copied()
        .unwrap_or(param_idx as u32);
    let expected_id = slot_param_identity(slot.node_id, slot.modulator_node_id, raw_idx);
    if plock_identity_matches(&slot.plock_param_ids, step_idx, param_idx, expected_id) {
        plock
    } else {
        default_value
    }
}

fn snapshot_slot_param_index_by_node_idx(
    slot: &EffectSlotSnapshot,
    node_param_idx: u32,
) -> Option<usize> {
    let num_params = slot.num_params as usize;
    (0..num_params).find(|&param_idx| {
        slot.param_node_indices
            .get(param_idx)
            .copied()
            .unwrap_or(param_idx as u32)
            == node_param_idx
    })
}

fn resolved_slot_node_param_value(
    slot: &EffectSlotSnapshot,
    step_idx: usize,
    node_param_idx: u32,
    default: f32,
) -> f32 {
    let Some(param_idx) = snapshot_slot_param_index_by_node_idx(slot, node_param_idx) else {
        return default;
    };
    resolved_slot_param_value(slot, step_idx, param_idx, default)
}

fn default_slot_node_param_value(
    slot: &EffectSlotSnapshot,
    node_param_idx: u32,
    default: f32,
) -> f32 {
    let Some(param_idx) = snapshot_slot_param_index_by_node_idx(slot, node_param_idx) else {
        return default;
    };
    slot.defaults.get(param_idx).copied().unwrap_or(default)
}

fn live_slot_param_index_by_node_idx(slot: &EffectSlotState, node_param_idx: u64) -> Option<usize> {
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    (0..num_params).find(|&param_idx| slot.resolve_node_idx(param_idx) == node_param_idx)
}

fn live_slot_resolved_param_value(
    slot: &EffectSlotState,
    step_idx: usize,
    param_idx: usize,
    default: f32,
) -> f32 {
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    if param_idx >= num_params {
        return default;
    }
    let default_value = slot.defaults.get(param_idx);
    let Some(plock) = slot.plocks.get(step_idx, param_idx) else {
        return default_value;
    };
    let expected_id = slot.param_node_id(param_idx);
    if expected_id.is_some() && slot.plocks.get_id(step_idx, param_idx) == expected_id {
        plock
    } else {
        default_value
    }
}

fn live_slot_resolved_node_param_value(
    slot: &EffectSlotState,
    step_idx: usize,
    node_param_idx: u64,
    default: f32,
) -> f32 {
    let Some(param_idx) = live_slot_param_index_by_node_idx(slot, node_param_idx) else {
        return default;
    };
    live_slot_resolved_param_value(slot, step_idx, param_idx, default)
}

fn live_slot_default_node_param_value(
    slot: &EffectSlotState,
    node_param_idx: u64,
    default: f32,
) -> f32 {
    let Some(param_idx) = live_slot_param_index_by_node_idx(slot, node_param_idx) else {
        return default;
    };
    slot.defaults.get(param_idx)
}

fn snapshot_slot_default_node_param_value(
    slot: &EffectSlotSnapshot,
    node_param_idx: u64,
    default: f32,
) -> f32 {
    let Some(param_idx) = slot
        .param_node_indices
        .iter()
        .position(|idx| u64::from(*idx) == node_param_idx)
    else {
        return default;
    };
    slot.defaults.get(param_idx).copied().unwrap_or(default)
}

fn key_lock_identity_matches(
    key_lock_ids: &std::collections::BTreeMap<u8, Vec<Option<crate::neural::ParamNodeId>>>,
    note: u8,
    param_idx: usize,
    expected: Option<crate::neural::ParamNodeId>,
) -> bool {
    let Some(expected) = expected else {
        return false;
    };
    key_lock_ids
        .get(&note)
        .and_then(|row| row.get(param_idx))
        .copied()
        .flatten()
        == Some(expected)
}

fn live_param_route(
    slot: &EffectSlotState,
    param_idx: usize,
) -> Option<(ScheduledInstrumentParamTarget, u64, u32)> {
    let raw_idx = slot.resolve_node_idx(param_idx);
    if raw_idx == u32::MAX as u64 {
        return None;
    }
    let span = slot.resolve_node_span(param_idx);
    if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE as u64 {
        Some((
            ScheduledInstrumentParamTarget::Modulator,
            raw_idx - crate::voice_modulator::MOD_PARAM_BASE as u64,
            span,
        ))
    } else {
        Some((ScheduledInstrumentParamTarget::Synth, raw_idx, span))
    }
}

fn snapshot_param_route(
    slot: &EffectSlotSnapshot,
    param_idx: usize,
) -> Option<(ScheduledInstrumentParamTarget, u64, u32)> {
    let raw_idx = slot
        .param_node_indices
        .get(param_idx)
        .copied()
        .unwrap_or(param_idx as u32);
    if raw_idx == u32::MAX {
        return None;
    }
    let span = slot
        .param_node_spans
        .get(param_idx)
        .copied()
        .unwrap_or(1)
        .max(1);
    if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
        Some((
            ScheduledInstrumentParamTarget::Modulator,
            (raw_idx - crate::voice_modulator::MOD_PARAM_BASE) as u64,
            span,
        ))
    } else {
        Some((ScheduledInstrumentParamTarget::Synth, raw_idx as u64, span))
    }
}

fn live_step_has_valid_plock(
    slot: &EffectSlotState,
    step_idx: Option<usize>,
    param_idx: usize,
) -> bool {
    let Some(step_idx) = step_idx else {
        return false;
    };
    if slot.plocks.get(step_idx, param_idx).is_none() {
        return false;
    }
    let expected_id = slot.param_node_id(param_idx);
    expected_id.is_some() && slot.plocks.get_id(step_idx, param_idx) == expected_id
}

fn snapshot_step_has_valid_plock(
    slot: &EffectSlotSnapshot,
    step_idx: Option<usize>,
    param_idx: usize,
) -> bool {
    let Some(step_idx) = step_idx else {
        return false;
    };
    if slot
        .plocks
        .get(step_idx)
        .and_then(|row| row.get(param_idx))
        .copied()
        .flatten()
        .is_none()
    {
        return false;
    }
    let raw_idx = slot
        .param_node_indices
        .get(param_idx)
        .copied()
        .unwrap_or(param_idx as u32);
    let expected_id = slot_param_identity(slot.node_id, slot.modulator_node_id, raw_idx);
    plock_identity_matches(&slot.plock_param_ids, step_idx, param_idx, expected_id)
}

fn upsert_instrument_param(
    params: &mut ScheduledInstrumentParams,
    target: ScheduledInstrumentParamTarget,
    idx: u64,
    span: u32,
    value: f32,
) {
    if let Some(existing) = params
        .iter_mut()
        .find(|param| param.target == target && param.idx == idx)
    {
        existing.span = span;
        existing.value = value;
        return;
    }
    if params.is_full() {
        return;
    }
    params.push(ScheduledInstrumentParam {
        target,
        idx,
        span,
        value,
    });
    params.sort_by_key(|param| match param.target {
        ScheduledInstrumentParamTarget::Synth => (0_u8, param.idx),
        ScheduledInstrumentParamTarget::Modulator => (1_u8, param.idx),
    });
}

fn key_locked_live_instrument_params(
    state: &SequencerState,
    track_idx: usize,
    transpose: f32,
    base_note_offset: f32,
    step_idx: Option<usize>,
    base_params: &ScheduledInstrumentParams,
) -> ScheduledInstrumentParams {
    let Some(slot) = state.pattern.instrument_slots.get(track_idx) else {
        return base_params.clone();
    };
    let note = custom_pitch_midi_note(transpose, base_note_offset);
    if !slot
        .key_locks
        .note_has_any_lock(note, slot.num_params.load(Ordering::Relaxed) as usize)
    {
        return base_params.clone();
    }

    let mut params = base_params.clone();
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params.min(MAX_SLOT_PARAMS) {
        if live_step_has_valid_plock(slot, step_idx, param_idx) {
            continue;
        }
        let Some(value) = slot.key_locks.get(note, param_idx) else {
            continue;
        };
        if !value.is_finite()
            || slot.key_locks.get_id(note, param_idx) != slot.param_node_id(param_idx)
        {
            continue;
        }
        let Some((target, idx, span)) = live_param_route(slot, param_idx) else {
            continue;
        };
        upsert_instrument_param(&mut params, target, idx, span, value);
    }
    params
}

fn key_locked_snapshot_instrument_params(
    slot: &EffectSlotSnapshot,
    transpose: f32,
    base_note_offset: f32,
    step_idx: Option<usize>,
    base_params: &ScheduledInstrumentParams,
) -> ScheduledInstrumentParams {
    let note = custom_pitch_midi_note(transpose, base_note_offset);
    let Some(row) = slot.key_locks.get(&note) else {
        return base_params.clone();
    };

    let mut params = base_params.clone();
    let num_params = (slot.num_params as usize).min(MAX_SLOT_PARAMS);
    for param_idx in 0..num_params.min(row.len()) {
        if snapshot_step_has_valid_plock(slot, step_idx, param_idx) {
            continue;
        }
        let Some(value) = row[param_idx] else {
            continue;
        };
        if !value.is_finite() {
            continue;
        }
        let raw_idx = slot
            .param_node_indices
            .get(param_idx)
            .copied()
            .unwrap_or(param_idx as u32);
        let expected_id = slot_param_identity(slot.node_id, slot.modulator_node_id, raw_idx);
        if !key_lock_identity_matches(&slot.key_lock_param_ids, note, param_idx, expected_id) {
            continue;
        }
        let Some((target, idx, span)) = snapshot_param_route(slot, param_idx) else {
            continue;
        };
        upsert_instrument_param(&mut params, target, idx, span, value);
    }
    params
}

fn resolve_live_instrument_defaults(
    state: &SequencerState,
    track_idx: usize,
) -> ScheduledInstrumentParams {
    let Some(slot) = state.pattern.instrument_slots.get(track_idx) else {
        return ScheduledInstrumentParams::new();
    };
    let mut params = ScheduledInstrumentParams::new();
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params.min(MAX_SLOT_PARAMS) {
        let Some((target, idx, span)) = live_param_route(slot, param_idx) else {
            continue;
        };
        let value = slot.defaults.get(param_idx);
        if !value.is_finite() {
            continue;
        }
        params.push(ScheduledInstrumentParam {
            target,
            idx,
            span,
            value,
        });
    }
    params.sort_by_key(|param| match param.target {
        ScheduledInstrumentParamTarget::Synth => (0_u8, param.idx),
        ScheduledInstrumentParamTarget::Modulator => (1_u8, param.idx),
    });
    params
}

fn resolve_snapshot_instrument_defaults(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
) -> ScheduledInstrumentParams {
    let Some(slot) = snapshot
        .tracks
        .get(track_idx)
        .map(|track| &track.instrument_slot)
    else {
        return ScheduledInstrumentParams::new();
    };
    let mut params = ScheduledInstrumentParams::new();
    let num_params = (slot.num_params as usize).min(slot.defaults.len());
    for param_idx in 0..num_params {
        let Some((target, idx, span)) = snapshot_param_route(slot, param_idx) else {
            continue;
        };
        let value = slot.defaults[param_idx];
        if !value.is_finite() {
            continue;
        }
        params.push(ScheduledInstrumentParam {
            target,
            idx,
            span,
            value,
        });
    }
    params.sort_by_key(|param| match param.target {
        ScheduledInstrumentParamTarget::Synth => (0_u8, param.idx),
        ScheduledInstrumentParamTarget::Modulator => (1_u8, param.idx),
    });
    params
}

fn resolve_live_instrument_tensor_defaults(
    state: &SequencerState,
    track_idx: usize,
) -> ScheduledInstrumentTensorParams {
    let Some(slot) = state.pattern.instrument_slots.get(track_idx) else {
        return ScheduledInstrumentTensorParams::new();
    };
    let mut params = ScheduledInstrumentTensorParams::new();
    let num_tensors = slot.tensor_params.num_params();
    for tensor_idx in 0..num_tensors {
        let Some(cell_offset) = slot.tensor_params.tensor_cell_offset(tensor_idx) else {
            continue;
        };
        let Some(values) = slot.tensor_params.default_values(tensor_idx) else {
            continue;
        };
        if values.iter().any(|value| !value.is_finite()) {
            continue;
        }
        if params.is_full() {
            break;
        }
        params.push(ScheduledInstrumentTensorParam {
            cell_offset,
            values,
        });
    }
    params.sort_by_key(|param| param.cell_offset);
    params
}

fn instrument_param_bundle_fingerprint(
    engine_id: usize,
    base_note_offset: f32,
    instrument_params: &[ScheduledInstrumentParam],
    instrument_tensor_params: &[ScheduledInstrumentTensorParam],
) -> u64 {
    let mut hasher = DefaultHasher::new();
    engine_id.hash(&mut hasher);
    base_note_offset.to_bits().hash(&mut hasher);
    for param in instrument_params {
        param.target.hash(&mut hasher);
        param.idx.hash(&mut hasher);
        param.span.hash(&mut hasher);
        param.value.to_bits().hash(&mut hasher);
    }
    for tensor in instrument_tensor_params {
        tensor.cell_offset.hash(&mut hasher);
        for value in &tensor.values {
            value.to_bits().hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn resolve_rack_slot_instrument_params(
    slot: &EffectSlotSnapshot,
    step_idx: usize,
) -> ScheduledInstrumentParams {
    let num_params = slot.num_params as usize;
    let mut params = ScheduledInstrumentParams::new();
    for param_idx in 0..num_params {
        let raw_idx = slot
            .param_node_indices
            .get(param_idx)
            .copied()
            .unwrap_or(param_idx as u32);
        if raw_idx == u32::MAX {
            continue;
        }
        let span = slot
            .param_node_spans
            .get(param_idx)
            .copied()
            .unwrap_or(1)
            .max(1);
        let (target, idx) = if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
            (
                ScheduledInstrumentParamTarget::Modulator,
                (raw_idx - crate::voice_modulator::MOD_PARAM_BASE) as u64,
            )
        } else {
            (ScheduledInstrumentParamTarget::Synth, raw_idx as u64)
        };
        let value = resolved_slot_param_value(slot, step_idx, param_idx, 0.0);
        if !value.is_finite() {
            continue;
        }
        params.push(ScheduledInstrumentParam {
            target,
            idx,
            span,
            value,
        });
    }
    params.sort_by_key(|param| match param.target {
        ScheduledInstrumentParamTarget::Synth => (0_u8, param.idx),
        ScheduledInstrumentParamTarget::Modulator => (1_u8, param.idx),
    });
    params
}

fn resolve_rack_slot_instrument_defaults(slot: &EffectSlotSnapshot) -> ScheduledInstrumentParams {
    let num_params = slot.num_params as usize;
    let mut params = ScheduledInstrumentParams::new();
    for param_idx in 0..num_params {
        let raw_idx = slot
            .param_node_indices
            .get(param_idx)
            .copied()
            .unwrap_or(param_idx as u32);
        if raw_idx == u32::MAX {
            continue;
        }
        let span = slot
            .param_node_spans
            .get(param_idx)
            .copied()
            .unwrap_or(1)
            .max(1);
        let (target, idx) = if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
            (
                ScheduledInstrumentParamTarget::Modulator,
                (raw_idx - crate::voice_modulator::MOD_PARAM_BASE) as u64,
            )
        } else {
            (ScheduledInstrumentParamTarget::Synth, raw_idx as u64)
        };
        let value = slot.defaults.get(param_idx).copied().unwrap_or(0.0);
        if !value.is_finite() {
            continue;
        }
        params.push(ScheduledInstrumentParam {
            target,
            idx,
            span,
            value,
        });
    }
    params.sort_by_key(|param| match param.target {
        ScheduledInstrumentParamTarget::Synth => (0_u8, param.idx),
        ScheduledInstrumentParamTarget::Modulator => (1_u8, param.idx),
    });
    params
}

fn resolve_rack_slot_sampler_params(
    slot: &EffectSlotSnapshot,
    step_idx: usize,
) -> ScheduledSamplerParams {
    let value = |param_idx: usize, default: f32| {
        resolved_slot_param_value(slot, step_idx, param_idx, default)
    };
    ScheduledSamplerParams {
        attack_ms: value(0, 0.0),
        release_ms: value(1, 0.0),
        start_point: value(2, 0.0),
        end_point: value(3, 1.0),
        instrument_enabled: value(4, 1.0),
        reverse: value(5, 0.0),
        loop_mode: value(6, 0.0),
        loop_xfade_ms: value(7, 0.0),
        sr_hz: value(8, 0.0),
        warp_enabled: value(9, 0.0),
        warp_mode: value(10, 0.0),
        sample_bpm: value(11, 120.0),
        playback_speed: value(12, 1.0),
        scrub: value(13, 0.0),
        warp_preserve: resolved_slot_node_param_value(
            slot,
            step_idx,
            crate::sampler::PARAM_WARP_PRESERVE as u32,
            crate::sampler::WARP_PRESERVE_DEFAULT as f32,
        ),
        warp_seg_loop_mode: resolved_slot_node_param_value(
            slot,
            step_idx,
            crate::sampler::PARAM_WARP_SEG_LOOP_MODE as u32,
            crate::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
        ),
        warp_seg_envelope: resolved_slot_node_param_value(
            slot,
            step_idx,
            crate::sampler::PARAM_WARP_SEG_ENVELOPE as u32,
            crate::sampler::WARP_SEG_ENVELOPE_DEFAULT,
        ),
    }
}

fn resolve_rack_slot_sampler_defaults(slot: &EffectSlotSnapshot) -> ScheduledSamplerParams {
    let value =
        |param_idx: usize, default: f32| slot.defaults.get(param_idx).copied().unwrap_or(default);
    ScheduledSamplerParams {
        attack_ms: value(0, 0.0),
        release_ms: value(1, 0.0),
        start_point: value(2, 0.0),
        end_point: value(3, 1.0),
        instrument_enabled: value(4, 1.0),
        reverse: value(5, 0.0),
        loop_mode: value(6, 0.0),
        loop_xfade_ms: value(7, 0.0),
        sr_hz: value(8, 0.0),
        warp_enabled: value(9, 0.0),
        warp_mode: value(10, 0.0),
        sample_bpm: value(11, 120.0),
        playback_speed: value(12, 1.0),
        scrub: value(13, 0.0),
        warp_preserve: default_slot_node_param_value(
            slot,
            crate::sampler::PARAM_WARP_PRESERVE as u32,
            crate::sampler::WARP_PRESERVE_DEFAULT as f32,
        ),
        warp_seg_loop_mode: default_slot_node_param_value(
            slot,
            crate::sampler::PARAM_WARP_SEG_LOOP_MODE as u32,
            crate::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
        ),
        warp_seg_envelope: default_slot_node_param_value(
            slot,
            crate::sampler::PARAM_WARP_SEG_ENVELOPE as u32,
            crate::sampler::WARP_SEG_ENVELOPE_DEFAULT,
        ),
    }
}

fn rack_slot_sound_fingerprint(
    slot: &RackSlotSnapshot,
    instrument_params: &[ScheduledInstrumentParam],
    base_note_offset: f32,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    slot.track_sound_state.engine_id.hash(&mut hasher);
    base_note_offset.to_bits().hash(&mut hasher);
    for param in instrument_params {
        param.target.hash(&mut hasher);
        param.idx.hash(&mut hasher);
        param.value.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

unsafe fn dispatch_effect_chain_for_track(
    lg: *mut LiveGraph,
    effect_params: &mut [ScheduledEffectParam],
) {
    effect_params.sort_by_key(|param| (param.logical_id, param.idx));
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

unsafe fn dispatch_instrument_tensor_params_to_voice(
    lg: *mut LiveGraph,
    synth_id: u64,
    instrument_tensor_params: &[ScheduledInstrumentTensorParam],
) {
    if synth_id == 0 {
        return;
    }
    for tensor in instrument_tensor_params {
        crate::lisp_host::queue_tensor_write(
            lg,
            synth_id as i32,
            tensor.cell_offset,
            &tensor.values,
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
    let mut param_indices: ArrayVec<usize, MAX_SLOT_PARAMS> = ArrayVec::new();
    for param_idx in 0..num_params.min(MAX_SLOT_PARAMS) {
        param_indices.push(param_idx);
    }
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
    let num_tensors = slot.tensor_params.num_params();
    for tensor_idx in 0..num_tensors {
        let Some(cell_offset) = slot.tensor_params.tensor_cell_offset(tensor_idx) else {
            continue;
        };
        let Some(values) = slot.tensor_params.default_values(tensor_idx) else {
            continue;
        };
        crate::lisp_host::queue_tensor_write(lg, synth_id as i32, cell_offset, &values);
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

fn dispatch_instrument_tensor_params_to_active_voices(
    data: &mut AudioCallbackData,
    track_idx: usize,
    instrument_tensor_params: &[ScheduledInstrumentTensorParam],
) {
    if instrument_tensor_params.is_empty() {
        return;
    }
    let Some(engine_id) = track_engine_id(&data.state, track_idx) else {
        return;
    };
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
        let synth_id =
            data.state.runtime.engine_synth_node_ids[engine_id][voice_idx].load(Ordering::Relaxed);
        if synth_id == 0 {
            continue;
        }
        unsafe {
            dispatch_instrument_tensor_params_to_voice(
                data.lg.0,
                synth_id as u64,
                instrument_tensor_params,
            );
        }
        pool.voices[voice_idx].fingerprint = 0;
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
    frame_offset: u32,
    track_idx: usize,
    step: usize,
    samples_per_step: f32,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    mut effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
    sampler_params: ScheduledSamplerParams,
    instrument_fingerprint: u64,
) {
    unsafe {
        dispatch_effect_chain_for_track(data.lg.0, &mut effect_params);
    }
    fire_resolved(
        data,
        frame_offset,
        track_idx,
        step,
        Some(step),
        samples_per_step as f64,
        resolved,
        chord,
        instrument_params,
        instrument_tensor_params,
        instrument_fingerprint,
        Some(sampler_params),
    );
}

fn dispatch_scheduled_network_step(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    track_idx: usize,
    key_lock_plock_step: Option<usize>,
    samples_per_step: f32,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    mut effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
    sampler_params: ScheduledSamplerParams,
    instrument_fingerprint: u64,
) {
    unsafe {
        dispatch_effect_chain_for_track(data.lg.0, &mut effect_params);
    }
    fire_resolved(
        data,
        frame_offset,
        track_idx,
        0,
        key_lock_plock_step,
        samples_per_step as f64,
        resolved,
        chord,
        instrument_params,
        instrument_tensor_params,
        instrument_fingerprint,
        Some(sampler_params),
    );
}

fn dispatch_scheduled_event(
    data: &mut AudioCallbackData,
    event: ScheduledEvent,
    frame_offset: u32,
) {
    match event.kind {
        ScheduledEventKind::ResolvedTrigger {
            track,
            step,
            samples_per_step,
            resolved,
            chord,
            effect_params,
            instrument_params,
            instrument_tensor_params,
            sampler_params,
            instrument_fingerprint,
        } => {
            dispatch_scheduled_step(
                data,
                frame_offset,
                track,
                step,
                samples_per_step,
                resolved,
                chord,
                effect_params,
                instrument_params,
                instrument_tensor_params,
                sampler_params,
                instrument_fingerprint,
            );
        }
        ScheduledEventKind::InstrumentParams {
            track,
            instrument_params,
            instrument_tensor_params,
        } => {
            dispatch_instrument_params_to_active_voices(data, track, &instrument_params);
            dispatch_instrument_tensor_params_to_active_voices(
                data,
                track,
                &instrument_tensor_params,
            );
        }
        ScheduledEventKind::EffectParams {
            mut effect_params, ..
        } => unsafe {
            dispatch_effect_chain_for_track(data.lg.0, &mut effect_params);
        },
        ScheduledEventKind::NetworkTrigger {
            track,
            samples_per_step,
            resolved,
            chord,
            effect_params,
            instrument_params,
            instrument_tensor_params,
            sampler_params,
            instrument_fingerprint,
            seed,
            ..
        } => {
            dispatch_scheduled_network_step(
                data,
                frame_offset,
                track,
                seed.map(|(_, step)| step),
                samples_per_step,
                resolved,
                chord,
                effect_params,
                instrument_params,
                instrument_tensor_params,
                sampler_params,
                instrument_fingerprint,
            );
        }
    }
}

fn scheduled_trigger_track(event: &ScheduledEvent) -> Option<usize> {
    match &event.kind {
        ScheduledEventKind::ResolvedTrigger { track, .. }
        | ScheduledEventKind::NetworkTrigger { track, .. } => Some(*track),
        ScheduledEventKind::InstrumentParams { .. } | ScheduledEventKind::EffectParams { .. } => {
            None
        }
    }
}

fn frame_offset_from_remaining(remaining_samples: f64, nframes: usize) -> u32 {
    remaining_samples
        .floor()
        .max(0.0)
        .min(nframes.saturating_sub(1) as f64) as u32
}

fn block_event_priority(kind: &BlockEventKind) -> u8 {
    match kind {
        BlockEventKind::GateOff(_) => 0,
        BlockEventKind::Scheduled(ScheduledEvent {
            kind:
                ScheduledEventKind::InstrumentParams { .. } | ScheduledEventKind::EffectParams { .. },
            ..
        }) => 1,
        BlockEventKind::Scheduled(_) | BlockEventKind::Chop(_) => 2,
    }
}

fn try_push_block_event(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    seq: u64,
    kind: BlockEventKind,
) {
    if data.block_events.len() >= SCHEDULED_BLOCK_SCRATCH_CAPACITY {
        data.dropped_scheduled_events = data.dropped_scheduled_events.saturating_add(1);
        if data.trace_audio {
            eprintln!(
                "audio-trace: dropped countdown event; block scratch full capacity={SCHEDULED_BLOCK_SCRATCH_CAPACITY}"
            );
        }
        return;
    }
    data.block_events.push(BlockEvent {
        frame_offset,
        seq,
        kind,
    });
    data.block_events_need_sort = true;
}

fn try_push_countdown_event(
    data: &mut AudioCallbackData,
    remaining_samples: f64,
    period_samples: f64,
    repeats: u32,
    pattern_epoch: u64,
    seq: u64,
    kind: CountdownEventKind,
) {
    if data.countdown_events.len() >= SCHEDULED_COUNTDOWN_CAPACITY {
        data.dropped_scheduled_events = data.dropped_scheduled_events.saturating_add(1);
        if data.trace_audio {
            eprintln!(
                "audio-trace: dropped countdown event; countdown pool full capacity={SCHEDULED_COUNTDOWN_CAPACITY}"
            );
        }
        return;
    }
    data.countdown_events.push(CountdownEvent {
        remaining_samples,
        period_samples,
        repeats,
        pattern_epoch,
        seq,
        kind,
    });
}

fn schedule_countdown_or_block_event(
    data: &mut AudioCallbackData,
    event_offset: f64,
    period_samples: f64,
    repeats: u32,
    pattern_epoch: u64,
    kind: CountdownEventKind,
) {
    if repeats == 0 {
        return;
    }
    let nframes = data.current_callback_nframes;
    match kind {
        CountdownEventKind::Scheduled(event) => {
            let seq = data.event_seq;
            data.event_seq = data.event_seq.wrapping_add(1);
            if event_offset < nframes as f64 {
                let frame_offset = frame_offset_from_remaining(event_offset, nframes);
                try_push_block_event(data, frame_offset, seq, BlockEventKind::Scheduled(event));
            } else {
                try_push_countdown_event(
                    data,
                    event_offset - nframes as f64,
                    0.0,
                    1,
                    pattern_epoch,
                    seq,
                    CountdownEventKind::Scheduled(event),
                );
            }
        }
        CountdownEventKind::GateOff(event) => {
            let seq = data.event_seq;
            data.event_seq = data.event_seq.wrapping_add(1);
            if event_offset < nframes as f64 {
                let frame_offset = frame_offset_from_remaining(event_offset, nframes);
                try_push_block_event(data, frame_offset, seq, BlockEventKind::GateOff(event));
            } else {
                try_push_countdown_event(
                    data,
                    event_offset - nframes as f64,
                    0.0,
                    1,
                    pattern_epoch,
                    seq,
                    CountdownEventKind::GateOff(event),
                );
            }
        }
        CountdownEventKind::Chop(event) => {
            let mut next_offset = event_offset;
            let mut remaining_repeats = repeats;
            while remaining_repeats > 0 && next_offset < nframes as f64 {
                let seq = data.event_seq;
                data.event_seq = data.event_seq.wrapping_add(1);
                let frame_offset = frame_offset_from_remaining(next_offset, nframes);
                try_push_block_event(data, frame_offset, seq, BlockEventKind::Chop(event));
                remaining_repeats -= 1;
                next_offset += period_samples.max(1.0);
            }
            if remaining_repeats > 0 {
                let seq = data.event_seq;
                data.event_seq = data.event_seq.wrapping_add(1);
                try_push_countdown_event(
                    data,
                    next_offset - nframes as f64,
                    period_samples.max(1.0),
                    remaining_repeats,
                    pattern_epoch,
                    seq,
                    CountdownEventKind::Chop(event),
                );
            }
        }
    }
}

fn enqueue_scheduled_event_for_callback(
    data: &mut AudioCallbackData,
    event: ScheduledEvent,
    block_start_sample: u64,
    nframes: usize,
    current_pattern_epoch: u64,
) {
    if event.pattern_epoch != current_pattern_epoch {
        return;
    }
    let seq = data.event_seq;
    data.event_seq = data.event_seq.wrapping_add(1);
    let remaining_samples = if event.sample_time >= block_start_sample {
        (event.sample_time - block_start_sample) as f64
    } else {
        data.late_scheduled_events = data.late_scheduled_events.saturating_add(1);
        0.0
    };
    if remaining_samples < nframes as f64 {
        let frame_offset = frame_offset_from_remaining(remaining_samples, nframes);
        try_push_block_event(data, frame_offset, seq, BlockEventKind::Scheduled(event));
    } else {
        try_push_countdown_event(
            data,
            remaining_samples - nframes as f64,
            0.0,
            1,
            event.pattern_epoch,
            seq,
            CountdownEventKind::Scheduled(event),
        );
    }
}

fn drain_scheduled_events_for_callback(
    data: &mut AudioCallbackData,
    block_start_sample: u64,
    nframes: usize,
    current_pattern_epoch: u64,
) {
    while let Some(event) = data.scheduled_events.pop() {
        enqueue_scheduled_event_for_callback(
            data,
            event,
            block_start_sample,
            nframes,
            current_pattern_epoch,
        );
    }
}

fn collect_due_countdown_events(
    data: &mut AudioCallbackData,
    nframes: usize,
    current_pattern_epoch: u64,
) {
    let block_len = nframes as f64;
    let mut i = 0usize;
    while i < data.countdown_events.len() {
        let stale = match data.countdown_events[i].kind {
            CountdownEventKind::GateOff(_) => false,
            CountdownEventKind::Scheduled(_) | CountdownEventKind::Chop(_) => {
                data.countdown_events[i].pattern_epoch != current_pattern_epoch
            }
        };
        if stale {
            data.countdown_events.swap_remove(i);
            continue;
        }
        if data.countdown_events[i].remaining_samples < block_len {
            let mut due = data.countdown_events.swap_remove(i);
            match due.kind {
                CountdownEventKind::Chop(event) => {
                    while due.repeats > 0 && due.remaining_samples < block_len {
                        let frame_offset =
                            frame_offset_from_remaining(due.remaining_samples, nframes);
                        try_push_block_event(
                            data,
                            frame_offset,
                            due.seq,
                            BlockEventKind::Chop(event),
                        );
                        due.repeats -= 1;
                        due.seq = data.event_seq;
                        data.event_seq = data.event_seq.wrapping_add(1);
                        due.remaining_samples += due.period_samples;
                    }
                    if due.repeats > 0 {
                        due.remaining_samples -= block_len;
                        data.countdown_events.push(due);
                    }
                }
                CountdownEventKind::Scheduled(event) => {
                    let frame_offset = frame_offset_from_remaining(due.remaining_samples, nframes);
                    try_push_block_event(
                        data,
                        frame_offset,
                        due.seq,
                        BlockEventKind::Scheduled(event),
                    );
                }
                CountdownEventKind::GateOff(event) => {
                    let frame_offset = frame_offset_from_remaining(due.remaining_samples, nframes);
                    try_push_block_event(
                        data,
                        frame_offset,
                        due.seq,
                        BlockEventKind::GateOff(event),
                    );
                }
            }
            continue;
        }
        data.countdown_events[i].remaining_samples -= block_len;
        i += 1;
    }
}

fn clear_countdown_events(data: &mut AudioCallbackData) {
    data.countdown_events.clear();
    data.block_events.clear();
    data.block_events_need_sort = false;
}

fn clear_transport_countdown_events(data: &mut AudioCallbackData) {
    data.countdown_events
        .retain(|event| matches!(event.kind, CountdownEventKind::GateOff(_)));
    data.block_events
        .retain(|event| matches!(event.kind, BlockEventKind::GateOff(_)));
    data.block_events_need_sort = true;
}

fn mute_group_winner_for_block_events(
    track: usize,
    group: u8,
    batch: &[BlockEvent],
    track_mute_groups: impl Fn(usize) -> u8,
) -> usize {
    batch
        .iter()
        .filter_map(|event| match &event.kind {
            BlockEventKind::Scheduled(scheduled) => scheduled_trigger_track(scheduled),
            BlockEventKind::GateOff(_) | BlockEventKind::Chop(_) => None,
        })
        .filter(|&candidate| track_mute_groups(candidate) == group)
        .max()
        .unwrap_or(track)
}

fn dispatch_block_events(data: &mut AudioCallbackData, block_start_sample: u64) {
    while !data.block_events.is_empty() {
        if data.block_events_need_sort {
            data.block_events.sort_unstable_by(|a, b| {
                (b.frame_offset, block_event_priority(&b.kind), b.seq).cmp(&(
                    a.frame_offset,
                    block_event_priority(&a.kind),
                    a.seq,
                ))
            });
            data.block_events_need_sort = false;
        }

        let Some(frame_offset) = data.block_events.last().map(|event| event.frame_offset) else {
            break;
        };
        let mut group_start = data.block_events.len();
        while group_start > 0 && data.block_events[group_start - 1].frame_offset == frame_offset {
            group_start -= 1;
        }

        let mut winning_group_tracks = [false; MAX_TRACKS];
        {
            let group = &data.block_events[group_start..];
            for event in group {
                let Some(track) = (match &event.kind {
                    BlockEventKind::Scheduled(scheduled) => scheduled_trigger_track(scheduled),
                    BlockEventKind::GateOff(_) | BlockEventKind::Chop(_) => None,
                }) else {
                    continue;
                };
                if track >= data.state.active_track_count() {
                    continue;
                }
                let group_id = data.state.pattern.track_params[track].get_mute_group();
                if group_id == 0 {
                    continue;
                }
                let winner =
                    mute_group_winner_for_block_events(track, group_id, group, |candidate| {
                        data.state
                            .pattern
                            .track_params
                            .get(candidate)
                            .map(|params| params.get_mute_group())
                            .unwrap_or(0)
                    });
                if winner < MAX_TRACKS {
                    winning_group_tracks[winner] = true;
                }
            }
        }

        let release_sample = block_start_sample + frame_offset as u64;
        for (track, is_winner) in winning_group_tracks.iter().copied().enumerate() {
            if is_winner {
                enforce_mute_group_for_winning_track(data, track, release_sample, frame_offset);
            }
        }

        while data
            .block_events
            .last()
            .is_some_and(|event| event.frame_offset == frame_offset)
        {
            let event = data.block_events.pop().unwrap();
            match event.kind {
                BlockEventKind::Scheduled(scheduled) => {
                    let dispatch = match scheduled_trigger_track(&scheduled) {
                        Some(track) if track < data.state.active_track_count() => {
                            let group = data.state.pattern.track_params[track].get_mute_group();
                            group == 0 || winning_group_tracks[track]
                        }
                        Some(_) => false,
                        None => true,
                    };
                    if dispatch {
                        dispatch_scheduled_event(data, scheduled, frame_offset);
                    }
                }
                BlockEventKind::GateOff(gate_off) => {
                    dispatch_gate_off_event(data, gate_off, frame_offset, block_start_sample);
                }
                BlockEventKind::Chop(chop) => {
                    dispatch_chop_event(data, chop, frame_offset);
                }
            }
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
    // Covers both per-track pools (0..MAX_TRACKS) and per-rack-slot pools
    // (rack_slot_pool_index, >= MAX_TRACKS) — previously capped at
    // MAX_TRACKS, which left every rack slot's mask permanently zero and
    // forced its voice_modulator nodes through an O(nframes) gate-timeline
    // scan every block instead of the O(1) active-mask check.
    for (pool_id, pool) in data.voice_pools.iter().enumerate() {
        if pool.num_voices == 0 {
            continue;
        }
        let mut mask = 0u64;
        for voice_idx in 0..pool.num_voices.min(MAX_VOICES) {
            if pool.voices[voice_idx].active {
                mask |= 1u64 << voice_idx;
            }
        }
        crate::voice_modulator::set_sampler_active_mask(pool_id, mask);
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
        let mut param_indices: ArrayVec<usize, MAX_SLOT_PARAMS> = ArrayVec::new();
        for param_idx in 0..num_params.min(MAX_SLOT_PARAMS) {
            param_indices.push(param_idx);
        }
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
            let span = slot
                .param_node_spans
                .get(param_idx)
                .copied()
                .unwrap_or(1)
                .max(1);
            push_param_span(lg, logical_id, idx, span, value);
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

fn compute_host_transport_clock(
    data: &mut AudioCallbackData,
    block_start_sample: u64,
) -> HostTransportClock {
    let playing = data.state.transport.playing.load(Ordering::Relaxed);
    if playing && !data.host_clock_was_playing {
        data.host_clock_play_start_sample = block_start_sample;
    }
    if !playing && data.host_clock_was_playing {
        data.host_clock_play_start_sample = block_start_sample;
    }
    data.host_clock_was_playing = playing;

    let bpm = data.state.transport.bpm.load(Ordering::Relaxed).max(1) as f64;
    let samples_per_bar = data.sample_rate * 240.0 / bpm;
    let elapsed_samples = block_start_sample.saturating_sub(data.host_clock_play_start_sample);
    let bar_phase = (elapsed_samples as f64 / samples_per_bar).fract() as f32;
    let bar_phase_increment = (1.0 / samples_per_bar) as f32;

    HostTransportClock {
        bar_phase,
        bar_phase_increment,
    }
}

fn sync_instrument_host_clock_params(data: &mut AudioCallbackData, clock: HostTransportClock) {
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
                        fvalue: clock.bar_phase,
                    },
                );
                params_push_wrapper(
                    data.lg.0,
                    ParamMsg {
                        idx: gatepitch::PARAM_CLOCK_INC,
                        logical_id: lid,
                        fvalue: clock.bar_phase_increment,
                    },
                );
            }
        }
    }

    for pool_id in 0..data.state.runtime.voice_counts.len() {
        let voice_count = data.state.runtime.voice_counts[pool_id].load(Ordering::Acquire) as usize;
        for voice_idx in 0..voice_count.min(MAX_VOICES) {
            let gatepitch_id = data.state.runtime.sampler_gatepitch_node_ids[pool_id][voice_idx]
                .load(Ordering::Acquire);
            if gatepitch_id == 0 {
                continue;
            }
            unsafe {
                params_push_wrapper(
                    data.lg.0,
                    ParamMsg {
                        idx: gatepitch::PARAM_CLOCK_PHASE,
                        logical_id: gatepitch_id as u64,
                        fvalue: clock.bar_phase,
                    },
                );
                params_push_wrapper(
                    data.lg.0,
                    ParamMsg {
                        idx: gatepitch::PARAM_CLOCK_INC,
                        logical_id: gatepitch_id as u64,
                        fvalue: clock.bar_phase_increment,
                    },
                );
            }
        }
    }
}

fn sync_effect_modulator_transport_clock_params(
    data: &mut AudioCallbackData,
    clock: HostTransportClock,
) {
    for chain in &data.state.pattern.effect_chains {
        for slot in chain {
            let modulator_id = slot.modulator_node_id.load(Ordering::Relaxed);
            if modulator_id == 0 {
                continue;
            }
            unsafe {
                dispatch_voice_modulator_transport_clock(data.lg.0, modulator_id as u64, clock);
            }
        }
    }

    let Ok(gates) = data.bus_gate_runtime.try_lock() else {
        return;
    };
    for gate in gates.iter() {
        for slot in &gate.effect_slots {
            if slot.modulator_node_id == 0 {
                continue;
            }
            unsafe {
                dispatch_voice_modulator_transport_clock(
                    data.lg.0,
                    slot.modulator_node_id as u64,
                    clock,
                );
            }
        }
    }
}

fn sync_dj_mixer_transport_phase(data: &mut AudioCallbackData, block_start_sample: u64) {
    let playing = data.state.transport.playing.load(Ordering::Relaxed);
    let bpm = data.state.transport.bpm.load(Ordering::Relaxed).max(1) as f64;
    let total_beats = if playing {
        block_start_sample as f64 * bpm / (data.sample_rate * 60.0)
    } else {
        0.0
    };
    let beat_phase = crate::dj_mixer::transport_beat_phase(total_beats);

    for chain in &data.state.pattern.effect_chains {
        for slot in chain {
            let param_idx = slot.transport_phase_param_idx.load(Ordering::Relaxed);
            if param_idx == crate::effects::NO_TRANSPORT_PHASE_PARAM {
                continue;
            }
            let node_id = slot.node_id.load(Ordering::Relaxed);
            if node_id == 0 {
                continue;
            }
            unsafe {
                dispatch_transport_phase(data.lg.0, node_id as u64, param_idx, beat_phase);
            }
        }
    }

    let Ok(gates) = data.bus_gate_runtime.try_lock() else {
        return;
    };
    for gate in gates.iter() {
        for slot in &gate.effect_slots {
            let param_idx = slot.transport_phase_param_idx;
            if param_idx == crate::effects::NO_TRANSPORT_PHASE_PARAM || slot.node_id == 0 {
                continue;
            }
            unsafe {
                dispatch_transport_phase(data.lg.0, slot.node_id as u64, param_idx, beat_phase);
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

fn rack_slot_accepts_trigger(slot: &RackSlotSnapshot, has_solo: bool) -> bool {
    if has_solo {
        slot.solo && !slot.mute
    } else {
        !slot.mute
    }
}

#[derive(Clone, Copy, Debug)]
struct ResolvedRackSlotParams {
    base_note_offset: f32,
    gain: f32,
    pan: f32,
    max_polyphony: usize,
    mute: bool,
    solo: bool,
}

fn resolve_rack_slot_params(slot: &RackSlotSnapshot, step: usize) -> ResolvedRackSlotParams {
    let value = |param: RackSlotParam| param.clamp(slot.param_value_at_step(param, step));
    let max_polyphony = value(RackSlotParam::MaxPolyphony)
        .round()
        .clamp(1.0, MAX_VOICES as f32) as usize;
    ResolvedRackSlotParams {
        base_note_offset: value(RackSlotParam::BaseNote),
        gain: value(RackSlotParam::Gain),
        pan: value(RackSlotParam::Pan),
        max_polyphony,
        mute: value(RackSlotParam::Mute) > 0.5,
        solo: value(RackSlotParam::Solo) > 0.5,
    }
}

fn rack_slot_accepts_resolved(params: ResolvedRackSlotParams, has_solo: bool) -> bool {
    if has_solo {
        params.solo && !params.mute
    } else {
        !params.mute
    }
}

fn rack_slot_matches_routing(
    slot: &RackSlotSnapshot,
    routing: RackRouting,
    transpose: f32,
) -> bool {
    match routing {
        RackRouting::Broadcast => true,
        RackRouting::ByPitch => slot.pad_note == Some(transpose.round() as i32),
    }
}

fn rack_slot_playback_transpose(routing: RackRouting, transpose: f32) -> f32 {
    match routing {
        RackRouting::Broadcast => transpose,
        RackRouting::ByPitch => 0.0,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RackSlotNoteOff {
    Custom { logical_id: u64 },
    Sampler { logical_id: u64 },
}

fn collect_rack_slot_active_voice_releases(
    voice_pools: &mut [VoicePool],
    custom_engine_pools: &mut [CustomEnginePool],
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    track_idx: usize,
    slot_idx: usize,
    slot: &RackSlotSnapshot,
    release_sample: u64,
) -> Vec<RackSlotNoteOff> {
    let mut note_offs = Vec::new();
    match slot.instrument_type {
        InstrumentType::Sampler => {
            let Some(pool_id) = rack_slot_pool_index(track_idx, slot_idx) else {
                return note_offs;
            };
            if pool_id >= voice_pools.len() {
                return note_offs;
            }
            let active: Vec<(u64, i32)> = voice_pools[pool_id].voices
                [..voice_pools[pool_id].num_voices]
                .iter()
                .filter(|voice| voice.active && voice.logical_id != 0)
                .map(|voice| (voice.logical_id, voice.gatepitch_id))
                .collect();
            for (lid, gatepitch_id) in active {
                voice_pools[pool_id].release_voice_by_logical_id(lid);
                cancel_gate_off_for_lid(countdown_events, block_events, lid);
                if gatepitch_id > 0 {
                    note_offs.push(RackSlotNoteOff::Custom {
                        logical_id: gatepitch_id as u64,
                    });
                }
                note_offs.push(RackSlotNoteOff::Sampler { logical_id: lid });
            }
        }
        InstrumentType::Custom => {
            let Some(engine_id) = slot.track_sound_state.engine_id else {
                return note_offs;
            };
            if engine_id >= custom_engine_pools.len() {
                return note_offs;
            }
            let free_patch = slot.instrument_run_mode == CustomInstrumentRunMode::FreePatch;
            let lids: Vec<u64> = custom_engine_pools[engine_id].voices
                [..custom_engine_pools[engine_id].num_voices]
                .iter()
                .filter(|voice| voice.active && voice.assigned_track == Some(track_idx))
                .map(|voice| voice.logical_id)
                .collect();
            for lid in lids {
                if free_patch {
                    custom_engine_pools[engine_id].release_free_patch_voice_by_logical_id(lid);
                } else {
                    custom_engine_pools[engine_id].release_voice_by_logical_id(lid, release_sample);
                }
                cancel_gate_off_for_lid(countdown_events, block_events, lid);
                note_offs.push(RackSlotNoteOff::Custom { logical_id: lid });
            }
        }
        InstrumentType::Modulator | InstrumentType::Rack => {}
    }
    note_offs
}

fn collect_rack_choke_group_voice_releases(
    voice_pools: &mut [VoicePool],
    custom_engine_pools: &mut [CustomEnginePool],
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    parent_track_idx: usize,
    rack: &RackTrackSnapshot,
    triggering_slot_idx: usize,
    choke_group: u8,
    release_sample: u64,
) -> Vec<RackSlotNoteOff> {
    let mut note_offs = Vec::new();
    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        if slot_idx == triggering_slot_idx || slot.choke_group != Some(choke_group) {
            continue;
        }
        note_offs.extend(collect_rack_slot_active_voice_releases(
            voice_pools,
            custom_engine_pools,
            countdown_events,
            block_events,
            parent_track_idx,
            slot_idx,
            slot,
            release_sample,
        ));
    }
    note_offs
}

fn dispatch_rack_slot_note_offs(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    note_offs: Vec<RackSlotNoteOff>,
) {
    for note_off in note_offs {
        let seq = next_block_event_sequence(data);
        unsafe {
            match note_off {
                RackSlotNoteOff::Custom { logical_id } => {
                    send_custom_note_off(data.lg.0, logical_id, frame_offset, seq);
                }
                RackSlotNoteOff::Sampler { logical_id } => {
                    send_sampler_note_off(data.lg.0, logical_id, frame_offset, seq);
                }
            }
        }
    }
}

fn release_rack_choke_group_voices(
    data: &mut AudioCallbackData,
    parent_track_idx: usize,
    rack: &RackTrackSnapshot,
    triggering_slot_idx: usize,
    choke_group: u8,
    frame_offset: u32,
) {
    let release_sample = data.rendered_samples.load(Ordering::Acquire) + frame_offset as u64;
    let note_offs = collect_rack_choke_group_voice_releases(
        &mut data.voice_pools,
        &mut data.custom_engine_pools,
        &mut data.countdown_events,
        &mut data.block_events,
        parent_track_idx,
        rack,
        triggering_slot_idx,
        choke_group,
        release_sample,
    );
    dispatch_rack_slot_note_offs(data, frame_offset, note_offs);
}

unsafe fn push_rack_slot_panner_params(
    lg: *mut LiveGraph,
    slot_pan_lid: u64,
    params: ResolvedRackSlotParams,
    muted_by_solo: bool,
) {
    if slot_pan_lid == 0 {
        return;
    }
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
            logical_id: slot_pan_lid,
            fvalue: params.gain,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::stereo_panner::STEREO_PANNER_PARAM_PAN,
            logical_id: slot_pan_lid,
            fvalue: params.pan,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::stereo_panner::STEREO_PANNER_PARAM_MUTE,
            logical_id: slot_pan_lid,
            fvalue: if params.mute { 1.0 } else { 0.0 },
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
            logical_id: slot_pan_lid,
            fvalue: if muted_by_solo { 1.0 } else { 0.0 },
        },
    );
}

fn rack_sampler_warp_runtime(
    state: &SequencerState,
    warp_enabled: f32,
    warp_mode: f32,
    sample_bpm: f32,
) -> (f32, f32, f32, f32, f32, f32, f32) {
    let project_bpm = state.transport.bpm.load(Ordering::Relaxed).max(1) as f32;
    let sample_bpm = sample_bpm.clamp(20.0, 400.0);
    if warp_enabled <= 0.5 {
        return (0.0, warp_mode, 1.0, sample_bpm, project_bpm, 0.0, 0.0);
    }
    // All warp modes run without analysis now (Beats falls back to the pure
    // beat grid when no onset table is present), so racks support every mode.
    let ratio = (project_bpm / sample_bpm).clamp(0.01, 32.0);
    (1.0, warp_mode, ratio, sample_bpm, project_bpm, 0.0, 0.0)
}

fn push_active_keyboard_voice(
    voices: &mut [ActiveKeyboardVoice; MAX_RACK_SLOTS],
    voice_count: &mut usize,
    voice: ActiveKeyboardVoice,
) {
    if *voice_count >= MAX_RACK_SLOTS || voice.logical_id == 0 {
        return;
    }
    voices[*voice_count] = voice;
    *voice_count += 1;
}

fn fire_live_keyboard_rack_note(
    data: &mut AudioCallbackData,
    parent_track_idx: usize,
    trigger: &KeyboardTrigger,
    transpose: f32,
    rack: RackTrackSnapshot,
) -> bool {
    let gate_mode = if data.state.pattern.track_params[parent_track_idx].is_gate_on() {
        1.0
    } else {
        0.0
    };
    let has_solo = rack.slots.iter().any(|slot| slot.solo);
    let mut active_voices = [ActiveKeyboardVoice::default(); MAX_RACK_SLOTS];
    let mut active_voice_count = 0;

    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        if !rack_slot_matches_routing(slot, rack.routing, transpose) {
            continue;
        }
        if !rack_slot_accepts_trigger(slot, has_solo) {
            continue;
        }
        if let Some(choke_group) = slot.choke_group {
            release_rack_choke_group_voices(
                data,
                parent_track_idx,
                &rack,
                slot_idx,
                choke_group,
                0,
            );
        }
        let playback_transpose = rack_slot_playback_transpose(rack.routing, transpose);
        let instrument_params = resolve_rack_slot_instrument_defaults(&slot.instrument_slot);
        match slot.instrument_type {
            InstrumentType::Sampler => {
                let Some(pool_id) = rack_slot_pool_index(parent_track_idx, slot_idx) else {
                    continue;
                };
                if pool_id >= data.voice_pools.len() {
                    continue;
                }
                let sampler_lid = data.state.runtime.sampler_lids[pool_id].load(Ordering::Acquire);
                if sampler_lid == 0 {
                    continue;
                }
                let sampler_params = resolve_rack_slot_sampler_defaults(&slot.instrument_slot);
                let attack_samples = sampler_params.attack_ms * data.sample_rate as f32 / 1000.0;
                let release_samples = sampler_params.release_ms * data.sample_rate as f32 / 1000.0;
                let loop_xfade_samples =
                    sampler_params.loop_xfade_ms * data.sample_rate as f32 / 1000.0;
                let (
                    warp_enabled,
                    warp_mode,
                    warp_ratio,
                    warp_sample_bpm,
                    warp_project_bpm,
                    warp_ptr_lo,
                    warp_ptr_hi,
                ) = rack_sampler_warp_runtime(
                    &data.state,
                    sampler_params.warp_enabled,
                    sampler_params.warp_mode,
                    sampler_params.sample_bpm,
                );
                data.voice_pools[pool_id].polyphonic = slot.max_polyphony > 1;
                let (voice_lid, gatepitch_id, modulator_id) = {
                    let voice = data.voice_pools[pool_id]
                        .allocate_voice_retriggering_same_note_with_limit(
                            playback_transpose,
                            slot.max_polyphony,
                        );
                    (voice.logical_id, voice.gatepitch_id, voice.modulator_id)
                };
                if voice_lid == 0 {
                    continue;
                }
                if modulator_id > 0 {
                    let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
                    unsafe {
                        dispatch_sampler_modulator_params_to_voice(
                            data.lg.0,
                            modulator_id as u64,
                            &instrument_params,
                        );
                        send_custom_trigger(
                            data.lg.0,
                            gatepitch_id as u64,
                            0,
                            gatepitch_seq,
                            custom_pitch_hz(
                                playback_transpose + slot.instrument_base_note_offset,
                                0.0,
                            ),
                            trigger.velocity,
                        );
                    }
                }
                let sampler_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    send_keyboard_trigger(
                        data.lg.0,
                        voice_lid,
                        0,
                        sampler_seq,
                        playback_transpose + slot.instrument_base_note_offset,
                        trigger.velocity,
                        sampler_params.playback_speed,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        sampler_params.start_point,
                        sampler_params.end_point,
                        sampler_params.instrument_enabled,
                        sampler_params.reverse,
                        sampler_params.loop_mode,
                        loop_xfade_samples,
                        sampler_params.sr_hz,
                        warp_enabled,
                        warp_mode,
                        warp_ratio,
                        warp_sample_bpm,
                        warp_project_bpm,
                        warp_ptr_lo,
                        warp_ptr_hi,
                        sampler_params.warp_preserve,
                        sampler_params.warp_seg_loop_mode,
                        sampler_params.warp_seg_envelope,
                        sampler_params.scrub,
                    );
                    dispatch_sampler_extra_params_to_voice(
                        data.lg.0,
                        voice_lid,
                        &instrument_params,
                    );
                }
                push_active_keyboard_voice(
                    &mut active_voices,
                    &mut active_voice_count,
                    ActiveKeyboardVoice {
                        logical_id: voice_lid,
                        gatepitch_id,
                        target: ActiveKeyboardVoiceTarget::Sampler { pool_id },
                    },
                );
            }
            InstrumentType::Custom => {
                let Some(engine_id) = slot.track_sound_state.engine_id else {
                    continue;
                };
                if engine_id >= data.custom_engine_pools.len() {
                    continue;
                }
                let free_patch = slot.instrument_run_mode == CustomInstrumentRunMode::FreePatch;
                let allocation = if free_patch {
                    let Some(allocation) = data.custom_engine_pools[engine_id]
                        .allocate_free_patch_voice(parent_track_idx, playback_transpose)
                    else {
                        continue;
                    };
                    allocation
                } else {
                    data.custom_engine_pools[engine_id].allocate_voice(
                        parent_track_idx,
                        playback_transpose,
                        slot.max_polyphony > 1,
                        slot.max_polyphony,
                    )
                };
                let voice_idx = allocation.voice_idx;
                data.custom_engine_pools[engine_id].note_voice_allocated(engine_id, voice_idx);
                let voice_lid = allocation.logical_id;
                let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                    .load(Ordering::Relaxed);
                let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id]
                    [voice_idx]
                    .load(Ordering::Relaxed);
                if voice_lid == 0 || synth_id == 0 || modulator_id == 0 {
                    continue;
                }
                let key_locked_instrument_params = key_locked_snapshot_instrument_params(
                    &slot.instrument_slot,
                    playback_transpose,
                    slot.instrument_base_note_offset,
                    None,
                    &instrument_params,
                );
                let instrument_fingerprint = rack_slot_sound_fingerprint(
                    slot,
                    &key_locked_instrument_params,
                    slot.instrument_base_note_offset,
                );
                let pitch_hz =
                    custom_pitch_hz(playback_transpose, slot.instrument_base_note_offset);
                cancel_gate_off_for_lid(
                    &mut data.countdown_events,
                    &mut data.block_events,
                    voice_lid,
                );
                unsafe {
                    route_custom_voice_to_track(
                        data.lg.0,
                        &data.state,
                        engine_id,
                        voice_idx,
                        parent_track_idx,
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != instrument_fingerprint
                    {
                        dispatch_instrument_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            modulator_id as u64,
                            &key_locked_instrument_params,
                        );
                    }
                    if allocation.stole_active_voice || slot.max_polyphony <= 1 || free_patch {
                        let off_seq = next_event_sequence_from(&mut data.event_seq);
                        send_custom_note_off(data.lg.0, voice_lid, 0, off_seq);
                    }
                    let on_seq = next_event_sequence_from(&mut data.event_seq);
                    send_custom_trigger(
                        data.lg.0,
                        voice_lid,
                        0,
                        on_seq,
                        pitch_hz,
                        trigger.velocity,
                    );
                }
                data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint =
                    instrument_fingerprint;
                push_active_keyboard_voice(
                    &mut active_voices,
                    &mut active_voice_count,
                    ActiveKeyboardVoice {
                        logical_id: voice_lid,
                        gatepitch_id: 0,
                        target: ActiveKeyboardVoiceTarget::Custom {
                            engine_id,
                            free_patch,
                        },
                    },
                );
            }
            InstrumentType::Modulator | InstrumentType::Rack => {}
        }
    }

    if active_voice_count == 0 {
        return false;
    }
    store_active_keyboard_note(
        &mut data.active_keyboard_notes,
        parent_track_idx,
        trigger.transpose,
        midi_note_from_transpose(
            transpose,
            f32::from_bits(
                data.state.pattern.instrument_base_note_offsets[parent_track_idx]
                    .load(Ordering::Relaxed),
            ),
        ),
        &active_voices[..active_voice_count],
    );
    true
}

#[allow(clippy::too_many_arguments)]
fn fire_rack_slot_note(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    parent_track_idx: usize,
    slot_idx: usize,
    slot: &RackSlotSnapshot,
    slot_params: ResolvedRackSlotParams,
    transpose: f32,
    velocity: f32,
    speed: f32,
    gate_samples: f32,
    gate_mode: f32,
    instrument_params: &ScheduledInstrumentParams,
    sampler_params: Option<ScheduledSamplerParams>,
    instrument_fingerprint: u64,
) {
    match slot.instrument_type {
        InstrumentType::Sampler => {
            let Some(pool_id) = rack_slot_pool_index(parent_track_idx, slot_idx) else {
                return;
            };
            if pool_id >= data.voice_pools.len() {
                return;
            }
            let sampler_lid = data.state.runtime.sampler_lids[pool_id].load(Ordering::Acquire);
            if sampler_lid == 0 {
                return;
            }
            let sampler_params = sampler_params.unwrap_or_default();
            let attack_samples = sampler_params.attack_ms * data.sample_rate as f32 / 1000.0;
            let release_samples = sampler_params.release_ms * data.sample_rate as f32 / 1000.0;
            let loop_xfade_samples =
                sampler_params.loop_xfade_ms * data.sample_rate as f32 / 1000.0;
            let (
                warp_enabled,
                warp_mode,
                warp_ratio,
                warp_sample_bpm,
                warp_project_bpm,
                warp_ptr_lo,
                warp_ptr_hi,
            ) = rack_sampler_warp_runtime(
                &data.state,
                sampler_params.warp_enabled,
                sampler_params.warp_mode,
                sampler_params.sample_bpm,
            );
            data.voice_pools[pool_id].polyphonic = slot_params.max_polyphony > 1;
            let voice = data.voice_pools[pool_id].allocate_voice_retriggering_same_note_with_limit(
                transpose,
                slot_params.max_polyphony,
            );
            let voice_lid = voice.logical_id;
            let lid = if voice_lid != 0 {
                voice_lid
            } else {
                sampler_lid
            };
            let gatepitch_id = voice.gatepitch_id;
            if voice.modulator_id > 0 {
                let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    dispatch_sampler_modulator_params_to_voice(
                        data.lg.0,
                        voice.modulator_id as u64,
                        instrument_params,
                    );
                    send_custom_trigger(
                        data.lg.0,
                        voice.gatepitch_id as u64,
                        frame_offset,
                        gatepitch_seq,
                        custom_pitch_hz(transpose + slot_params.base_note_offset, 0.0),
                        velocity,
                    );
                }
            }
            let sampler_seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                dispatch_sampler_extra_params_to_voice(data.lg.0, lid, instrument_params);
                send_trigger(
                    data.lg.0,
                    lid,
                    frame_offset,
                    sampler_seq,
                    velocity,
                    speed * sampler_params.playback_speed,
                    gate_samples,
                    attack_samples,
                    release_samples,
                    gate_mode,
                    transpose + slot_params.base_note_offset,
                    sampler_params.start_point,
                    sampler_params.end_point,
                    sampler_params.instrument_enabled,
                    sampler_params.reverse,
                    sampler_params.loop_mode,
                    loop_xfade_samples,
                    sampler_params.sr_hz,
                    warp_enabled,
                    warp_mode,
                    warp_ratio,
                    warp_sample_bpm,
                    warp_project_bpm,
                    warp_ptr_lo,
                    warp_ptr_hi,
                    sampler_params.warp_preserve,
                    sampler_params.warp_seg_loop_mode,
                    sampler_params.warp_seg_envelope,
                    sampler_params.scrub,
                );
            }
            if gate_mode > 0.5 {
                schedule_gate_off_event(
                    data,
                    pool_id,
                    lid,
                    frame_offset,
                    gate_samples as f64,
                    GateOffTarget::Sampler { gatepitch_id },
                );
            }
        }
        InstrumentType::Custom => {
            let Some(engine_id) = slot.track_sound_state.engine_id else {
                return;
            };
            if engine_id >= data.custom_engine_pools.len() {
                return;
            }
            let free_patch = slot.instrument_run_mode == CustomInstrumentRunMode::FreePatch;
            let allocation = if free_patch {
                let Some(allocation) = data.custom_engine_pools[engine_id]
                    .allocate_free_patch_voice(parent_track_idx, transpose)
                else {
                    return;
                };
                allocation
            } else {
                data.custom_engine_pools[engine_id].allocate_voice(
                    parent_track_idx,
                    transpose,
                    slot_params.max_polyphony > 1,
                    slot_params.max_polyphony,
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
            let pitch_hz = custom_pitch_hz(transpose, slot_params.base_note_offset);
            cancel_gate_off_for_lid(&mut data.countdown_events, &mut data.block_events, lid);
            unsafe {
                if allocation.stole_active_voice || slot_params.max_polyphony <= 1 || free_patch {
                    let off_seq = next_event_sequence_from(&mut data.event_seq);
                    send_custom_note_off(data.lg.0, lid, frame_offset, off_seq);
                }
                route_custom_voice_to_track(
                    data.lg.0,
                    &data.state,
                    engine_id,
                    voice_idx,
                    parent_track_idx,
                );
                if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                    != instrument_fingerprint
                {
                    dispatch_instrument_params_to_voice(
                        data.lg.0,
                        synth_id as u64,
                        modulator_id as u64,
                        instrument_params,
                    );
                }
            }
            data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint =
                instrument_fingerprint;
            let on_seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                send_custom_trigger(data.lg.0, lid, frame_offset, on_seq, pitch_hz, velocity);
            }
            if gate_mode > 0.5 {
                schedule_gate_off_event(
                    data,
                    parent_track_idx,
                    lid,
                    frame_offset,
                    gate_samples as f64,
                    GateOffTarget::Custom {
                        engine_id,
                        free_patch,
                    },
                );
            }
        }
        InstrumentType::Modulator | InstrumentType::Rack => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn fire_rack_resolved(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    track_idx: usize,
    step: usize,
    key_lock_plock_step: Option<usize>,
    samples_per_step: f64,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    rack: RackTrackSnapshot,
) {
    let (track_pan, track_send, gate_mode) = {
        let tp = &data.state.pattern.track_params[track_idx];
        (
            tp.get_pan(),
            tp.get_send(),
            if tp.is_gate_on() { 1.0 } else { 0.0 },
        )
    };
    let chop = (resolved.chop.round() as u32).max(1);
    let total_gate = (resolved.duration as f64 * samples_per_step) as f32;
    let rack_gate = total_gate / chop as f32;

    let pan_lid = data.state.runtime.pan_lids[track_idx].load(Ordering::Acquire);
    if pan_lid != 0 {
        let effective_pan = (track_pan + resolved.pan).clamp(-1.0, 1.0);
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

    let resolved_slot_params: Vec<ResolvedRackSlotParams> = rack
        .slots
        .iter()
        .map(|slot| resolve_rack_slot_params(slot, step))
        .collect();
    let has_solo = resolved_slot_params.iter().any(|params| params.solo);
    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        let Some(slot_params) = resolved_slot_params.get(slot_idx).copied() else {
            continue;
        };
        let muted_by_solo = has_solo && !slot_params.solo;
        let slot_pan_lid =
            data.state.runtime.rack_slot_pan_lids[track_idx][slot_idx].load(Ordering::Acquire);
        unsafe {
            push_rack_slot_panner_params(data.lg.0, slot_pan_lid, slot_params, muted_by_solo);
        }
        if !rack_slot_accepts_resolved(slot_params, has_solo) {
            continue;
        }
        let instrument_params = resolve_rack_slot_instrument_params(&slot.instrument_slot, step);
        let sampler_params = if slot.instrument_type == InstrumentType::Sampler {
            Some(resolve_rack_slot_sampler_params(
                &slot.instrument_slot,
                step,
            ))
        } else {
            None
        };

        if chord.count > 0 {
            for n in 0..chord.count {
                let note_duration = chord.durations[n].max(0.0);
                let note_total_gate = if note_duration > 0.0 {
                    (note_duration as f64 * samples_per_step) as f32
                } else {
                    total_gate
                };
                let note_gate = note_total_gate / chop as f32;
                let transpose = resolved_chord_transpose(
                    chord.notes[n],
                    chord.step_transpose,
                    resolved.transpose,
                );
                if !rack_slot_matches_routing(slot, rack.routing, transpose) {
                    continue;
                }
                if let Some(choke_group) = slot.choke_group {
                    release_rack_choke_group_voices(
                        data,
                        track_idx,
                        &rack,
                        slot_idx,
                        choke_group,
                        frame_offset,
                    );
                }
                let playback_transpose = rack_slot_playback_transpose(rack.routing, transpose);
                let note_instrument_params = if slot.instrument_type == InstrumentType::Custom {
                    key_locked_snapshot_instrument_params(
                        &slot.instrument_slot,
                        playback_transpose,
                        slot_params.base_note_offset,
                        key_lock_plock_step,
                        &instrument_params,
                    )
                } else {
                    instrument_params.clone()
                };
                let instrument_fingerprint = rack_slot_sound_fingerprint(
                    slot,
                    &note_instrument_params,
                    slot_params.base_note_offset,
                );
                fire_rack_slot_note(
                    data,
                    frame_offset,
                    track_idx,
                    slot_idx,
                    slot,
                    slot_params,
                    playback_transpose,
                    resolved.velocity,
                    resolved.speed,
                    note_gate,
                    gate_mode,
                    &note_instrument_params,
                    sampler_params,
                    instrument_fingerprint,
                );
            }
        } else {
            if !rack_slot_matches_routing(slot, rack.routing, resolved.transpose) {
                continue;
            }
            if let Some(choke_group) = slot.choke_group {
                release_rack_choke_group_voices(
                    data,
                    track_idx,
                    &rack,
                    slot_idx,
                    choke_group,
                    frame_offset,
                );
            }
            let playback_transpose = rack_slot_playback_transpose(rack.routing, resolved.transpose);
            let note_instrument_params = if slot.instrument_type == InstrumentType::Custom {
                key_locked_snapshot_instrument_params(
                    &slot.instrument_slot,
                    playback_transpose,
                    slot_params.base_note_offset,
                    key_lock_plock_step,
                    &instrument_params,
                )
            } else {
                instrument_params.clone()
            };
            let instrument_fingerprint = rack_slot_sound_fingerprint(
                slot,
                &note_instrument_params,
                slot_params.base_note_offset,
            );
            fire_rack_slot_note(
                data,
                frame_offset,
                track_idx,
                slot_idx,
                slot,
                slot_params,
                playback_transpose,
                resolved.velocity,
                resolved.speed,
                rack_gate,
                gate_mode,
                &note_instrument_params,
                sampler_params,
                instrument_fingerprint,
            );
        }
    }

    let send_lid = data.state.runtime.send_lids[track_idx].load(Ordering::Acquire);
    if send_lid != 0 {
        unsafe {
            params_push_wrapper(
                data.lg.0,
                ParamMsg {
                    idx: 0,
                    logical_id: send_lid,
                    fvalue: track_send,
                },
            );
        }
    }
    cancel_chops_for_track(
        &mut data.countdown_events,
        &mut data.block_events,
        track_idx,
    );
    data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
}

/// Fire a resolved step trigger for a track (handles gate, chop setup, envelope params).
/// Uses voice pool allocation for polyphonic playback.
fn midi_note_from_transpose(transpose: f32, base_note_offset: f32) -> Option<u8> {
    let note = (60.0 + transpose + base_note_offset).round();
    (0.0..=127.0).contains(&note).then_some(note as u8)
}

fn mark_resolved_note_activity(
    data: &AudioCallbackData,
    frame_offset: u32,
    track_idx: usize,
    samples_per_step: f64,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
) {
    let base_note_offset = f32::from_bits(
        data.state.pattern.instrument_base_note_offsets[track_idx].load(Ordering::Relaxed),
    );
    let start_sample = data.rendered_samples.load(Ordering::Acquire) + frame_offset as u64;
    let mark = |transpose: f32, duration_steps: f32| {
        let Some(note) = midi_note_from_transpose(transpose, base_note_offset) else {
            return;
        };
        let gate_samples = (duration_steps.max(0.0) as f64 * samples_per_step.max(0.0))
            .round()
            .max(1.0) as u64;
        data.state.mark_scheduled_note_active_until(
            track_idx,
            note,
            start_sample.saturating_add(gate_samples),
        );
    };

    if chord.count > 0 {
        for idx in 0..chord.count.min(MAX_VOICES) {
            let duration = if chord.durations[idx] > 0.0 {
                chord.durations[idx]
            } else {
                resolved.duration
            };
            mark(
                crate::scheduled_event::resolved_chord_transpose(
                    chord.notes[idx],
                    chord.step_transpose,
                    resolved.transpose,
                ),
                duration,
            );
        }
    } else {
        mark(resolved.transpose, resolved.duration);
    }
}

fn fire_resolved(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    track_idx: usize,
    step: usize,
    key_lock_plock_step: Option<usize>,
    samples_per_step: f64,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
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
    mark_resolved_note_activity(
        data,
        frame_offset,
        track_idx,
        samples_per_step,
        resolved,
        chord,
    );
    if instrument_type == InstrumentType::Rack {
        let rack = data
            .scheduler_snapshot
            .tracks
            .get(track_idx)
            .and_then(|track| track.rack_track.clone());
        if let Some(rack) = rack {
            fire_rack_resolved(
                data,
                frame_offset,
                track_idx,
                step,
                key_lock_plock_step,
                samples_per_step,
                resolved,
                chord,
                rack,
            );
        }
        return;
    }
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
            warp_preserve: live_slot_resolved_node_param_value(
                inst_slot,
                step,
                crate::sampler::PARAM_WARP_PRESERVE,
                crate::sampler::WARP_PRESERVE_DEFAULT as f32,
            ),
            warp_seg_loop_mode: live_slot_resolved_node_param_value(
                inst_slot,
                step,
                crate::sampler::PARAM_WARP_SEG_LOOP_MODE,
                crate::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
            ),
            warp_seg_envelope: live_slot_resolved_node_param_value(
                inst_slot,
                step,
                crate::sampler::PARAM_WARP_SEG_ENVELOPE,
                crate::sampler::WARP_SEG_ENVELOPE_DEFAULT,
            ),
        }
    };
    let sampler_params = scheduled_sampler_params.unwrap_or_else(fallback_sampler_params);
    let attack_ms = sampler_params.attack_ms;
    let release_ms = sampler_params.release_ms;
    let attack_samples = attack_ms * data.sample_rate as f32 / 1000.0;
    let release_samples = release_ms * data.sample_rate as f32 / 1000.0;
    let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
    let track_send = tp.get_send();
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
    let warp_preserve = sampler_params.warp_preserve;
    let warp_seg_loop_mode = sampler_params.warp_seg_loop_mode;
    let warp_seg_envelope = sampler_params.warp_seg_envelope;
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
        let seq = next_block_event_sequence(data);
        unsafe {
            dispatch_modulator_params(data.lg.0, lid, &instrument_params);
            trigger_modulator_pulse(
                data.lg.0,
                lid,
                frame_offset,
                seq,
                chop_gate,
                resolved.velocity,
            );
        }
        if chop > 1 {
            schedule_chop_events(
                data,
                track_idx,
                frame_offset,
                chop_gate as f64,
                chop_gate as f64,
                chop - 1,
                step,
                chop_gate,
            );
        } else {
            cancel_chops_for_track(
                &mut data.countdown_events,
                &mut data.block_events,
                track_idx,
            );
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
                let key_locked_params = key_locked_live_instrument_params(
                    &data.state,
                    track_idx,
                    transpose,
                    base_note_offset,
                    key_lock_plock_step,
                    &instrument_params,
                );
                let note_fingerprint = instrument_param_bundle_fingerprint(
                    engine_id,
                    base_note_offset,
                    &key_locked_params,
                    &instrument_tensor_params,
                );
                cancel_gate_off_for_lid(&mut data.countdown_events, &mut data.block_events, lid);
                if allocation.stole_active_voice || !track_polyphonic || free_patch {
                    let off_seq = next_event_sequence_from(&mut data.event_seq);
                    unsafe {
                        send_custom_note_off(data.lg.0, lid, frame_offset, off_seq);
                        route_custom_voice_to_track(
                            data.lg.0,
                            &data.state,
                            engine_id,
                            voice_idx,
                            track_idx,
                        );
                        if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                            != note_fingerprint
                        {
                            dispatch_instrument_params_to_voice(
                                data.lg.0,
                                synth_id as u64,
                                modulator_id as u64,
                                &key_locked_params,
                            );
                            dispatch_instrument_tensor_params_to_voice(
                                data.lg.0,
                                synth_id as u64,
                                &instrument_tensor_params,
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
                            != note_fingerprint
                        {
                            dispatch_instrument_params_to_voice(
                                data.lg.0,
                                synth_id as u64,
                                modulator_id as u64,
                                &key_locked_params,
                            );
                            dispatch_instrument_tensor_params_to_voice(
                                data.lg.0,
                                synth_id as u64,
                                &instrument_tensor_params,
                            );
                        }
                    }
                }
                data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint =
                    note_fingerprint;
                let on_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    send_custom_trigger(data.lg.0, lid, frame_offset, on_seq, pitch_hz, velocity);
                }
                if gate_mode > 0.5 {
                    schedule_gate_off_event(
                        data,
                        track_idx,
                        lid,
                        frame_offset,
                        note_total_gate as f64,
                        GateOffTarget::Custom {
                            engine_id,
                            free_patch,
                        },
                    );
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
                let gatepitch_id = voice.gatepitch_id;
                if voice.modulator_id > 0 {
                    let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
                    unsafe {
                        dispatch_sampler_modulator_params_to_voice(
                            data.lg.0,
                            voice.modulator_id as u64,
                            &instrument_params,
                        );
                        send_custom_trigger(
                            data.lg.0,
                            voice.gatepitch_id as u64,
                            frame_offset,
                            gatepitch_seq,
                            custom_pitch_hz(transpose + base_note_offset, 0.0),
                            velocity,
                        );
                    }
                }
                let sampler_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    dispatch_sampler_extra_params_to_voice(data.lg.0, lid, &instrument_params);
                    send_trigger(
                        data.lg.0,
                        lid,
                        frame_offset,
                        sampler_seq,
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
                        warp_preserve,
                        warp_seg_loop_mode,
                        warp_seg_envelope,
                        scrub,
                    );
                }
                if gate_mode > 0.5 {
                    schedule_gate_off_event(
                        data,
                        track_idx,
                        lid,
                        frame_offset,
                        note_total_gate as f64,
                        GateOffTarget::Sampler { gatepitch_id },
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
            let key_locked_params = key_locked_live_instrument_params(
                &data.state,
                track_idx,
                transpose,
                base_note_offset,
                key_lock_plock_step,
                &instrument_params,
            );
            let note_fingerprint = instrument_param_bundle_fingerprint(
                engine_id,
                base_note_offset,
                &key_locked_params,
                &instrument_tensor_params,
            );
            cancel_gate_off_for_lid(&mut data.countdown_events, &mut data.block_events, lid);
            if allocation.stole_active_voice || !track_polyphonic || free_patch {
                let off_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    send_custom_note_off(data.lg.0, lid, frame_offset, off_seq);
                    route_custom_voice_to_track(
                        data.lg.0,
                        &data.state,
                        engine_id,
                        voice_idx,
                        track_idx,
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != note_fingerprint
                    {
                        dispatch_instrument_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            modulator_id as u64,
                            &key_locked_params,
                        );
                        dispatch_instrument_tensor_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            &instrument_tensor_params,
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
                        != note_fingerprint
                    {
                        dispatch_instrument_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            modulator_id as u64,
                            &key_locked_params,
                        );
                        dispatch_instrument_tensor_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            &instrument_tensor_params,
                        );
                    }
                }
            }
            data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint = note_fingerprint;
            let on_seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                send_custom_trigger(data.lg.0, lid, frame_offset, on_seq, pitch_hz, velocity);
            }
            if gate_mode > 0.5 {
                schedule_gate_off_event(
                    data,
                    track_idx,
                    lid,
                    frame_offset,
                    total_gate as f64,
                    GateOffTarget::Custom {
                        engine_id,
                        free_patch,
                    },
                );
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
            let gatepitch_id = voice.gatepitch_id;
            if voice.modulator_id > 0 {
                let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    dispatch_sampler_modulator_params_to_voice(
                        data.lg.0,
                        voice.modulator_id as u64,
                        &instrument_params,
                    );
                    send_custom_trigger(
                        data.lg.0,
                        voice.gatepitch_id as u64,
                        frame_offset,
                        gatepitch_seq,
                        custom_pitch_hz(transpose + base_note_offset, 0.0),
                        velocity,
                    );
                }
            }
            let sampler_seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                dispatch_sampler_extra_params_to_voice(data.lg.0, lid, &instrument_params);
                send_trigger(
                    data.lg.0,
                    lid,
                    frame_offset,
                    sampler_seq,
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
                    warp_preserve,
                    warp_seg_loop_mode,
                    warp_seg_envelope,
                    scrub,
                );
            }
            if gate_mode > 0.5 {
                schedule_gate_off_event(
                    data,
                    track_idx,
                    lid,
                    frame_offset,
                    total_gate as f64,
                    GateOffTarget::Sampler { gatepitch_id },
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
                    fvalue: track_send,
                },
            );
        }
    }

    data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);

    // Setup chop re-triggers (sampler only — custom instruments handle gate duration internally)
    if !is_custom && chop > 1 {
        schedule_chop_events(
            data,
            track_idx,
            frame_offset,
            samples_per_step / chop as f64,
            samples_per_step / chop as f64,
            chop - 1,
            step,
            chop_gate,
        );
    } else {
        cancel_chops_for_track(
            &mut data.countdown_events,
            &mut data.block_events,
            track_idx,
        );
    }
}

fn dispatch_chop_event(data: &mut AudioCallbackData, event: ChopEvent, frame_offset: u32) {
    let track_idx = event.track_idx;
    if track_idx >= data.state.active_track_count() {
        return;
    }
    if InstrumentType::from_runtime_flag(
        data.state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed),
    ) == InstrumentType::Modulator
    {
        let lid = data.state.runtime.modulator_lids[track_idx].load(Ordering::Acquire);
        if lid == 0 {
            return;
        }
        let slot = &data.state.pattern.instrument_slots[track_idx];
        let rise = slot
            .plocks
            .get(event.step, 0)
            .unwrap_or_else(|| slot.defaults.get(0));
        let fall = slot
            .plocks
            .get(event.step, 1)
            .unwrap_or_else(|| slot.defaults.get(1));
        let velocity = data.state.pattern.step_data[track_idx].get(event.step, StepParam::Velocity);
        let seq = next_event_sequence_from(&mut data.event_seq);
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
            trigger_modulator_pulse(data.lg.0, lid, frame_offset, seq, event.chop_gate, velocity);
        }
        data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
        return;
    }

    let tp = &data.state.pattern.track_params[track_idx];
    let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
    let chop_inst_slot = &data.state.pattern.instrument_slots[track_idx];
    let attack_samples = chop_inst_slot
        .plocks
        .get(event.step, 0)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(0))
        * data.sample_rate as f32
        / 1000.0;
    let release_samples = chop_inst_slot
        .plocks
        .get(event.step, 1)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(1))
        * data.sample_rate as f32
        / 1000.0;
    let chop_start = chop_inst_slot
        .plocks
        .get(event.step, 2)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(2));
    let chop_end = chop_inst_slot
        .plocks
        .get(event.step, 3)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(3));
    let chop_reverse = chop_inst_slot
        .plocks
        .get(event.step, 5)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(5));
    let chop_loop_mode = chop_inst_slot
        .plocks
        .get(event.step, 6)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(6));
    let chop_loop_xfade_samples = chop_inst_slot
        .plocks
        .get(event.step, 7)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(7))
        * data.sample_rate as f32
        / 1000.0;
    let chop_sr_hz = chop_inst_slot
        .plocks
        .get(event.step, 8)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(8));
    let chop_warp_enabled = chop_inst_slot
        .plocks
        .get(event.step, 9)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(9));
    let chop_warp_mode = chop_inst_slot
        .plocks
        .get(event.step, 10)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(10));
    let chop_sample_bpm = chop_inst_slot
        .plocks
        .get(event.step, 11)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(11));
    let chop_playback_speed = chop_inst_slot
        .plocks
        .get(event.step, 12)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(12));
    let chop_scrub = chop_inst_slot
        .plocks
        .get(event.step, 13)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(13));
    let chop_warp_preserve = live_slot_resolved_node_param_value(
        chop_inst_slot,
        event.step,
        crate::sampler::PARAM_WARP_PRESERVE,
        crate::sampler::WARP_PRESERVE_DEFAULT as f32,
    );
    let chop_warp_seg_loop_mode = live_slot_resolved_node_param_value(
        chop_inst_slot,
        event.step,
        crate::sampler::PARAM_WARP_SEG_LOOP_MODE,
        crate::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
    );
    let chop_warp_seg_envelope = live_slot_resolved_node_param_value(
        chop_inst_slot,
        event.step,
        crate::sampler::PARAM_WARP_SEG_ENVELOPE,
        crate::sampler::WARP_SEG_ENVELOPE_DEFAULT,
    );
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
    let transpose = sd.get(event.step, StepParam::Transpose);
    let voice = data.voice_pools[track_idx].allocate_voice_retriggering_same_note(transpose);
    let voice_lid = voice.logical_id;
    let sampler_lid = data.state.runtime.sampler_lids[track_idx].load(Ordering::Acquire);
    let lid = if voice_lid != 0 {
        voice_lid
    } else {
        sampler_lid
    };
    if lid == 0 {
        return;
    }
    if voice.modulator_id > 0 {
        let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
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
                frame_offset,
                gatepitch_seq,
                custom_pitch_hz(transpose + chop_base_note_offset, 0.0),
                sd.get(event.step, StepParam::Velocity),
            );
        }
    }
    let sampler_seq = next_event_sequence_from(&mut data.event_seq);
    unsafe {
        dispatch_sampler_extra_defaults_to_voice(data.lg.0, &data.state, track_idx, lid);
        send_trigger(
            data.lg.0,
            lid,
            frame_offset,
            sampler_seq,
            sd.get(event.step, StepParam::Velocity),
            sd.get(event.step, StepParam::Speed) * chop_playback_speed,
            event.chop_gate,
            attack_samples,
            release_samples,
            gate_mode,
            transpose + chop_base_note_offset,
            chop_start,
            chop_end,
            chop_inst_slot
                .plocks
                .get(event.step, 4)
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
            chop_warp_preserve,
            chop_warp_seg_loop_mode,
            chop_warp_seg_envelope,
            chop_scrub,
        );
    }
    data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
}

fn audio_callback(data: &mut AudioCallbackData, output: &mut [f32]) {
    let callback_start = Instant::now();
    let nframes = output.len() / data.num_channels;
    data.current_callback_nframes = nframes;
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
        clear_countdown_events(data);
        data.event_seq = 0;
    }
    let scheduler_snapshot_version = data.state.scheduler_snapshot_version();
    if scheduler_snapshot_version != data.scheduler_snapshot_version {
        data.scheduler_snapshot = data.state.latest_scheduler_snapshot();
        data.scheduler_snapshot_version = scheduler_snapshot_version;
    }
    let block_start_sample = data.rendered_samples.load(Ordering::Acquire);
    let block_end_sample = block_start_sample + nframes as u64;
    let host_transport_clock = compute_host_transport_clock(data, block_start_sample);
    sync_bus_gate_params(data, block_start_sample);
    sync_instrument_host_clock_params(data, host_transport_clock);
    sync_effect_modulator_transport_clock_params(data, host_transport_clock);
    sync_dj_mixer_transport_phase(data, block_start_sample);

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
    sync_rack_voice_pools(data, num_tracks);
    sync_free_patch_transport_routes(data, num_tracks);

    // Process keyboard triggers
    let mut processed_keyboard_trigger = false;
    while let Ok(kt) = data.keyboard_rx.try_recv() {
        processed_keyboard_trigger = true;
        if kt.track >= num_tracks {
            continue;
        }
        let instrument_type = InstrumentType::from_runtime_flag(
            data.state.runtime.instrument_type_flags[kt.track].load(Ordering::Relaxed),
        );
        let is_custom = instrument_type == InstrumentType::Custom;
        let track_polyphonic = data.state.pattern.track_params[kt.track].is_polyphonic();
        let track_max_polyphony = data.state.pattern.track_params[kt.track].get_max_polyphony();
        data.voice_pools[kt.track].polyphonic = track_polyphonic;
        let base_note_offset = f32::from_bits(
            data.state.pattern.instrument_base_note_offsets[kt.track].load(Ordering::Relaxed),
        );

        if kt.note_off {
            if let Some(active_note) =
                take_active_keyboard_note(&mut data.active_keyboard_notes, kt.track, kt.transpose)
            {
                release_active_keyboard_note(data, active_note, 0, block_end_sample);
            }
        } else {
            // Note-on: allocate voice and trigger
            enforce_mute_group_for_winning_track(data, kt.track, block_start_sample, 0);
            let resolved_transpose = resolve_live_keyboard_transpose(
                &data.state,
                data.accumulator_states[kt.track],
                kt.track,
                kt.transpose,
            );
            if instrument_type == InstrumentType::Rack {
                let rack = data
                    .scheduler_snapshot
                    .tracks
                    .get(kt.track)
                    .and_then(|track| track.rack_track.clone());
                if let Some(rack) = rack {
                    if !fire_live_keyboard_rack_note(data, kt.track, &kt, resolved_transpose, rack)
                    {
                        continue;
                    }
                } else {
                    continue;
                }
            } else if is_custom {
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
                let default_params =
                    resolve_snapshot_instrument_defaults(&data.scheduler_snapshot, kt.track);
                let default_tensor_params =
                    resolve_live_instrument_tensor_defaults(&data.state, kt.track);
                let key_locked_params = key_locked_live_instrument_params(
                    &data.state,
                    kt.track,
                    resolved_transpose,
                    base_note_offset,
                    None,
                    &default_params,
                );
                let fingerprint = instrument_param_bundle_fingerprint(
                    engine_id,
                    base_note_offset,
                    &key_locked_params,
                    &default_tensor_params,
                );
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
                cancel_gate_off_for_lid(
                    &mut data.countdown_events,
                    &mut data.block_events,
                    voice_lid,
                );
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
                        dispatch_instrument_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            modulator_id as u64,
                            &key_locked_params,
                        );
                        dispatch_instrument_tensor_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            &default_tensor_params,
                        );
                    }
                }
                data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint = fingerprint;
                if allocation.stole_active_voice || !track_polyphonic || free_patch {
                    let off_seq = next_block_event_sequence(data);
                    unsafe {
                        send_custom_note_off(data.lg.0, voice_lid, 0, off_seq);
                    }
                }
                let on_seq = next_block_event_sequence(data);
                unsafe {
                    send_custom_trigger(data.lg.0, voice_lid, 0, on_seq, pitch_hz, kt.velocity);
                }
                store_active_keyboard_note(
                    &mut data.active_keyboard_notes,
                    kt.track,
                    kt.transpose,
                    midi_note_from_transpose(resolved_transpose, base_note_offset),
                    &[ActiveKeyboardVoice {
                        logical_id: voice_lid,
                        gatepitch_id: 0,
                        target: ActiveKeyboardVoiceTarget::Custom {
                            engine_id,
                            free_patch,
                        },
                    }],
                );
            } else {
                let voice = data.voice_pools[kt.track]
                    .allocate_voice_retriggering_same_note(resolved_transpose);
                let voice_lid = voice.logical_id;
                if voice_lid == 0 {
                    continue;
                }
                let tp = &data.state.pattern.track_params[kt.track];
                let Some(kb_inst_slot) = data
                    .scheduler_snapshot
                    .tracks
                    .get(kt.track)
                    .map(|track| &track.instrument_slot)
                else {
                    continue;
                };
                let kb_default =
                    |param_idx: usize| kb_inst_slot.defaults.get(param_idx).copied().unwrap_or(0.0);
                let kb_instrument_params =
                    resolve_snapshot_instrument_defaults(&data.scheduler_snapshot, kt.track);
                let attack_samples = kb_default(0) * data.sample_rate as f32 / 1000.0;
                let release_samples = kb_default(1) * data.sample_rate as f32 / 1000.0;
                let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
                let kb_start = kb_default(2);
                let kb_end = kb_default(3);
                let kb_enabled = kb_default(4);
                let kb_reverse = kb_default(5);
                let kb_loop_mode = kb_default(6);
                let kb_loop_xfade_samples = kb_default(7) * data.sample_rate as f32 / 1000.0;
                let kb_sr_hz = kb_default(8);
                let kb_playback_speed = kb_default(12);
                let kb_warp_preserve = snapshot_slot_default_node_param_value(
                    kb_inst_slot,
                    crate::sampler::PARAM_WARP_PRESERVE,
                    crate::sampler::WARP_PRESERVE_DEFAULT as f32,
                );
                let kb_warp_seg_loop_mode = snapshot_slot_default_node_param_value(
                    kb_inst_slot,
                    crate::sampler::PARAM_WARP_SEG_LOOP_MODE,
                    crate::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
                );
                let kb_warp_seg_envelope = snapshot_slot_default_node_param_value(
                    kb_inst_slot,
                    crate::sampler::PARAM_WARP_SEG_ENVELOPE,
                    crate::sampler::WARP_SEG_ENVELOPE_DEFAULT,
                );
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
                    kb_default(9),
                    kb_default(10),
                    kb_default(11),
                );
                if voice.modulator_id > 0 {
                    let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
                    unsafe {
                        dispatch_sampler_modulator_params_to_voice(
                            data.lg.0,
                            voice.modulator_id as u64,
                            &kb_instrument_params,
                        );
                        send_custom_trigger(
                            data.lg.0,
                            voice.gatepitch_id as u64,
                            0,
                            gatepitch_seq,
                            custom_pitch_hz(resolved_transpose + base_note_offset, 0.0),
                            kt.velocity,
                        );
                    }
                }
                let sampler_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    send_keyboard_trigger(
                        data.lg.0,
                        voice_lid,
                        0,
                        sampler_seq,
                        resolved_transpose + base_note_offset,
                        kt.velocity,
                        kb_playback_speed,
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
                        kb_warp_preserve,
                        kb_warp_seg_loop_mode,
                        kb_warp_seg_envelope,
                        kb_default(13),
                    );
                    dispatch_sampler_extra_params_to_voice(
                        data.lg.0,
                        voice_lid,
                        &kb_instrument_params,
                    );
                }
                store_active_keyboard_note(
                    &mut data.active_keyboard_notes,
                    kt.track,
                    kt.transpose,
                    midi_note_from_transpose(resolved_transpose, base_note_offset),
                    &[ActiveKeyboardVoice {
                        logical_id: voice_lid,
                        gatepitch_id: voice.gatepitch_id,
                        target: ActiveKeyboardVoiceTarget::Sampler { pool_id: kt.track },
                    }],
                );
            }
            data.state.transport.trigger_flash[kt.track].store(255, Ordering::Relaxed);
        }
    }
    for track in 0..num_tracks {
        data.state.replace_live_notes(
            track,
            data.active_keyboard_notes[track]
                .iter()
                .filter_map(|note| note.and_then(|note| note.midi_note)),
        );
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
            clear_transport_countdown_events(data);
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
                        dispatch_voice_modulator_bpm(data.lg.0, logical_id as u64, bpm_f);
                    }
                }
            }
        }
        for pool in &data.voice_pools {
            for voice in pool.voices.iter().take(pool.num_voices) {
                if voice.modulator_id > 0 {
                    unsafe {
                        dispatch_voice_modulator_bpm(data.lg.0, voice.modulator_id as u64, bpm_f);
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

    let current_pattern_epoch = data.state.transport.pattern_epoch.load(Ordering::Relaxed);
    collect_due_countdown_events(data, nframes, current_pattern_epoch);
    drain_scheduled_events_for_callback(data, block_start_sample, nframes, current_pattern_epoch);
    dispatch_block_events(data, block_start_sample);

    let custom_release_tail_samples =
        (CUSTOM_ENGINE_RELEASE_TAIL_SECONDS * data.sample_rate).round() as u64;
    for engine_id in 0..data.state.runtime.engine_voice_counts.len() {
        if data.state.runtime.engine_voice_counts[engine_id].load(Ordering::Acquire) == 0 {
            continue;
        }
        data.custom_engine_pools[engine_id].shrink_released_voices(
            engine_id,
            block_end_sample,
            custom_release_tail_samples,
        );
    }

    let probe_render = data.trace_audio && data.trace_render_probe_blocks > 0;
    if probe_render {
        eprintln!(
            "audio-trace: render-start callback={} nframes={nframes} tracks={num_tracks} countdown_len={} rendered_samples={block_start_sample}",
            data.trace_callback_counter,
            data.countdown_events.len(),
        );
    }
    let render_start = Instant::now();
    render_chunk(data, output);
    let render_elapsed = render_start.elapsed();
    if probe_render {
        let (chunk_peak_l, chunk_peak_r) = interleaved_peak(output, data.num_channels);
        eprintln!(
            "audio-trace: render-done callback={} nframes={nframes} elapsed_us={} peak_l={chunk_peak_l:.6} peak_r={chunk_peak_r:.6}",
            data.trace_callback_counter,
            render_elapsed.as_micros(),
        );
        data.trace_render_probe_blocks -= 1;
    }
    if render_elapsed.as_millis() >= 10 {
        eprintln!(
            "audio: slow render_chunk; nframes={nframes} elapsed_ms={} countdown_len={} block_start_sample={block_start_sample}",
            render_elapsed.as_millis(),
            data.countdown_events.len(),
        );
    }
    data.rendered_samples
        .store(block_end_sample, Ordering::Release);
    data.state.set_audio_rendered_sample(block_end_sample);

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
                    "audio-trace: silent while voices active callbacks={} streak={} tracks={num_tracks} custom_active={active_custom_voices} sampler_active={active_sampler_voices} rendered_samples={} topology_epoch={} playing={} countdown_len={} late_events={} dropped_events={}",
                    data.trace_callback_counter,
                    data.trace_silent_active_callbacks,
                    data.rendered_samples.load(Ordering::Acquire),
                    topology_epoch,
                    data.state.transport.playing.load(Ordering::Relaxed),
                    data.countdown_events.len(),
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
    // Initialize voice pools from state
    let mut voice_pools: Vec<VoicePool> =
        (0..MAX_SAMPLER_POOLS).map(|_| VoicePool::new()).collect();
    let mut custom_engine_pools: Vec<CustomEnginePool> = (0..MAX_INSTRUMENT_ENGINES)
        .map(|_| CustomEnginePool::new())
        .collect();

    // Pre-populate voice pools for any existing tracks
    let num_tracks = state.active_track_count();
    for t in 0..num_tracks {
        sync_sampler_voice_pool(&state, t, &mut voice_pools[t]);

        if let Some(engine_id) = track_engine_id(&state, t) {
            sync_custom_engine_pool(&state, engine_id, &mut custom_engine_pools[engine_id]);
        }
    }

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
    let initial_scheduler_snapshot_version = state.scheduler_snapshot_version();
    let initial_scheduler_snapshot = state.latest_scheduler_snapshot();
    let trace_audio = env_flag("TINYSEQ_AUDIO_TRACE", false);
    crate::voice_modulator::set_process_stats_enabled(trace_audio);
    if trace_audio {
        eprintln!("audio-trace: enabled");
    }

    let mut cb_data = AudioCallbackData {
        lg: LiveGraphPtr(lg),
        state,
        num_channels,
        sample_rate: sample_rate as f64,
        last_bpm: 0,
        last_mod_reset_counter: 0,
        voice_pools,
        custom_engine_pools,
        scheduler_snapshot: initial_scheduler_snapshot,
        scheduler_snapshot_version: initial_scheduler_snapshot_version,
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
        countdown_events: Vec::with_capacity(SCHEDULED_COUNTDOWN_CAPACITY),
        block_events: Vec::with_capacity(SCHEDULED_BLOCK_SCRATCH_CAPACITY),
        block_events_need_sort: false,
        current_callback_nframes: block_size,
        rendered_samples: Arc::clone(&rendered_samples),
        bus_gate_runtime,
        bus_gate_playheads,
        bus_gate_clocks: Vec::new(),
        bus_gate_was_playing: false,
        bus_gate_play_start_sample: 0,
        dropped_scheduled_events: 0,
        late_scheduled_events: 0,
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
        bus_gate_target_at, clear_active_keyboard_note_by_lid,
        collect_rack_choke_group_voice_releases, free_patch_transport_route_cache_is_fresh,
        free_patch_transport_route_target, instrument_sound_fingerprint,
        key_locked_live_instrument_params, mute_group_winner_for_block_events,
        rack_slot_matches_routing, rack_slot_playback_transpose, resolve_live_instrument_defaults,
        resolve_live_keyboard_transpose, resolve_snapshot_instrument_defaults,
        resolved_chord_transpose, sampler_warp_runtime, select_output_channels,
        select_output_config, store_active_keyboard_note, swing_delay_samples,
        take_active_keyboard_note, track_accepts_scheduled_trigger, ActiveKeyboardNote,
        ActiveKeyboardVoice, ActiveKeyboardVoiceTarget, BlockEvent, BlockEventKind, ChopEvent,
        CountdownEvent, CountdownEventKind, CustomEnginePool, FreePatchTransportRouteState,
        FreePatchTransportRouteTarget, GateOffEvent, GateOffTarget, OutputDeviceConfig,
        OutputFormatRange, RackSlotNoteOff, FALLBACK_SAMPLE_RATE,
    };
    use crate::accumulator::{AccumulatorRuntimeState, ResolvedStep};
    use crate::analysis::{pack_ptr, OnsetTableShared};
    use crate::effects::{
        EffectDescriptor, EffectSlotSnapshot, EffectSlotState, ParamDescriptor, ParamKind,
        ParamScaling, TensorParamDescriptor,
    };
    use crate::scheduled_event::{
        ScheduledChordData, ScheduledEvent, ScheduledEventKind, ScheduledInstrumentParamTarget,
        ScheduledInstrumentParams, ScheduledInstrumentTensorParams, ScheduledSamplerParams,
    };
    use crate::sequencer::{
        CustomInstrumentRunMode, InstrumentType, RackRouting, RackSlotParamPlocks,
        RackSlotSnapshot, RackTrackSnapshot, SequencerState, SwingResolution, TrackSoundState,
    };
    use crate::voice::VoicePool;

    fn active_keyboard_notes_fixture(
    ) -> [[Option<ActiveKeyboardNote>; crate::voice::MAX_VOICES]; crate::sequencer::MAX_TRACKS]
    {
        [[None; crate::voice::MAX_VOICES]; crate::sequencer::MAX_TRACKS]
    }

    fn rack_routing_test_slot(pad_note: Option<i32>) -> RackSlotSnapshot {
        RackSlotSnapshot {
            instrument_type: InstrumentType::Sampler,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: 0.0,
            pad_note,
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            max_polyphony: 1,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: EffectSlotSnapshot::new_empty(),
            track_sound_state: TrackSoundState::default(),
            sample_id: None,
        }
    }

    fn audio_test_param(name: &str, default: f32, node_param_idx: u32) -> ParamDescriptor {
        ParamDescriptor {
            name: name.to_string(),
            min: -20_000.0,
            max: 20_000.0,
            default,
            kind: ParamKind::Continuous { unit: None },
            scaling: ParamScaling::Linear,
            node_param_idx,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        }
    }

    fn param_value(params: &ScheduledInstrumentParams, idx: u64) -> Option<f32> {
        params
            .iter()
            .find(|param| param.target == ScheduledInstrumentParamTarget::Synth && param.idx == idx)
            .map(|param| param.value)
    }

    #[test]
    fn custom_pitch_midi_note_uses_c4_zero_and_base_offset() {
        assert_eq!(super::custom_pitch_midi_note(0.0, 0.0), 60);
        assert_eq!(super::custom_pitch_midi_note(2.0, 0.0), 62);
        assert_eq!(super::custom_pitch_midi_note(0.0, 12.0), 72);
    }

    #[test]
    fn key_locked_live_instrument_params_apply_per_note_after_base_offset() {
        let state = SequencerState::new(1, Vec::new());
        let desc = EffectDescriptor {
            name: "key-lock-test".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                audio_test_param("cutoff", 100.0, 0),
                audio_test_param("detune", 0.0, 1),
            ],
        };
        let slot = &state.pattern.instrument_slots[0];
        slot.apply_descriptor(&desc, 42);
        slot.defaults.set(0, 100.0);
        slot.defaults.set(1, 0.0);
        slot.set_key_lock(60, 1, -12.0);
        slot.set_key_lock(62, 1, -7.0);
        slot.set_key_lock(72, 1, 7.0);

        let base_params = resolve_live_instrument_defaults(&state, 0);
        let c4_params = key_locked_live_instrument_params(&state, 0, 0.0, 0.0, None, &base_params);
        let d4_params = key_locked_live_instrument_params(&state, 0, 2.0, 0.0, None, &base_params);
        let offset_params =
            key_locked_live_instrument_params(&state, 0, 0.0, 12.0, None, &base_params);

        assert_eq!(param_value(&c4_params, 1), Some(-12.0));
        assert_eq!(param_value(&d4_params, 1), Some(-7.0));
        assert_eq!(param_value(&offset_params, 1), Some(7.0));
    }

    #[test]
    fn live_keyboard_defaults_use_macro_effective_scheduler_snapshot() {
        let desc = EffectDescriptor::builtin_filter();
        let state = SequencerState::new(1, vec![crate::sequencer::default_empty_effect_chain()]);
        state.pattern.instrument_slots[0].apply_descriptor(&desc, 42);
        let param_id = state.pattern.instrument_slots[0].param_node_id(0);
        let mut macros = crate::macro_engine::MacroEngine::default();
        let macro_id = macros
            .create_macro("keyboard", crate::macro_engine::MacroKind::Mapped)
            .expect("macro");
        macros
            .add_mapping(
                macro_id,
                crate::macro_engine::MacroMapping::new_resolved(
                    0,
                    crate::process::ParamTarget::InstrumentParam {
                        param: desc.params[0].name.clone(),
                        param_id,
                    },
                    Some(0),
                    100.0,
                    900.0,
                    crate::macro_engine::MacroCurve::Linear,
                )
                .expect("mapping"),
            )
            .expect("mapped");

        let snapshot = state.publish_macro_overrides(macros.override_snapshot());
        let defaults = resolve_snapshot_instrument_defaults(&snapshot, 0);

        assert_eq!(
            param_value(&defaults, desc.params[0].node_param_idx as u64),
            Some(100.0),
            "a macro at zero must initialize a new keyboard voice at its mapped minimum"
        );
    }

    #[test]
    fn key_locked_live_instrument_params_keep_step_plocks_per_param() {
        let state = SequencerState::new(1, Vec::new());
        let desc = EffectDescriptor {
            name: "key-lock-precedence-test".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                audio_test_param("cutoff", 100.0, 0),
                audio_test_param("detune", 0.0, 1),
            ],
        };
        let slot = &state.pattern.instrument_slots[0];
        slot.apply_descriptor(&desc, 42);
        slot.defaults.set(0, 100.0);
        slot.defaults.set(1, 0.0);
        slot.set_key_lock(60, 0, 3_000.0);
        slot.set_key_lock(60, 1, -40.0);
        slot.set_plock(5, 0, 2_000.0);

        let mut base_params = resolve_live_instrument_defaults(&state, 0);
        for param in &mut base_params {
            if param.target == ScheduledInstrumentParamTarget::Synth && param.idx == 0 {
                param.value = 2_000.0;
            }
        }

        let merged = key_locked_live_instrument_params(&state, 0, 0.0, 0.0, Some(5), &base_params);

        assert_eq!(
            param_value(&merged, 0),
            Some(2_000.0),
            "valid step p-lock should win for the same param"
        );
        assert_eq!(
            param_value(&merged, 1),
            Some(-40.0),
            "step p-lock on cutoff must not suppress detune's key lock"
        );
    }

    #[test]
    fn key_locked_live_instrument_params_drop_stale_key_lock_identity() {
        let state = SequencerState::new(1, Vec::new());
        let desc = EffectDescriptor {
            name: "key-lock-stale-id-test".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![audio_test_param("cutoff", 100.0, 0)],
        };
        let slot = &state.pattern.instrument_slots[0];
        slot.apply_descriptor(&desc, 42);
        slot.defaults.set(0, 100.0);
        slot.key_locks.set(60, 0, 3_000.0);

        let base_params = resolve_live_instrument_defaults(&state, 0);
        let merged = key_locked_live_instrument_params(&state, 0, 0.0, 0.0, None, &base_params);

        assert_eq!(param_value(&merged, 0), Some(100.0));
    }

    #[test]
    fn rack_by_pitch_matches_only_the_selected_pad_and_plays_fixed_slot_pitch() {
        let c1_slot = rack_routing_test_slot(Some(0));
        let d1_slot = rack_routing_test_slot(Some(2));
        let unmapped_slot = rack_routing_test_slot(None);

        assert!(rack_slot_matches_routing(
            &c1_slot,
            RackRouting::Broadcast,
            9.0
        ));
        assert!(rack_slot_matches_routing(
            &c1_slot,
            RackRouting::ByPitch,
            0.0
        ));
        assert!(rack_slot_matches_routing(
            &d1_slot,
            RackRouting::ByPitch,
            2.0
        ));
        assert!(!rack_slot_matches_routing(
            &c1_slot,
            RackRouting::ByPitch,
            2.0
        ));
        assert!(!rack_slot_matches_routing(
            &unmapped_slot,
            RackRouting::ByPitch,
            0.0
        ));

        assert_eq!(
            rack_slot_playback_transpose(RackRouting::Broadcast, 7.0),
            7.0
        );
        assert_eq!(rack_slot_playback_transpose(RackRouting::ByPitch, 7.0), 0.0);
    }

    #[test]
    fn rack_choke_group_releases_matching_sampler_and_custom_slots() {
        let mut released_sampler = rack_routing_test_slot(Some(0));
        released_sampler.choke_group = Some(2);
        let mut triggering_sampler = rack_routing_test_slot(Some(1));
        triggering_sampler.choke_group = Some(2);
        let mut unrelated_sampler = rack_routing_test_slot(Some(2));
        unrelated_sampler.choke_group = Some(3);

        let released_pool_id = crate::sequencer::rack_slot_pool_index(0, 0).unwrap();
        let unrelated_pool_id = crate::sequencer::rack_slot_pool_index(0, 2).unwrap();
        let mut voice_pools: Vec<VoicePool> =
            (0..=unrelated_pool_id).map(|_| VoicePool::new()).collect();
        let mut custom_engine_pools: Vec<CustomEnginePool> =
            (0..=6).map(|_| CustomEnginePool::new()).collect();
        let mut countdown_events = vec![CountdownEvent {
            remaining_samples: 32.0,
            period_samples: 0.0,
            repeats: 0,
            pattern_epoch: 0,
            seq: 0,
            kind: CountdownEventKind::GateOff(GateOffEvent {
                track_idx: 0,
                logical_id: 10,
                target: GateOffTarget::Sampler { gatepitch_id: 0 },
            }),
        }];
        let mut block_events = vec![BlockEvent {
            frame_offset: 4,
            seq: 1,
            kind: BlockEventKind::GateOff(GateOffEvent {
                track_idx: 0,
                logical_id: 20,
                target: GateOffTarget::Sampler { gatepitch_id: 0 },
            }),
        }];
        voice_pools[released_pool_id].add_voice(10, 100);
        voice_pools[released_pool_id].voices[0].active = true;
        voice_pools[unrelated_pool_id].add_voice(20, 200);
        voice_pools[unrelated_pool_id].voices[0].active = true;

        let sampler_rack = RackTrackSnapshot {
            routing: RackRouting::ByPitch,
            slots: vec![released_sampler, triggering_sampler, unrelated_sampler],
        };
        let sampler_note_offs = collect_rack_choke_group_voice_releases(
            &mut voice_pools,
            &mut custom_engine_pools,
            &mut countdown_events,
            &mut block_events,
            0,
            &sampler_rack,
            1,
            2,
            1_000,
        );

        assert!(
            !voice_pools[released_pool_id].voices[0].active,
            "matching sampler slot should be released"
        );
        assert!(
            voice_pools[unrelated_pool_id].voices[0].active,
            "unrelated sampler choke group should remain active"
        );
        assert_eq!(
            sampler_note_offs,
            vec![RackSlotNoteOff::Sampler { logical_id: 10 }]
        );
        assert!(
            countdown_events.is_empty(),
            "released sampler gate-off countdown should be cancelled"
        );
        assert_eq!(
            block_events.len(),
            1,
            "unrelated sampler gate-off block event should remain"
        );

        let mut released_custom = rack_routing_test_slot(Some(3));
        released_custom.instrument_type = InstrumentType::Custom;
        released_custom.choke_group = Some(7);
        released_custom.track_sound_state.engine_id = Some(4);
        let mut triggering_custom = rack_routing_test_slot(Some(4));
        triggering_custom.instrument_type = InstrumentType::Custom;
        triggering_custom.choke_group = Some(7);
        triggering_custom.track_sound_state.engine_id = Some(5);
        let mut unrelated_custom = rack_routing_test_slot(Some(5));
        unrelated_custom.instrument_type = InstrumentType::Custom;
        unrelated_custom.choke_group = Some(8);
        unrelated_custom.track_sound_state.engine_id = Some(6);

        custom_engine_pools[4].add_voice(30);
        custom_engine_pools[4].voices[0].active = true;
        custom_engine_pools[4].voices[0].assigned_track = Some(0);
        custom_engine_pools[6].add_voice(40);
        custom_engine_pools[6].voices[0].active = true;
        custom_engine_pools[6].voices[0].assigned_track = Some(0);
        countdown_events.push(CountdownEvent {
            remaining_samples: 32.0,
            period_samples: 0.0,
            repeats: 0,
            pattern_epoch: 0,
            seq: 2,
            kind: CountdownEventKind::GateOff(GateOffEvent {
                track_idx: 0,
                logical_id: 30,
                target: GateOffTarget::Custom {
                    engine_id: 4,
                    free_patch: false,
                },
            }),
        });
        block_events.push(BlockEvent {
            frame_offset: 12,
            seq: 3,
            kind: BlockEventKind::GateOff(GateOffEvent {
                track_idx: 0,
                logical_id: 40,
                target: GateOffTarget::Custom {
                    engine_id: 6,
                    free_patch: false,
                },
            }),
        });

        let custom_rack = RackTrackSnapshot {
            routing: RackRouting::ByPitch,
            slots: vec![released_custom, triggering_custom, unrelated_custom],
        };
        let custom_note_offs = collect_rack_choke_group_voice_releases(
            &mut voice_pools,
            &mut custom_engine_pools,
            &mut countdown_events,
            &mut block_events,
            0,
            &custom_rack,
            1,
            7,
            1_016,
        );

        assert!(
            !custom_engine_pools[4].voices[0].active,
            "matching custom slot should be released"
        );
        assert_eq!(
            custom_engine_pools[4].voices[0].release_started_sample,
            Some(1_016),
            "custom release should use the provided release sample"
        );
        assert!(
            custom_engine_pools[6].voices[0].active,
            "unrelated custom choke group should remain active"
        );
        assert_eq!(
            custom_note_offs,
            vec![RackSlotNoteOff::Custom { logical_id: 30 }]
        );
        assert!(
            countdown_events.is_empty(),
            "released custom gate-off countdown should be cancelled"
        );
        assert_eq!(
            block_events.len(),
            2,
            "unrelated sampler and custom gate-off block events should remain"
        );
    }

    #[test]
    fn dj_mixer_slots_carry_explicit_transport_phase_param() {
        let dj = EffectDescriptor::builtin_dj_mixer();
        let dj_slot = EffectSlotState::new(&dj, 42);
        assert_eq!(
            dj_slot.transport_phase_param_idx.load(Ordering::Relaxed),
            crate::dj_mixer::DJ_MIXER_PARAM_TRANSPORT_BEAT_PHASE as u32
        );
        assert_eq!(
            EffectSlotSnapshot::capture(&dj_slot).transport_phase_param_idx,
            crate::dj_mixer::DJ_MIXER_PARAM_TRANSPORT_BEAT_PHASE as u32
        );

        let str8 = EffectDescriptor::builtin_str8_delay();
        let str8_slot = EffectSlotState::new(&str8, 43);
        assert_eq!(
            str8_slot.transport_phase_param_idx.load(Ordering::Relaxed),
            crate::effects::NO_TRANSPORT_PHASE_PARAM
        );
    }

    #[test]
    fn active_keyboard_note_stores_all_rack_slot_voices_for_one_key() {
        let mut notes = active_keyboard_notes_fixture();
        let voices = [
            ActiveKeyboardVoice {
                logical_id: 11,
                gatepitch_id: 21,
                target: ActiveKeyboardVoiceTarget::Sampler { pool_id: 64 },
            },
            ActiveKeyboardVoice {
                logical_id: 12,
                gatepitch_id: 0,
                target: ActiveKeyboardVoiceTarget::Custom {
                    engine_id: 7,
                    free_patch: false,
                },
            },
        ];

        store_active_keyboard_note(&mut notes, 0, 3.0, Some(63), &voices);
        let note = take_active_keyboard_note(&mut notes, 0, 3.0).unwrap();

        assert_eq!(note.source_transpose, 3.0);
        assert_eq!(note.midi_note, Some(63));
        assert_eq!(note.voices(), &voices);
    }

    #[test]
    fn active_keyboard_note_clear_by_lid_preserves_other_slot_voices() {
        let mut notes = active_keyboard_notes_fixture();
        let voices = [
            ActiveKeyboardVoice {
                logical_id: 11,
                gatepitch_id: 21,
                target: ActiveKeyboardVoiceTarget::Sampler { pool_id: 64 },
            },
            ActiveKeyboardVoice {
                logical_id: 12,
                gatepitch_id: 0,
                target: ActiveKeyboardVoiceTarget::Custom {
                    engine_id: 7,
                    free_patch: false,
                },
            },
            ActiveKeyboardVoice {
                logical_id: 13,
                gatepitch_id: 23,
                target: ActiveKeyboardVoiceTarget::Sampler { pool_id: 65 },
            },
        ];

        store_active_keyboard_note(&mut notes, 0, 3.0, Some(63), &voices);
        clear_active_keyboard_note_by_lid(&mut notes, 12);
        let note = take_active_keyboard_note(&mut notes, 0, 3.0).unwrap();

        assert_eq!(note.voices(), &[voices[0], voices[2]]);
    }

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

    fn test_block_trigger(seq: u64, track: usize) -> BlockEvent {
        BlockEvent {
            frame_offset: 128,
            seq,
            kind: BlockEventKind::Scheduled(ScheduledEvent {
                pattern_epoch: 1,
                sample_time: 128,
                kind: ScheduledEventKind::ResolvedTrigger {
                    track,
                    step: 0,
                    samples_per_step: 1024.0,
                    resolved: ResolvedStep {
                        duration: 1.0,
                        velocity: 1.0,
                        speed: 1.0,
                        aux_a: 0.0,
                        aux_b: 0.0,
                        transpose: 0.0,
                        pan: 0.0,
                        chop: 1.0,
                    },
                    chord: ScheduledChordData {
                        count: 0,
                        notes: [0.0; crate::voice::MAX_VOICES],
                        durations: [0.0; crate::voice::MAX_VOICES],
                        delays: [0.0; crate::voice::MAX_VOICES],
                        step_transpose: 0.0,
                    },
                    effect_params: Vec::new(),
                    instrument_params: ScheduledInstrumentParams::new(),
                    instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
                    sampler_params: ScheduledSamplerParams::default(),
                    instrument_fingerprint: 0,
                },
            }),
        }
    }

    fn test_block_network_trigger(seq: u64, track: usize) -> BlockEvent {
        BlockEvent {
            frame_offset: 128,
            seq,
            kind: BlockEventKind::Scheduled(ScheduledEvent {
                pattern_epoch: 1,
                sample_time: 128,
                kind: ScheduledEventKind::NetworkTrigger {
                    track,
                    source_neuron: 0,
                    seed: None,
                    samples_per_step: 1024.0,
                    resolved: ResolvedStep {
                        duration: 1.0,
                        velocity: 1.0,
                        speed: 1.0,
                        aux_a: 0.0,
                        aux_b: 0.0,
                        transpose: 0.0,
                        pan: 0.0,
                        chop: 1.0,
                    },
                    chord: ScheduledChordData {
                        count: 0,
                        notes: [0.0; crate::voice::MAX_VOICES],
                        durations: [0.0; crate::voice::MAX_VOICES],
                        delays: [0.0; crate::voice::MAX_VOICES],
                        step_transpose: 0.0,
                    },
                    effect_params: Vec::new(),
                    instrument_params: ScheduledInstrumentParams::new(),
                    instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
                    sampler_params: ScheduledSamplerParams::default(),
                    instrument_fingerprint: 0,
                },
            }),
        }
    }

    #[test]
    fn mute_group_same_sample_uses_highest_track_as_winner() {
        let batch = vec![test_block_trigger(0, 0), test_block_trigger(1, 2)];

        let winner = mute_group_winner_for_block_events(0, 1, &batch, |track| match track {
            0 | 2 => 1,
            _ => 0,
        });

        assert_eq!(winner, 2);
    }

    #[test]
    fn mute_group_same_sample_considers_neural_and_step_triggers_together() {
        let batch = vec![test_block_network_trigger(0, 0), test_block_trigger(1, 2)];

        let winner = mute_group_winner_for_block_events(0, 1, &batch, |track| match track {
            0 | 2 => 1,
            _ => 0,
        });

        assert_eq!(winner, 2);
    }

    #[test]
    fn mute_group_winner_ignores_off_and_other_groups() {
        let batch = vec![
            test_block_trigger(0, 0),
            test_block_trigger(1, 1),
            test_block_trigger(2, 2),
        ];

        let winner = mute_group_winner_for_block_events(0, 1, &batch, |track| match track {
            0 => 1,
            1 => 0,
            2 => 2,
            _ => 0,
        });

        assert_eq!(winner, 0);
    }

    #[test]
    fn instrument_sound_fingerprint_changes_for_tensor_default_and_plock_values() {
        let state = SequencerState::new(1, Vec::new());
        let desc = EffectDescriptor {
            name: "tensor instrument".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: vec![TensorParamDescriptor {
                name: "strike_mask".to_string(),
                shape: vec![2, 2],
                cell_offset: 64,
                default: vec![0.1, 0.2, 0.3, 0.4],
                min: 0.0,
                max: 1.0,
            }],
            params: Vec::new(),
        };
        state.pattern.instrument_slots[0].apply_descriptor(&desc, 42);

        let default_fingerprint = instrument_sound_fingerprint(&state, 0, 5, None);
        state.pattern.instrument_slots[0]
            .tensor_params
            .set_default_cell(0, 0, 0.75)
            .expect("default tensor edit");
        let edited_default_fingerprint = instrument_sound_fingerprint(&state, 0, 5, None);
        state.pattern.instrument_slots[0]
            .tensor_params
            .set_plock_cell(3, 0, 1, 0.95)
            .expect("tensor p-lock edit");
        let plocked_fingerprint = instrument_sound_fingerprint(&state, 0, 5, Some(3));

        assert_ne!(default_fingerprint, edited_default_fingerprint);
        assert_ne!(edited_default_fingerprint, plocked_fingerprint);
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
    fn sampler_warp_repitch_mode_needs_no_analysis() {
        let state = SequencerState::new(1, Vec::new());
        state.transport.bpm.store(120, Ordering::Relaxed);
        // No analysis status, no onset table: re-pitch must still engage.
        let (enabled, mode, ratio, _, _, ptr_lo, ptr_hi) = sampler_warp_runtime(
            &state,
            0,
            1.0,
            crate::sampler::WARP_MODE_REPITCH as f32,
            174.0,
        );
        assert!(enabled > 0.5);
        assert_eq!(mode.round() as i32, crate::sampler::WARP_MODE_REPITCH);
        // 174 BPM sample in a 120 BPM project: the read head must consume
        // source slower, at 120/174 ≈ 0.69 source frames per host frame.
        assert!((ratio - (120.0 / 174.0)).abs() < 0.0001);
        assert_eq!((ptr_lo, ptr_hi), (0.0, 0.0));

        // Beats mode without analysis: enabled on the pure beat grid, with a
        // null onset table (Preserve=Transients degrades to the plain grid).
        let (enabled, _, ratio, _, _, ptr_lo, ptr_hi) =
            sampler_warp_runtime(&state, 0, 1.0, 0.0, 174.0);
        assert!(enabled > 0.5);
        assert!((ratio - (120.0 / 174.0)).abs() < 0.0001);
        assert_eq!((ptr_lo, ptr_hi), (0.0, 0.0));
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
    fn countdown_gate_off_cancel_removes_matching_pending_lids() {
        let mut events = vec![
            CountdownEvent {
                remaining_samples: 100.0,
                period_samples: 0.0,
                repeats: 1,
                pattern_epoch: 1,
                seq: 0,
                kind: CountdownEventKind::GateOff(GateOffEvent {
                    track_idx: 0,
                    logical_id: 10,
                    target: GateOffTarget::Sampler { gatepitch_id: 100 },
                }),
            },
            CountdownEvent {
                remaining_samples: 100.0,
                period_samples: 0.0,
                repeats: 1,
                pattern_epoch: 1,
                seq: 1,
                kind: CountdownEventKind::GateOff(GateOffEvent {
                    track_idx: 0,
                    logical_id: 20,
                    target: GateOffTarget::Sampler { gatepitch_id: 200 },
                }),
            },
        ];
        let mut block_events = vec![
            BlockEvent {
                frame_offset: 12,
                seq: 2,
                kind: BlockEventKind::GateOff(GateOffEvent {
                    track_idx: 0,
                    logical_id: 10,
                    target: GateOffTarget::Sampler { gatepitch_id: 100 },
                }),
            },
            BlockEvent {
                frame_offset: 16,
                seq: 3,
                kind: BlockEventKind::GateOff(GateOffEvent {
                    track_idx: 0,
                    logical_id: 20,
                    target: GateOffTarget::Sampler { gatepitch_id: 200 },
                }),
            },
        ];

        super::cancel_gate_off_for_lid(&mut events, &mut block_events, 10);

        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].kind,
            CountdownEventKind::GateOff(GateOffEvent { logical_id: 20, .. })
        ));
        assert_eq!(block_events.len(), 1);
        assert!(matches!(
            block_events[0].kind,
            BlockEventKind::GateOff(GateOffEvent { logical_id: 20, .. })
        ));
    }

    #[test]
    fn chop_cancel_preserves_scheduled_triggers_for_later_mute_group_winner() {
        let scheduled_countdown = match test_block_trigger(0, 1).kind {
            BlockEventKind::Scheduled(event) => event,
            _ => unreachable!(),
        };
        let mut countdown_events = vec![
            CountdownEvent {
                remaining_samples: 32.0,
                period_samples: 0.0,
                repeats: 1,
                pattern_epoch: 1,
                seq: 0,
                kind: CountdownEventKind::Scheduled(scheduled_countdown),
            },
            CountdownEvent {
                remaining_samples: 48.0,
                period_samples: 16.0,
                repeats: 1,
                pattern_epoch: 1,
                seq: 1,
                kind: CountdownEventKind::Chop(ChopEvent {
                    track_idx: 1,
                    step: 0,
                    chop_gate: 16.0,
                }),
            },
        ];
        let mut block_events = vec![
            test_block_trigger(2, 1),
            BlockEvent {
                frame_offset: 20,
                seq: 3,
                kind: BlockEventKind::Chop(ChopEvent {
                    track_idx: 1,
                    step: 0,
                    chop_gate: 16.0,
                }),
            },
        ];

        super::cancel_chops_for_track(&mut countdown_events, &mut block_events, 1);

        assert_eq!(countdown_events.len(), 1);
        assert!(matches!(
            countdown_events[0].kind,
            CountdownEventKind::Scheduled(_)
        ));
        assert_eq!(block_events.len(), 1);
        assert!(matches!(block_events[0].kind, BlockEventKind::Scheduled(_)));
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
