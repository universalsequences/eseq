use std::os::raw::{c_int, c_void};

use crate::audiograph::NodeVTable;
use crate::effects::{ParamDescriptor, ParamKind, ParamScaling, SyncDivision};

pub const NUM_OUTPUTS: usize = 6;
pub const MOD_PARAM_BASE: u32 = 1_000_000;

const IDX_LFO1_PHASE: usize = 0;
const IDX_LFO2_PHASE: usize = 1;
const IDX_LFO3_PHASE: usize = 2;
const IDX_ENV2: usize = 3;
const IDX_RAND_PHASE: usize = 4;
const IDX_DRIFT: usize = 5;
const IDX_PREV_GATE: usize = 6;
const IDX_RNG: usize = 7;
const IDX_RAND_HOLD: usize = 8;
const IDX_RAND_SMOOTH: usize = 9;
const IDX_LAST_RESET_COUNTER: usize = 10;
const IDX_ENV_STAGE: usize = 11;
const IDX_SAMPLE_RATE: usize = 48;

pub const PARAM_BPM: usize = 49;
pub const PARAM_RESET_COUNTER: usize = 50;

pub const PARAM_LFO1_RATE_HZ: usize = 13;
pub const PARAM_LFO1_SYNC: usize = 14;
pub const PARAM_LFO1_DIV: usize = 15;
pub const PARAM_LFO1_SHAPE: usize = 16;
pub const PARAM_LFO1_PW: usize = 17;
pub const PARAM_LFO1_RETRIGGER: usize = 18;
pub const PARAM_LFO2_RATE_HZ: usize = 19;
pub const PARAM_LFO2_SYNC: usize = 20;
pub const PARAM_LFO2_DIV: usize = 21;
pub const PARAM_LFO2_SHAPE: usize = 22;
pub const PARAM_LFO2_PW: usize = 23;
pub const PARAM_LFO2_RETRIGGER: usize = 24;
pub const PARAM_LFO3_RATE_HZ: usize = 25;
pub const PARAM_LFO3_SYNC: usize = 26;
pub const PARAM_LFO3_DIV: usize = 27;
pub const PARAM_LFO3_SHAPE: usize = 28;
pub const PARAM_LFO3_PW: usize = 29;
pub const PARAM_LFO3_RETRIGGER: usize = 30;
pub const PARAM_ENV_ATTACK_MS: usize = 31;
pub const PARAM_ENV_DECAY_MS: usize = 32;
pub const PARAM_ENV_SUSTAIN: usize = 33;
pub const PARAM_ENV_RELEASE_MS: usize = 34;
pub const PARAM_RAND_RATE_HZ: usize = 35;
pub const PARAM_RAND_SYNC: usize = 36;
pub const PARAM_RAND_DIV: usize = 37;
pub const PARAM_RAND_SLEW: usize = 38;
pub const PARAM_DRIFT_RATE: usize = 39;
pub const PARAM_DRIFT_SYNC: usize = 40;
pub const PARAM_DRIFT_DIV: usize = 41;
pub const PARAM_MOD1_DEPTH: usize = 42;
pub const PARAM_MOD2_DEPTH: usize = 43;
pub const PARAM_MOD3_DEPTH: usize = 44;
pub const PARAM_MOD4_DEPTH: usize = 45;
pub const PARAM_MOD5_DEPTH: usize = 46;
pub const PARAM_MOD6_DEPTH: usize = 47;

pub const PARAM_COUNT: usize = 37;
pub const STATE_SIZE: usize = 51;

const SHAPE_TRIANGLE: usize = 0;
const SHAPE_SINE: usize = 1;
const SHAPE_PULSE: usize = 2;
const SHAPE_SAW: usize = 3;
const ENV_STAGE_IDLE: f32 = 0.0;
const ENV_STAGE_ATTACK: f32 = 1.0;
const ENV_STAGE_DECAY: f32 = 2.0;
const ENV_STAGE_SUSTAIN: f32 = 3.0;
const ENV_STAGE_RELEASE: f32 = 4.0;

fn sync_labels() -> Vec<String> {
    SyncDivision::ALL
        .iter()
        .map(|division| division.label().to_string())
        .collect()
}

