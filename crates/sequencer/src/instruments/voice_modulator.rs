//! Per-voice modulation DSP and modulation-parameter metadata.

use std::os::raw::{c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::audiograph::NodeVTable;
use crate::effects::{ParamDescriptor, ParamKind, ParamScaling, SyncDivision};
use crate::sequencer::{MAX_INSTRUMENT_ENGINES, MAX_SAMPLER_POOLS};

pub const SLOT_COUNT: usize = 4;
pub const NUM_OUTPUTS: usize = SLOT_COUNT;
pub const INPUT_GATE: usize = 0;
pub const INPUT_PITCH: usize = 1;
pub const INPUT_VELOCITY: usize = 2;
pub const INPUT_TRIGGER: usize = 3;
pub const INPUT_EXT_BASE: usize = 4;
pub const INPUT_TRANSPORT_BAR_PHASE: usize = 8;
pub const INPUT_TRANSPORT_BAR_PHASE_INC: usize = 9;
pub const INPUT_COUNT: usize = 10;
pub const EXT_INPUT_COUNT: usize = 4;

/// Sentinel base for modulation-source param indices: any `node_param_idx`
/// at or above this is a modulation param routed to the modulator node, not a
/// real effect cell. It must sit ABOVE the largest real effect `node_param_idx`
/// (HEADER_SLOTS + cellId). Spectral effects like the Convolution Reverb need
/// millions of state slots, so this is set far above any plausible real index
/// (1e9 ≈ 4 GB of state — unreachable) rather than the old 1.1M.
pub const MOD_PARAM_BASE: u32 = 1_000_000_000;
/// Start of the frozen legacy window [1_000_000, 1_100_000) used by the very
/// first fixed-source layout. Only consulted when migrating old projects.
pub const LEGACY_FIXED_MOD_PARAM_BASE: u32 = 1_000_000;
/// End (exclusive) of that frozen legacy window. Pinned to the old MOD_PARAM_BASE
/// value so raising MOD_PARAM_BASE doesn't widen the legacy-detection range.
pub const LEGACY_FIXED_MOD_PARAM_BASE_END: u32 = 1_100_000;

const SLOT_STATE_STRIDE: usize = 8;
const IDX_SLOT_STATE_BASE: usize = 0;
const IDX_LFO_PHASE: usize = 0;
const IDX_ENV: usize = 1;
const IDX_RAND_PHASE: usize = 2;
const IDX_DRIFT: usize = 3;
const IDX_RNG: usize = 4;
const IDX_RAND_HOLD: usize = 5;
const IDX_RAND_SMOOTH: usize = 6;
const IDX_ENV_STAGE: usize = 7;

const IDX_PREV_GATE: usize = IDX_SLOT_STATE_BASE + SLOT_STATE_STRIDE * SLOT_COUNT;
const IDX_LAST_RESET_COUNTER: usize = IDX_PREV_GATE + 1;
const IDX_SAMPLE_RATE: usize = IDX_LAST_RESET_COUNTER + 1;
const IDX_CUSTOM_ENGINE_ID: usize = IDX_SAMPLE_RATE + 1;
const IDX_CUSTOM_VOICE_IDX: usize = IDX_CUSTOM_ENGINE_ID + 1;
const IDX_SAMPLER_TRACK_IDX: usize = IDX_CUSTOM_VOICE_IDX + 1;
const IDX_SAMPLER_VOICE_IDX: usize = IDX_SAMPLER_TRACK_IDX + 1;
const IDX_TRANSPORT_CLOCK_SOURCE: usize = IDX_SAMPLER_VOICE_IDX + 1;
const IDX_EVENT_SLICE_START: usize = IDX_TRANSPORT_CLOCK_SOURCE + 1;

pub const PARAM_SLOT_BASE: usize = 64;
pub const PARAM_SLOT_STRIDE: usize = 18;
pub const PARAM_SLOT_SOURCE: usize = 0;
pub const PARAM_LFO_RATE_HZ: usize = 1;
pub const PARAM_LFO_SYNC: usize = 2;
pub const PARAM_LFO_DIV: usize = 3;
pub const PARAM_LFO_SHAPE: usize = 4;
pub const PARAM_LFO_PW: usize = 5;
pub const PARAM_LFO_RETRIGGER: usize = 6;
pub const PARAM_ENV_ATTACK_MS: usize = 7;
pub const PARAM_ENV_DECAY_MS: usize = 8;
pub const PARAM_ENV_SUSTAIN: usize = 9;
pub const PARAM_ENV_RELEASE_MS: usize = 10;
pub const PARAM_RAND_RATE_HZ: usize = 11;
pub const PARAM_RAND_SYNC: usize = 12;
pub const PARAM_RAND_DIV: usize = 13;
pub const PARAM_RAND_SLEW: usize = 14;
pub const PARAM_DRIFT_RATE: usize = 15;
pub const PARAM_DRIFT_SYNC: usize = 16;
pub const PARAM_DRIFT_DIV: usize = 17;

pub const PARAM_BPM: usize = PARAM_SLOT_BASE + PARAM_SLOT_STRIDE * SLOT_COUNT;
pub const PARAM_TRANSPORT_BAR_PHASE: usize = PARAM_BPM + 1;
pub const PARAM_TRANSPORT_BAR_PHASE_INC: usize = PARAM_TRANSPORT_BAR_PHASE + 1;
pub const PARAM_RESET_COUNTER: usize = PARAM_TRANSPORT_BAR_PHASE_INC + 1;

/// Display tail (eseq-dtx.13). `voice_modulator_process` stores the block's
/// last output value per slot here so a UI-thread poller can read the live
/// modulation signal back off the audiograph watchlist and render effective
/// (post-modulation) parameter values. Four plain `f32` stores per block on
/// the audio thread — no allocation, no locking, no atomics needed because
/// the watchlist snapshot is taken between blocks, and a torn read would at
/// worst show one stale display value.
pub const STATE_DISPLAY_SLOT_VALUE: usize = PARAM_RESET_COUNTER + 1;

pub const STATE_SIZE: usize = STATE_DISPLAY_SLOT_VALUE + SLOT_COUNT;

/// The `@mod-mode additive` contract that `(mod name)` compiles to inside the
/// DGen engine (and that the hand-written Rust built-ins implement inline):
/// the base cell plus, when the destination's `__dgen_mod_active__` flag is
/// set, the sum of every depth lane scaled by its modulator slot's 0..1
/// output. Dual-maintained with the DGenLisp compiler's `mod` expansion; the
/// UI reuses it to draw the effective value without touching the engine.
pub fn additive_modulated_value(
    base: f32,
    active: bool,
    depths: &[f32; SLOT_COUNT],
    slot_values: &[f32; SLOT_COUNT],
) -> f32 {
    if !active {
        return base;
    }
    let mut value = base;
    for slot in 0..SLOT_COUNT {
        value += depths[slot] * slot_values[slot].clamp(0.0, 1.0);
    }
    value
}
const UNBOUND_CUSTOM_ENGINE: f32 = -1.0;
const TRANSPORT_CLOCK_SOURCE_PARAMS: f32 = 0.0;
const TRANSPORT_CLOCK_SOURCE_INPUT: f32 = 1.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoiceModulatorInitialState {
    pub custom_engine_id: u32,
    pub custom_voice_idx: u32,
    pub sampler_track_idx: u32,
    pub sampler_voice_idx: u32,
    pub transport_clock_source: u32,
}

pub fn custom_engine_initial_state(
    custom_engine_id: usize,
    custom_voice_idx: usize,
) -> VoiceModulatorInitialState {
    VoiceModulatorInitialState {
        custom_engine_id: custom_engine_id as u32,
        custom_voice_idx: custom_voice_idx as u32,
        sampler_track_idx: u32::MAX,
        sampler_voice_idx: u32::MAX,
        transport_clock_source: TRANSPORT_CLOCK_SOURCE_INPUT as u32,
    }
}

pub fn sampler_voice_initial_state(
    sampler_track_idx: usize,
    sampler_voice_idx: usize,
) -> VoiceModulatorInitialState {
    VoiceModulatorInitialState {
        custom_engine_id: u32::MAX,
        custom_voice_idx: u32::MAX,
        sampler_track_idx: sampler_track_idx as u32,
        sampler_voice_idx: sampler_voice_idx as u32,
        transport_clock_source: TRANSPORT_CLOCK_SOURCE_INPUT as u32,
    }
}

static PROCESS_STATS_ENABLED: AtomicBool = AtomicBool::new(false);
static SAMPLER_ACTIVE_MASKS: [AtomicU64; MAX_SAMPLER_POOLS] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_SAMPLER_POOLS]
};
static PROCESS_CALLS: AtomicU64 = AtomicU64::new(0);
static RENDERED_CALLS: AtomicU64 = AtomicU64::new(0);
static DISABLED_CUSTOM_SKIPS: AtomicU64 = AtomicU64::new(0);
static DISABLED_SAMPLER_SKIPS: AtomicU64 = AtomicU64::new(0);
static ALL_SLOTS_OFF_CALLS: AtomicU64 = AtomicU64::new(0);
static UNBOUND_RENDERED_CALLS: AtomicU64 = AtomicU64::new(0);
static RENDERED_FRAMES: AtomicU64 = AtomicU64::new(0);
static DISABLED_FRAMES: AtomicU64 = AtomicU64::new(0);
static ALL_SLOTS_OFF_FRAMES: AtomicU64 = AtomicU64::new(0);
static CUSTOM_ENGINE_CALLS: [AtomicU64; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_INSTRUMENT_ENGINES]
};
static CUSTOM_ENGINE_RENDERED_CALLS: [AtomicU64; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_INSTRUMENT_ENGINES]
};
static CUSTOM_ENGINE_DISABLED_SKIPS: [AtomicU64; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_INSTRUMENT_ENGINES]
};
static CUSTOM_ENGINE_RENDERED_FRAMES: [AtomicU64; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_INSTRUMENT_ENGINES]
};
static CUSTOM_ENGINE_DISABLED_FRAMES: [AtomicU64; MAX_INSTRUMENT_ENGINES] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_INSTRUMENT_ENGINES]
};
static SAMPLER_TRACK_CALLS: [AtomicU64; MAX_SAMPLER_POOLS] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_SAMPLER_POOLS]
};
static SAMPLER_TRACK_RENDERED_CALLS: [AtomicU64; MAX_SAMPLER_POOLS] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_SAMPLER_POOLS]
};
static SAMPLER_TRACK_DISABLED_SKIPS: [AtomicU64; MAX_SAMPLER_POOLS] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_SAMPLER_POOLS]
};
static SAMPLER_TRACK_RENDERED_FRAMES: [AtomicU64; MAX_SAMPLER_POOLS] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_SAMPLER_POOLS]
};
static SAMPLER_TRACK_DISABLED_FRAMES: [AtomicU64; MAX_SAMPLER_POOLS] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; MAX_SAMPLER_POOLS]
};

#[derive(Clone, Debug, Default)]
pub struct VoiceModulatorEngineProcessStats {
    pub engine_id: usize,
    pub enabled_voices: usize,
    pub calls: u64,
    pub rendered_calls: u64,
    pub disabled_skips: u64,
    pub rendered_frames: u64,
    pub disabled_frames: u64,
}

