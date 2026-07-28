//! Filterbank — Sherman Filterbank 2 style dual-filter mangler.
//!
//! Spec: docs/sherman-filterbank-spec.md. The heart is the switched-capacitor
//! clock model (§4): each SVF core runs at `f_clk = ratio × f_c` with
//! zero-order hold between updates and a *constant* `g = tan(π/ratio)` —
//! cutoff modulation moves the clock, not the coefficient, which is what
//! leaks the stepping/aliasing/clock-bleed character into low cutoffs.
//!
//! Input ports: 0/1 = L/R audio, 2..5 = host ext-mod sources (audio-rate),
//! 6 = FM sidechain, 7 = AM sidechain. The FM/AM sidechains only replace the
//! normalled sources (post-drive input / filter-2 output) when the host sets
//! `STATE_FM_EXT_ACTIVE` / `STATE_AM_EXT_ACTIVE` — never based on buffer
//! content.

use crate::audiograph::NodeVTable;
use crate::effects::roar::{shaper_transfer, SHAPER_DIODE, SHAPER_TUBE};
use std::os::raw::{c_int, c_void};

// ── Knob parameter slots ──
const STATE_ENABLED: usize = 0;
const STATE_INPUT_DB: usize = 1; // -12..+30 dB drive into the filters
const STATE_HI_EQ: usize = 2; // 0=Cut, 1=Flat, 2=Boost (±6 dB shelf @3 kHz)
const STATE_SENSE: usize = 3; // 0..100 trigger sensitivity
const STATE_NOISE: usize = 4; // 0..100
const STATE_FEEDBACK: usize = 5; // 0..100
const STATE_CRUNCH: usize = 6; // 0..100, morphs clock ratio 100:1 → 25:1
const STATE_CORRECTION: usize = 7; // 0..100, subtract inverted BP
const STATE_SER_PAR: usize = 8; // 0=serial .. 100=parallel
const STATE_HARMONICS: usize = 9; // option index into HARMONIC_RATIOS
const STATE_FM_AMOUNT: usize = 10; // 0..100
const STATE_AM_DEPTH: usize = 11; // 0..100 (0-50 AM, 50-100 → ring)
const STATE_ENV_MODE: usize = 12; // 0=ADSR, 1=Follower
const STATE_ATTACK_MS: usize = 13; // 0.5..4000
const STATE_DECAY_MS: usize = 14; // 1..4000
const STATE_SUSTAIN: usize = 15; // -100..+100 (bipolar!)
const STATE_RELEASE_MS: usize = 16; // 1..8000
const STATE_ENV_F1: usize = 17; // -100..+100 attenuverter, ±4 oct
const STATE_ENV_F2: usize = 18; // -100..+100
const STATE_RES_BLEED: usize = 19; // 0..100 env → both resonances
const STATE_LFO_RATE: usize = 20; // 0.01..2000 Hz
const STATE_LFO_WAVE: usize = 21; // 0=Sine 1=Saw 2=Ramp 3=Square
const STATE_LFO_DEPTH: usize = 22; // -100..+100 (negative inverts on F2 only)
const STATE_LFO_TRIG: usize = 23; // Sense trigger resets LFO phase
const STATE_AR_ATTACK_MS: usize = 24; // 0.5..2000
const STATE_AR_RELEASE_MS: usize = 25; // 1..4000
const STATE_AR_DEPTH: usize = 26; // 0..100 output-amplitude envelope
const STATE_STEREO_SPLIT: usize = 27; // wet L = F1 only, wet R = F2 only
const STATE_OUTPUT_DB: usize = 28; // ±24 dB wet trim
const STATE_DRY_WET: usize = 29; // 0..100 equal-power
const STATE_F1_FREQ: usize = 30; // 20..16000 Hz
const STATE_F1_RES: usize = 31; // 0..110 %
const STATE_F1_MODE: usize = 32; // 0..100 LP→BP→HP morph
const STATE_F2_FREQ: usize = 33;
const STATE_F2_RES: usize = 34;
const STATE_F2_MODE: usize = 35;
const STATE_SAMPLE_RATE: usize = 36;
// Host plumbing sets these to 1 when an FM/AM sidechain track is selected;
// 0 (default) engages the normalled internal sources.
const STATE_FM_EXT_ACTIVE: usize = 37;
const STATE_AM_EXT_ACTIVE: usize = 38;

// ── Host-modulation depth slots (§5a): 10 targets × 4 contiguous slots ──
const STATE_MOD_F1_FREQ_DEPTH_1: usize = 39;
const STATE_MOD_F2_FREQ_DEPTH_1: usize = 43;
const STATE_MOD_F1_RES_DEPTH_1: usize = 47;
const STATE_MOD_F2_RES_DEPTH_1: usize = 51;
const STATE_MOD_F1_MODE_DEPTH_1: usize = 55;
const STATE_MOD_F2_MODE_DEPTH_1: usize = 59;
const STATE_MOD_FM_DEPTH_1: usize = 63;
const STATE_MOD_AM_DEPTH_1: usize = 67;
const STATE_MOD_SER_PAR_DEPTH_1: usize = 71;
const STATE_MOD_CRUNCH_DEPTH_1: usize = 75;

// ── Runtime state ──
// Knob smoothers (~20 Hz one-pole, filter.rs pattern).
const STATE_SM_INPUT_GAIN: usize = 79; // linear amp
const STATE_SM_NOISE: usize = 80; // 0..1
const STATE_SM_FEEDBACK: usize = 81;
const STATE_SM_CRUNCH: usize = 82;
const STATE_SM_CORRECTION: usize = 83;
const STATE_SM_SER_PAR: usize = 84;
const STATE_SM_FM: usize = 85;
const STATE_SM_AM: usize = 86;
const STATE_SM_ENV_F1: usize = 87; // -1..1
const STATE_SM_ENV_F2: usize = 88;
const STATE_SM_RES_BLEED: usize = 89; // 0..1
const STATE_SM_LFO_DEPTH: usize = 90; // -1..1
const STATE_SM_AR_DEPTH: usize = 91; // 0..1
const STATE_SM_OUTPUT_GAIN: usize = 92; // linear amp
const STATE_SM_DRY_WET: usize = 93; // 0..1
const STATE_SM_F1_FREQ: usize = 94; // Hz
const STATE_SM_F1_RES: usize = 95; // 0..110
const STATE_SM_F1_MODE: usize = 96; // 0..1
const STATE_SM_F2_FREQ: usize = 97;
const STATE_SM_F2_RES: usize = 98;
const STATE_SM_F2_MODE: usize = 99;
const STATE_SM_SHELF_GAIN: usize = 100; // hi-eq shelf gain delta (amp - 1)

// Clocked-SVF blocks: [ic1, ic2, clk_phase, hold_lp, hold_bp, hold_hp].
const SVF_BLOCK_LEN: usize = 6;
const STATE_F1L: usize = 101;
const STATE_F1R: usize = STATE_F1L + SVF_BLOCK_LEN; // 107
const STATE_F2L: usize = STATE_F1R + SVF_BLOCK_LEN; // 113
const STATE_F2R: usize = STATE_F2L + SVF_BLOCK_LEN; // 119

const STATE_SENSE_ENV: usize = 125; // post-drive detector follower
const STATE_SENSE_PREV: usize = 126; // previous detector value (edge detect)
const STATE_GATE: usize = 127;
const STATE_HOLDOFF: usize = 128; // retrigger holdoff, samples remaining
const STATE_ENV_STAGE: usize = 129; // 0 idle, 1 attack, 2 decay, 3 release
const STATE_ENV_VALUE: usize = 130; // bipolar ADSR / follower value
const STATE_LFO_PHASE: usize = 131;
const STATE_AR_ENV: usize = 132;
const STATE_TRIG_COUNT: usize = 133; // gate-on event counter (tests/debug)
const STATE_NOISE_SEED: usize = 134;
const STATE_FB_LP_L: usize = 135; // 30 Hz HP (as one-pole LP) on feedback
const STATE_FB_LP_R: usize = 136;
const STATE_FB_PREV_L: usize = 137; // previous wet sample fed back
const STATE_FB_PREV_R: usize = 138;
const STATE_SHELF_LP_L: usize = 139; // hi-eq shelf one-pole state
const STATE_SHELF_LP_R: usize = 140;
// 2× oversampling biquads around the drive nonlinearity (space-echo shape).
const STATE_OS_UP_Z1_L: usize = 141;
const STATE_OS_UP_Z2_L: usize = 142;
const STATE_OS_DOWN_Z1_L: usize = 143;
const STATE_OS_DOWN_Z2_L: usize = 144;
const STATE_OS_UP_Z1_R: usize = 145;
const STATE_OS_UP_Z2_R: usize = 146;
const STATE_OS_DOWN_Z1_R: usize = 147;
const STATE_OS_DOWN_Z2_R: usize = 148;
const STATE_DC_X1_L: usize = 149; // post-drive DC blockers
const STATE_DC_Y1_L: usize = 150;
const STATE_DC_X1_R: usize = 151;
const STATE_DC_Y1_R: usize = 152;
// ~2 ms one-pole lag on the *linear* mod targets only (§5a); the freq
// targets go straight in — the clocked SVF eats steps by design.
const STATE_LAG_F1_RES: usize = 153;
const STATE_LAG_F2_RES: usize = 154;
const STATE_LAG_F1_MODE: usize = 155;
const STATE_LAG_F2_MODE: usize = 156;
const STATE_LAG_FM: usize = 157;
const STATE_LAG_AM: usize = 158;
const STATE_LAG_SER_PAR: usize = 159;
const STATE_LAG_CRUNCH: usize = 160;

// ── Live-meter tail (§9): read by the analyzer / display widget ──
const STATE_METER_INPUT_DB: usize = 161; // post-drive level, 5/250 ms
const STATE_METER_ENV: usize = 162; // bipolar env value
const STATE_METER_GATE: usize = 163; // 0/1
const STATE_METER_F1_HZ: usize = 164; // effective cutoff post all modulation
const STATE_METER_F2_HZ: usize = 165;

// Split-mode edge detector: split mode only ticks F1L/F2R, so F1R/F2L would
// otherwise resume with stale integrator state when the toggle flips back.
const STATE_SPLIT_PREV: usize = 166;

// ── Params appended after the runtime-reset span (bypass must NOT clear
// these — they're host-written settings, not runtime state) ──
const STATE_LFO_SYNC: usize = 167; // 0 free, 1 tempo-synced
const STATE_LFO_DIV: usize = 168; // index into SYNC_BEATS
const STATE_BPM: usize = 169; // host-pushed transport BPM

pub const FILTERBANK_STATE_SIZE: usize = 170;
// Bypass resets [FIRST_RUNTIME_RESET, RUNTIME_RESET_END): SVF blocks through
// SPLIT_PREV. The appended param slots above survive.
const RUNTIME_RESET_END: usize = STATE_LFO_SYNC;
// Everything from the first SVF block on is runtime state reset on bypass.
const FIRST_RUNTIME_RESET: usize = STATE_F1L;