fn shape_labels() -> Vec<String> {
    vec![
        "triangle".to_string(),
        "sine".to_string(),
        "pulse".to_string(),
        "sawtooth".to_string(),
    ]
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

unsafe fn init_param_defaults(s: *mut f32) {
    *s.add(PARAM_BPM) = 120.0;
    *s.add(PARAM_RESET_COUNTER) = 0.0;

    *s.add(PARAM_LFO1_RATE_HZ) = 5.0;
    *s.add(PARAM_LFO1_SYNC) = 0.0;
    *s.add(PARAM_LFO1_DIV) = sync_division_index(SyncDivision::Quarter);
    *s.add(PARAM_LFO1_SHAPE) = SHAPE_TRIANGLE as f32;
    *s.add(PARAM_LFO1_PW) = 0.5;
    *s.add(PARAM_LFO1_RETRIGGER) = 0.0;

    *s.add(PARAM_LFO2_RATE_HZ) = 1.7;
    *s.add(PARAM_LFO2_SYNC) = 0.0;
    *s.add(PARAM_LFO2_DIV) = sync_division_index(SyncDivision::Half);
    *s.add(PARAM_LFO2_SHAPE) = SHAPE_TRIANGLE as f32;
    *s.add(PARAM_LFO2_PW) = 0.5;
    *s.add(PARAM_LFO2_RETRIGGER) = 0.0;

    *s.add(PARAM_LFO3_RATE_HZ) = 0.37;
    *s.add(PARAM_LFO3_SYNC) = 0.0;
    *s.add(PARAM_LFO3_DIV) = sync_division_index(SyncDivision::Whole);
    *s.add(PARAM_LFO3_SHAPE) = SHAPE_TRIANGLE as f32;
    *s.add(PARAM_LFO3_PW) = 0.5;
    *s.add(PARAM_LFO3_RETRIGGER) = 0.0;

    *s.add(PARAM_ENV_ATTACK_MS) = 6.0;
    *s.add(PARAM_ENV_DECAY_MS) = 180.0;
    *s.add(PARAM_ENV_SUSTAIN) = 0.55;
    *s.add(PARAM_ENV_RELEASE_MS) = 240.0;

    *s.add(PARAM_RAND_RATE_HZ) = 3.0;
    *s.add(PARAM_RAND_SYNC) = 0.0;
    *s.add(PARAM_RAND_DIV) = sync_division_index(SyncDivision::Whole);
    *s.add(PARAM_RAND_SLEW) = 0.0;

    *s.add(PARAM_DRIFT_RATE) = 0.00035;
    *s.add(PARAM_DRIFT_SYNC) = 0.0;
    *s.add(PARAM_DRIFT_DIV) = sync_division_index(SyncDivision::Whole);

    *s.add(PARAM_MOD1_DEPTH) = 1.0;
    *s.add(PARAM_MOD2_DEPTH) = 1.0;
    *s.add(PARAM_MOD3_DEPTH) = 1.0;
    *s.add(PARAM_MOD4_DEPTH) = 1.0;
    *s.add(PARAM_MOD5_DEPTH) = 1.0;
    *s.add(PARAM_MOD6_DEPTH) = 1.0;
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
    });
}