#[derive(Clone, Debug, Default)]
pub struct VoiceModulatorSamplerTrackProcessStats {
    pub track_idx: usize,
    pub active_mask: u64,
    pub calls: u64,
    pub rendered_calls: u64,
    pub disabled_skips: u64,
    pub rendered_frames: u64,
    pub disabled_frames: u64,
}

#[derive(Clone, Debug, Default)]
pub struct VoiceModulatorProcessStats {
    pub calls: u64,
    pub rendered_calls: u64,
    pub disabled_custom_skips: u64,
    pub disabled_sampler_skips: u64,
    pub all_slots_off_calls: u64,
    pub unbound_rendered_calls: u64,
    pub rendered_frames: u64,
    pub disabled_frames: u64,
    pub all_slots_off_frames: u64,
    pub engines: Vec<VoiceModulatorEngineProcessStats>,
    pub sampler_tracks: Vec<VoiceModulatorSamplerTrackProcessStats>,
}

pub fn set_process_stats_enabled(enabled: bool) {
    PROCESS_STATS_ENABLED.store(enabled, Ordering::Release);
}

pub fn set_sampler_active_mask(track_idx: usize, active_mask: u64) {
    if track_idx < MAX_SAMPLER_POOLS {
        SAMPLER_ACTIVE_MASKS[track_idx].store(active_mask, Ordering::Release);
    }
}

pub fn take_process_stats() -> VoiceModulatorProcessStats {
    let engines = (0..MAX_INSTRUMENT_ENGINES)
        .filter_map(|engine_id| {
            let calls = CUSTOM_ENGINE_CALLS[engine_id].swap(0, Ordering::AcqRel);
            let rendered_calls = CUSTOM_ENGINE_RENDERED_CALLS[engine_id].swap(0, Ordering::AcqRel);
            let disabled_skips = CUSTOM_ENGINE_DISABLED_SKIPS[engine_id].swap(0, Ordering::AcqRel);
            let rendered_frames =
                CUSTOM_ENGINE_RENDERED_FRAMES[engine_id].swap(0, Ordering::AcqRel);
            let disabled_frames =
                CUSTOM_ENGINE_DISABLED_FRAMES[engine_id].swap(0, Ordering::AcqRel);
            (calls > 0 || rendered_calls > 0 || disabled_skips > 0).then(|| {
                VoiceModulatorEngineProcessStats {
                    engine_id,
                    enabled_voices: crate::lisp_host::get_dgen_engine_enabled_voices(engine_id),
                    calls,
                    rendered_calls,
                    disabled_skips,
                    rendered_frames,
                    disabled_frames,
                }
            })
        })
        .collect();
    let sampler_tracks = (0..MAX_SAMPLER_POOLS)
        .filter_map(|track_idx| {
            let calls = SAMPLER_TRACK_CALLS[track_idx].swap(0, Ordering::AcqRel);
            let rendered_calls = SAMPLER_TRACK_RENDERED_CALLS[track_idx].swap(0, Ordering::AcqRel);
            let disabled_skips = SAMPLER_TRACK_DISABLED_SKIPS[track_idx].swap(0, Ordering::AcqRel);
            let rendered_frames =
                SAMPLER_TRACK_RENDERED_FRAMES[track_idx].swap(0, Ordering::AcqRel);
            let disabled_frames =
                SAMPLER_TRACK_DISABLED_FRAMES[track_idx].swap(0, Ordering::AcqRel);
            (calls > 0 || rendered_calls > 0 || disabled_skips > 0).then(|| {
                VoiceModulatorSamplerTrackProcessStats {
                    track_idx,
                    active_mask: SAMPLER_ACTIVE_MASKS[track_idx].load(Ordering::Acquire),
                    calls,
                    rendered_calls,
                    disabled_skips,
                    rendered_frames,
                    disabled_frames,
                }
            })
        })
        .collect();

    VoiceModulatorProcessStats {
        calls: PROCESS_CALLS.swap(0, Ordering::AcqRel),
        rendered_calls: RENDERED_CALLS.swap(0, Ordering::AcqRel),
        disabled_custom_skips: DISABLED_CUSTOM_SKIPS.swap(0, Ordering::AcqRel),
        disabled_sampler_skips: DISABLED_SAMPLER_SKIPS.swap(0, Ordering::AcqRel),
        all_slots_off_calls: ALL_SLOTS_OFF_CALLS.swap(0, Ordering::AcqRel),
        unbound_rendered_calls: UNBOUND_RENDERED_CALLS.swap(0, Ordering::AcqRel),
        rendered_frames: RENDERED_FRAMES.swap(0, Ordering::AcqRel),
        disabled_frames: DISABLED_FRAMES.swap(0, Ordering::AcqRel),
        all_slots_off_frames: ALL_SLOTS_OFF_FRAMES.swap(0, Ordering::AcqRel),
        engines,
        sampler_tracks,
    }
}

const SHAPE_TRIANGLE: usize = 0;
const SHAPE_SINE: usize = 1;
const SHAPE_PULSE: usize = 2;
const SHAPE_SAW: usize = 3;

const SOURCE_OFF: usize = 0;
const SOURCE_LFO: usize = 1;
const SOURCE_ENV: usize = 2;
const SOURCE_RAND: usize = 3;
const SOURCE_DRIFT: usize = 4;
const SOURCE_EXT1: usize = 5;
const SOURCE_EXT2: usize = 6;
const SOURCE_EXT3: usize = 7;
const SOURCE_EXT4: usize = 8;

const ENV_STAGE_IDLE: f32 = 0.0;
const ENV_STAGE_ATTACK: f32 = 1.0;
const ENV_STAGE_DECAY: f32 = 2.0;
const ENV_STAGE_SUSTAIN: f32 = 3.0;
const ENV_STAGE_RELEASE: f32 = 4.0;

fn slot_state_idx(slot: usize, offset: usize) -> usize {
    IDX_SLOT_STATE_BASE + slot * SLOT_STATE_STRIDE + offset
}

pub fn slot_param_idx(slot: usize, offset: usize) -> usize {
    PARAM_SLOT_BASE + slot * PARAM_SLOT_STRIDE + offset
}

pub fn slot_source_param_idx(slot: usize) -> usize {
    slot_param_idx(slot, PARAM_SLOT_SOURCE)
}

pub fn slot_source_node_param_idx(slot: usize) -> u32 {
    MOD_PARAM_BASE + slot_source_param_idx(slot) as u32
}

pub fn is_source_param(node_param_idx: u32) -> bool {
    node_param_idx >= MOD_PARAM_BASE
}

pub fn is_envelope_source_param_value(node_param_idx: u32, value: f32) -> bool {
    is_source_param(node_param_idx) && source_type_label(value.round().max(0.0) as usize) == "env"
}

fn sync_labels() -> Vec<String> {
    SyncDivision::ALL
        .iter()
        .map(|division| division.label().to_string())
        .collect()
}

fn source_labels() -> Vec<String> {
    [
        "off", "lfo", "env", "rand", "drift", "ext1", "ext2", "ext3", "ext4",
    ]
    .iter()
    .map(|label| label.to_string())
    .collect()
}

fn shape_labels() -> Vec<String> {
    ["triangle", "sine", "pulse", "sawtooth"]
        .iter()
        .map(|label| label.to_string())
        .collect()
}

pub fn source_type_label(source: usize) -> &'static str {
    match source {
        SOURCE_LFO => "lfo",
        SOURCE_ENV => "env",
        SOURCE_RAND => "rand",
        SOURCE_DRIFT => "drift",
        SOURCE_EXT1 => "ext1",
        SOURCE_EXT2 => "ext2",
        SOURCE_EXT3 => "ext3",
        SOURCE_EXT4 => "ext4",
        _ => "off",
    }
}

pub fn source_type_name_from_param_name(name: &str) -> Option<&'static str> {
    let parts = parse_slot_param_name(name)?;
    match parts.1 {
        "source" => Some("source"),
        "lfo_rate" | "lfo_sync" | "lfo_div" | "lfo_shape" | "lfo_pw" | "lfo_retrigger" => {
            Some("lfo")
        }
        "env_attack" | "env_decay" | "env_sustain" | "env_release" => Some("env"),
        "rand_rate" | "rand_sync" | "rand_div" | "rand_slew" => Some("rand"),
        "drift_rate" | "drift_sync" | "drift_div" => Some("drift"),
        _ => None,
    }
}

pub fn source_param_display_name(name: &str) -> String {
    let Some((_, suffix)) = parse_slot_param_name(name) else {
        return name.to_string();
    };
    match suffix {
        "source" => "type",
        "lfo_rate" | "rand_rate" | "drift_rate" => "rate",
        "lfo_div" | "rand_div" | "drift_div" => "division",
        "lfo_sync" | "rand_sync" | "drift_sync" => "sync",
        "lfo_shape" => "shape",
        "lfo_pw" => "pulse width",
        "lfo_retrigger" => "retrigger",
        "env_attack" => "attack",
        "env_decay" => "decay",
        "env_sustain" => "sustain",
        "env_release" => "release",
        "rand_slew" => "slew",
        other => other,
    }
    .to_string()
}

pub fn slot_from_param_name(name: &str) -> Option<usize> {
    parse_slot_param_name(name).map(|(slot, _)| slot)
}

pub fn selected_source_param_indices<F>(
    params: &[ParamDescriptor],
    mut value_for_param: F,
) -> Vec<usize>
where
    F: FnMut(usize, &ParamDescriptor) -> f32,
{
    let source_indices = params
        .iter()
        .enumerate()
        .filter_map(|(idx, param)| is_source_param(param.node_param_idx).then_some(idx))
        .collect::<Vec<_>>();

    let find_idx_by_node = |node_param_idx: u32| {
        source_indices
            .iter()
            .copied()
            .find(|&idx| params[idx].node_param_idx == node_param_idx)
    };

    let mut out = Vec::new();
    for slot in 0..SLOT_COUNT {
        let Some(source_idx) = find_idx_by_node(slot_source_node_param_idx(slot)) else {
            continue;
        };
        out.push(source_idx);

        let source_value = value_for_param(source_idx, &params[source_idx]).round() as usize;
        let source_name = source_type_label(source_value);
        if source_name == "off" || source_name.starts_with("ext") {
            continue;
        }

        let sync = source_indices
            .iter()
            .copied()
            .find(|&idx| {
                slot_from_param_name(&params[idx].name) == Some(slot + 1)
                    && params[idx].name.ends_with("_sync")
                    && source_type_name_from_param_name(&params[idx].name) == Some(source_name)
            })
            .map(|idx| value_for_param(idx, &params[idx]) > 0.5)
            .unwrap_or(false);

        let pulse = source_indices
            .iter()
            .copied()
            .find(|&idx| {
                slot_from_param_name(&params[idx].name) == Some(slot + 1)
                    && params[idx].name.ends_with("_shape")
            })
            .map(|idx| value_for_param(idx, &params[idx]).round() as i32 == SHAPE_PULSE as i32)
            .unwrap_or(false);

        for &idx in &source_indices {
            let param = &params[idx];
            if slot_from_param_name(&param.name) != Some(slot + 1)
                || source_type_name_from_param_name(&param.name) != Some(source_name)
            {
                continue;
            }
            if (param.name.ends_with("_rate") && sync)
                || (param.name.ends_with("_div") && !sync)
                || (param.name.ends_with("_pw") && !pulse)
            {
                continue;
            }
            out.push(idx);
        }
    }

    out
}