// ── Param indices for external control ──
pub const FILTERBANK_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const FILTERBANK_PARAM_INPUT_DB: u64 = STATE_INPUT_DB as u64;
pub const FILTERBANK_PARAM_HI_EQ: u64 = STATE_HI_EQ as u64;
pub const FILTERBANK_PARAM_SENSE: u64 = STATE_SENSE as u64;
pub const FILTERBANK_PARAM_NOISE: u64 = STATE_NOISE as u64;
pub const FILTERBANK_PARAM_FEEDBACK: u64 = STATE_FEEDBACK as u64;
pub const FILTERBANK_PARAM_CRUNCH: u64 = STATE_CRUNCH as u64;
pub const FILTERBANK_PARAM_CORRECTION: u64 = STATE_CORRECTION as u64;
pub const FILTERBANK_PARAM_SER_PAR: u64 = STATE_SER_PAR as u64;
pub const FILTERBANK_PARAM_HARMONICS: u64 = STATE_HARMONICS as u64;
pub const FILTERBANK_PARAM_FM_AMOUNT: u64 = STATE_FM_AMOUNT as u64;
pub const FILTERBANK_PARAM_AM_DEPTH: u64 = STATE_AM_DEPTH as u64;
pub const FILTERBANK_PARAM_ENV_MODE: u64 = STATE_ENV_MODE as u64;
pub const FILTERBANK_PARAM_ATTACK_MS: u64 = STATE_ATTACK_MS as u64;
pub const FILTERBANK_PARAM_DECAY_MS: u64 = STATE_DECAY_MS as u64;
pub const FILTERBANK_PARAM_SUSTAIN: u64 = STATE_SUSTAIN as u64;
pub const FILTERBANK_PARAM_RELEASE_MS: u64 = STATE_RELEASE_MS as u64;
pub const FILTERBANK_PARAM_ENV_F1: u64 = STATE_ENV_F1 as u64;
pub const FILTERBANK_PARAM_ENV_F2: u64 = STATE_ENV_F2 as u64;
pub const FILTERBANK_PARAM_RES_BLEED: u64 = STATE_RES_BLEED as u64;
pub const FILTERBANK_PARAM_LFO_RATE: u64 = STATE_LFO_RATE as u64;
pub const FILTERBANK_PARAM_LFO_SYNC: u64 = STATE_LFO_SYNC as u64;
pub const FILTERBANK_PARAM_LFO_DIV: u64 = STATE_LFO_DIV as u64;
pub const FILTERBANK_PARAM_BPM: u64 = STATE_BPM as u64;
pub const FILTERBANK_PARAM_LFO_WAVE: u64 = STATE_LFO_WAVE as u64;
pub const FILTERBANK_PARAM_LFO_DEPTH: u64 = STATE_LFO_DEPTH as u64;
pub const FILTERBANK_PARAM_LFO_TRIG: u64 = STATE_LFO_TRIG as u64;
pub const FILTERBANK_PARAM_AR_ATTACK_MS: u64 = STATE_AR_ATTACK_MS as u64;
pub const FILTERBANK_PARAM_AR_RELEASE_MS: u64 = STATE_AR_RELEASE_MS as u64;
pub const FILTERBANK_PARAM_AR_DEPTH: u64 = STATE_AR_DEPTH as u64;
pub const FILTERBANK_PARAM_STEREO_SPLIT: u64 = STATE_STEREO_SPLIT as u64;
pub const FILTERBANK_PARAM_OUTPUT_DB: u64 = STATE_OUTPUT_DB as u64;
pub const FILTERBANK_PARAM_DRY_WET: u64 = STATE_DRY_WET as u64;
pub const FILTERBANK_PARAM_F1_FREQ: u64 = STATE_F1_FREQ as u64;
pub const FILTERBANK_PARAM_F1_RES: u64 = STATE_F1_RES as u64;
pub const FILTERBANK_PARAM_F1_MODE: u64 = STATE_F1_MODE as u64;
pub const FILTERBANK_PARAM_F2_FREQ: u64 = STATE_F2_FREQ as u64;
pub const FILTERBANK_PARAM_F2_RES: u64 = STATE_F2_RES as u64;
pub const FILTERBANK_PARAM_F2_MODE: u64 = STATE_F2_MODE as u64;
pub const FILTERBANK_PARAM_FM_EXT_ACTIVE: u64 = STATE_FM_EXT_ACTIVE as u64;
pub const FILTERBANK_PARAM_AM_EXT_ACTIVE: u64 = STATE_AM_EXT_ACTIVE as u64;

pub const FILTERBANK_PARAM_MOD_F1_FREQ_DEPTH_1: u64 = STATE_MOD_F1_FREQ_DEPTH_1 as u64;
pub const FILTERBANK_PARAM_MOD_F1_FREQ_DEPTH_2: u64 = STATE_MOD_F1_FREQ_DEPTH_1 as u64 + 1;
pub const FILTERBANK_PARAM_MOD_F1_FREQ_DEPTH_3: u64 = STATE_MOD_F1_FREQ_DEPTH_1 as u64 + 2;
pub const FILTERBANK_PARAM_MOD_F1_FREQ_DEPTH_4: u64 = STATE_MOD_F1_FREQ_DEPTH_1 as u64 + 3;
pub const FILTERBANK_PARAM_MOD_F2_FREQ_DEPTH_1: u64 = STATE_MOD_F2_FREQ_DEPTH_1 as u64;
pub const FILTERBANK_PARAM_MOD_F2_FREQ_DEPTH_2: u64 = STATE_MOD_F2_FREQ_DEPTH_1 as u64 + 1;
pub const FILTERBANK_PARAM_MOD_F2_FREQ_DEPTH_3: u64 = STATE_MOD_F2_FREQ_DEPTH_1 as u64 + 2;
pub const FILTERBANK_PARAM_MOD_F2_FREQ_DEPTH_4: u64 = STATE_MOD_F2_FREQ_DEPTH_1 as u64 + 3;
pub const FILTERBANK_PARAM_MOD_F1_RES_DEPTH_1: u64 = STATE_MOD_F1_RES_DEPTH_1 as u64;
pub const FILTERBANK_PARAM_MOD_F1_RES_DEPTH_2: u64 = STATE_MOD_F1_RES_DEPTH_1 as u64 + 1;
pub const FILTERBANK_PARAM_MOD_F1_RES_DEPTH_3: u64 = STATE_MOD_F1_RES_DEPTH_1 as u64 + 2;
pub const FILTERBANK_PARAM_MOD_F1_RES_DEPTH_4: u64 = STATE_MOD_F1_RES_DEPTH_1 as u64 + 3;
pub const FILTERBANK_PARAM_MOD_F2_RES_DEPTH_1: u64 = STATE_MOD_F2_RES_DEPTH_1 as u64;
pub const FILTERBANK_PARAM_MOD_F2_RES_DEPTH_2: u64 = STATE_MOD_F2_RES_DEPTH_1 as u64 + 1;
pub const FILTERBANK_PARAM_MOD_F2_RES_DEPTH_3: u64 = STATE_MOD_F2_RES_DEPTH_1 as u64 + 2;
pub const FILTERBANK_PARAM_MOD_F2_RES_DEPTH_4: u64 = STATE_MOD_F2_RES_DEPTH_1 as u64 + 3;
pub const FILTERBANK_PARAM_MOD_F1_MODE_DEPTH_1: u64 = STATE_MOD_F1_MODE_DEPTH_1 as u64;
pub const FILTERBANK_PARAM_MOD_F1_MODE_DEPTH_2: u64 = STATE_MOD_F1_MODE_DEPTH_1 as u64 + 1;
pub const FILTERBANK_PARAM_MOD_F1_MODE_DEPTH_3: u64 = STATE_MOD_F1_MODE_DEPTH_1 as u64 + 2;
pub const FILTERBANK_PARAM_MOD_F1_MODE_DEPTH_4: u64 = STATE_MOD_F1_MODE_DEPTH_1 as u64 + 3;
pub const FILTERBANK_PARAM_MOD_F2_MODE_DEPTH_1: u64 = STATE_MOD_F2_MODE_DEPTH_1 as u64;
pub const FILTERBANK_PARAM_MOD_F2_MODE_DEPTH_2: u64 = STATE_MOD_F2_MODE_DEPTH_1 as u64 + 1;
pub const FILTERBANK_PARAM_MOD_F2_MODE_DEPTH_3: u64 = STATE_MOD_F2_MODE_DEPTH_1 as u64 + 2;
pub const FILTERBANK_PARAM_MOD_F2_MODE_DEPTH_4: u64 = STATE_MOD_F2_MODE_DEPTH_1 as u64 + 3;
pub const FILTERBANK_PARAM_MOD_FM_DEPTH_1: u64 = STATE_MOD_FM_DEPTH_1 as u64;
pub const FILTERBANK_PARAM_MOD_FM_DEPTH_2: u64 = STATE_MOD_FM_DEPTH_1 as u64 + 1;
pub const FILTERBANK_PARAM_MOD_FM_DEPTH_3: u64 = STATE_MOD_FM_DEPTH_1 as u64 + 2;
pub const FILTERBANK_PARAM_MOD_FM_DEPTH_4: u64 = STATE_MOD_FM_DEPTH_1 as u64 + 3;
pub const FILTERBANK_PARAM_MOD_AM_DEPTH_1: u64 = STATE_MOD_AM_DEPTH_1 as u64;
pub const FILTERBANK_PARAM_MOD_AM_DEPTH_2: u64 = STATE_MOD_AM_DEPTH_1 as u64 + 1;
pub const FILTERBANK_PARAM_MOD_AM_DEPTH_3: u64 = STATE_MOD_AM_DEPTH_1 as u64 + 2;
pub const FILTERBANK_PARAM_MOD_AM_DEPTH_4: u64 = STATE_MOD_AM_DEPTH_1 as u64 + 3;
pub const FILTERBANK_PARAM_MOD_SER_PAR_DEPTH_1: u64 = STATE_MOD_SER_PAR_DEPTH_1 as u64;
pub const FILTERBANK_PARAM_MOD_SER_PAR_DEPTH_2: u64 = STATE_MOD_SER_PAR_DEPTH_1 as u64 + 1;
pub const FILTERBANK_PARAM_MOD_SER_PAR_DEPTH_3: u64 = STATE_MOD_SER_PAR_DEPTH_1 as u64 + 2;
pub const FILTERBANK_PARAM_MOD_SER_PAR_DEPTH_4: u64 = STATE_MOD_SER_PAR_DEPTH_1 as u64 + 3;
pub const FILTERBANK_PARAM_MOD_CRUNCH_DEPTH_1: u64 = STATE_MOD_CRUNCH_DEPTH_1 as u64;
pub const FILTERBANK_PARAM_MOD_CRUNCH_DEPTH_2: u64 = STATE_MOD_CRUNCH_DEPTH_1 as u64 + 1;
pub const FILTERBANK_PARAM_MOD_CRUNCH_DEPTH_3: u64 = STATE_MOD_CRUNCH_DEPTH_1 as u64 + 2;
pub const FILTERBANK_PARAM_MOD_CRUNCH_DEPTH_4: u64 = STATE_MOD_CRUNCH_DEPTH_1 as u64 + 3;

// Meter tail (read-only for the host).
pub const FILTERBANK_METER_INPUT_DB: usize = STATE_METER_INPUT_DB;
pub const FILTERBANK_METER_ENV: usize = STATE_METER_ENV;
pub const FILTERBANK_METER_GATE: usize = STATE_METER_GATE;
pub const FILTERBANK_METER_F1_HZ: usize = STATE_METER_F1_HZ;
pub const FILTERBANK_METER_F2_HZ: usize = STATE_METER_F2_HZ;