pub fn param_descriptors() -> Vec<ParamDescriptor> {
    let mut out = Vec::new();
    let sync_div_labels = sync_labels();
    let shape_labels = shape_labels();

    for &(
        rate_idx,
        sync_idx,
        div_idx,
        shape_idx,
        pw_idx,
        retrig_idx,
        prefix,
        rate_default,
        div_default,
        retrig_default,
    ) in &[
        (
            PARAM_LFO1_RATE_HZ,
            PARAM_LFO1_SYNC,
            PARAM_LFO1_DIV,
            PARAM_LFO1_SHAPE,
            PARAM_LFO1_PW,
            PARAM_LFO1_RETRIGGER,
            "mod_lfo1",
            5.0,
            sync_division_index(SyncDivision::Quarter),
            0.0,
        ),
        (
            PARAM_LFO2_RATE_HZ,
            PARAM_LFO2_SYNC,
            PARAM_LFO2_DIV,
            PARAM_LFO2_SHAPE,
            PARAM_LFO2_PW,
            PARAM_LFO2_RETRIGGER,
            "mod_lfo2",
            1.7,
            sync_division_index(SyncDivision::Half),
            0.0,
        ),
        (
            PARAM_LFO3_RATE_HZ,
            PARAM_LFO3_SYNC,
            PARAM_LFO3_DIV,
            PARAM_LFO3_SHAPE,
            PARAM_LFO3_PW,
            PARAM_LFO3_RETRIGGER,
            "mod_lfo3",
            0.37,
            sync_division_index(SyncDivision::Whole),
            0.0,
        ),
    ] {
        push_param(
            &mut out,
            &format!("{prefix}_rate"),
            0.01,
            20.0,
            rate_default,
            ParamKind::Continuous {
                unit: Some("Hz".to_string()),
            },
            ParamScaling::Exponential,
            rate_idx,
        );
        push_param(
            &mut out,
            &format!("{prefix}_sync"),
            0.0,
            1.0,
            0.0,
            ParamKind::Boolean,
            ParamScaling::Linear,
            sync_idx,
        );
        push_param(
            &mut out,
            &format!("{prefix}_div"),
            0.0,
            (sync_div_labels.len() - 1) as f32,
            div_default,
            ParamKind::Enum {
                labels: sync_div_labels.clone(),
            },
            ParamScaling::Linear,
            div_idx,
        );
        push_param(
            &mut out,
            &format!("{prefix}_shape"),
            0.0,
            (shape_labels.len() - 1) as f32,
            0.0,
            ParamKind::Enum {
                labels: shape_labels.clone(),
            },
            ParamScaling::Linear,
            shape_idx,
        );
        push_param(
            &mut out,
            &format!("{prefix}_pw"),
            0.05,
            0.95,
            0.5,
            ParamKind::Continuous { unit: None },
            ParamScaling::Linear,
            pw_idx,
        );
        push_param(
            &mut out,
            &format!("{prefix}_retrigger"),
            0.0,
            1.0,
            retrig_default,
            ParamKind::Boolean,
            ParamScaling::Linear,
            retrig_idx,
        );
    }

    push_param(
        &mut out,
        "mod_env_attack",
        1.0,
        2000.0,
        6.0,
        ParamKind::Continuous {
            unit: Some("ms".to_string()),
        },
        ParamScaling::Exponential,
        PARAM_ENV_ATTACK_MS,
    );
    push_param(
        &mut out,
        "mod_env_decay",
        5.0,
        4000.0,
        180.0,
        ParamKind::Continuous {
            unit: Some("ms".to_string()),
        },
        ParamScaling::Exponential,
        PARAM_ENV_DECAY_MS,
    );
    push_param(
        &mut out,
        "mod_env_sustain",
        0.0,
        1.0,
        0.55,
        ParamKind::Continuous {
            unit: Some("%".to_string()),
        },
        ParamScaling::Linear,
        PARAM_ENV_SUSTAIN,
    );
    push_param(
        &mut out,
        "mod_env_release",
        5.0,
        4000.0,
        240.0,
        ParamKind::Continuous {
            unit: Some("ms".to_string()),
        },
        ParamScaling::Exponential,
        PARAM_ENV_RELEASE_MS,
    );

    push_param(
        &mut out,
        "mod_rand_rate",
        0.05,
        20.0,
        3.0,
        ParamKind::Continuous {
            unit: Some("Hz".to_string()),
        },
        ParamScaling::Exponential,
        PARAM_RAND_RATE_HZ,
    );
    push_param(
        &mut out,
        "mod_rand_sync",
        0.0,
        1.0,
        0.0,
        ParamKind::Boolean,
        ParamScaling::Linear,
        PARAM_RAND_SYNC,
    );
    push_param(
        &mut out,
        "mod_rand_div",
        0.0,
        (sync_div_labels.len() - 1) as f32,
        sync_division_index(SyncDivision::Whole),
        ParamKind::Enum {
            labels: sync_div_labels.clone(),
        },
        ParamScaling::Linear,
        PARAM_RAND_DIV,
    );
    push_param(
        &mut out,
        "mod_rand_slew",
        0.0,
        0.999,
        0.0,
        ParamKind::Continuous { unit: None },
        ParamScaling::Linear,
        PARAM_RAND_SLEW,
    );

    push_param(
        &mut out,
        "mod_drift_rate",
        0.00001,
        0.01,
        0.00035,
        ParamKind::Continuous {
            unit: Some("Hz".to_string()),
        },
        ParamScaling::Exponential,
        PARAM_DRIFT_RATE,
    );
    push_param(
        &mut out,
        "mod_drift_sync",
        0.0,
        1.0,
        0.0,
        ParamKind::Boolean,
        ParamScaling::Linear,
        PARAM_DRIFT_SYNC,
    );
    push_param(
        &mut out,
        "mod_drift_div",
        0.0,
        (sync_div_labels.len() - 1) as f32,
        sync_division_index(SyncDivision::Whole),
        ParamKind::Enum {
            labels: sync_div_labels,
        },
        ParamScaling::Linear,
        PARAM_DRIFT_DIV,
    );

    for (name, idx) in [
        ("mod1_depth", PARAM_MOD1_DEPTH),
        ("mod2_depth", PARAM_MOD2_DEPTH),
        ("mod3_depth", PARAM_MOD3_DEPTH),
        ("mod4_depth", PARAM_MOD4_DEPTH),
        ("mod5_depth", PARAM_MOD5_DEPTH),
        ("mod6_depth", PARAM_MOD6_DEPTH),
    ] {
        push_param(
            &mut out,
            name,
            0.0,
            1.0,
            1.0,
            ParamKind::Continuous { unit: None },
            ParamScaling::Linear,
            idx,
        );
    }

    out
}