fn parse_slot_param_name(name: &str) -> Option<(usize, &str)> {
    let rest = name.strip_prefix("mod")?;
    let digit_end = rest
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_digit())
        .map(|(idx, ch)| idx + ch.len_utf8())
        .last()?;
    let slot = rest[..digit_end].parse::<usize>().ok()?;
    if !(1..=SLOT_COUNT).contains(&slot) {
        return None;
    }
    let suffix = rest[digit_end..].strip_prefix('_')?;
    Some((slot, suffix))
}

fn next_rand(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    let v = ((*state >> 8) as f32) / ((u32::MAX >> 8) as f32);
    v * 2.0 - 1.0
}

fn triangle(phase: f32) -> f32 {
    1.0 - 4.0 * (phase - 0.5).abs()
}

fn shape_value(shape: usize, phase: f32, pulse_width: f32) -> f32 {
    match shape {
        SHAPE_SINE => (std::f32::consts::TAU * phase).sin(),
        SHAPE_PULSE => {
            if phase < pulse_width.clamp(0.05, 0.95) {
                1.0
            } else {
                -1.0
            }
        }
        SHAPE_SAW => phase * 2.0 - 1.0,
        _ => triangle(phase),
    }
}

fn bipolar_to_unipolar(value: f32) -> f32 {
    ((value.clamp(-1.0, 1.0) + 1.0) * 0.5).clamp(0.0, 1.0)
}

fn synced_rate_hz(div_idx: usize, bpm: f32) -> f32 {
    let beats = SyncDivision::from_index(div_idx).to_beats() as f32;
    (bpm.max(20.0) / 60.0) / beats.max(0.0001)
}

fn normalize_phase(value: f32) -> f32 {
    if value.is_finite() {
        value - value.floor()
    } else {
        0.0
    }
}

fn transport_bar_phase_for_frame(base_phase: f32, phase_inc: f32, frame: usize) -> f32 {
    normalize_phase(base_phase + phase_inc * frame as f32)
}

fn synced_phase_from_bar_phase(div_idx: usize, bar_phase: f32) -> f32 {
    let beats = SyncDivision::from_index(div_idx).to_beats() as f32;
    normalize_phase(normalize_phase(bar_phase) * 4.0 / beats.max(0.0001))
}

fn sync_division_index(division: SyncDivision) -> f32 {
    SyncDivision::ALL
        .iter()
        .position(|candidate| *candidate == division)
        .unwrap_or(0) as f32
}

fn default_source(slot: usize) -> f32 {
    match slot {
        0 => SOURCE_LFO as f32,
        1 => SOURCE_ENV as f32,
        2 => SOURCE_RAND as f32,
        3 => SOURCE_DRIFT as f32,
        _ => SOURCE_OFF as f32,
    }
}

unsafe fn init_param_defaults_with_source<F>(s: *mut f32, mut source_for_slot: F)
where
    F: FnMut(usize) -> f32,
{
    *s.add(PARAM_BPM) = 120.0;
    *s.add(PARAM_TRANSPORT_BAR_PHASE) = 0.0;
    *s.add(PARAM_TRANSPORT_BAR_PHASE_INC) = 0.0;
    *s.add(PARAM_RESET_COUNTER) = 0.0;

    for slot in 0..SLOT_COUNT {
        *s.add(slot_source_param_idx(slot)) = source_for_slot(slot);

        *s.add(slot_param_idx(slot, PARAM_LFO_RATE_HZ)) = match slot {
            1 => 1.7,
            2 => 0.37,
            _ => 5.0,
        };
        *s.add(slot_param_idx(slot, PARAM_LFO_SYNC)) = 0.0;
        *s.add(slot_param_idx(slot, PARAM_LFO_DIV)) = match slot {
            1 => sync_division_index(SyncDivision::Half),
            2 => sync_division_index(SyncDivision::Whole),
            _ => sync_division_index(SyncDivision::Quarter),
        };
        *s.add(slot_param_idx(slot, PARAM_LFO_SHAPE)) = SHAPE_TRIANGLE as f32;
        *s.add(slot_param_idx(slot, PARAM_LFO_PW)) = 0.5;
        *s.add(slot_param_idx(slot, PARAM_LFO_RETRIGGER)) = 0.0;

        *s.add(slot_param_idx(slot, PARAM_ENV_ATTACK_MS)) = 6.0;
        *s.add(slot_param_idx(slot, PARAM_ENV_DECAY_MS)) = 180.0;
        *s.add(slot_param_idx(slot, PARAM_ENV_SUSTAIN)) = 0.55;
        *s.add(slot_param_idx(slot, PARAM_ENV_RELEASE_MS)) = 240.0;

        *s.add(slot_param_idx(slot, PARAM_RAND_RATE_HZ)) = 3.0;
        *s.add(slot_param_idx(slot, PARAM_RAND_SYNC)) = 0.0;
        *s.add(slot_param_idx(slot, PARAM_RAND_DIV)) = sync_division_index(SyncDivision::Whole);
        *s.add(slot_param_idx(slot, PARAM_RAND_SLEW)) = 0.0;

        *s.add(slot_param_idx(slot, PARAM_DRIFT_RATE)) = 0.00035;
        *s.add(slot_param_idx(slot, PARAM_DRIFT_SYNC)) = 0.0;
        *s.add(slot_param_idx(slot, PARAM_DRIFT_DIV)) = sync_division_index(SyncDivision::Whole);
    }
}

unsafe fn init_param_defaults(s: *mut f32) {
    init_param_defaults_with_source(s, default_source);
}

unsafe fn init_effect_param_defaults(s: *mut f32) {
    init_param_defaults_with_source(s, |_| SOURCE_OFF as f32);
}

fn push_param(
    out: &mut Vec<ParamDescriptor>,
    name: &str,
    min: f32,
    max: f32,
    default: f32,
    kind: ParamKind,
    scaling: ParamScaling,
    idx: usize,
) {
    out.push(ParamDescriptor {
        name: name.to_string(),
        min,
        max,
        default,
        kind,
        scaling,
        node_param_idx: MOD_PARAM_BASE + idx as u32,
        node_param_span: 1,
        host_control: None,
        ui_metadata: None,
    });
}

pub fn param_descriptors() -> Vec<ParamDescriptor> {
    param_descriptors_with_default_source(default_source)
}

pub fn effect_param_descriptors() -> Vec<ParamDescriptor> {
    param_descriptors_with_default_source(|_| SOURCE_OFF as f32)
}

fn param_descriptors_with_default_source<F>(mut source_for_slot: F) -> Vec<ParamDescriptor>
where
    F: FnMut(usize) -> f32,
{
    let mut out = Vec::new();
    let source_labels = source_labels();
    let sync_div_labels = sync_labels();
    let shape_labels = shape_labels();

    for slot in 0..SLOT_COUNT {
        let prefix = format!("mod{}", slot + 1);
        push_param(
            &mut out,
            &format!("{prefix}_source"),
            SOURCE_OFF as f32,
            SOURCE_EXT4 as f32,
            source_for_slot(slot),
            ParamKind::Enum {
                labels: source_labels.clone(),
            },
            ParamScaling::Linear,
            slot_source_param_idx(slot),
        );
        push_param(
            &mut out,
            &format!("{prefix}_lfo_rate"),
            0.01,
            20.0,
            match slot {
                1 => 1.7,
                2 => 0.37,
                _ => 5.0,
            },
            ParamKind::Continuous {
                unit: Some("Hz".to_string()),
            },
            ParamScaling::Exponential,
            slot_param_idx(slot, PARAM_LFO_RATE_HZ),
        );
        push_param(
            &mut out,
            &format!("{prefix}_lfo_sync"),
            0.0,
            1.0,
            0.0,
            ParamKind::Boolean,
            ParamScaling::Linear,
            slot_param_idx(slot, PARAM_LFO_SYNC),
        );
        push_param(
            &mut out,
            &format!("{prefix}_lfo_div"),
            0.0,
            (sync_div_labels.len() - 1) as f32,
            match slot {
                1 => sync_division_index(SyncDivision::Half),
                2 => sync_division_index(SyncDivision::Whole),
                _ => sync_division_index(SyncDivision::Quarter),
            },
            ParamKind::Enum {
                labels: sync_div_labels.clone(),
            },
            ParamScaling::Linear,
            slot_param_idx(slot, PARAM_LFO_DIV),
        );
        push_param(
            &mut out,
            &format!("{prefix}_lfo_shape"),
            0.0,
            (shape_labels.len() - 1) as f32,
            0.0,
            ParamKind::Enum {
                labels: shape_labels.clone(),
            },
            ParamScaling::Linear,
            slot_param_idx(slot, PARAM_LFO_SHAPE),
        );
        push_param(
            &mut out,
            &format!("{prefix}_lfo_pw"),
            0.05,
            0.95,
            0.5,
            ParamKind::Continuous { unit: None },
            ParamScaling::Linear,
            slot_param_idx(slot, PARAM_LFO_PW),
        );
        push_param(
            &mut out,
            &format!("{prefix}_lfo_retrigger"),
            0.0,
            1.0,
            0.0,
            ParamKind::Boolean,
            ParamScaling::Linear,
            slot_param_idx(slot, PARAM_LFO_RETRIGGER),
        );

        for (suffix, offset, min, max, default, unit, scaling) in [
            (
                "env_attack",
                PARAM_ENV_ATTACK_MS,
                1.0,
                2000.0,
                6.0,
                Some("ms"),
                ParamScaling::Exponential,
            ),
            (
                "env_decay",
                PARAM_ENV_DECAY_MS,
                5.0,
                4000.0,
                180.0,
                Some("ms"),
                ParamScaling::Exponential,
            ),
            (
                "env_sustain",
                PARAM_ENV_SUSTAIN,
                0.0,
                1.0,
                0.55,
                Some("%"),
                ParamScaling::Linear,
            ),
            (
                "env_release",
                PARAM_ENV_RELEASE_MS,
                5.0,
                4000.0,
                240.0,
                Some("ms"),
                ParamScaling::Exponential,
            ),
        ] {
            push_param(
                &mut out,
                &format!("{prefix}_{suffix}"),
                min,
                max,
                default,
                ParamKind::Continuous {
                    unit: unit.map(str::to_string),
                },
                scaling,
                slot_param_idx(slot, offset),
            );
        }

        push_param(
            &mut out,
            &format!("{prefix}_rand_rate"),
            0.05,
            20.0,
            3.0,
            ParamKind::Continuous {
                unit: Some("Hz".to_string()),
            },
            ParamScaling::Exponential,
            slot_param_idx(slot, PARAM_RAND_RATE_HZ),
        );
        push_param(
            &mut out,
            &format!("{prefix}_rand_sync"),
            0.0,
            1.0,
            0.0,
            ParamKind::Boolean,
            ParamScaling::Linear,
            slot_param_idx(slot, PARAM_RAND_SYNC),
        );
        push_param(
            &mut out,
            &format!("{prefix}_rand_div"),
            0.0,
            (sync_div_labels.len() - 1) as f32,
            sync_division_index(SyncDivision::Whole),
            ParamKind::Enum {
                labels: sync_div_labels.clone(),
            },
            ParamScaling::Linear,
            slot_param_idx(slot, PARAM_RAND_DIV),
        );
        push_param(
            &mut out,
            &format!("{prefix}_rand_slew"),
            0.0,
            0.999,
            0.0,
            ParamKind::Continuous { unit: None },
            ParamScaling::Linear,
            slot_param_idx(slot, PARAM_RAND_SLEW),
        );

        push_param(
            &mut out,
            &format!("{prefix}_drift_rate"),
            0.00001,
            0.01,
            0.00035,
            ParamKind::Continuous {
                unit: Some("Hz".to_string()),
            },
            ParamScaling::Exponential,
            slot_param_idx(slot, PARAM_DRIFT_RATE),
        );
        push_param(
            &mut out,
            &format!("{prefix}_drift_sync"),
            0.0,
            1.0,
            0.0,
            ParamKind::Boolean,
            ParamScaling::Linear,
            slot_param_idx(slot, PARAM_DRIFT_SYNC),
        );
        push_param(
            &mut out,
            &format!("{prefix}_drift_div"),
            0.0,
            (sync_div_labels.len() - 1) as f32,
            sync_division_index(SyncDivision::Whole),
            ParamKind::Enum {
                labels: sync_div_labels.clone(),
            },
            ParamScaling::Linear,
            slot_param_idx(slot, PARAM_DRIFT_DIV),
        );
    }

    out
}