/// FM / AM sidechain audio arrives on these input channels (mono).
pub const FILTERBANK_FM_INPUT_CHANNEL: usize = 6;
pub const FILTERBANK_AM_INPUT_CHANNEL: usize = 7;

/// Harmonics option: index 0 = Free, others slave F2 to F1freq ÷ ratio.
pub const HARMONIC_RATIOS: [f32; 12] =
    [0.0, 1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 9.0, 12.0, 16.0];

// Clocked-SVF block layout.
const SVF_IC1: usize = 0;
const SVF_IC2: usize = 1;
const SVF_CLK_PHASE: usize = 2;
const SVF_HOLD_LP: usize = 3;
const SVF_HOLD_BP: usize = 4;
const SVF_HOLD_HP: usize = 5;

/// The clock never stalls below this (§4).
const FCLK_FLOOR_HZ: f32 = 200.0;

/// Tempo-sync divisions — same table/order as Str8 Delay / Space Echo /
/// Phaser-Flanger so the UI labels ("1/32".."1", idx 6 = "1/4") line up.
const SYNC_BEATS: [f32; 11] = [
    0.125,
    0.25,
    1.0 / 6.0,
    0.5,
    1.0 / 3.0,
    0.75,
    1.0,
    2.0 / 3.0,
    1.5,
    2.0,
    4.0,
];

/// LFO rate: free Hz, or one cycle per synced division at the host BPM.
#[inline]
fn lfo_rate_hz(sync: f32, div: f32, bpm: f32, free_hz: f32) -> f32 {
    if sync > 0.5 {
        let idx = (div.clamp(0.0, (SYNC_BEATS.len() - 1) as f32).round() as usize)
            .min(SYNC_BEATS.len() - 1);
        let bpm = if bpm.is_finite() {
            bpm.clamp(20.0, 999.0)
        } else {
            120.0
        };
        bpm / (60.0 * SYNC_BEATS[idx])
    } else {
        free_hz
    }
}
/// bp-state soft-limit scale: `ic1 = LIMIT·tanh(ic1/LIMIT)`.
const BP_LIMIT: f32 = 4.0;
/// Damping floor. Spec §3 words this as "a small positive floor never
/// reached until res > 100%" — a positive k cannot self-oscillate, so the
/// intent is read as: k stays above the floor for res ≤ 100% and is allowed
/// to go (slightly) negative beyond, with the bp tanh bounding the scream.
/// -0.25 sits just below k(res=110%) = -0.2 so the law is never clipped.
const K_FLOOR: f32 = -0.25;

#[inline]
fn db_to_amp(db: f32) -> f32 {
    (10.0_f32).powf(db / 20.0)
}

#[inline]
fn amp_to_db(amp: f32) -> f32 {
    20.0 * amp.max(1.0e-9).log10()
}

#[inline]
fn time_coef(ms: f32, sr: f32) -> f32 {
    1.0 - (-1.0 / (ms.max(0.01) * 0.001 * sr.max(1.0))).exp()
}

#[inline]
fn one_pole_coef(freq: f32, sr: f32) -> f32 {
    1.0 - (-std::f32::consts::TAU * freq / sr.max(1.0)).exp()
}

/// Deterministic 24-bit LCG noise (copied from roar.rs — exactly
/// representable in f32 state).
#[inline]
fn next_noise(state: &mut f32) -> f32 {
    const MASK: u32 = (1 << 24) - 1;
    let seed = (*state as u32) & MASK;
    let next = seed.wrapping_mul(1_140_671_485).wrapping_add(12_820_163) & MASK;
    *state = next as f32;
    next as f32 * (2.0 / MASK as f32) - 1.0
}

/// Resonance law (§3): k = 2 − 2·(res/100); res > 100% goes negative
/// (active) so the filter self-oscillates, bounded by the bp soft limit.
#[inline]
fn res_to_k(res_pct: f32) -> f32 {
    (2.0 - res_pct.clamp(0.0, 110.0) * 0.02).max(K_FLOOR)
}

/// One Simper-SVF update on `st[SVF_IC1..=SVF_IC2]`, with the §3 tanh
/// soft-limit on the bp integrator inside the loop. Returns (lp, bp, hp).
#[inline]
fn svf_core(x: f32, g: f32, k: f32, st: &mut [f32; SVF_BLOCK_LEN]) -> (f32, f32, f32) {
    // Stability floor: with negative k (res > 100%) at large g (per-sample
    // branch, cutoff near the 0.45·sr clamp) the linearized update matrix
    // goes unstable (|λ| ≈ 1.03 at g = 6.31, k = −0.2). The ic1 tanh bounds
    // it into a limit cycle in practice, but that bound relies on the
    // saturating ic1 phase-cancelling the ic2 growth and its margin shrinks
    // as |state| grows — a large transient kick could still run away to NaN,
    // which would latch through the feedback path. Flooring k at −0.9/g
    // keeps the a3 denominator ≥ 0.1+g², guaranteeing contraction. Only
    // bites when g > 4.5 (cutoff > ~0.4·sr, ≈19 kHz at 48 k), so audible
    // self-oscillation is unaffected.
    let k = k.max(-0.9 / g.max(1.0e-4));
    let ic1 = st[SVF_IC1];
    let ic2 = st[SVF_IC2];
    let a1 = 1.0 / (1.0 + g * (g + k));
    let a2 = g * a1;
    let a3 = g * a2;
    let v3 = x - ic2;
    let v1 = a1 * ic1 + a2 * v3;
    let v2 = ic2 + a2 * ic1 + a3 * v3;
    let n1 = 2.0 * v1 - ic1;
    st[SVF_IC1] = BP_LIMIT * (n1 / BP_LIMIT).tanh();
    st[SVF_IC2] = 2.0 * v2 - ic2;
    (v2, v1, x - k * v1 - v2)
}

/// §4 switched-capacitor clocked SVF tick, one host sample.
///
/// The core runs at `f_clk = ratio × fc` with ZOH between updates and a
/// constant `g = tan(π/ratio)`. Crossover: once `ratio × fc ≥ sr` the clock
/// meets the host rate and this degenerates to a conventional per-sample SVF
/// with `g = tan(π·fc/sr)` (fc clamped below Nyquist) — identical math,
/// no fidelity or CPU cliff at high cutoffs. Below that, `f_clk` is floored
/// at 200 Hz so the filter never stalls. Returns held (lp, bp, hp).
#[inline]
pub(crate) fn svf_clocked_tick(
    x: f32,
    fc: f32,
    ratio: f32,
    sr: f32,
    k: f32,
    st: &mut [f32; SVF_BLOCK_LEN],
) -> (f32, f32, f32) {
    let f_clk = ratio * fc;
    if f_clk >= sr {
        let g = (std::f32::consts::PI * fc.clamp(1.0, sr * 0.49) / sr).tan();
        let (lp, bp, hp) = svf_core(x, g, k, st);
        st[SVF_HOLD_LP] = lp;
        st[SVF_HOLD_BP] = bp;
        st[SVF_HOLD_HP] = hp;
        return (lp, bp, hp);
    }
    let f_clk = f_clk.max(FCLK_FLOOR_HZ);
    st[SVF_CLK_PHASE] += f_clk / sr;
    if st[SVF_CLK_PHASE] >= 1.0 {
        st[SVF_CLK_PHASE] -= 1.0;
        let g = (std::f32::consts::PI / ratio.max(2.1)).tan();
        let (lp, bp, hp) = svf_core(x, g, k, st);
        st[SVF_HOLD_LP] = lp;
        st[SVF_HOLD_BP] = bp;
        st[SVF_HOLD_HP] = hp;
    }
    (st[SVF_HOLD_LP], st[SVF_HOLD_BP], st[SVF_HOLD_HP])
}

/// Mode morph weights (§3): 0 = LP, 0.5 = BP, 1 = HP.
#[inline]
fn morph_weights(mode: f32) -> (f32, f32, f32) {
    let m = mode.clamp(0.0, 1.0);
    if m < 0.5 {
        let t = m * 2.0;
        (1.0 - t, t, 0.0)
    } else {
        let t = (m - 0.5) * 2.0;
        (0.0, 1.0 - t, t)
    }
}

/// LFO shapes (§5): 0 Sine, 1 Saw (up), 2 Ramp (down), 3 Square.
#[inline]
fn lfo_wave_value(wave: i32, phase: f32) -> f32 {
    match wave {
        1 => phase * 2.0 - 1.0,
        2 => 1.0 - phase * 2.0,
        3 => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        _ => (phase * std::f32::consts::TAU).sin(),
    }
}

/// Input drive nonlinearity (§2): tube/diode-flavored asymmetric curve via
/// roar's transfer bank (reused, not copied). Small-signal slope ≈ 1.
#[inline]
fn drive_shape(x: f32) -> f32 {
    0.55 * shaper_transfer(SHAPER_TUBE, 0.5, x) + 0.45 * shaper_transfer(SHAPER_DIODE, 0.5, x)
}

// RBJ butterworth lowpass for the 2× oversampling boundary (space-echo
// pattern), transposed direct form II.
#[derive(Clone, Copy)]
struct BiquadCoeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

#[inline]
fn biquad_sample(input: f32, c: BiquadCoeffs, z1: &mut f32, z2: &mut f32) -> f32 {
    let out = c.b0 * input + *z1;
    *z1 = c.b1 * input - c.a1 * out + *z2;
    *z2 = c.b2 * input - c.a2 * out;
    out
}

fn lowpass_coeffs(freq: f32, sr: f32) -> BiquadCoeffs {
    let omega = std::f32::consts::TAU * freq.clamp(20.0, sr * 0.49) / sr.max(1.0);
    let sin = omega.sin();
    let cos = omega.cos();
    let alpha = sin * std::f32::consts::FRAC_1_SQRT_2;
    let b0 = (1.0 - cos) * 0.5;
    let a0 = 1.0 + alpha;
    BiquadCoeffs {
        b0: b0 / a0,
        b1: (1.0 - cos) / a0,
        b2: b0 / a0,
        a1: (-2.0 * cos) / a0,
        a2: (1.0 - alpha) / a0,
    }
}

/// Filter output stage: morph blend − correction·bp (§3) + clock bleed (§4).
#[allow(clippy::too_many_arguments)]
#[inline]
unsafe fn filter_channel(
    s: *mut f32,
    base: usize,
    x: f32,
    fc: f32,
    ratio: f32,
    sr: f32,
    k: f32,
    weights: (f32, f32, f32),
    correction: f32,
    bleed_level: f32,
) -> f32 {
    let st = &mut *(s.add(base) as *mut [f32; SVF_BLOCK_LEN]);
    let (lp, bp, hp) = svf_clocked_tick(x, fc, ratio, sr, k, st);
    let mut out = weights.0 * lp + weights.1 * bp + weights.2 * hp - correction * bp;
    // Clock bleed: square-ish tone at f_clk (its ZOH aliases come free),
    // keyed to Crunch and rising as the clock drops into the audible band.
    if bleed_level > 0.0 && ratio * fc < sr {
        let square = if st[SVF_CLK_PHASE] < 0.5 { 1.0 } else { -1.0 };
        out += bleed_level * square;
    }
    out
}