pub fn ui_param_descriptors() -> Vec<ParamDescriptor> {
    let mut out = Vec::new();
    let sync_div_labels = sync_labels();
    let shape_labels = shape_labels();

    for &(
        rate_idx,
        sync_idx,
        div_idx,
        shape_idx,
        pw_idx,
        retrig_idx,
        rate_default,
        div_default,
        retrig_default,
    ) in &[(
        PARAM_LFO1_RATE_HZ,
        PARAM_LFO1_SYNC,
        PARAM_LFO1_DIV,
        PARAM_LFO1_SHAPE,
        PARAM_LFO1_PW,
        PARAM_LFO1_RETRIGGER,
        5.0,
        sync_division_index(SyncDivision::Quarter),
        0.0,
    )] {
        push_param(
            &mut out,
            "rate",
            0.01,
            20.0,
            rate_default,
            ParamKind::Continuous {
                unit: Some("Hz".to_string()),
            },
            ParamScaling::Exponential,
            rate_idx,
        );
        push_param(
            &mut out,
            "sync",
            0.0,
            1.0,
            0.0,
            ParamKind::Boolean,
            ParamScaling::Linear,
            sync_idx,
        );
        push_param(
            &mut out,
            "division",
            0.0,
            (sync_div_labels.len() - 1) as f32,
            div_default,
            ParamKind::Enum {
                labels: sync_div_labels.clone(),
            },
            ParamScaling::Linear,
            div_idx,
        );
        push_param(
            &mut out,
            "shape",
            0.0,
            (shape_labels.len() - 1) as f32,
            0.0,
            ParamKind::Enum {
                labels: shape_labels.clone(),
            },
            ParamScaling::Linear,
            shape_idx,
        );
        push_param(
            &mut out,
            "pulse width",
            0.05,
            0.95,
            0.5,
            ParamKind::Continuous { unit: None },
            ParamScaling::Linear,
            pw_idx,
        );
        push_param(
            &mut out,
            "retrigger",
            0.0,
            1.0,
            retrig_default,
            ParamKind::Boolean,
            ParamScaling::Linear,
            retrig_idx,
        );
    }

    push_param(
        &mut out,
        "attack",
        1.0,
        2000.0,
        6.0,
        ParamKind::Continuous {
            unit: Some("ms".to_string()),
        },
        ParamScaling::Exponential,
        PARAM_ENV_ATTACK_MS,
    );
    push_param(
        &mut out,
        "decay",
        5.0,
        4000.0,
        180.0,
        ParamKind::Continuous {
            unit: Some("ms".to_string()),
        },
        ParamScaling::Exponential,
        PARAM_ENV_DECAY_MS,
    );
    push_param(
        &mut out,
        "sustain",
        0.0,
        1.0,
        0.55,
        ParamKind::Continuous {
            unit: Some("%".to_string()),
        },
        ParamScaling::Linear,
        PARAM_ENV_SUSTAIN,
    );
    push_param(
        &mut out,
        "release",
        5.0,
        4000.0,
        240.0,
        ParamKind::Continuous {
            unit: Some("ms".to_string()),
        },
        ParamScaling::Exponential,
        PARAM_ENV_RELEASE_MS,
    );

    push_param(
        &mut out,
        "rate",
        0.05,
        20.0,
        3.0,
        ParamKind::Continuous {
            unit: Some("Hz".to_string()),
        },
        ParamScaling::Exponential,
        PARAM_RAND_RATE_HZ,
    );
    push_param(
        &mut out,
        "sync",
        0.0,
        1.0,
        0.0,
        ParamKind::Boolean,
        ParamScaling::Linear,
        PARAM_RAND_SYNC,
    );
    push_param(
        &mut out,
        "division",
        0.0,
        (sync_div_labels.len() - 1) as f32,
        sync_division_index(SyncDivision::Whole),
        ParamKind::Enum {
            labels: sync_div_labels.clone(),
        },
        ParamScaling::Linear,
        PARAM_RAND_DIV,
    );
    push_param(
        &mut out,
        "slew",
        0.0,
        0.999,
        0.0,
        ParamKind::Continuous { unit: None },
        ParamScaling::Linear,
        PARAM_RAND_SLEW,
    );

    push_param(
        &mut out,
        "rate",
        0.00001,
        0.01,
        0.00035,
        ParamKind::Continuous {
            unit: Some("Hz".to_string()),
        },
        ParamScaling::Exponential,
        PARAM_DRIFT_RATE,
    );
    push_param(
        &mut out,
        "sync",
        0.0,
        1.0,
        0.0,
        ParamKind::Boolean,
        ParamScaling::Linear,
        PARAM_DRIFT_SYNC,
    );
    push_param(
        &mut out,
        "division",
        0.0,
        (sync_div_labels.len() - 1) as f32,
        sync_division_index(SyncDivision::Whole),
        ParamKind::Enum {
            labels: sync_div_labels.clone(),
        },
        ParamScaling::Linear,
        PARAM_DRIFT_DIV,
    );

    for &(
        rate_idx,
        sync_idx,
        div_idx,
        shape_idx,
        pw_idx,
        retrig_idx,
        rate_default,
        div_default,
        retrig_default,
    ) in &[
        (
            PARAM_LFO2_RATE_HZ,
            PARAM_LFO2_SYNC,
            PARAM_LFO2_DIV,
            PARAM_LFO2_SHAPE,
            PARAM_LFO2_PW,
            PARAM_LFO2_RETRIGGER,
            1.7,
            sync_division_index(SyncDivision::Half),
            0.0,
        ),
        (
            PARAM_LFO3_RATE_HZ,
            PARAM_LFO3_SYNC,
            PARAM_LFO3_DIV,
            PARAM_LFO3_SHAPE,
            PARAM_LFO3_PW,
            PARAM_LFO3_RETRIGGER,
            0.37,
            sync_division_index(SyncDivision::Whole),
            0.0,
        ),
    ] {
        push_param(
            &mut out,
            "rate",
            0.01,
            20.0,
            rate_default,
            ParamKind::Continuous {
                unit: Some("Hz".to_string()),
            },
            ParamScaling::Exponential,
            rate_idx,
        );
        push_param(
            &mut out,
            "sync",
            0.0,
            1.0,
            0.0,
            ParamKind::Boolean,
            ParamScaling::Linear,
            sync_idx,
        );
        push_param(
            &mut out,
            "division",
            0.0,
            (sync_div_labels.len() - 1) as f32,
            div_default,
            ParamKind::Enum {
                labels: sync_div_labels.clone(),
            },
            ParamScaling::Linear,
            div_idx,
        );
        push_param(
            &mut out,
            "shape",
            0.0,
            (shape_labels.len() - 1) as f32,
            0.0,
            ParamKind::Enum {
                labels: shape_labels.clone(),
            },
            ParamScaling::Linear,
            shape_idx,
        );
        push_param(
            &mut out,
            "pulse width",
            0.05,
            0.95,
            0.5,
            ParamKind::Continuous { unit: None },
            ParamScaling::Linear,
            pw_idx,
        );
        push_param(
            &mut out,
            "retrigger",
            0.0,
            1.0,
            retrig_default,
            ParamKind::Boolean,
            ParamScaling::Linear,
            retrig_idx,
        );
    }

    out
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
    *s.add(IDX_RNG) = 0x1234_5678u32 as f32;
    *s.add(IDX_SAMPLE_RATE) = sample_rate as f32;
    init_param_defaults(s);
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

    let out_mod1 = *out.add(0);
    let out_mod2 = *out.add(1);
    let out_mod3 = *out.add(2);
    let out_mod4 = *out.add(3);
    let out_mod5 = *out.add(4);
    let out_mod6 = *out.add(5);

    let mut lfo1_phase = *s.add(IDX_LFO1_PHASE);
    let mut lfo2_phase = *s.add(IDX_LFO2_PHASE);
    let mut lfo3_phase = *s.add(IDX_LFO3_PHASE);
    let mut env2 = *s.add(IDX_ENV2);
    let mut rand_phase = *s.add(IDX_RAND_PHASE);
    let mut drift = *s.add(IDX_DRIFT);
    let mut prev_gate = *s.add(IDX_PREV_GATE);
    let mut rng_state = *s.add(IDX_RNG) as u32;
    let mut rand_hold = *s.add(IDX_RAND_HOLD);
    let mut rand_smooth = *s.add(IDX_RAND_SMOOTH);
    let mut last_reset_counter = *s.add(IDX_LAST_RESET_COUNTER);
    let mut env_stage = *s.add(IDX_ENV_STAGE);

    let sample_rate = (*s.add(IDX_SAMPLE_RATE)).max(1.0);
    let bpm = (*s.add(PARAM_BPM)).clamp(20.0, 400.0);
    let reset_counter = *s.add(PARAM_RESET_COUNTER);

    if reset_counter != last_reset_counter {
        lfo1_phase = 0.0;
        lfo2_phase = 0.0;
        lfo3_phase = 0.0;
        rand_phase = 0.0;
        rand_hold = 0.0;
        rand_smooth = 0.0;
        drift = 0.0;
        env2 = 0.0;
        env_stage = ENV_STAGE_IDLE;
        last_reset_counter = reset_counter;
    }

    let lfo1_rate = if *s.add(PARAM_LFO1_SYNC) > 0.5 {
        synced_rate_hz((*s.add(PARAM_LFO1_DIV)).round() as usize, bpm)
    } else {
        (*s.add(PARAM_LFO1_RATE_HZ)).clamp(0.01, 20.0)
    };
    let lfo2_rate = if *s.add(PARAM_LFO2_SYNC) > 0.5 {
        synced_rate_hz((*s.add(PARAM_LFO2_DIV)).round() as usize, bpm)
    } else {
        (*s.add(PARAM_LFO2_RATE_HZ)).clamp(0.01, 20.0)
    };
    let lfo3_rate = if *s.add(PARAM_LFO3_SYNC) > 0.5 {
        synced_rate_hz((*s.add(PARAM_LFO3_DIV)).round() as usize, bpm)
    } else {
        (*s.add(PARAM_LFO3_RATE_HZ)).clamp(0.01, 20.0)
    };
    let rand_rate = if *s.add(PARAM_RAND_SYNC) > 0.5 {
        synced_rate_hz((*s.add(PARAM_RAND_DIV)).round() as usize, bpm)
    } else {
        (*s.add(PARAM_RAND_RATE_HZ)).clamp(0.01, 20.0)
    };
    let drift_rate = if *s.add(PARAM_DRIFT_SYNC) > 0.5 {
        synced_rate_hz((*s.add(PARAM_DRIFT_DIV)).round() as usize, bpm) * 0.2
    } else {
        (*s.add(PARAM_DRIFT_RATE)).clamp(0.00001, 0.01) * sample_rate
    };

    let lfo1_inc = lfo1_rate / sample_rate;
    let lfo2_inc = lfo2_rate / sample_rate;
    let lfo3_inc = lfo3_rate / sample_rate;
    let rand_inc = rand_rate / sample_rate;

    let env_attack = 1.0 / ((*s.add(PARAM_ENV_ATTACK_MS)).clamp(1.0, 2000.0) * 0.001 * sample_rate);
    let env_decay = 1.0 / ((*s.add(PARAM_ENV_DECAY_MS)).clamp(5.0, 4000.0) * 0.001 * sample_rate);
    let env_sustain = (*s.add(PARAM_ENV_SUSTAIN)).clamp(0.0, 1.0);
    let env_release =
        1.0 / ((*s.add(PARAM_ENV_RELEASE_MS)).clamp(5.0, 4000.0) * 0.001 * sample_rate);
    let rand_slew = (*s.add(PARAM_RAND_SLEW)).clamp(0.0, 0.999);

    let lfo1_shape = (*s.add(PARAM_LFO1_SHAPE)).round() as usize;
    let lfo2_shape = (*s.add(PARAM_LFO2_SHAPE)).round() as usize;
    let lfo3_shape = (*s.add(PARAM_LFO3_SHAPE)).round() as usize;
    let lfo1_pw = (*s.add(PARAM_LFO1_PW)).clamp(0.05, 0.95);
    let lfo2_pw = (*s.add(PARAM_LFO2_PW)).clamp(0.05, 0.95);
    let lfo3_pw = (*s.add(PARAM_LFO3_PW)).clamp(0.05, 0.95);
    let lfo1_retrigger = *s.add(PARAM_LFO1_RETRIGGER) > 0.5;
    let lfo2_retrigger = *s.add(PARAM_LFO2_RETRIGGER) > 0.5;
    let lfo3_retrigger = *s.add(PARAM_LFO3_RETRIGGER) > 0.5;

    let mod1_depth = (*s.add(PARAM_MOD1_DEPTH)).clamp(0.0, 1.0);
    let mod2_depth = (*s.add(PARAM_MOD2_DEPTH)).clamp(0.0, 1.0);
    let mod3_depth = (*s.add(PARAM_MOD3_DEPTH)).clamp(0.0, 1.0);
    let mod4_depth = (*s.add(PARAM_MOD4_DEPTH)).clamp(0.0, 1.0);
    let mod5_depth = (*s.add(PARAM_MOD5_DEPTH)).clamp(0.0, 1.0);
    let mod6_depth = (*s.add(PARAM_MOD6_DEPTH)).clamp(0.0, 1.0);

    for i in 0..nf {
        let gate = (*gate_in.add(i)).clamp(0.0, 1.0);
        let velocity = (*velocity_in.add(i)).clamp(0.0, 1.0);
        let trigger = (*trigger_in.add(i)).max(0.0);

        let gate_rising = gate > 0.5 && prev_gate <= 0.5;
        let note_on = gate_rising || trigger > 0.5;
        if note_on {
            env2 = 0.0;
            env_stage = ENV_STAGE_ATTACK;
            rand_hold = next_rand(&mut rng_state);
            if lfo1_retrigger {
                lfo1_phase = 0.0;
            }
            if lfo2_retrigger {
                lfo2_phase = 0.0;
            }
            if lfo3_retrigger {
                lfo3_phase = 0.0;
            }
        }

        lfo1_phase = (lfo1_phase + lfo1_inc).fract();
        lfo2_phase = (lfo2_phase + lfo2_inc).fract();
        lfo3_phase = (lfo3_phase + lfo3_inc).fract();

        let prev_rand_phase = rand_phase;
        rand_phase = (rand_phase + rand_inc).fract();
        if rand_phase < prev_rand_phase {
            rand_hold = next_rand(&mut rng_state);
        }
        rand_smooth += (rand_hold - rand_smooth) * (1.0 - rand_slew);

        if gate <= 0.5 && env_stage != ENV_STAGE_IDLE && env_stage != ENV_STAGE_RELEASE {
            env_stage = ENV_STAGE_RELEASE;
        }

        if env_stage == ENV_STAGE_ATTACK {
            env2 = (env2 + env_attack).min(1.0);
            if env2 >= 0.999 {
                env2 = 1.0;
                env_stage = ENV_STAGE_DECAY;
            }
        } else if env_stage == ENV_STAGE_DECAY {
            env2 += (env_sustain - env2) * env_decay;
            if (env2 - env_sustain).abs() <= 0.001 {
                env2 = env_sustain;
                env_stage = if gate > 0.5 {
                    ENV_STAGE_SUSTAIN
                } else {
                    ENV_STAGE_RELEASE
                };
            }
        } else if env_stage == ENV_STAGE_SUSTAIN {
            env2 = env_sustain;
            if gate <= 0.5 {
                env_stage = ENV_STAGE_RELEASE;
            }
        } else if env_stage == ENV_STAGE_RELEASE {
            env2 += (0.0 - env2) * env_release;
            if env2 <= 0.0005 {
                env2 = 0.0;
                env_stage = ENV_STAGE_IDLE;
            }
        }
        env2 = env2.clamp(0.0, 1.0);

        drift +=
            (next_rand(&mut rng_state) * 0.08 - drift) * (drift_rate / sample_rate).clamp(0.0, 1.0);
        drift = drift.clamp(-1.0, 1.0);

        let drift_source = (drift * (0.4 + velocity * 0.6)).clamp(-1.0, 1.0);

        *out_mod1.add(i) = (bipolar_to_unipolar(shape_value(lfo1_shape, lfo1_phase, lfo1_pw))
            * mod1_depth)
            .clamp(0.0, 1.0);
        *out_mod2.add(i) = (env2 * mod2_depth).clamp(0.0, 1.0);
        *out_mod3.add(i) = (bipolar_to_unipolar(rand_smooth) * mod3_depth).clamp(0.0, 1.0);
        *out_mod4.add(i) = (bipolar_to_unipolar(drift_source) * mod4_depth).clamp(0.0, 1.0);
        *out_mod5.add(i) = (bipolar_to_unipolar(shape_value(lfo2_shape, lfo2_phase, lfo2_pw))
            * mod5_depth)
            .clamp(0.0, 1.0);
        *out_mod6.add(i) = (bipolar_to_unipolar(shape_value(lfo3_shape, lfo3_phase, lfo3_pw))
            * mod6_depth)
            .clamp(0.0, 1.0);

        prev_gate = gate;
    }

    *s.add(IDX_LFO1_PHASE) = lfo1_phase;
    *s.add(IDX_LFO2_PHASE) = lfo2_phase;
    *s.add(IDX_LFO3_PHASE) = lfo3_phase;
    *s.add(IDX_ENV2) = env2;
    *s.add(IDX_RAND_PHASE) = rand_phase;
    *s.add(IDX_DRIFT) = drift;
    *s.add(IDX_PREV_GATE) = prev_gate;
    *s.add(IDX_RNG) = rng_state as f32;
    *s.add(IDX_RAND_HOLD) = rand_hold;
    *s.add(IDX_RAND_SMOOTH) = rand_smooth;
    *s.add(IDX_LAST_RESET_COUNTER) = last_reset_counter;
    *s.add(IDX_ENV_STAGE) = env_stage;
}