pub fn ui_param_descriptors() -> Vec<ParamDescriptor> {
    param_descriptors()
}

unsafe fn init_identity(s: *mut f32, initial_state: *const c_void) {
    *s.add(IDX_CUSTOM_ENGINE_ID) = UNBOUND_CUSTOM_ENGINE;
    *s.add(IDX_CUSTOM_VOICE_IDX) = UNBOUND_CUSTOM_ENGINE;
    *s.add(IDX_SAMPLER_TRACK_IDX) = UNBOUND_CUSTOM_ENGINE;
    *s.add(IDX_SAMPLER_VOICE_IDX) = UNBOUND_CUSTOM_ENGINE;
    *s.add(IDX_TRANSPORT_CLOCK_SOURCE) = TRANSPORT_CLOCK_SOURCE_PARAMS;
    *s.add(IDX_EVENT_SLICE_START) = 0.0;

    if initial_state.is_null() {
        return;
    }

    let initial = &*(initial_state as *const VoiceModulatorInitialState);
    *s.add(IDX_TRANSPORT_CLOCK_SOURCE) = if initial.transport_clock_source != 0 {
        TRANSPORT_CLOCK_SOURCE_INPUT
    } else {
        TRANSPORT_CLOCK_SOURCE_PARAMS
    };
    if initial.custom_engine_id != u32::MAX && initial.custom_voice_idx != u32::MAX {
        *s.add(IDX_CUSTOM_ENGINE_ID) = initial.custom_engine_id as f32;
        *s.add(IDX_CUSTOM_VOICE_IDX) = initial.custom_voice_idx as f32;
    }
    if initial.sampler_track_idx != u32::MAX && initial.sampler_voice_idx != u32::MAX {
        *s.add(IDX_SAMPLER_TRACK_IDX) = initial.sampler_track_idx as f32;
        *s.add(IDX_SAMPLER_VOICE_IDX) = initial.sampler_voice_idx as f32;
    }
}

unsafe extern "C" fn voice_modulator_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    initial_state: *const c_void,
) {
    let s = state as *mut f32;
    for i in 0..STATE_SIZE {
        *s.add(i) = 0.0;
    }
    for slot in 0..SLOT_COUNT {
        *s.add(slot_state_idx(slot, IDX_RNG)) = 0x1234_5678u32.wrapping_add(slot as u32) as f32;
    }
    *s.add(IDX_SAMPLE_RATE) = sample_rate as f32;
    init_identity(s, initial_state);
    init_param_defaults(s);
}

unsafe extern "C" fn effect_modulator_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    initial_state: *const c_void,
) {
    let s = state as *mut f32;
    for i in 0..STATE_SIZE {
        *s.add(i) = 0.0;
    }
    for slot in 0..SLOT_COUNT {
        *s.add(slot_state_idx(slot, IDX_RNG)) = 0x1234_5678u32.wrapping_add(slot as u32) as f32;
    }
    *s.add(IDX_SAMPLE_RATE) = sample_rate as f32;
    init_identity(s, initial_state);
    init_effect_param_defaults(s);
}

unsafe extern "C" fn voice_modulator_begin_event_slice(
    state: *mut c_void,
    _block_serial: u64,
    slice_start: c_int,
    _slice_nframes: c_int,
) {
    let s = state as *mut f32;
    *s.add(IDX_EVENT_SLICE_START) = slice_start.max(0) as f32;
}

unsafe fn clear_outputs(out: *const *mut f32, nf: usize) {
    if out.is_null() {
        return;
    }
    for slot in 0..NUM_OUTPUTS {
        let out_slot = *out.add(slot);
        if out_slot.is_null() {
            continue;
        }
        for i in 0..nf {
            *out_slot.add(i) = 0.0;
        }
    }
}

unsafe fn clear_output_slot(out: *const *mut f32, slot: usize, nf: usize) {
    if out.is_null() || slot >= NUM_OUTPUTS {
        return;
    }
    let out_slot = *out.add(slot);
    if out_slot.is_null() {
        return;
    }
    for i in 0..nf {
        *out_slot.add(i) = 0.0;
    }
}

/// Copy the block's last output frame into the display tail
/// (`STATE_DISPLAY_SLOT_VALUE`). Bounded, allocation-free, lock-free; see the
/// constant's docs.
unsafe fn publish_slot_display_values(s: *mut f32, out: *const *mut f32, nf: usize) {
    if out.is_null() || nf == 0 {
        return;
    }
    for slot in 0..NUM_OUTPUTS {
        let out_slot = *out.add(slot);
        let value = if out_slot.is_null() {
            0.0
        } else {
            *out_slot.add(nf - 1)
        };
        *s.add(STATE_DISPLAY_SLOT_VALUE + slot) = value;
    }
}

unsafe fn clear_output_frame(out: *const *mut f32, frame: usize) {
    if out.is_null() {
        return;
    }
    for slot in 0..NUM_OUTPUTS {
        let out_slot = *out.add(slot);
        if !out_slot.is_null() {
            *out_slot.add(frame) = 0.0;
        }
    }
}

unsafe fn gate_timeline_has_activity(
    gate_in: *const f32,
    trigger_in: *const f32,
    nf: usize,
    prev_gate: f32,
) -> bool {
    if prev_gate > 0.5 {
        return true;
    }
    if gate_in.is_null() || trigger_in.is_null() {
        return false;
    }
    for i in 0..nf {
        if (*gate_in.add(i)).clamp(0.0, 1.0) > 0.5 || (*trigger_in.add(i)).max(0.0) > 0.5 {
            return true;
        }
    }
    false
}

unsafe fn slot_source(s: *const f32, slot: usize) -> usize {
    let source = (*s.add(slot_source_param_idx(slot))).round() as usize;
    if source <= SOURCE_EXT4 {
        source
    } else {
        SOURCE_OFF
    }
}

unsafe fn custom_engine_identity(s: *const f32) -> Option<(usize, usize)> {
    let engine_id = *s.add(IDX_CUSTOM_ENGINE_ID);
    if engine_id < 0.0 {
        return None;
    }
    let voice_idx = *s.add(IDX_CUSTOM_VOICE_IDX);
    if voice_idx < 0.0 {
        return None;
    }

    Some((engine_id as usize, voice_idx as usize))
}

unsafe fn sampler_identity(s: *const f32) -> Option<(usize, usize)> {
    let track_idx = *s.add(IDX_SAMPLER_TRACK_IDX);
    if track_idx < 0.0 {
        return None;
    }
    let voice_idx = *s.add(IDX_SAMPLER_VOICE_IDX);
    if voice_idx < 0.0 {
        return None;
    }

    Some((track_idx as usize, voice_idx as usize))
}

fn sampler_voice_is_active(track_idx: usize, voice_idx: usize) -> bool {
    if track_idx >= MAX_SAMPLER_POOLS || voice_idx >= 64 {
        return false;
    }
    let mask = SAMPLER_ACTIVE_MASKS[track_idx].load(Ordering::Acquire);
    (mask & (1u64 << voice_idx)) != 0
}

fn record_disabled_custom_skip(engine_id: usize, nf: usize) {
    if !PROCESS_STATS_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    PROCESS_CALLS.fetch_add(1, Ordering::Relaxed);
    DISABLED_CUSTOM_SKIPS.fetch_add(1, Ordering::Relaxed);
    DISABLED_FRAMES.fetch_add(nf as u64, Ordering::Relaxed);
    if engine_id < MAX_INSTRUMENT_ENGINES {
        CUSTOM_ENGINE_CALLS[engine_id].fetch_add(1, Ordering::Relaxed);
        CUSTOM_ENGINE_DISABLED_SKIPS[engine_id].fetch_add(1, Ordering::Relaxed);
        CUSTOM_ENGINE_DISABLED_FRAMES[engine_id].fetch_add(nf as u64, Ordering::Relaxed);
    }
}

fn record_disabled_sampler_skip(track_idx: usize, nf: usize) {
    if !PROCESS_STATS_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    PROCESS_CALLS.fetch_add(1, Ordering::Relaxed);
    DISABLED_SAMPLER_SKIPS.fetch_add(1, Ordering::Relaxed);
    DISABLED_FRAMES.fetch_add(nf as u64, Ordering::Relaxed);
    if track_idx < MAX_SAMPLER_POOLS {
        SAMPLER_TRACK_CALLS[track_idx].fetch_add(1, Ordering::Relaxed);
        SAMPLER_TRACK_DISABLED_SKIPS[track_idx].fetch_add(1, Ordering::Relaxed);
        SAMPLER_TRACK_DISABLED_FRAMES[track_idx].fetch_add(nf as u64, Ordering::Relaxed);
    }
}

fn record_all_slots_off(nf: usize) {
    if !PROCESS_STATS_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    PROCESS_CALLS.fetch_add(1, Ordering::Relaxed);
    ALL_SLOTS_OFF_CALLS.fetch_add(1, Ordering::Relaxed);
    ALL_SLOTS_OFF_FRAMES.fetch_add(nf as u64, Ordering::Relaxed);
}

