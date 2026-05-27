use std::os::raw::{c_int, c_void};

use crate::audiograph::NodeVTable;
use crate::effects::{ParamDescriptor, ParamKind, ParamScaling, SyncDivision};

pub const SLOT_COUNT: usize = 4;
pub const NUM_OUTPUTS: usize = SLOT_COUNT;
pub const INPUT_COUNT: usize = 8;
pub const EXT_INPUT_COUNT: usize = 4;

/// Deliberately versioned away from the old fixed-source layout at 1_000_000.
/// Old project/preset cells must not collide with the new configurable slot
/// params and be silently interpreted as different modulation controls.
pub const MOD_PARAM_BASE: u32 = 1_100_000;
pub const LEGACY_FIXED_MOD_PARAM_BASE: u32 = 1_000_000;

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
pub const PARAM_RESET_COUNTER: usize = PARAM_BPM + 1;

pub const STATE_SIZE: usize = PARAM_RESET_COUNTER + 1;

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

unsafe extern "C" fn voice_modulator_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    for i in 0..STATE_SIZE {
        *s.add(i) = 0.0;
    }
    for slot in 0..SLOT_COUNT {
        *s.add(slot_state_idx(slot, IDX_RNG)) = 0x1234_5678u32.wrapping_add(slot as u32) as f32;
    }
    *s.add(IDX_SAMPLE_RATE) = sample_rate as f32;
    init_param_defaults(s);
}

unsafe extern "C" fn effect_modulator_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    for i in 0..STATE_SIZE {
        *s.add(i) = 0.0;
    }
    for slot in 0..SLOT_COUNT {
        *s.add(slot_state_idx(slot, IDX_RNG)) = 0x1234_5678u32.wrapping_add(slot as u32) as f32;
    }
    *s.add(IDX_SAMPLE_RATE) = sample_rate as f32;
    init_effect_param_defaults(s);
}

unsafe fn render_slot(
    s: *mut f32,
    ext_inputs: &[*mut f32; EXT_INPUT_COUNT],
    slot: usize,
    frame: usize,
    gate: f32,
    velocity: f32,
    note_on: bool,
    sample_rate: f32,
    bpm: f32,
) -> f32 {
    let source = (*s.add(slot_source_param_idx(slot))).round() as usize;
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
            if note_on && *s.add(slot_param_idx(slot, PARAM_LFO_RETRIGGER)) > 0.5 {
                phase = 0.0;
            }
            let rate = if *s.add(slot_param_idx(slot, PARAM_LFO_SYNC)) > 0.5 {
                synced_rate_hz(
                    (*s.add(slot_param_idx(slot, PARAM_LFO_DIV))).round() as usize,
                    bpm,
                )
            } else {
                (*s.add(slot_param_idx(slot, PARAM_LFO_RATE_HZ))).clamp(0.01, 20.0)
            };
            phase = (phase + rate / sample_rate).fract();
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

    let gate_in = *inp.add(0);
    let velocity_in = *inp.add(2);
    let trigger_in = *inp.add(3);
    let ext_inputs = [*inp.add(4), *inp.add(5), *inp.add(6), *inp.add(7)];

    let mut prev_gate = *s.add(IDX_PREV_GATE);
    let mut last_reset_counter = *s.add(IDX_LAST_RESET_COUNTER);
    let sample_rate = (*s.add(IDX_SAMPLE_RATE)).max(1.0);
    let bpm = (*s.add(PARAM_BPM)).clamp(20.0, 400.0);
    let reset_counter = *s.add(PARAM_RESET_COUNTER);

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

    for i in 0..nf {
        let gate = (*gate_in.add(i)).clamp(0.0, 1.0);
        let velocity = (*velocity_in.add(i)).clamp(0.0, 1.0);
        let trigger = (*trigger_in.add(i)).max(0.0);
        let note_on = (gate > 0.5 && prev_gate <= 0.5) || trigger > 0.5;

        for slot in 0..SLOT_COUNT {
            let out_slot = *out.add(slot);
            *out_slot.add(i) = render_slot(
                s,
                &ext_inputs,
                slot,
                i,
                gate,
                velocity,
                note_on,
                sample_rate,
                bpm,
            );
        }

        prev_gate = gate;
    }

    *s.add(IDX_PREV_GATE) = prev_gate;
    *s.add(IDX_LAST_RESET_COUNTER) = last_reset_counter;
}

pub fn voice_modulator_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(voice_modulator_process),
        init: Some(voice_modulator_init),
        reset: None,
        migrate: None,
    }
}

pub fn effect_modulator_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(voice_modulator_process),
        init: Some(effect_modulator_init),
        reset: None,
        migrate: None,
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
        assert!(frames <= 64);

        let gate = [1.0f32; 64];
        let pitch = [440.0f32; 64];
        let velocity = [1.0f32; 64];
        let trigger = [0.0f32; 64];
        let inputs = [
            gate.as_ptr() as *mut f32,
            pitch.as_ptr() as *mut f32,
            velocity.as_ptr() as *mut f32,
            trigger.as_ptr() as *mut f32,
            ext[0].as_ptr() as *mut f32,
            ext[1].as_ptr() as *mut f32,
            ext[2].as_ptr() as *mut f32,
            ext[3].as_ptr() as *mut f32,
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
}