unsafe extern "C" fn filterbank_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    initial_state: *const c_void,
) {
    let s = state as *mut f32;
    let _ = initial_state;
    for i in 0..FILTERBANK_STATE_SIZE {
        *s.add(i) = 0.0;
    }
    *s.add(STATE_ENABLED) = 0.0;
    *s.add(STATE_INPUT_DB) = 0.0;
    *s.add(STATE_HI_EQ) = 1.0; // Flat
    *s.add(STATE_SENSE) = 30.0;
    *s.add(STATE_CRUNCH) = 25.0;
    *s.add(STATE_SER_PAR) = 100.0; // parallel
    *s.add(STATE_ATTACK_MS) = 5.0;
    *s.add(STATE_DECAY_MS) = 200.0;
    *s.add(STATE_RELEASE_MS) = 300.0;
    *s.add(STATE_ENV_F1) = 50.0;
    *s.add(STATE_ENV_F2) = 50.0;
    *s.add(STATE_RES_BLEED) = 10.0;
    *s.add(STATE_LFO_RATE) = 0.5;
    *s.add(STATE_LFO_SYNC) = 0.0;
    *s.add(STATE_LFO_DIV) = 6.0; // "1/4"
    *s.add(STATE_BPM) = 120.0;
    *s.add(STATE_LFO_WAVE) = 1.0; // Saw
    *s.add(STATE_AR_ATTACK_MS) = 5.0;
    *s.add(STATE_AR_RELEASE_MS) = 200.0;
    *s.add(STATE_DRY_WET) = 100.0;
    *s.add(STATE_F1_FREQ) = 500.0;
    *s.add(STATE_F1_RES) = 20.0;
    *s.add(STATE_F2_FREQ) = 500.0;
    *s.add(STATE_F2_RES) = 20.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    // Smoothers land on the defaults so first-block audio doesn't glide.
    *s.add(STATE_SM_INPUT_GAIN) = 1.0;
    *s.add(STATE_SM_CRUNCH) = 0.25;
    *s.add(STATE_SM_SER_PAR) = 1.0;
    *s.add(STATE_SM_ENV_F1) = 0.5;
    *s.add(STATE_SM_ENV_F2) = 0.5;
    *s.add(STATE_SM_RES_BLEED) = 0.1;
    *s.add(STATE_SM_OUTPUT_GAIN) = 1.0;
    *s.add(STATE_SM_DRY_WET) = 1.0;
    *s.add(STATE_SM_F1_FREQ) = 500.0;
    *s.add(STATE_SM_F1_RES) = 20.0;
    *s.add(STATE_SM_F2_FREQ) = 500.0;
    *s.add(STATE_SM_F2_RES) = 20.0;
    *s.add(STATE_METER_INPUT_DB) = -90.0;
}