fn record_rendered_call(
    custom_identity: Option<(usize, usize)>,
    sampler_identity: Option<(usize, usize)>,
    nf: usize,
) {
    if !PROCESS_STATS_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    PROCESS_CALLS.fetch_add(1, Ordering::Relaxed);
    RENDERED_CALLS.fetch_add(1, Ordering::Relaxed);
    RENDERED_FRAMES.fetch_add(nf as u64, Ordering::Relaxed);

    if let Some((engine_id, _)) = custom_identity {
        if engine_id < MAX_INSTRUMENT_ENGINES {
            CUSTOM_ENGINE_CALLS[engine_id].fetch_add(1, Ordering::Relaxed);
            CUSTOM_ENGINE_RENDERED_CALLS[engine_id].fetch_add(1, Ordering::Relaxed);
            CUSTOM_ENGINE_RENDERED_FRAMES[engine_id].fetch_add(nf as u64, Ordering::Relaxed);
        }
    } else if let Some((track_idx, _)) = sampler_identity {
        if track_idx < MAX_SAMPLER_POOLS {
            SAMPLER_TRACK_CALLS[track_idx].fetch_add(1, Ordering::Relaxed);
            SAMPLER_TRACK_RENDERED_CALLS[track_idx].fetch_add(1, Ordering::Relaxed);
            SAMPLER_TRACK_RENDERED_FRAMES[track_idx].fetch_add(nf as u64, Ordering::Relaxed);
        }
    } else {
        UNBOUND_RENDERED_CALLS.fetch_add(1, Ordering::Relaxed);
    }
}

unsafe fn render_slot(
    s: *mut f32,
    ext_inputs: &[*mut f32; EXT_INPUT_COUNT],
    slot: usize,
    source: usize,
    frame: usize,
    gate: f32,
    velocity: f32,
    note_on: bool,
    sample_rate: f32,
    bpm: f32,
    transport_bar_phase: f32,
    transport_clock_advancing: bool,
) -> f32 {
    if source == SOURCE_OFF {
        return 0.0;
    }
    if (SOURCE_EXT1..=SOURCE_EXT4).contains(&source) {
        let ext_idx = source - SOURCE_EXT1;
        let ptr = ext_inputs[ext_idx];
        return if ptr.is_null() {
            0.0
        } else {
            (*ptr.add(frame)).clamp(0.0, 1.0)
        };
    }

    let mut rng_state = *s.add(slot_state_idx(slot, IDX_RNG)) as u32;
    if rng_state == 0 {
        rng_state = 0x1234_5678u32.wrapping_add(slot as u32);
    }

    let out = match source {
        SOURCE_LFO => {
            let mut phase = *s.add(slot_state_idx(slot, IDX_LFO_PHASE));
            let sync = *s.add(slot_param_idx(slot, PARAM_LFO_SYNC)) > 0.5;
            if sync && transport_clock_advancing {
                phase = synced_phase_from_bar_phase(
                    (*s.add(slot_param_idx(slot, PARAM_LFO_DIV))).round() as usize,
                    transport_bar_phase,
                );
            } else {
                if note_on && *s.add(slot_param_idx(slot, PARAM_LFO_RETRIGGER)) > 0.5 {
                    phase = 0.0;
                }
                let rate = if sync {
                    synced_rate_hz(
                        (*s.add(slot_param_idx(slot, PARAM_LFO_DIV))).round() as usize,
                        bpm,
                    )
                } else {
                    (*s.add(slot_param_idx(slot, PARAM_LFO_RATE_HZ))).clamp(0.01, 20.0)
                };
                phase = normalize_phase(phase + rate / sample_rate);
            }
            *s.add(slot_state_idx(slot, IDX_LFO_PHASE)) = phase;
            let shape = (*s.add(slot_param_idx(slot, PARAM_LFO_SHAPE))).round() as usize;
            let pw = (*s.add(slot_param_idx(slot, PARAM_LFO_PW))).clamp(0.05, 0.95);
            bipolar_to_unipolar(shape_value(shape, phase, pw))
        }
        SOURCE_ENV => {
            let mut env = *s.add(slot_state_idx(slot, IDX_ENV));
            let mut stage = *s.add(slot_state_idx(slot, IDX_ENV_STAGE));
            if note_on {
                env = 0.0;
                stage = ENV_STAGE_ATTACK;
            }
            if gate <= 0.5 && stage != ENV_STAGE_IDLE && stage != ENV_STAGE_RELEASE {
                stage = ENV_STAGE_RELEASE;
            }
            let attack = 1.0
                / ((*s.add(slot_param_idx(slot, PARAM_ENV_ATTACK_MS))).clamp(1.0, 2000.0)
                    * 0.001
                    * sample_rate);
            let decay = 1.0
                / ((*s.add(slot_param_idx(slot, PARAM_ENV_DECAY_MS))).clamp(5.0, 4000.0)
                    * 0.001
                    * sample_rate);
            let sustain = (*s.add(slot_param_idx(slot, PARAM_ENV_SUSTAIN))).clamp(0.0, 1.0);
            let release = 1.0
                / ((*s.add(slot_param_idx(slot, PARAM_ENV_RELEASE_MS))).clamp(5.0, 4000.0)
                    * 0.001
                    * sample_rate);

            if stage == ENV_STAGE_ATTACK {
                env = (env + attack).min(1.0);
                if env >= 0.999 {
                    env = 1.0;
                    stage = ENV_STAGE_DECAY;
                }
            } else if stage == ENV_STAGE_DECAY {
                env += (sustain - env) * decay;
                if (env - sustain).abs() <= 0.001 {
                    env = sustain;
                    stage = if gate > 0.5 {
                        ENV_STAGE_SUSTAIN
                    } else {
                        ENV_STAGE_RELEASE
                    };
                }
            } else if stage == ENV_STAGE_SUSTAIN {
                env = sustain;
                if gate <= 0.5 {
                    stage = ENV_STAGE_RELEASE;
                }
            } else if stage == ENV_STAGE_RELEASE {
                env += (0.0 - env) * release;
                if env <= 0.0005 {
                    env = 0.0;
                    stage = ENV_STAGE_IDLE;
                }
            }
            env = env.clamp(0.0, 1.0);
            *s.add(slot_state_idx(slot, IDX_ENV)) = env;
            *s.add(slot_state_idx(slot, IDX_ENV_STAGE)) = stage;
            env
        }
        SOURCE_RAND => {
            let mut phase = *s.add(slot_state_idx(slot, IDX_RAND_PHASE));
            let mut hold = *s.add(slot_state_idx(slot, IDX_RAND_HOLD));
            let mut smooth = *s.add(slot_state_idx(slot, IDX_RAND_SMOOTH));
            if note_on {
                hold = next_rand(&mut rng_state);
            }
            let rate = if *s.add(slot_param_idx(slot, PARAM_RAND_SYNC)) > 0.5 {
                synced_rate_hz(
                    (*s.add(slot_param_idx(slot, PARAM_RAND_DIV))).round() as usize,
                    bpm,
                )
            } else {
                (*s.add(slot_param_idx(slot, PARAM_RAND_RATE_HZ))).clamp(0.01, 20.0)
            };
            let prev_phase = phase;
            phase = (phase + rate / sample_rate).fract();
            if phase < prev_phase {
                hold = next_rand(&mut rng_state);
            }
            let slew = (*s.add(slot_param_idx(slot, PARAM_RAND_SLEW))).clamp(0.0, 0.999);
            smooth += (hold - smooth) * (1.0 - slew);
            *s.add(slot_state_idx(slot, IDX_RAND_PHASE)) = phase;
            *s.add(slot_state_idx(slot, IDX_RAND_HOLD)) = hold;
            *s.add(slot_state_idx(slot, IDX_RAND_SMOOTH)) = smooth;
            bipolar_to_unipolar(smooth)
        }
        SOURCE_DRIFT => {
            let mut drift = *s.add(slot_state_idx(slot, IDX_DRIFT));
            let rate = if *s.add(slot_param_idx(slot, PARAM_DRIFT_SYNC)) > 0.5 {
                synced_rate_hz(
                    (*s.add(slot_param_idx(slot, PARAM_DRIFT_DIV))).round() as usize,
                    bpm,
                ) * 0.2
            } else {
                (*s.add(slot_param_idx(slot, PARAM_DRIFT_RATE))).clamp(0.00001, 0.01) * sample_rate
            };
            drift +=
                (next_rand(&mut rng_state) * 0.08 - drift) * (rate / sample_rate).clamp(0.0, 1.0);
            drift = drift.clamp(-1.0, 1.0);
            *s.add(slot_state_idx(slot, IDX_DRIFT)) = drift;
            bipolar_to_unipolar((drift * (0.4 + velocity * 0.6)).clamp(-1.0, 1.0))
        }
        _ => 0.0,
    };

    *s.add(slot_state_idx(slot, IDX_RNG)) = rng_state as f32;
    out.clamp(0.0, 1.0)
}