pub fn voice_modulator_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(voice_modulator_process),
        init: Some(voice_modulator_init),
        reset: None,
        migrate: None,
    }
}

pub fn is_bar_resync_param(node_param_idx: u32) -> bool {
    let timed_params = [
        MOD_PARAM_BASE + PARAM_LFO1_SYNC as u32,
        MOD_PARAM_BASE + PARAM_LFO1_DIV as u32,
        MOD_PARAM_BASE + PARAM_LFO1_RETRIGGER as u32,
        MOD_PARAM_BASE + PARAM_LFO2_SYNC as u32,
        MOD_PARAM_BASE + PARAM_LFO2_DIV as u32,
        MOD_PARAM_BASE + PARAM_LFO2_RETRIGGER as u32,
        MOD_PARAM_BASE + PARAM_LFO3_SYNC as u32,
        MOD_PARAM_BASE + PARAM_LFO3_DIV as u32,
        MOD_PARAM_BASE + PARAM_LFO3_RETRIGGER as u32,
        MOD_PARAM_BASE + PARAM_RAND_SYNC as u32,
        MOD_PARAM_BASE + PARAM_RAND_DIV as u32,
        MOD_PARAM_BASE + PARAM_DRIFT_SYNC as u32,
        MOD_PARAM_BASE + PARAM_DRIFT_DIV as u32,
    ];
    timed_params.contains(&node_param_idx)
}