unsafe extern "C" fn filterbank_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let nf = nframes as usize;
    let in0 = *inp.add(0);
    let in1 = *inp.add(1);
    let mod_inputs = [*inp.add(2), *inp.add(3), *inp.add(4), *inp.add(5)];
    let fm_input = *inp.add(FILTERBANK_FM_INPUT_CHANNEL);
    let am_input = *inp.add(FILTERBANK_AM_INPUT_CHANNEL);
    let out0 = *out.add(0);
    let out1 = *out.add(1);

    if *s.add(STATE_ENABLED) <= 0.5 {
        // Bypass: bit-exact passthrough; reset runtime state so re-enable
        // starts from silence (no click from stale integrators/envelopes).
        for i in FIRST_RUNTIME_RESET..RUNTIME_RESET_END {
            *s.add(i) = 0.0;
        }
        *s.add(STATE_METER_INPUT_DB) = -90.0;
        std::ptr::copy_nonoverlapping(in0 as *const f32, out0, nf);
        std::ptr::copy_nonoverlapping(in1 as *const f32, out1, nf);
        return;
    }

    let sr = (*s.add(STATE_SAMPLE_RATE)).max(1.0);

    // ── Knob targets (clamped; smoothed per sample below) ──
    let t_input_gain = db_to_amp((*s.add(STATE_INPUT_DB)).clamp(-12.0, 30.0));
    let hi_eq = (*s.add(STATE_HI_EQ)).round().clamp(0.0, 2.0) as i32;
    let t_shelf_gain = db_to_amp((hi_eq - 1) as f32 * 6.0) - 1.0;
    let sense = (*s.add(STATE_SENSE)).clamp(0.0, 100.0) / 100.0;
    let t_noise = (*s.add(STATE_NOISE)).clamp(0.0, 100.0) / 100.0;
    let t_feedback = (*s.add(STATE_FEEDBACK)).clamp(0.0, 100.0) / 100.0;
    let t_crunch = (*s.add(STATE_CRUNCH)).clamp(0.0, 100.0) / 100.0;
    let t_correction = (*s.add(STATE_CORRECTION)).clamp(0.0, 100.0) / 100.0;
    let t_ser_par = (*s.add(STATE_SER_PAR)).clamp(0.0, 100.0) / 100.0;
    let harmonics_idx = (*s.add(STATE_HARMONICS)).round().clamp(0.0, 11.0) as usize;
    let harmonic_ratio = HARMONIC_RATIOS[harmonics_idx];
    let t_fm = (*s.add(STATE_FM_AMOUNT)).clamp(0.0, 100.0) / 100.0;
    let t_am = (*s.add(STATE_AM_DEPTH)).clamp(0.0, 100.0) / 100.0;
    let follower_mode = *s.add(STATE_ENV_MODE) > 0.5;
    let attack_ms = (*s.add(STATE_ATTACK_MS)).clamp(0.5, 4000.0);
    let decay_ms = (*s.add(STATE_DECAY_MS)).clamp(1.0, 4000.0);
    let sustain = (*s.add(STATE_SUSTAIN)).clamp(-100.0, 100.0) / 100.0;
    let release_ms = (*s.add(STATE_RELEASE_MS)).clamp(1.0, 8000.0);
    let t_env_f1 = (*s.add(STATE_ENV_F1)).clamp(-100.0, 100.0) / 100.0;
    let t_env_f2 = (*s.add(STATE_ENV_F2)).clamp(-100.0, 100.0) / 100.0;
    let t_res_bleed = (*s.add(STATE_RES_BLEED)).clamp(0.0, 100.0) / 100.0;
    let lfo_rate = lfo_rate_hz(
        *s.add(STATE_LFO_SYNC),
        *s.add(STATE_LFO_DIV),
        *s.add(STATE_BPM),
        (*s.add(STATE_LFO_RATE)).clamp(0.01, 2000.0),
    );
    let lfo_wave = (*s.add(STATE_LFO_WAVE)).round().clamp(0.0, 3.0) as i32;
    let t_lfo_depth = (*s.add(STATE_LFO_DEPTH)).clamp(-100.0, 100.0) / 100.0;
    let lfo_trig = *s.add(STATE_LFO_TRIG) > 0.5;
    let ar_attack_ms = (*s.add(STATE_AR_ATTACK_MS)).clamp(0.5, 2000.0);
    let ar_release_ms = (*s.add(STATE_AR_RELEASE_MS)).clamp(1.0, 4000.0);
    let t_ar_depth = (*s.add(STATE_AR_DEPTH)).clamp(0.0, 100.0) / 100.0;
    let stereo_split = *s.add(STATE_STEREO_SPLIT) > 0.5;
    // Split mode only ticks the F1L/F2R blocks; reset the idle pair on any
    // toggle so neither direction resumes from stale integrator state.
    if stereo_split != (*s.add(STATE_SPLIT_PREV) > 0.5) {
        *s.add(STATE_SPLIT_PREV) = if stereo_split { 1.0 } else { 0.0 };
        for idx in 0..SVF_BLOCK_LEN {
            *s.add(STATE_F1R + idx) = 0.0;
            *s.add(STATE_F2L + idx) = 0.0;
        }
    }
    let t_output_gain = db_to_amp((*s.add(STATE_OUTPUT_DB)).clamp(-24.0, 24.0));
    let t_dry_wet = (*s.add(STATE_DRY_WET)).clamp(0.0, 100.0) / 100.0;
    let t_f1_freq = (*s.add(STATE_F1_FREQ)).clamp(20.0, 16_000.0);
    let t_f1_res = (*s.add(STATE_F1_RES)).clamp(0.0, 110.0);
    let t_f1_mode = (*s.add(STATE_F1_MODE)).clamp(0.0, 100.0) / 100.0;
    let t_f2_freq = (*s.add(STATE_F2_FREQ)).clamp(20.0, 16_000.0);
    let t_f2_res = (*s.add(STATE_F2_RES)).clamp(0.0, 110.0);
    let t_f2_mode = (*s.add(STATE_F2_MODE)).clamp(0.0, 100.0) / 100.0;
    let fm_ext = *s.add(STATE_FM_EXT_ACTIVE) > 0.5;
    let am_ext = *s.add(STATE_AM_EXT_ACTIVE) > 0.5;

    let read_depths = |base: usize| -> [f32; 4] {
        [
            *s.add(base),
            *s.add(base + 1),
            *s.add(base + 2),
            *s.add(base + 3),
        ]
    };
    let d_f1_freq = read_depths(STATE_MOD_F1_FREQ_DEPTH_1);
    let d_f2_freq = read_depths(STATE_MOD_F2_FREQ_DEPTH_1);
    let d_f1_res = read_depths(STATE_MOD_F1_RES_DEPTH_1);
    let d_f2_res = read_depths(STATE_MOD_F2_RES_DEPTH_1);
    let d_f1_mode = read_depths(STATE_MOD_F1_MODE_DEPTH_1);
    let d_f2_mode = read_depths(STATE_MOD_F2_MODE_DEPTH_1);
    let d_fm = read_depths(STATE_MOD_FM_DEPTH_1);
    let d_am = read_depths(STATE_MOD_AM_DEPTH_1);
    let d_ser_par = read_depths(STATE_MOD_SER_PAR_DEPTH_1);
    let d_crunch = read_depths(STATE_MOD_CRUNCH_DEPTH_1);

    // ── Coefficients ──
    let smooth_coeff = one_pole_coef(20.0, sr);
    let lag_coeff = time_coef(2.0, sr); // §5a linear-target lag
    let lfo_inc = lfo_rate / sr;
    let attack_coeff = time_coef(attack_ms, sr);
    let decay_coeff = time_coef(decay_ms, sr);
    let release_coeff = time_coef(release_ms, sr);
    let ar_attack_coeff = time_coef(ar_attack_ms, sr);
    let ar_release_coeff = time_coef(ar_release_ms, sr);
    // Sense detector (§2): fast attack, ~50 ms release, hysteresis ~3 dB,
    // 20 ms retrigger holdoff. Higher Sense = lower threshold.
    let sense_attack = time_coef(1.0, sr);
    let sense_release = time_coef(50.0, sr);
    let thr_on = db_to_amp(-3.0 - 57.0 * sense);
    let thr_off = thr_on * db_to_amp(-3.0);
    let holdoff_samples = (0.020 * sr).round();
    let fb_hp_coeff = one_pole_coef(30.0, sr);
    let shelf_coeff = one_pole_coef(3000.0, sr);
    let dc_r = (-std::f32::consts::TAU * 10.0 / sr).exp();
    let fs_os = sr * 2.0;
    let os_lp = lowpass_coeffs(sr * 0.45, fs_os);
    let meter_attack = time_coef(5.0, sr);
    let meter_release = time_coef(250.0, sr);

    // ── Load runtime state into locals ──
    let mut sm_input_gain = *s.add(STATE_SM_INPUT_GAIN);
    let mut sm_noise = *s.add(STATE_SM_NOISE);
    let mut sm_feedback = *s.add(STATE_SM_FEEDBACK);
    let mut sm_crunch = *s.add(STATE_SM_CRUNCH);
    let mut sm_correction = *s.add(STATE_SM_CORRECTION);
    let mut sm_ser_par = *s.add(STATE_SM_SER_PAR);
    let mut sm_fm = *s.add(STATE_SM_FM);
    let mut sm_am = *s.add(STATE_SM_AM);
    let mut sm_env_f1 = *s.add(STATE_SM_ENV_F1);
    let mut sm_env_f2 = *s.add(STATE_SM_ENV_F2);
    let mut sm_res_bleed = *s.add(STATE_SM_RES_BLEED);
    let mut sm_lfo_depth = *s.add(STATE_SM_LFO_DEPTH);
    let mut sm_ar_depth = *s.add(STATE_SM_AR_DEPTH);
    let mut sm_output_gain = *s.add(STATE_SM_OUTPUT_GAIN);
    let mut sm_dry_wet = *s.add(STATE_SM_DRY_WET);
    let mut sm_f1_freq = *s.add(STATE_SM_F1_FREQ);
    let mut sm_f1_res = *s.add(STATE_SM_F1_RES);
    let mut sm_f1_mode = *s.add(STATE_SM_F1_MODE);
    let mut sm_f2_freq = *s.add(STATE_SM_F2_FREQ);
    let mut sm_f2_res = *s.add(STATE_SM_F2_RES);
    let mut sm_f2_mode = *s.add(STATE_SM_F2_MODE);
    let mut sm_shelf_gain = *s.add(STATE_SM_SHELF_GAIN);
    let mut sense_env = *s.add(STATE_SENSE_ENV);
    let mut sense_prev = *s.add(STATE_SENSE_PREV);
    let mut gate = *s.add(STATE_GATE) > 0.5;
    let mut holdoff = *s.add(STATE_HOLDOFF);
    let mut env_stage = (*s.add(STATE_ENV_STAGE)).round() as i32;
    let mut env_value = *s.add(STATE_ENV_VALUE);
    let mut lfo_phase = *s.add(STATE_LFO_PHASE);
    let mut ar_env = *s.add(STATE_AR_ENV);
    let mut trig_count = *s.add(STATE_TRIG_COUNT);
    let mut noise_seed = *s.add(STATE_NOISE_SEED);
    let mut fb_lp_l = *s.add(STATE_FB_LP_L);
    let mut fb_lp_r = *s.add(STATE_FB_LP_R);
    let mut fb_prev_l = *s.add(STATE_FB_PREV_L);
    let mut fb_prev_r = *s.add(STATE_FB_PREV_R);
    let mut shelf_lp_l = *s.add(STATE_SHELF_LP_L);
    let mut shelf_lp_r = *s.add(STATE_SHELF_LP_R);
    let mut os_up_z1_l = *s.add(STATE_OS_UP_Z1_L);
    let mut os_up_z2_l = *s.add(STATE_OS_UP_Z2_L);
    let mut os_down_z1_l = *s.add(STATE_OS_DOWN_Z1_L);
    let mut os_down_z2_l = *s.add(STATE_OS_DOWN_Z2_L);
    let mut os_up_z1_r = *s.add(STATE_OS_UP_Z1_R);
    let mut os_up_z2_r = *s.add(STATE_OS_UP_Z2_R);
    let mut os_down_z1_r = *s.add(STATE_OS_DOWN_Z1_R);
    let mut os_down_z2_r = *s.add(STATE_OS_DOWN_Z2_R);
    let mut dc_x1_l = *s.add(STATE_DC_X1_L);
    let mut dc_y1_l = *s.add(STATE_DC_Y1_L);
    let mut dc_x1_r = *s.add(STATE_DC_X1_R);
    let mut dc_y1_r = *s.add(STATE_DC_Y1_R);
    let mut lag_f1_res = *s.add(STATE_LAG_F1_RES);
    let mut lag_f2_res = *s.add(STATE_LAG_F2_RES);
    let mut lag_f1_mode = *s.add(STATE_LAG_F1_MODE);
    let mut lag_f2_mode = *s.add(STATE_LAG_F2_MODE);
    let mut lag_fm = *s.add(STATE_LAG_FM);
    let mut lag_am = *s.add(STATE_LAG_AM);
    let mut lag_ser_par = *s.add(STATE_LAG_SER_PAR);
    let mut lag_crunch = *s.add(STATE_LAG_CRUNCH);
    let mut meter_input = *s.add(STATE_METER_INPUT_DB);
    if noise_seed == 0.0 {
        noise_seed = 0x2f_6e_2b as f32;
    }

    let mut last_f1_hz = *s.add(STATE_METER_F1_HZ);
    let mut last_f2_hz = *s.add(STATE_METER_F2_HZ);

    for i in 0..nf {
        // ── Knob smoothing ──
        sm_input_gain += smooth_coeff * (t_input_gain - sm_input_gain);
        sm_noise += smooth_coeff * (t_noise - sm_noise);
        sm_feedback += smooth_coeff * (t_feedback - sm_feedback);
        sm_crunch += smooth_coeff * (t_crunch - sm_crunch);
        sm_correction += smooth_coeff * (t_correction - sm_correction);
        sm_ser_par += smooth_coeff * (t_ser_par - sm_ser_par);
        sm_fm += smooth_coeff * (t_fm - sm_fm);
        sm_am += smooth_coeff * (t_am - sm_am);
        sm_env_f1 += smooth_coeff * (t_env_f1 - sm_env_f1);
        sm_env_f2 += smooth_coeff * (t_env_f2 - sm_env_f2);
        sm_res_bleed += smooth_coeff * (t_res_bleed - sm_res_bleed);
        sm_lfo_depth += smooth_coeff * (t_lfo_depth - sm_lfo_depth);
        sm_ar_depth += smooth_coeff * (t_ar_depth - sm_ar_depth);
        sm_output_gain += smooth_coeff * (t_output_gain - sm_output_gain);
        sm_dry_wet += smooth_coeff * (t_dry_wet - sm_dry_wet);
        sm_f1_freq += smooth_coeff * (t_f1_freq - sm_f1_freq);
        sm_f1_res += smooth_coeff * (t_f1_res - sm_f1_res);
        sm_f1_mode += smooth_coeff * (t_f1_mode - sm_f1_mode);
        sm_f2_freq += smooth_coeff * (t_f2_freq - sm_f2_freq);
        sm_f2_res += smooth_coeff * (t_f2_res - sm_f2_res);
        sm_f2_mode += smooth_coeff * (t_f2_mode - sm_f2_mode);
        sm_shelf_gain += smooth_coeff * (t_shelf_gain - sm_shelf_gain);

        let dry_l = *in0.add(i);
        let dry_r = *in1.add(i);

        // ── §2 input drive: gain → tube/diode shaper at 2× → DC block ──
        let mut post = [0.0_f32; 2];
        for (ch, (&x, (up_z1, up_z2, down_z1, down_z2, dc_x1, dc_y1, shelf_lp))) in [dry_l, dry_r]
            .iter()
            .zip([
                (
                    &mut os_up_z1_l,
                    &mut os_up_z2_l,
                    &mut os_down_z1_l,
                    &mut os_down_z2_l,
                    &mut dc_x1_l,
                    &mut dc_y1_l,
                    &mut shelf_lp_l,
                ),
                (
                    &mut os_up_z1_r,
                    &mut os_up_z2_r,
                    &mut os_down_z1_r,
                    &mut os_down_z2_r,
                    &mut dc_x1_r,
                    &mut dc_y1_r,
                    &mut shelf_lp_r,
                ),
            ])
            .enumerate()
        {
            let gained = x * sm_input_gain;
            let mut shaped_down = 0.0;
            for k_os in 0..2 {
                let stuffed = if k_os == 0 { gained * 2.0 } else { 0.0 };
                let up = biquad_sample(stuffed, os_lp, up_z1, up_z2);
                let shaped = drive_shape(up);
                shaped_down = biquad_sample(shaped, os_lp, down_z1, down_z2);
            }
            // DC block (the asymmetric curve rides on an offset).
            let blocked = {
                let y = shaped_down - *dc_x1 + dc_r * *dc_y1;
                *dc_x1 = shaped_down;
                *dc_y1 = y;
                y
            };
            // Hi EQ: ±6 dB high shelf @ ~3 kHz (post-drive per spec §12).
            *shelf_lp += shelf_coeff * (blocked - *shelf_lp);
            post[ch] = blocked + sm_shelf_gain * (blocked - *shelf_lp);
        }
        let post_mono = 0.5 * (post[0] + post[1]);

        // ── Sense trigger (§2) ──
        let amp = post_mono.abs();
        let coeff = if amp > sense_env {
            sense_attack
        } else {
            sense_release
        };
        sense_env += coeff * (amp - sense_env);
        if holdoff > 0.0 {
            holdoff -= 1.0;
        }
        let crossed = sense_prev < thr_on && sense_env >= thr_on;
        sense_prev = sense_env;
        if crossed && !gate && holdoff <= 0.0 {
            gate = true;
            env_stage = 1;
            trig_count += 1.0;
            holdoff = holdoff_samples;
            if lfo_trig {
                lfo_phase = 0.0;
            }
        }
        if gate && sense_env < thr_off {
            gate = false;
            env_stage = 3;
        }

        // ── Envelope (§5): ADSR with bipolar sustain, or follower ──
        if follower_mode {
            let fc = if amp > env_value {
                attack_coeff
            } else {
                release_coeff
            };
            env_value += fc * (amp - env_value);
        } else {
            match env_stage {
                1 => {
                    env_value += attack_coeff * (1.25 - env_value);
                    if env_value >= 1.0 {
                        env_value = 1.0;
                        env_stage = 2;
                    }
                }
                2 => env_value += decay_coeff * (sustain - env_value),
                3 => {
                    env_value += release_coeff * (0.0 - env_value);
                    if env_value.abs() < 1.0e-4 {
                        env_value = 0.0;
                        env_stage = 0;
                    }
                }
                _ => {}
            }
        }

        // ── LFO (§5): audio-rate; negative depth inverts on F2 only ──
        lfo_phase = (lfo_phase + lfo_inc).fract();
        let lfo = lfo_wave_value(lfo_wave, lfo_phase);
        let lfo_f1_oct = lfo * sm_lfo_depth.abs() * 4.0;
        let lfo_f2_oct = lfo * sm_lfo_depth * 4.0;

        // ── §5a host mod: raw per-sample sums, lag on linear targets ──
        let mod_now = [
            (*mod_inputs[0].add(i)).clamp(-1.0, 1.0),
            (*mod_inputs[1].add(i)).clamp(-1.0, 1.0),
            (*mod_inputs[2].add(i)).clamp(-1.0, 1.0),
            (*mod_inputs[3].add(i)).clamp(-1.0, 1.0),
        ];
        let mod_sum = |d: &[f32; 4]| -> f32 {
            d[0] * mod_now[0] + d[1] * mod_now[1] + d[2] * mod_now[2] + d[3] * mod_now[3]
        };
        let ext_f1_oct = mod_sum(&d_f1_freq) * 4.0;
        let ext_f2_oct = mod_sum(&d_f2_freq) * 4.0;
        lag_f1_res += lag_coeff * (mod_sum(&d_f1_res) - lag_f1_res);
        lag_f2_res += lag_coeff * (mod_sum(&d_f2_res) - lag_f2_res);
        lag_f1_mode += lag_coeff * (mod_sum(&d_f1_mode) - lag_f1_mode);
        lag_f2_mode += lag_coeff * (mod_sum(&d_f2_mode) - lag_f2_mode);
        lag_fm += lag_coeff * (mod_sum(&d_fm) - lag_fm);
        lag_am += lag_coeff * (mod_sum(&d_am) - lag_am);
        lag_ser_par += lag_coeff * (mod_sum(&d_ser_par) - lag_ser_par);
        lag_crunch += lag_coeff * (mod_sum(&d_crunch) - lag_crunch);

        // ── FM (§5): normalled to the post-drive input unless ext active ──
        let fm_eff = (sm_fm + lag_fm).clamp(0.0, 1.0);
        let fm_src = if fm_ext { *fm_input.add(i) } else { post_mono };
        let fm_oct = fm_src.tanh() * fm_eff * 4.0;

        // ── Effective cutoffs / res / modes ──
        let f1_oct = env_value * sm_env_f1 * 4.0 + lfo_f1_oct + fm_oct + ext_f1_oct;
        let f1_hz = (sm_f1_freq * (2.0_f32).powf(f1_oct.clamp(-8.0, 8.0))).clamp(20.0, sr * 0.45);
        // Harmonics link (§3): F2 = modulated F1 ÷ ratio — mod applies
        // pre-link so linked sweeps stay harmonic. F2 freq knob + mods
        // are ignored while linked.
        let f2_hz = if harmonic_ratio > 0.0 {
            (f1_hz / harmonic_ratio).clamp(20.0, sr * 0.45)
        } else {
            let f2_oct = env_value * sm_env_f2 * 4.0 + lfo_f2_oct + fm_oct + ext_f2_oct;
            (sm_f2_freq * (2.0_f32).powf(f2_oct.clamp(-8.0, 8.0))).clamp(20.0, sr * 0.45)
        };
        let res_bleed_pct = env_value * sm_res_bleed * 100.0;
        let r1 = (sm_f1_res + res_bleed_pct + lag_f1_res * 110.0).clamp(0.0, 110.0);
        let r2 = (sm_f2_res + res_bleed_pct + lag_f2_res * 110.0).clamp(0.0, 110.0);
        let k1 = res_to_k(r1);
        let k2 = res_to_k(r2);
        let w1 = morph_weights(sm_f1_mode + lag_f1_mode);
        let w2 = morph_weights(sm_f2_mode + lag_f2_mode);
        let correction = sm_correction.clamp(0.0, 1.0);
        let crunch_eff = (sm_crunch + lag_crunch).clamp(0.0, 1.0);
        // §4: Crunch morphs the clock ratio 100:1 → 25:1 (log).
        let ratio = 100.0 * (0.25_f32).powf(crunch_eff);
        // Signal-presence gate on the clock bleed: the hardware leaks clock
        // noise even at idle, but a builtin must be silent with no program
        // material. Rides the sense envelope (post-drive |level|), fully open
        // by ~-50 dBFS, so bleed tracks the signal and dies in silence.
        let bleed_presence = (sense_env * 320.0).clamp(0.0, 1.0);
        let bleed = |fc: f32| -> f32 {
            let f_clk = (ratio * fc).max(FCLK_FLOOR_HZ);
            crunch_eff * 0.02 * bleed_presence * ((1.0 - f_clk / 12_000.0).clamp(0.0, 1.0))
        };
        let bleed1 = bleed(f1_hz);
        let bleed2 = bleed(f2_hz);
        last_f1_hz = f1_hz;
        last_f2_hz = f2_hz;

        // ── Filter input: post-drive + noise + tanh-limited feedback ──
        let noise = next_noise(&mut noise_seed);
        let noise_term = sm_noise * 0.5 * noise;
        let fb_gain = sm_feedback * 0.95;
        let fb_l = {
            let lim = fb_prev_l.tanh();
            fb_lp_l += fb_hp_coeff * (lim - fb_lp_l);
            fb_gain * (lim - fb_lp_l)
        };
        let fb_r = {
            let lim = fb_prev_r.tanh();
            fb_lp_r += fb_hp_coeff * (lim - fb_lp_r);
            fb_gain * (lim - fb_lp_r)
        };
        let xin_l = post[0] + noise_term + fb_l;
        let xin_r = post[1] + noise_term + fb_r;

        // ── Topology: ser/par morph (§3) or stereo split (§6) ──
        let (mut wet_l, mut wet_r, f2_out_l, f2_out_r);
        if stereo_split {
            // Forced parallel: wet L = filter 1 only, wet R = filter 2 only.
            let o1 = filter_channel(
                s, STATE_F1L, xin_l, f1_hz, ratio, sr, k1, w1, correction, bleed1,
            );
            let o2 = filter_channel(
                s, STATE_F2R, xin_r, f2_hz, ratio, sr, k2, w2, correction, bleed2,
            );
            wet_l = o1;
            wet_r = o2;
            f2_out_l = o2;
            f2_out_r = o2;
        } else {
            let sp = (sm_ser_par + lag_ser_par).clamp(0.0, 1.0); // 1 = parallel
            let o1_l = filter_channel(
                s, STATE_F1L, xin_l, f1_hz, ratio, sr, k1, w1, correction, bleed1,
            );
            let o1_r = filter_channel(
                s, STATE_F1R, xin_r, f1_hz, ratio, sr, k1, w1, correction, bleed1,
            );
            // F2 runs once per channel, fed the crossfaded blend of input
            // (parallel) and F1 output (serial).
            let f2_in_l = sp * xin_l + (1.0 - sp) * o1_l;
            let f2_in_r = sp * xin_r + (1.0 - sp) * o1_r;
            let o2_l = filter_channel(
                s, STATE_F2L, f2_in_l, f2_hz, ratio, sr, k2, w2, correction, bleed2,
            );
            let o2_r = filter_channel(
                s, STATE_F2R, f2_in_r, f2_hz, ratio, sr, k2, w2, correction, bleed2,
            );
            wet_l = (1.0 - sp) * o2_l + sp * 0.5 * (o1_l + o2_l);
            wet_r = (1.0 - sp) * o2_r + sp * 0.5 * (o1_r + o2_r);
            f2_out_l = o2_l;
            f2_out_r = o2_r;
        }
        fb_prev_l = wet_l;
        fb_prev_r = wet_r;

        // ── AM / ring (§5): 0-50 = AM depth, 50-100 = fade into ring ──
        let am_eff = (sm_am + lag_am).clamp(0.0, 1.0);
        if am_eff > 1.0e-4 {
            let m_l = if am_ext {
                (*am_input.add(i)).tanh()
            } else {
                f2_out_l.tanh()
            };
            let m_r = if am_ext {
                (*am_input.add(i)).tanh()
            } else {
                f2_out_r.tanh()
            };
            let am_gain = |m: f32| -> f32 {
                let m_uni = 0.5 * (1.0 + m);
                if am_eff <= 0.5 {
                    let a = am_eff * 2.0;
                    1.0 + a * (m_uni - 1.0)
                } else {
                    let x = (am_eff - 0.5) * 2.0;
                    m_uni + x * (m - m_uni)
                }
            };
            wet_l *= am_gain(m_l);
            wet_r *= am_gain(m_r);
        }

        // ── AR output envelope (§6) ──
        if gate {
            ar_env += ar_attack_coeff * (1.0 - ar_env);
        } else {
            ar_env += ar_release_coeff * (0.0 - ar_env);
        }
        let ar_gain = 1.0 + sm_ar_depth * (ar_env - 1.0);
        wet_l *= ar_gain * sm_output_gain;
        wet_r *= ar_gain * sm_output_gain;

        // ── Equal-power dry/wet ──
        let theta = sm_dry_wet.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
        let dry_g = theta.cos();
        let wet_g = theta.sin();
        *out0.add(i) = dry_l * dry_g + wet_l * wet_g;
        *out1.add(i) = dry_r * dry_g + wet_r * wet_g;

        // Input meter ballistics (5/250 ms) on the post-drive level.
        let level_db = amp_to_db(amp);
        let mc = if level_db > meter_input {
            meter_attack
        } else {
            meter_release
        };
        meter_input += mc * (level_db - meter_input);
    }

    // ── Store runtime state ──
    *s.add(STATE_SM_INPUT_GAIN) = sm_input_gain;
    *s.add(STATE_SM_NOISE) = sm_noise;
    *s.add(STATE_SM_FEEDBACK) = sm_feedback;
    *s.add(STATE_SM_CRUNCH) = sm_crunch;
    *s.add(STATE_SM_CORRECTION) = sm_correction;
    *s.add(STATE_SM_SER_PAR) = sm_ser_par;
    *s.add(STATE_SM_FM) = sm_fm;
    *s.add(STATE_SM_AM) = sm_am;
    *s.add(STATE_SM_ENV_F1) = sm_env_f1;
    *s.add(STATE_SM_ENV_F2) = sm_env_f2;
    *s.add(STATE_SM_RES_BLEED) = sm_res_bleed;
    *s.add(STATE_SM_LFO_DEPTH) = sm_lfo_depth;
    *s.add(STATE_SM_AR_DEPTH) = sm_ar_depth;
    *s.add(STATE_SM_OUTPUT_GAIN) = sm_output_gain;
    *s.add(STATE_SM_DRY_WET) = sm_dry_wet;
    *s.add(STATE_SM_F1_FREQ) = sm_f1_freq;
    *s.add(STATE_SM_F1_RES) = sm_f1_res;
    *s.add(STATE_SM_F1_MODE) = sm_f1_mode;
    *s.add(STATE_SM_F2_FREQ) = sm_f2_freq;
    *s.add(STATE_SM_F2_RES) = sm_f2_res;
    *s.add(STATE_SM_F2_MODE) = sm_f2_mode;
    *s.add(STATE_SM_SHELF_GAIN) = sm_shelf_gain;
    *s.add(STATE_SENSE_ENV) = sense_env;
    *s.add(STATE_SENSE_PREV) = sense_prev;
    *s.add(STATE_GATE) = if gate { 1.0 } else { 0.0 };
    *s.add(STATE_HOLDOFF) = holdoff;
    *s.add(STATE_ENV_STAGE) = env_stage as f32;
    *s.add(STATE_ENV_VALUE) = env_value;
    *s.add(STATE_LFO_PHASE) = lfo_phase;
    *s.add(STATE_AR_ENV) = ar_env;
    *s.add(STATE_TRIG_COUNT) = trig_count;
    *s.add(STATE_NOISE_SEED) = noise_seed;
    *s.add(STATE_FB_LP_L) = fb_lp_l;
    *s.add(STATE_FB_LP_R) = fb_lp_r;
    *s.add(STATE_FB_PREV_L) = fb_prev_l;
    *s.add(STATE_FB_PREV_R) = fb_prev_r;
    *s.add(STATE_SHELF_LP_L) = shelf_lp_l;
    *s.add(STATE_SHELF_LP_R) = shelf_lp_r;
    *s.add(STATE_OS_UP_Z1_L) = os_up_z1_l;
    *s.add(STATE_OS_UP_Z2_L) = os_up_z2_l;
    *s.add(STATE_OS_DOWN_Z1_L) = os_down_z1_l;
    *s.add(STATE_OS_DOWN_Z2_L) = os_down_z2_l;
    *s.add(STATE_OS_UP_Z1_R) = os_up_z1_r;
    *s.add(STATE_OS_UP_Z2_R) = os_up_z2_r;
    *s.add(STATE_OS_DOWN_Z1_R) = os_down_z1_r;
    *s.add(STATE_OS_DOWN_Z2_R) = os_down_z2_r;
    *s.add(STATE_DC_X1_L) = dc_x1_l;
    *s.add(STATE_DC_Y1_L) = dc_y1_l;
    *s.add(STATE_DC_X1_R) = dc_x1_r;
    *s.add(STATE_DC_Y1_R) = dc_y1_r;
    *s.add(STATE_LAG_F1_RES) = lag_f1_res;
    *s.add(STATE_LAG_F2_RES) = lag_f2_res;
    *s.add(STATE_LAG_F1_MODE) = lag_f1_mode;
    *s.add(STATE_LAG_F2_MODE) = lag_f2_mode;
    *s.add(STATE_LAG_FM) = lag_fm;
    *s.add(STATE_LAG_AM) = lag_am;
    *s.add(STATE_LAG_SER_PAR) = lag_ser_par;
    *s.add(STATE_LAG_CRUNCH) = lag_crunch;

    // ── §9 live-meter tail ──
    *s.add(STATE_METER_INPUT_DB) = meter_input;
    *s.add(STATE_METER_ENV) = env_value;
    *s.add(STATE_METER_GATE) = if gate { 1.0 } else { 0.0 };
    *s.add(STATE_METER_F1_HZ) = last_f1_hz;
    *s.add(STATE_METER_F2_HZ) = last_f2_hz;
}