unsafe extern "C" fn voice_modulator_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let nf = nframes as usize;
    let s = state as *mut f32;

    let custom_identity = custom_engine_identity(s);
    if let Some((engine_id, voice_idx)) = custom_identity {
        let enabled = crate::lisp_host::get_dgen_engine_enabled_voices(engine_id);
        if voice_idx >= enabled {
            record_disabled_custom_skip(engine_id, nf);
            clear_outputs(out, nf);
            publish_slot_display_values(s, out, nf);
            return;
        }
    }
    let sampler_identity = sampler_identity(s);

    let gate_in = *inp.add(INPUT_GATE);
    let velocity_in = *inp.add(INPUT_VELOCITY);
    let trigger_in = *inp.add(INPUT_TRIGGER);
    let transport_bar_phase_in = *inp.add(INPUT_TRANSPORT_BAR_PHASE);
    let transport_bar_phase_inc_in = *inp.add(INPUT_TRANSPORT_BAR_PHASE_INC);
    let ext_inputs = [
        *inp.add(INPUT_EXT_BASE),
        *inp.add(INPUT_EXT_BASE + 1),
        *inp.add(INPUT_EXT_BASE + 2),
        *inp.add(INPUT_EXT_BASE + 3),
    ];

    let mut prev_gate = *s.add(IDX_PREV_GATE);
    if let Some((track_idx, voice_idx)) = sampler_identity {
        let sampler_active = sampler_voice_is_active(track_idx, voice_idx);
        if !sampler_active && !gate_timeline_has_activity(gate_in, trigger_in, nf, prev_gate) {
            record_disabled_sampler_skip(track_idx, nf);
            clear_outputs(out, nf);
            publish_slot_display_values(s, out, nf);
            return;
        }
    }

    let mut last_reset_counter = *s.add(IDX_LAST_RESET_COUNTER);
    let sample_rate = (*s.add(IDX_SAMPLE_RATE)).max(1.0);
    let bpm = (*s.add(PARAM_BPM)).clamp(20.0, 400.0);
    let reset_counter = *s.add(PARAM_RESET_COUNTER);
    let transport_clock_source = *s.add(IDX_TRANSPORT_CLOCK_SOURCE);
    let param_transport_bar_phase = *s.add(PARAM_TRANSPORT_BAR_PHASE);
    let param_transport_bar_phase_inc = *s.add(PARAM_TRANSPORT_BAR_PHASE_INC);
    let event_slice_start = (*s.add(IDX_EVENT_SLICE_START)).max(0.0) as usize;

    if reset_counter != last_reset_counter {
        for slot in 0..SLOT_COUNT {
            *s.add(slot_state_idx(slot, IDX_LFO_PHASE)) = 0.0;
            *s.add(slot_state_idx(slot, IDX_ENV)) = 0.0;
            *s.add(slot_state_idx(slot, IDX_RAND_PHASE)) = 0.0;
            *s.add(slot_state_idx(slot, IDX_DRIFT)) = 0.0;
            *s.add(slot_state_idx(slot, IDX_RAND_HOLD)) = 0.0;
            *s.add(slot_state_idx(slot, IDX_RAND_SMOOTH)) = 0.0;
            *s.add(slot_state_idx(slot, IDX_ENV_STAGE)) = ENV_STAGE_IDLE;
        }
        last_reset_counter = reset_counter;
    }

    let slot_sources = [
        slot_source(s, 0),
        slot_source(s, 1),
        slot_source(s, 2),
        slot_source(s, 3),
    ];
    if slot_sources.iter().all(|source| *source == SOURCE_OFF) {
        record_all_slots_off(nf);
        clear_outputs(out, nf);
        // The depth lanes and `__dgen_mod_active__` stay set when a source is
        // switched to Off, so the host keeps reading the display tail. Publish
        // the just-zeroed outputs or the visualizer freezes at the last
        // pre-Off value while the DSP renders the base (eseq-dtx.13).
        publish_slot_display_values(s, out, nf);
        if nf > 0 && !gate_in.is_null() {
            *s.add(IDX_PREV_GATE) = (*gate_in.add(nf - 1)).clamp(0.0, 1.0);
        }
        *s.add(IDX_LAST_RESET_COUNTER) = last_reset_counter;
        return;
    }
    record_rendered_call(custom_identity, sampler_identity, nf);

    for (slot, source) in slot_sources.iter().copied().enumerate() {
        if source == SOURCE_OFF {
            clear_output_slot(out, slot, nf);
        }
    }

    let mut sampler_voice_started = sampler_identity.is_none() || prev_gate > 0.5;
    for i in 0..nf {
        let gate = (*gate_in.add(i)).clamp(0.0, 1.0);
        let velocity = (*velocity_in.add(i)).clamp(0.0, 1.0);
        let trigger = (*trigger_in.add(i)).max(0.0);
        if !sampler_voice_started && gate <= 0.5 && trigger <= 0.5 {
            clear_output_frame(out, i);
            prev_gate = gate;
            continue;
        }
        if sampler_identity.is_some() && (gate > 0.5 || trigger > 0.5) {
            sampler_voice_started = true;
        }
        let note_on = (gate > 0.5 && prev_gate <= 0.5) || trigger > 0.5;
        let transport_bar_phase =
            if transport_clock_source > 0.5 && !transport_bar_phase_in.is_null() {
                normalize_phase(*transport_bar_phase_in.add(i))
            } else {
                transport_bar_phase_for_frame(
                    param_transport_bar_phase,
                    param_transport_bar_phase_inc,
                    event_slice_start + i,
                )
            };
        let transport_clock_advancing =
            if transport_clock_source > 0.5 && !transport_bar_phase_inc_in.is_null() {
                *transport_bar_phase_inc_in.add(i) > 0.0
            } else {
                param_transport_bar_phase_inc > 0.0
            };

        for (slot, source) in slot_sources.iter().copied().enumerate() {
            if source == SOURCE_OFF {
                continue;
            }
            let out_slot = *out.add(slot);
            *out_slot.add(i) = render_slot(
                s,
                &ext_inputs,
                slot,
                source,
                i,
                gate,
                velocity,
                note_on,
                sample_rate,
                bpm,
                transport_bar_phase,
                transport_clock_advancing,
            );
        }

        prev_gate = gate;
    }

    *s.add(IDX_PREV_GATE) = prev_gate;
    *s.add(IDX_LAST_RESET_COUNTER) = last_reset_counter;
    publish_slot_display_values(s, out, nf);
}

pub fn voice_modulator_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(voice_modulator_process),
        init: Some(voice_modulator_init),
        reset: None,
        migrate: None,
        begin_event_slice: Some(voice_modulator_begin_event_slice),
        ..NodeVTable::default()
    }
}

pub fn effect_modulator_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(voice_modulator_process),
        init: Some(effect_modulator_init),
        reset: None,
        migrate: None,
        begin_event_slice: Some(voice_modulator_begin_event_slice),
        ..NodeVTable::default()
    }
}

pub fn is_bar_resync_param(node_param_idx: u32) -> bool {
    if node_param_idx < MOD_PARAM_BASE {
        return false;
    }
    let idx = (node_param_idx - MOD_PARAM_BASE) as usize;
    for slot in 0..SLOT_COUNT {
        let base = slot_param_idx(slot, 0);
        if matches!(
            idx,
            x if x == base + PARAM_LFO_SYNC
                || x == base + PARAM_LFO_DIV
                || x == base + PARAM_LFO_RETRIGGER
                || x == base + PARAM_RAND_SYNC
                || x == base + PARAM_RAND_DIV
                || x == base + PARAM_DRIFT_SYNC
                || x == base + PARAM_DRIFT_DIV
        ) {
            return true;
        }
    }
    false
}

pub fn modulator_slot_label(slot: usize, fallback_name: &str) -> String {
    if (1..=SLOT_COUNT).contains(&slot) {
        format!("Mod {slot}")
    } else {
        fallback_name.to_string()
    }
}