pub fn modulator_slot_label(slot: usize, fallback_name: &str) -> String {
    match slot {
        1 => "LFO 1",
        2 => "ENV 1",
        3 => "RAND",
        4 => "DRIFT",
        5 => "LFO 2",
        6 => "LFO 3",
        7 => "Ext 1",
        8 => "Ext 2",
        9 => "Ext 3",
        10 => "Ext 4",
        _ => fallback_name,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_voice_modulator(
        state: &mut [f32; STATE_SIZE],
        frames: usize,
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
        ];

        let mut outputs = [[0.0f32; 64]; NUM_OUTPUTS];
        let output_ptrs = [
            outputs[0].as_mut_ptr(),
            outputs[1].as_mut_ptr(),
            outputs[2].as_mut_ptr(),
            outputs[3].as_mut_ptr(),
            outputs[4].as_mut_ptr(),
            outputs[5].as_mut_ptr(),
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
    fn sync_labels_match_builtin_effect_division_indices() {
        let labels = sync_labels();

        assert_eq!(labels[SyncDivision::Quarter as usize], "1/4");
        assert_eq!(
            sync_division_index(SyncDivision::Quarter),
            SyncDivision::Quarter as usize as f32
        );
    }

    #[test]
    fn lfo1_does_not_retrigger_by_default() {
        let desc = param_descriptors();
        let retrigger = desc
            .iter()
            .find(|param| param.node_param_idx == MOD_PARAM_BASE + PARAM_LFO1_RETRIGGER as u32)
            .expect("LFO 1 retrigger descriptor");

        assert_eq!(retrigger.default, 0.0);
    }

    #[test]
    fn transport_params_do_not_overlap_runtime_state() {
        let runtime_indices = [
            IDX_LFO1_PHASE,
            IDX_LFO2_PHASE,
            IDX_LFO3_PHASE,
            IDX_ENV2,
            IDX_RAND_PHASE,
            IDX_DRIFT,
            IDX_PREV_GATE,
            IDX_RNG,
            IDX_RAND_HOLD,
            IDX_RAND_SMOOTH,
            IDX_LAST_RESET_COUNTER,
            IDX_ENV_STAGE,
            IDX_SAMPLE_RATE,
        ];

        assert!(!runtime_indices.contains(&PARAM_BPM));
        assert!(!runtime_indices.contains(&PARAM_RESET_COUNTER));
        assert!(PARAM_BPM < STATE_SIZE);
        assert!(PARAM_RESET_COUNTER < STATE_SIZE);
    }

    #[test]
    fn built_in_modulator_outputs_are_unipolar() {
        let mut state = [0.0f32; STATE_SIZE];
        unsafe {
            voice_modulator_init(state.as_mut_ptr().cast(), 48_000, 64, std::ptr::null());
        }

        let outputs = render_voice_modulator(&mut state, 64);

        for (output_idx, output) in outputs.iter().enumerate() {
            for (frame_idx, value) in output.iter().take(64).enumerate() {
                assert!(
                    (0.0..=1.0).contains(value),
                    "mod output {} frame {} should be unipolar, got {}",
                    output_idx + 1,
                    frame_idx,
                    value
                );
            }
        }
    }

    #[test]
    fn modulator_depth_zero_outputs_zero_not_center_bias() {
        let mut state = [0.0f32; STATE_SIZE];
        unsafe {
            voice_modulator_init(state.as_mut_ptr().cast(), 48_000, 64, std::ptr::null());
        }
        state[PARAM_MOD1_DEPTH] = 0.0;
        state[PARAM_MOD2_DEPTH] = 0.0;
        state[PARAM_MOD3_DEPTH] = 0.0;
        state[PARAM_MOD4_DEPTH] = 0.0;
        state[PARAM_MOD5_DEPTH] = 0.0;
        state[PARAM_MOD6_DEPTH] = 0.0;

        let outputs = render_voice_modulator(&mut state, 64);

        for output in &outputs {
            assert_eq!(&output[..64], &[0.0; 64]);
        }
    }
}