pub fn filterbank_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(filterbank_process),
        init: Some(filterbank_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::raw::c_void;

    const SR: f32 = 48_000.0;

    fn init_state() -> Vec<f32> {
        let mut st = vec![0.0_f32; FILTERBANK_STATE_SIZE];
        unsafe {
            filterbank_init(
                st.as_mut_ptr().cast::<c_void>(),
                SR as i32,
                512,
                std::ptr::null(),
            );
        }
        st
    }

    /// Drive `filterbank_process` with all 8 input ports.
    fn process_block(state: &mut [f32], ins: &[&[f32]; 8]) -> (Vec<f32>, Vec<f32>) {
        let n = ins[0].len();
        let mut bufs: Vec<Vec<f32>> = ins.iter().map(|c| c.to_vec()).collect();
        for b in &bufs {
            assert_eq!(b.len(), n);
        }
        let in_ptrs: Vec<*mut f32> = bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        let mut out_l = vec![0.0_f32; n];
        let mut out_r = vec![0.0_f32; n];
        let outs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        unsafe {
            filterbank_process(
                in_ptrs.as_ptr(),
                outs.as_ptr(),
                n as i32,
                state.as_mut_ptr().cast::<c_void>(),
                std::ptr::null_mut(),
            );
        }
        (out_l, out_r)
    }

    fn render(state: &mut [f32], in_l: &[f32], in_r: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let z = vec![0.0_f32; in_l.len()];
        process_block(state, &[in_l, in_r, &z, &z, &z, &z, &z, &z])
    }

    fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / SR).sin())
            .collect()
    }

    fn rms(buf: &[f32]) -> f32 {
        (buf.iter().map(|x| x * x).sum::<f32>() / buf.len().max(1) as f32).sqrt()
    }

    /// Quadrature magnitude at `freq` over the tail of `buf`.
    fn tone_magnitude(buf: &[f32], freq: f32) -> f32 {
        let n = buf.len();
        let (mut re, mut im) = (0.0_f64, 0.0_f64);
        for (i, &x) in buf.iter().enumerate() {
            let ph = std::f64::consts::TAU * freq as f64 * i as f64 / SR as f64;
            re += x as f64 * ph.cos();
            im += x as f64 * ph.sin();
        }
        ((re * re + im * im).sqrt() / n as f64) as f32
    }

    // ── 1. clocked SVF degenerates to the per-sample SVF at f_clk ≥ sr ──
    #[test]
    fn clocked_svf_matches_per_sample_at_high_clock() {
        let fc = 1000.0;
        let ratio = 100.0; // f_clk = 100 kHz ≥ 48 kHz → per-sample branch
        let k = 1.0;
        let input = sine(300.0, 0.05, 4096);
        let mut st = [0.0_f32; SVF_BLOCK_LEN];
        // filter.rs-style per-sample reference (no bp limiter — the small
        // input keeps the tanh in its linear region).
        let g = (std::f32::consts::PI * fc / SR).tan();
        let (mut ic1, mut ic2) = (0.0_f32, 0.0_f32);
        for &x in &input {
            let (lp, _, _) = svf_clocked_tick(x, fc, ratio, SR, k, &mut st);
            let a1 = 1.0 / (1.0 + g * (g + k));
            let a2 = g * a1;
            let a3 = g * a2;
            let v3 = x - ic2;
            let v1 = a1 * ic1 + a2 * v3;
            let v2 = ic2 + a2 * ic1 + a3 * v3;
            ic1 = 2.0 * v1 - ic1;
            ic2 = 2.0 * v2 - ic2;
            assert!(
                (lp - v2).abs() < 1.0e-3,
                "clocked ({lp}) vs per-sample ({v2}) diverged"
            );
        }
    }

    // ── 2. low cutoff → ZOH plateaus (consecutive equal samples) ──
    #[test]
    fn low_cutoff_shows_zoh_plateaus() {
        let fc = 30.0;
        let ratio = 100.0; // f_clk = 3 kHz → ~16-sample holds at 48 kHz
        let mut st = [0.0_f32; SVF_BLOCK_LEN];
        let input = sine(500.0, 0.5, 4096);
        let mut outputs = Vec::with_capacity(input.len());
        for &x in &input {
            let (lp, _, _) = svf_clocked_tick(x, fc, ratio, SR, 1.0, &mut st);
            outputs.push(lp);
        }
        let held = outputs.windows(2).filter(|w| w[0] == w[1]).count();
        assert!(
            held > outputs.len() / 2,
            "expected ZOH plateaus, only {held}/{} held pairs",
            outputs.len()
        );
    }

    // ── 3. self-oscillation pitch tracks Freq at res 105% ──
    #[test]
    fn self_oscillation_tracks_freq() {
        for fc in [300.0_f32, 500.0, 1200.0] {
            let k = res_to_k(105.0); // = -0.1
            let ratio = 100.0; // per-sample branch at these cutoffs
            let mut st = [0.0_f32; SVF_BLOCK_LEN];
            let total = (2.0 * SR) as usize;
            let mut bp_tail = Vec::with_capacity(total / 2);
            for i in 0..total {
                // Short burst rather than a 1-sample impulse: in the clocked
                // branch a single sample can fall between clock updates.
                let x = if i < 16 { 1.0 } else { 0.0 };
                let (_, bp, _) = svf_clocked_tick(x, fc, ratio, SR, k, &mut st);
                if i >= total / 2 {
                    bp_tail.push(bp);
                }
            }
            assert!(
                rms(&bp_tail) > 0.1,
                "filter did not self-oscillate at fc={fc}"
            );
            let crossings = bp_tail
                .windows(2)
                .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
                .count();
            let seconds = bp_tail.len() as f32 / SR;
            let measured = crossings as f32 / 2.0 / seconds;
            assert!(
                (measured - fc).abs() / fc < 0.03,
                "self-osc at {measured} Hz, expected {fc} Hz"
            );
        }
    }

    // ── 4. Sense trigger: one fire per burst, holdoff respected ──
    #[test]
    fn sense_trigger_fires_once_with_holdoff() {
        let mut state = init_state();
        state[STATE_ENABLED] = 1.0;
        state[STATE_SENSE] = 50.0;

        // Burst (100 samples) then silence: exactly one trigger.
        let mut buf = vec![0.0_f32; 300];
        for (i, v) in sine(1000.0, 0.5, 100).iter().enumerate() {
            buf[i] = *v;
        }
        let l = buf.clone();
        render(&mut state, &l, &buf);
        assert_eq!(state[STATE_TRIG_COUNT], 1.0, "burst should fire once");
        assert_eq!(state[STATE_METER_GATE], 1.0, "gate should be high");

        // Force the detector back down while the 20 ms holdoff (960 samples,
        // set at the fire ~sample 0) is still running: a new burst crossing
        // the threshold must NOT retrigger.
        state[STATE_SENSE_ENV] = 0.0;
        state[STATE_SENSE_PREV] = 0.0;
        state[STATE_GATE] = 0.0;
        assert!(state[STATE_HOLDOFF] > 0.0, "holdoff should still be armed");
        let burst = sine(1000.0, 0.5, 100);
        render(&mut state, &burst, &burst);
        assert_eq!(
            state[STATE_TRIG_COUNT], 1.0,
            "burst inside holdoff must not retrigger"
        );

        // Well past the holdoff with the detector re-armed: fires again.
        let silence = vec![0.0_f32; 4000];
        render(&mut state, &silence, &silence);
        state[STATE_SENSE_ENV] = 0.0;
        state[STATE_SENSE_PREV] = 0.0;
        state[STATE_GATE] = 0.0;
        let burst = sine(1000.0, 0.5, 300);
        render(&mut state, &burst, &burst);
        assert_eq!(
            state[STATE_TRIG_COUNT], 2.0,
            "a later burst should fire a second trigger"
        );
    }

    // ── 5. harmonics link: F2 = F1 ÷ ratio under an env sweep ──
    #[test]
    fn harmonics_link_tracks_ratio_under_env_sweep() {
        let mut state = init_state();
        state[STATE_ENABLED] = 1.0;
        state[STATE_HARMONICS] = 3.0; // ratio 2.0
        state[STATE_SENSE] = 50.0;
        state[STATE_ENV_F1] = 100.0;
        state[STATE_ATTACK_MS] = 30.0;
        state[STATE_DECAY_MS] = 300.0;
        state[STATE_SUSTAIN] = 50.0;
        state[STATE_RES_BLEED] = 0.0;

        let input = sine(200.0, 0.6, 128);
        let mut checked = 0;
        let mut max_f1 = 0.0_f32;
        for _ in 0..64 {
            render(&mut state, &input, &input);
            let f1 = state[FILTERBANK_METER_F1_HZ];
            let f2 = state[FILTERBANK_METER_F2_HZ];
            max_f1 = max_f1.max(f1);
            if state[FILTERBANK_METER_GATE] > 0.5 && state[FILTERBANK_METER_ENV] > 0.05 {
                let expected = (f1 / 2.0).clamp(20.0, SR * 0.45);
                assert!(
                    (f2 - expected).abs() / expected < 1.0e-3,
                    "linked F2 {f2} Hz should equal F1/2 = {expected} Hz"
                );
                checked += 1;
            }
        }
        assert!(checked > 10, "env sweep never engaged ({checked} blocks)");
        assert!(
            max_f1 > 1_500.0,
            "envelope should have swept F1 well above rest ({max_f1} Hz)"
        );
    }

    // ── 6. AM depth 100 + sine modulator = ring mod, suppressed carrier ──
    #[test]
    fn full_am_depth_suppresses_carrier() {
        let run = |am_depth: f32| -> Vec<f32> {
            let mut state = init_state();
            state[STATE_ENABLED] = 1.0;
            state[STATE_F1_FREQ] = 16_000.0;
            state[STATE_F2_FREQ] = 16_000.0;
            state[STATE_SM_F1_FREQ] = 16_000.0;
            state[STATE_SM_F2_FREQ] = 16_000.0;
            state[STATE_SENSE] = 0.0; // don't trigger the envelope
            state[STATE_ENV_F1] = 0.0;
            state[STATE_ENV_F2] = 0.0;
            state[STATE_SM_ENV_F1] = 0.0;
            state[STATE_SM_ENV_F2] = 0.0;
            state[STATE_RES_BLEED] = 0.0;
            state[STATE_AM_DEPTH] = am_depth;
            state[STATE_SM_AM] = am_depth / 100.0;
            state[STATE_AM_EXT_ACTIVE] = 1.0;
            let n = (SR * 0.3) as usize;
            let carrier = sine(1000.0, 0.5, n);
            let modulator = sine(250.0, 0.9, n);
            let z = vec![0.0_f32; n];
            let (out_l, _) = process_block(
                &mut state,
                &[&carrier, &carrier, &z, &z, &z, &z, &z, &modulator],
            );
            out_l[n - 4800..].to_vec()
        };
        let base = tone_magnitude(&run(0.0), 1000.0);
        let ringed = tone_magnitude(&run(100.0), 1000.0);
        assert!(base > 0.01, "carrier should pass with AM off ({base})");
        assert!(
            ringed < base * 0.15,
            "ring mod should suppress the carrier: {ringed} vs {base}"
        );
    }

    // ── 7. feedback 100% + impulse stays bounded ──
    #[test]
    fn full_feedback_impulse_stays_bounded() {
        let mut state = init_state();
        state[STATE_ENABLED] = 1.0;
        state[STATE_FEEDBACK] = 100.0;
        state[STATE_SM_FEEDBACK] = 1.0;
        state[STATE_F1_RES] = 80.0;
        state[STATE_F2_RES] = 80.0;
        state[STATE_CRUNCH] = 50.0;
        let n = SR as usize;
        let mut in_l = vec![0.0_f32; n];
        in_l[0] = 1.0;
        let in_r = in_l.clone();
        let (out_l, out_r) = render(&mut state, &in_l, &in_r);
        for v in out_l.iter().chain(out_r.iter()) {
            assert!(v.is_finite(), "non-finite sample: {v}");
            assert!(v.abs() < 16.0, "runaway sample: {v}");
        }
    }

    // ── 8. ext-mod: exponential freq target, linear am-depth target ──
    #[test]
    fn ext_mod_moves_freq_exponentially_and_am_linearly() {
        // DC 1.0 on port 2 with full f1-freq depth: +4 octaves → 500 → 8000.
        let mut state = init_state();
        state[STATE_ENABLED] = 1.0;
        state[STATE_SENSE] = 0.0;
        state[STATE_MOD_F1_FREQ_DEPTH_1] = 1.0;
        let n = 4096;
        let z = vec![0.0_f32; n];
        let dc = vec![1.0_f32; n];
        process_block(&mut state, &[&z, &z, &dc, &z, &z, &z, &z, &z]);
        let f1 = state[FILTERBANK_METER_F1_HZ];
        assert!(
            (f1 - 8_000.0).abs() < 400.0,
            "full-depth DC should move F1 to 500·2^4 = 8 kHz, got {f1}"
        );

        // Linear target: DC-driven am depth (knob at 0) with a silent ext AM
        // sidechain rings the wet signal against zero → output collapses.
        let run = |depth: f32| -> f32 {
            let mut state = init_state();
            state[STATE_ENABLED] = 1.0;
            state[STATE_F1_FREQ] = 16_000.0;
            state[STATE_F2_FREQ] = 16_000.0;
            state[STATE_SM_F1_FREQ] = 16_000.0;
            state[STATE_SM_F2_FREQ] = 16_000.0;
            state[STATE_SENSE] = 0.0;
            state[STATE_ENV_F1] = 0.0;
            state[STATE_ENV_F2] = 0.0;
            state[STATE_SM_ENV_F1] = 0.0;
            state[STATE_SM_ENV_F2] = 0.0;
            state[STATE_AM_EXT_ACTIVE] = 1.0;
            state[STATE_MOD_AM_DEPTH_1] = depth;
            let n = (SR * 0.2) as usize;
            let carrier = sine(1000.0, 0.5, n);
            let dc = vec![1.0_f32; n];
            let z = vec![0.0_f32; n];
            let (out_l, _) =
                process_block(&mut state, &[&carrier, &carrier, &dc, &z, &z, &z, &z, &z]);
            rms(&out_l[n / 2..])
        };
        let base = run(0.0);
        let modded = run(1.0);
        assert!(base > 0.05, "carrier should pass unmodulated ({base})");
        assert!(
            modded < base * 0.1,
            "full am-depth mod with silent ext AM should mute wet: {modded} vs {base}"
        );
    }

    // ── 8b. stability canary: res > 100% with the cutoff modulated to the
    // top of its range puts the per-sample branch in linearly-unstable
    // territory (g·k < −1). The −0.9/g floor in svf_core guarantees
    // contraction there; this locks the worst reachable settings to a
    // finite, bounded output. ──
    #[test]
    fn max_res_with_pinned_high_cutoff_stays_finite() {
        let mut state = init_state();
        state[STATE_ENABLED] = 1.0;
        state[STATE_SENSE] = 0.0;
        state[STATE_F1_FREQ] = 16_000.0;
        state[STATE_SM_F1_FREQ] = 16_000.0;
        state[STATE_F1_RES] = 110.0;
        state[STATE_SM_F1_RES] = 110.0;
        state[STATE_F2_RES] = 110.0;
        state[STATE_SM_F2_RES] = 110.0;
        state[STATE_INPUT_DB] = 30.0;
        state[STATE_FEEDBACK] = 100.0;
        // Full-depth DC ext-mod pins the effective cutoff at its clamp,
        // forcing the per-sample branch with the largest reachable g.
        state[STATE_MOD_F1_FREQ_DEPTH_1] = 1.0;
        state[STATE_MOD_F2_FREQ_DEPTH_1] = 1.0;
        let n = 512;
        let dc = vec![1.0_f32; n];
        for block in 0..96 {
            let x = sine(220.0, 1.0, n);
            let (out_l, out_r) = process_block(&mut state, &[&x, &x, &dc, &dc, &dc, &dc, &dc, &dc]);
            for (i, (&l, &r)) in out_l.iter().zip(out_r.iter()).enumerate() {
                assert!(
                    l.is_finite() && r.is_finite() && l.abs() < 100.0 && r.abs() < 100.0,
                    "output diverged at block {block} sample {i}: l={l} r={r}"
                );
            }
        }
        for (idx, &v) in state.iter().enumerate() {
            assert!(v.is_finite(), "state slot {idx} went non-finite: {v}");
        }
    }

    // ── 8c. regression: clock bleed must not sound with no program
    // material (it rides the sense envelope), even at full crunch with the
    // clock deep in the audible band ──
    #[test]
    fn silent_input_stays_silent_at_full_crunch() {
        let mut state = init_state();
        state[STATE_ENABLED] = 1.0;
        state[STATE_CRUNCH] = 100.0;
        state[STATE_SM_CRUNCH] = 100.0;
        state[STATE_F1_FREQ] = 271.0;
        state[STATE_SM_F1_FREQ] = 271.0;
        state[STATE_F2_FREQ] = 300.0;
        state[STATE_SM_F2_FREQ] = 300.0;
        let n = (SR * 0.5) as usize;
        let z = vec![0.0_f32; n];
        let (out_l, out_r) = render(&mut state, &z, &z);
        let (l, r) = (rms(&out_l), rms(&out_r));
        assert!(
            l < 1.0e-5 && r < 1.0e-5,
            "idle output must be silent, got rms l={l} r={r}"
        );

        // And bleed still engages with signal present: a loud low tone
        // through the same settings carries audible clock content (output
        // has energy the input lacks above the filter band).
        let x = sine(80.0, 0.8, n);
        let (out_l, _) = render(&mut state, &x, &x);
        assert!(rms(&out_l) > 1.0e-3, "bleed path should pass signal");
    }

    // ── 8d. LFO tempo sync: one cycle per division at host BPM ──
    #[test]
    fn lfo_sync_rate_follows_bpm_and_division() {
        // Free mode passes the knob through.
        assert_eq!(lfo_rate_hz(0.0, 6.0, 120.0, 3.3), 3.3);
        // 1/4 (idx 6, 1 beat) at 120 BPM = 2 Hz.
        assert!((lfo_rate_hz(1.0, 6.0, 120.0, 3.3) - 2.0).abs() < 1e-4);
        // "1" (idx 10, 4 beats) at 120 BPM = 0.5 Hz.
        assert!((lfo_rate_hz(1.0, 10.0, 120.0, 3.3) - 0.5).abs() < 1e-4);
        // 1/32 (idx 0) at 174 BPM = 174/60/0.125 = 23.2 Hz.
        assert!((lfo_rate_hz(1.0, 0.0, 174.0, 3.3) - 23.2).abs() < 1e-3);
        // Garbage BPM/div fall back sanely.
        assert!(lfo_rate_hz(1.0, 99.0, f32::NAN, 3.3).is_finite());
    }

    // ── 9. bypass is bit-exact; re-enable resets state without a click ──
    #[test]
    fn bypass_passes_bit_exact_and_reenable_is_clean() {
        let mut state = init_state();
        state[STATE_ENABLED] = 0.0;
        // Dirty some runtime state so the bypass reset has work to do.
        state[STATE_F1L] = 3.0;
        state[STATE_ENV_VALUE] = 0.7;
        state[STATE_FB_PREV_L] = 2.0;
        let n = 1024;
        let in_l = sine(440.0, 0.8, n);
        let in_r = sine(333.0, 0.6, n);
        let (out_l, out_r) = render(&mut state, &in_l, &in_r);
        assert_eq!(out_l, in_l, "bypass must be bit-exact (L)");
        assert_eq!(out_r, in_r, "bypass must be bit-exact (R)");
        state[STATE_LFO_SYNC] = 1.0;
        state[STATE_LFO_DIV] = 3.0;
        state[STATE_BPM] = 174.0;
        let (_, _) = render(&mut state, &in_l, &in_r);
        for idx in FIRST_RUNTIME_RESET..RUNTIME_RESET_END {
            if idx == STATE_METER_INPUT_DB {
                continue; // parked at -90 dB, not zero
            }
            assert_eq!(state[idx], 0.0, "runtime slot {idx} not reset by bypass");
        }
        // Param slots appended past the reset span must survive bypass.
        assert_eq!(state[STATE_LFO_SYNC], 1.0);
        assert_eq!(state[STATE_LFO_DIV], 3.0);
        assert_eq!(state[STATE_BPM], 174.0);

        // Re-enable: starts from silence, no stale-state click.
        state[STATE_ENABLED] = 1.0;
        let (out_l, out_r) = render(&mut state, &in_l, &in_r);
        for v in out_l.iter().chain(out_r.iter()) {
            assert!(v.is_finite());
            assert!(v.abs() < 2.0, "re-enable click: {v}");
        }
        assert!(rms(&out_l) > 0.01, "re-enabled effect should pass audio");
    }
}