pub fn modulator_slot_label_static(slot: usize) -> Option<&'static str> {
    match slot {
        1 => Some("Mod 1"),
        2 => Some("Mod 2"),
        3 => Some("Mod 3"),
        4 => Some("Mod 4"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_voice_modulator(
        state: &mut [f32; STATE_SIZE],
        frames: usize,
        ext: [[f32; 64]; EXT_INPUT_COUNT],
    ) -> [[f32; 64]; NUM_OUTPUTS] {
        let gate = [1.0f32; 64];
        let velocity = [1.0f32; 64];
        let trigger = [0.0f32; 64];
        render_voice_modulator_with_gate(state, frames, gate, velocity, trigger, ext)
    }

    fn render_voice_modulator_with_gate(
        state: &mut [f32; STATE_SIZE],
        frames: usize,
        gate: [f32; 64],
        velocity: [f32; 64],
        trigger: [f32; 64],
        ext: [[f32; 64]; EXT_INPUT_COUNT],
    ) -> [[f32; 64]; NUM_OUTPUTS] {
        render_voice_modulator_with_gate_and_clock(
            state, frames, gate, velocity, trigger, [0.0; 64], [0.0; 64], ext,
        )
    }

    fn render_voice_modulator_with_gate_and_clock(
        state: &mut [f32; STATE_SIZE],
        frames: usize,
        gate: [f32; 64],
        velocity: [f32; 64],
        trigger: [f32; 64],
        transport_bar_phase: [f32; 64],
        transport_bar_phase_inc: [f32; 64],
        ext: [[f32; 64]; EXT_INPUT_COUNT],
    ) -> [[f32; 64]; NUM_OUTPUTS] {
        assert!(frames <= 64);

        let pitch = [440.0f32; 64];
        let inputs = [
            gate.as_ptr() as *mut f32,
            pitch.as_ptr() as *mut f32,
            velocity.as_ptr() as *mut f32,
            trigger.as_ptr() as *mut f32,
            ext[0].as_ptr() as *mut f32,
            ext[1].as_ptr() as *mut f32,
            ext[2].as_ptr() as *mut f32,
            ext[3].as_ptr() as *mut f32,
            transport_bar_phase.as_ptr() as *mut f32,
            transport_bar_phase_inc.as_ptr() as *mut f32,
        ];

        let mut outputs = [[0.0f32; 64]; NUM_OUTPUTS];
        let output_ptrs = [
            outputs[0].as_mut_ptr(),
            outputs[1].as_mut_ptr(),
            outputs[2].as_mut_ptr(),
            outputs[3].as_mut_ptr(),
        ];

        unsafe {
            voice_modulator_process(
                inputs.as_ptr(),
                output_ptrs.as_ptr(),
                frames as c_int,
                state.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            );
        }

        outputs
    }

    fn init_state() -> [f32; STATE_SIZE] {
        let mut state = [0.0f32; STATE_SIZE];
        unsafe {
            voice_modulator_init(state.as_mut_ptr().cast(), 48_000, 64, std::ptr::null());
        }
        state
    }

    fn init_custom_engine_state(engine_id: usize, voice_idx: usize) -> [f32; STATE_SIZE] {
        let mut state = [0.0f32; STATE_SIZE];
        let initial = custom_engine_initial_state(engine_id, voice_idx);
        unsafe {
            voice_modulator_init(
                state.as_mut_ptr().cast(),
                48_000,
                64,
                (&initial as *const VoiceModulatorInitialState).cast(),
            );
        }
        state
    }

    fn init_sampler_voice_state(track_idx: usize, voice_idx: usize) -> [f32; STATE_SIZE] {
        let mut state = [0.0f32; STATE_SIZE];
        let initial = sampler_voice_initial_state(track_idx, voice_idx);
        unsafe {
            voice_modulator_init(
                state.as_mut_ptr().cast(),
                48_000,
                64,
                (&initial as *const VoiceModulatorInitialState).cast(),
            );
        }
        state
    }

    #[test]
    fn synced_lfo_divisions_are_calibrated_to_quarter_note_beats() {
        let bpm = 120.0;

        assert!(
            (synced_rate_hz(SyncDivision::ThirtySecond as usize, bpm) - 16.0).abs() < f32::EPSILON
        );
        assert!((synced_rate_hz(SyncDivision::Quarter as usize, bpm) - 2.0).abs() < f32::EPSILON);
        assert!((synced_rate_hz(SyncDivision::Whole as usize, bpm) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn synced_lfo_uses_transport_phase_params_instead_of_stored_phase() {
        let mut state = init_state();
        state[slot_source_param_idx(0)] = SOURCE_LFO as f32;
        state[slot_param_idx(0, PARAM_LFO_SYNC)] = 1.0;
        state[slot_param_idx(0, PARAM_LFO_DIV)] = sync_division_index(SyncDivision::Quarter);
        state[slot_param_idx(0, PARAM_LFO_SHAPE)] = SHAPE_SAW as f32;
        state[slot_state_idx(0, IDX_LFO_PHASE)] = 0.875;
        state[PARAM_TRANSPORT_BAR_PHASE] = 0.125;
        state[PARAM_TRANSPORT_BAR_PHASE_INC] = 0.001;

        let outputs = render_voice_modulator(&mut state, 1, [[0.0; 64]; EXT_INPUT_COUNT]);

        assert!((outputs[0][0] - 0.5).abs() <= 0.00001);
        assert!((state[slot_state_idx(0, IDX_LFO_PHASE)] - 0.5).abs() <= 0.00001);
    }

    /// eseq-dtx.13: the block's last output frame per slot lands in the
    /// display tail so a UI poller can read the live modulation signal back
    /// off the watchlist. The Filter Table spectrum visualizer is the first
    /// consumer.
    #[test]
    fn process_publishes_last_frame_slot_values_to_the_display_tail() {
        let mut state = init_state();
        state[slot_source_param_idx(0)] = SOURCE_LFO as f32;
        state[slot_param_idx(0, PARAM_LFO_SYNC)] = 1.0;
        state[slot_param_idx(0, PARAM_LFO_DIV)] = sync_division_index(SyncDivision::Quarter);
        state[slot_param_idx(0, PARAM_LFO_SHAPE)] = SHAPE_SAW as f32;
        state[PARAM_TRANSPORT_BAR_PHASE] = 0.0;
        state[PARAM_TRANSPORT_BAR_PHASE_INC] = 0.0625;
        // A stale display value must be overwritten, not left behind.
        for slot in 0..SLOT_COUNT {
            state[STATE_DISPLAY_SLOT_VALUE + slot] = -7.0;
        }

        let outputs = render_voice_modulator(&mut state, 4, [[0.0; 64]; EXT_INPUT_COUNT]);

        assert!(
            (state[STATE_DISPLAY_SLOT_VALUE] - outputs[0][3]).abs() <= 0.00001,
            "display tail should carry the block's last frame, got {} vs {}",
            state[STATE_DISPLAY_SLOT_VALUE],
            outputs[0][3],
        );
        assert!(
            outputs[0][3] > outputs[0][0],
            "the synced saw should have advanced across the block",
        );
        for slot in 1..SLOT_COUNT {
            assert!(
                (state[STATE_DISPLAY_SLOT_VALUE + slot] - outputs[slot][3]).abs() <= 0.00001,
                "slot {slot} display tail should mirror its own last frame",
            );
        }
    }

    #[test]
    fn additive_modulation_sums_every_assigned_depth_lane() {
        let depths = [0.25, -0.5, 0.0, 0.0];
        let slot_values = [1.0, 0.5, 1.0, 1.0];
        // Inactive destinations render the base value untouched.
        assert_eq!(
            additive_modulated_value(0.4, false, &depths, &slot_values),
            0.4
        );
        // Two simultaneous mods both land (requirement (d)).
        let value = additive_modulated_value(0.4, true, &depths, &slot_values);
        assert!((value - (0.4 + 0.25 - 0.25)).abs() <= 0.000001, "got {value}");
        // A modulator resting at zero is exactly the base value, which is what
        // makes the display settle back when modulation stops.
        assert_eq!(
            additive_modulated_value(0.4, true, &depths, &[0.0; SLOT_COUNT]),
            0.4
        );
    }

    #[test]
    fn synced_lfo_advances_sample_accurately_from_transport_increment() {
        let mut state = init_state();
        state[slot_source_param_idx(0)] = SOURCE_LFO as f32;
        state[slot_param_idx(0, PARAM_LFO_SYNC)] = 1.0;
        state[slot_param_idx(0, PARAM_LFO_DIV)] = sync_division_index(SyncDivision::Quarter);
        state[slot_param_idx(0, PARAM_LFO_SHAPE)] = SHAPE_SAW as f32;
        state[PARAM_TRANSPORT_BAR_PHASE] = 0.0;
        state[PARAM_TRANSPORT_BAR_PHASE_INC] = 0.0625;

        let outputs = render_voice_modulator(&mut state, 4, [[0.0; 64]; EXT_INPUT_COUNT]);

        assert!((outputs[0][0] - 0.0).abs() <= 0.00001);
        assert!((outputs[0][1] - 0.25).abs() <= 0.00001);
        assert!((outputs[0][2] - 0.5).abs() <= 0.00001);
        assert!((outputs[0][3] - 0.75).abs() <= 0.00001);
    }

    #[test]
    fn param_clocked_synced_lfo_runs_locally_when_transport_is_paused() {
        let mut state = init_state();
        state[slot_source_param_idx(0)] = SOURCE_LFO as f32;
        state[slot_param_idx(0, PARAM_LFO_SYNC)] = 1.0;
        state[slot_param_idx(0, PARAM_LFO_DIV)] = sync_division_index(SyncDivision::Quarter);
        state[slot_param_idx(0, PARAM_LFO_SHAPE)] = SHAPE_SAW as f32;
        state[PARAM_BPM] = 120.0;
        state[PARAM_TRANSPORT_BAR_PHASE] = 0.0;
        state[PARAM_TRANSPORT_BAR_PHASE_INC] = 0.0;
        state[slot_state_idx(0, IDX_LFO_PHASE)] = 0.25;

        let outputs = render_voice_modulator(&mut state, 2, [[0.0; 64]; EXT_INPUT_COUNT]);

        assert!(outputs[0][1] > outputs[0][0]);
        assert!(state[slot_state_idx(0, IDX_LFO_PHASE)] > 0.25);
    }

    #[test]
    fn param_clocked_synced_lfo_accounts_for_event_slice_start() {
        let mut state = init_state();
        state[slot_source_param_idx(0)] = SOURCE_LFO as f32;
        state[slot_param_idx(0, PARAM_LFO_SYNC)] = 1.0;
        state[slot_param_idx(0, PARAM_LFO_DIV)] = sync_division_index(SyncDivision::Quarter);
        state[slot_param_idx(0, PARAM_LFO_SHAPE)] = SHAPE_SAW as f32;
        state[PARAM_TRANSPORT_BAR_PHASE] = 0.0;
        state[PARAM_TRANSPORT_BAR_PHASE_INC] = 0.0625;
        unsafe {
            voice_modulator_begin_event_slice(state.as_mut_ptr().cast(), 7, 2, 2);
        }

        let outputs = render_voice_modulator(&mut state, 2, [[0.0; 64]; EXT_INPUT_COUNT]);

        assert!((outputs[0][0] - 0.5).abs() <= 0.00001);
        assert!((outputs[0][1] - 0.75).abs() <= 0.00001);
    }

    #[test]
    fn voice_synced_lfo_uses_transport_phase_input_clock() {
        let track_idx = 61;
        let voice_idx = 2;
        set_sampler_active_mask(track_idx, 1u64 << voice_idx);
        let mut state = init_sampler_voice_state(track_idx, voice_idx);
        state[slot_source_param_idx(0)] = SOURCE_LFO as f32;
        state[slot_param_idx(0, PARAM_LFO_SYNC)] = 1.0;
        state[slot_param_idx(0, PARAM_LFO_DIV)] = sync_division_index(SyncDivision::Quarter);
        state[slot_param_idx(0, PARAM_LFO_SHAPE)] = SHAPE_SAW as f32;
        state[PARAM_TRANSPORT_BAR_PHASE] = 0.0;
        state[PARAM_TRANSPORT_BAR_PHASE_INC] = 0.0;

        let mut clock = [0.0f32; 64];
        clock[0] = 0.125;
        let mut clock_inc = [0.0f32; 64];
        clock_inc[0] = 0.0625;
        let outputs = render_voice_modulator_with_gate_and_clock(
            &mut state,
            1,
            [1.0; 64],
            [1.0; 64],
            [0.0; 64],
            clock,
            clock_inc,
            [[0.0; 64]; EXT_INPUT_COUNT],
        );

        assert!((outputs[0][0] - 0.5).abs() <= 0.00001);
        set_sampler_active_mask(track_idx, 0);
    }

    #[test]
    fn voice_synced_lfo_runs_locally_when_transport_input_clock_is_paused() {
        let track_idx = 60;
        let voice_idx = 2;
        set_sampler_active_mask(track_idx, 1u64 << voice_idx);
        let mut state = init_sampler_voice_state(track_idx, voice_idx);
        state[slot_source_param_idx(0)] = SOURCE_LFO as f32;
        state[slot_param_idx(0, PARAM_LFO_SYNC)] = 1.0;
        state[slot_param_idx(0, PARAM_LFO_DIV)] = sync_division_index(SyncDivision::Quarter);
        state[slot_param_idx(0, PARAM_LFO_SHAPE)] = SHAPE_SAW as f32;
        state[PARAM_BPM] = 120.0;
        state[slot_state_idx(0, IDX_LFO_PHASE)] = 0.25;

        let outputs = render_voice_modulator_with_gate_and_clock(
            &mut state,
            2,
            [1.0; 64],
            [1.0; 64],
            [0.0; 64],
            [0.0; 64],
            [0.0; 64],
            [[0.0; 64]; EXT_INPUT_COUNT],
        );

        assert!(outputs[0][1] > outputs[0][0]);
        assert!(state[slot_state_idx(0, IDX_LFO_PHASE)] > 0.25);
        set_sampler_active_mask(track_idx, 0);
    }

    #[test]
    fn source_type_is_enum_and_defaults_to_legacy_first_four_classes() {
        let desc = param_descriptors();
        let sources = desc
            .iter()
            .filter(|param| param.name.ends_with("_source"))
            .map(|param| (param.name.as_str(), param.default, &param.kind))
            .collect::<Vec<_>>();
        assert_eq!(sources.len(), SLOT_COUNT);
        assert_eq!(sources[0].0, "mod1_source");
        assert_eq!(sources[0].1, SOURCE_LFO as f32);
        assert_eq!(sources[1].1, SOURCE_ENV as f32);
        assert_eq!(sources[2].1, SOURCE_RAND as f32);
        assert_eq!(sources[3].1, SOURCE_DRIFT as f32);
        match sources[0].2 {
            ParamKind::Enum { labels } => assert_eq!(
                labels,
                &vec![
                    "off".to_string(),
                    "lfo".to_string(),
                    "env".to_string(),
                    "rand".to_string(),
                    "drift".to_string(),
                    "ext1".to_string(),
                    "ext2".to_string(),
                    "ext3".to_string(),
                    "ext4".to_string(),
                ]
            ),
            other => panic!("source type should be enum, got {other:?}"),
        }
    }

    #[test]
    fn selected_source_param_indices_follow_slot_source_state() {
        let params = param_descriptors();
        let mut values = params.iter().map(|param| param.default).collect::<Vec<_>>();
        let idx = |name: &str| {
            params
                .iter()
                .position(|param| param.name == name)
                .unwrap_or_else(|| panic!("missing param {name}"))
        };
        let selected_names = |values: &[f32]| {
            selected_source_param_indices(&params, |idx, _| values[idx])
                .into_iter()
                .map(|idx| params[idx].name.as_str())
                .collect::<Vec<_>>()
        };

        let names = selected_names(&values);
        assert!(names.contains(&"mod1_source"));
        assert!(names.contains(&"mod1_lfo_rate"));
        assert!(!names.contains(&"mod1_lfo_div"));
        assert!(!names.contains(&"mod1_lfo_pw"));
        assert!(names.contains(&"mod2_env_attack"));
        assert!(names.contains(&"mod3_rand_rate"));
        assert!(names.contains(&"mod4_drift_rate"));

        values[idx("mod1_lfo_sync")] = 1.0;
        values[idx("mod1_lfo_shape")] = SHAPE_PULSE as f32;
        let names = selected_names(&values);
        assert!(!names.contains(&"mod1_lfo_rate"));
        assert!(names.contains(&"mod1_lfo_div"));
        assert!(names.contains(&"mod1_lfo_pw"));

        values[idx("mod1_source")] = SOURCE_EXT2 as f32;
        let names = selected_names(&values);
        assert!(names.contains(&"mod1_source"));
        assert!(!names.contains(&"mod1_lfo_div"));
        assert!(!names.contains(&"mod1_lfo_pw"));
    }

    #[test]
    fn default_slots_render_lfo_env_rand_and_drift() {
        let mut state = init_state();
        let outputs = render_voice_modulator(&mut state, 64, [[0.0; 64]; EXT_INPUT_COUNT]);

        assert!(outputs[0].iter().any(|value| *value > 0.0));
        assert!(outputs[1].iter().any(|value| *value > 0.0));
        assert!(outputs[2].iter().any(|value| *value > 0.0));
        assert!(outputs[3].iter().any(|value| *value > 0.0));
    }

    #[test]
    fn duplicate_envelope_slots_are_independent() {
        let mut state = init_state();
        state[slot_source_param_idx(0)] = SOURCE_ENV as f32;
        state[slot_source_param_idx(1)] = SOURCE_ENV as f32;
        state[slot_param_idx(0, PARAM_ENV_ATTACK_MS)] = 10.0;
        state[slot_param_idx(1, PARAM_ENV_ATTACK_MS)] = 1000.0;

        let outputs = render_voice_modulator(&mut state, 64, [[0.0; 64]; EXT_INPUT_COUNT]);

        assert!(outputs[0][63] > outputs[1][63] * 10.0);
        assert_ne!(
            state[slot_state_idx(0, IDX_ENV)],
            state[slot_state_idx(1, IDX_ENV)]
        );
    }

    #[test]
    fn ext_source_copies_selected_external_input() {
        let mut state = init_state();
        state[slot_source_param_idx(0)] = SOURCE_EXT3 as f32;
        let mut ext = [[0.0f32; 64]; EXT_INPUT_COUNT];
        ext[2][0] = 0.25;
        ext[2][1] = 0.75;
        ext[2][2] = 1.25;

        let outputs = render_voice_modulator(&mut state, 3, ext);

        assert_eq!(&outputs[0][..3], &[0.25, 0.75, 1.0]);
    }

    #[test]
    fn off_slot_outputs_zero_and_does_not_advance_slot_state() {
        let mut state = init_state();
        state[slot_source_param_idx(0)] = SOURCE_OFF as f32;
        let before_phase = state[slot_state_idx(0, IDX_LFO_PHASE)];
        let before_env = state[slot_state_idx(0, IDX_ENV)];

        let outputs = render_voice_modulator(&mut state, 64, [[0.0; 64]; EXT_INPUT_COUNT]);

        assert!(outputs[0].iter().all(|value| *value == 0.0));
        assert_eq!(state[slot_state_idx(0, IDX_LFO_PHASE)], before_phase);
        assert_eq!(state[slot_state_idx(0, IDX_ENV)], before_env);
    }

    #[test]
    fn all_off_slots_clear_outputs_without_advancing_any_slot_state() {
        let mut state = init_state();
        for slot in 0..SLOT_COUNT {
            state[slot_source_param_idx(slot)] = SOURCE_OFF as f32;
        }
        state[slot_state_idx(0, IDX_LFO_PHASE)] = 0.5;
        state[PARAM_RESET_COUNTER] = 1.0;
        let before_lfo_phase = state[slot_state_idx(0, IDX_LFO_PHASE)];
        let before_env = state[slot_state_idx(1, IDX_ENV)];
        let before_rand_phase = state[slot_state_idx(2, IDX_RAND_PHASE)];
        let before_drift = state[slot_state_idx(3, IDX_DRIFT)];

        let outputs = render_voice_modulator(&mut state, 64, [[0.0; 64]; EXT_INPUT_COUNT]);

        assert!(outputs
            .iter()
            .all(|slot| slot.iter().all(|value| *value == 0.0)));
        assert_ne!(before_lfo_phase, 0.0);
        assert_eq!(state[slot_state_idx(0, IDX_LFO_PHASE)], 0.0);
        assert_eq!(state[slot_state_idx(1, IDX_ENV)], before_env);
        assert_eq!(state[slot_state_idx(2, IDX_RAND_PHASE)], before_rand_phase);
        assert_eq!(state[slot_state_idx(3, IDX_DRIFT)], before_drift);
        assert_eq!(state[IDX_PREV_GATE], 1.0);
        assert_eq!(state[IDX_LAST_RESET_COUNTER], 1.0);
    }

    /// eseq-dtx.13: switching every slot source to Off leaves the destination's
    /// depth lanes and `__dgen_mod_active__` flag set, so the UI keeps reading
    /// the display tail. The all-off early return therefore has to publish the
    /// zeroed outputs; otherwise the visualizer freezes at the last pre-Off
    /// offset while the DSP renders the base value.
    #[test]
    fn all_off_slots_publish_zero_display_values() {
        let mut state = init_state();
        for slot in 0..SLOT_COUNT {
            state[slot_source_param_idx(slot)] = SOURCE_OFF as f32;
            // A stale pre-Off value must not survive the early return.
            state[STATE_DISPLAY_SLOT_VALUE + slot] = 0.75;
        }

        let _ = render_voice_modulator(&mut state, 64, [[0.0; 64]; EXT_INPUT_COUNT]);

        for slot in 0..SLOT_COUNT {
            assert_eq!(
                state[STATE_DISPLAY_SLOT_VALUE + slot],
                0.0,
                "slot {slot} display tail should settle to zero when the source is Off",
            );
        }
    }

    #[test]
    fn custom_engine_disabled_voice_outputs_zero_without_advancing_modulators() {
        let engine_id = 63;
        crate::lisp_host::set_dgen_engine_enabled_voices(engine_id, 1);
        let mut state = init_custom_engine_state(engine_id, 3);
        let before_lfo_phase = state[slot_state_idx(0, IDX_LFO_PHASE)];
        let before_rand_phase = state[slot_state_idx(2, IDX_RAND_PHASE)];

        let outputs = render_voice_modulator(&mut state, 64, [[0.0; 64]; EXT_INPUT_COUNT]);

        assert!(outputs
            .iter()
            .all(|slot| slot.iter().all(|value| *value == 0.0)));
        assert_eq!(state[slot_state_idx(0, IDX_LFO_PHASE)], before_lfo_phase);
        assert_eq!(state[slot_state_idx(2, IDX_RAND_PHASE)], before_rand_phase);

        crate::lisp_host::set_dgen_engine_enabled_voices(engine_id, 4);
        let outputs = render_voice_modulator(&mut state, 64, [[0.0; 64]; EXT_INPUT_COUNT]);

        assert!(outputs[0].iter().any(|value| *value > 0.0));
        crate::lisp_host::reset_dgen_engine_enabled_voices(engine_id);
    }

    #[test]
    fn inactive_sampler_voice_outputs_zero_without_advancing_modulators() {
        let track_idx = 62;
        set_sampler_active_mask(track_idx, 0);
        let mut state = init_sampler_voice_state(track_idx, 2);
        let before_lfo_phase = state[slot_state_idx(0, IDX_LFO_PHASE)];
        let before_rand_phase = state[slot_state_idx(2, IDX_RAND_PHASE)];
        let gate = [0.0f32; 64];
        let velocity = [1.0f32; 64];
        let trigger = [0.0f32; 64];

        let outputs = render_voice_modulator_with_gate(
            &mut state,
            64,
            gate,
            velocity,
            trigger,
            [[0.0; 64]; EXT_INPUT_COUNT],
        );

        assert!(outputs
            .iter()
            .all(|slot| slot.iter().all(|value| *value == 0.0)));
        assert_eq!(state[slot_state_idx(0, IDX_LFO_PHASE)], before_lfo_phase);
        assert_eq!(state[slot_state_idx(2, IDX_RAND_PHASE)], before_rand_phase);

        set_sampler_active_mask(track_idx, 1u64 << 2);
        let outputs = render_voice_modulator(&mut state, 64, [[0.0; 64]; EXT_INPUT_COUNT]);

        assert!(outputs[0].iter().any(|value| *value > 0.0));
        set_sampler_active_mask(track_idx, 0);
    }

    #[test]
    fn active_sampler_voice_starts_modulating_at_in_block_trigger_frame() {
        let track_idx = 61;
        set_sampler_active_mask(track_idx, 1u64 << 1);
        let mut state = init_sampler_voice_state(track_idx, 1);
        state[slot_source_param_idx(0)] = SOURCE_LFO as f32;
        for slot in 1..SLOT_COUNT {
            state[slot_source_param_idx(slot)] = SOURCE_OFF as f32;
        }
        let mut gate = [0.0f32; 64];
        let velocity = [1.0f32; 64];
        let mut trigger = [0.0f32; 64];
        for sample in gate.iter_mut().skip(3) {
            *sample = 1.0;
        }
        trigger[3] = 1.0;

        let outputs = render_voice_modulator_with_gate(
            &mut state,
            8,
            gate,
            velocity,
            trigger,
            [[0.0; 64]; EXT_INPUT_COUNT],
        );

        assert_eq!(&outputs[0][..3], &[0.0, 0.0, 0.0]);
        assert!(outputs[0][3] > 0.0);
        set_sampler_active_mask(track_idx, 0);
    }

    #[test]
    fn inactive_sampler_voice_with_in_block_gate_activity_does_not_skip_whole_block() {
        let track_idx = 60;
        set_sampler_active_mask(track_idx, 0);
        let mut state = init_sampler_voice_state(track_idx, 0);
        state[slot_source_param_idx(0)] = SOURCE_LFO as f32;
        for slot in 1..SLOT_COUNT {
            state[slot_source_param_idx(slot)] = SOURCE_OFF as f32;
        }
        let mut gate = [0.0f32; 64];
        let velocity = [1.0f32; 64];
        let mut trigger = [0.0f32; 64];
        gate[2] = 1.0;
        gate[3] = 1.0;
        trigger[2] = 1.0;

        let outputs = render_voice_modulator_with_gate(
            &mut state,
            6,
            gate,
            velocity,
            trigger,
            [[0.0; 64]; EXT_INPUT_COUNT],
        );

        assert_eq!(&outputs[0][..2], &[0.0, 0.0]);
        assert!(outputs[0][2] > 0.0);
        assert!(state[slot_state_idx(0, IDX_LFO_PHASE)] > 0.0);
    }

    #[test]
    fn inactive_sampler_voice_with_previous_gate_renders_until_in_block_release() {
        let track_idx = 59;
        set_sampler_active_mask(track_idx, 0);
        let mut state = init_sampler_voice_state(track_idx, 0);
        state[slot_source_param_idx(0)] = SOURCE_LFO as f32;
        for slot in 1..SLOT_COUNT {
            state[slot_source_param_idx(slot)] = SOURCE_OFF as f32;
        }
        state[IDX_PREV_GATE] = 1.0;
        let mut gate = [0.0f32; 64];
        let velocity = [1.0f32; 64];
        let trigger = [0.0f32; 64];
        gate[0] = 1.0;
        gate[1] = 1.0;
        gate[2] = 1.0;

        let outputs = render_voice_modulator_with_gate(
            &mut state,
            6,
            gate,
            velocity,
            trigger,
            [[0.0; 64]; EXT_INPUT_COUNT],
        );

        assert!(outputs[0][0] > 0.0);
        assert!(outputs[0][2] > 0.0);
        assert_eq!(state[IDX_PREV_GATE], 0.0);
    }
}
