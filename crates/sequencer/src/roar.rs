//! Roar: multi-stage coloring / saturation builtin (Ableton Live 12 style).
//!
//! Up to three identical waveshaper stages arranged by one of seven routing
//! topologies (single / serial / parallel / multiband / mid-side / feedback /
//! delay). Each stage: optional pre filter -> amount drive -> bias -> shaper
//! (12 curves, 2x oversampled) -> DC block -> optional post filter -> level.
//! Globals: input Drive, Tone tilt/shelf EQ, a feedback network (delay +
//! stage-2 return + bandpass + soft limiter, audible in the Feedback/Delay
//! routings), a one-knob output compressor with sidechain HPF, output trim
//! and equal-power dry/wet. The dry tap is the device input (pre-Drive).
//!
//! The state tail carries display meters (per-stage pre-shaper min/max of the
//! driven signal for the shaper-view drive-region overlay, and post-stage
//! output dB) that the UI polls via the node-state watchlist, like `ott.rs`.

use crate::audiograph::NodeVTable;
use crate::ott::{butterworth, split_bands};
use std::os::raw::{c_int, c_void};

pub const NUM_STAGES: usize = 3;
pub const NUM_SHAPERS: usize = 12;
pub const NUM_FILTERS: usize = 9;

// Feedback delay line, power of two so reads wrap with a mask. 65536 samples
// is ~1.36 s at 48 kHz; longer synced divisions clamp to the buffer.
const FB_BUF_LEN: usize = 65536;
const FB_BUF_MASK: usize = FB_BUF_LEN - 1;
// Per-stage-channel comb filter line (down to ~12 Hz at 48 kHz).
const COMB_BUF_LEN: usize = 4096;

// Routing modes.
pub const ROUTING_SINGLE: usize = 0;
pub const ROUTING_SERIAL: usize = 1;
pub const ROUTING_PARALLEL: usize = 2;
pub const ROUTING_MULTIBAND: usize = 3;
pub const ROUTING_MID_SIDE: usize = 4;
pub const ROUTING_FEEDBACK: usize = 5;
pub const ROUTING_DELAY: usize = 6;

// Shaper curves (see `shaper_transfer`).
pub const SHAPER_SOFT_SINE: usize = 0;
pub const SHAPER_DIGITAL_CLIP: usize = 1;
pub const SHAPER_BIT_CRUSHER: usize = 2;
pub const SHAPER_DIODE: usize = 3;
pub const SHAPER_TUBE: usize = 4;
pub const SHAPER_HALF_WAVE: usize = 5;
pub const SHAPER_FULL_WAVE: usize = 6;
pub const SHAPER_POLYNOMIAL: usize = 7;
pub const SHAPER_FRACTAL: usize = 8;
pub const SHAPER_TRI_FOLD: usize = 9;
pub const SHAPER_NOISE: usize = 10;
pub const SHAPER_SHARDS: usize = 11;

// Stage filter types.
pub const FILTER_LP: usize = 0;
pub const FILTER_BP: usize = 1;
pub const FILTER_HP: usize = 2;
pub const FILTER_NOTCH: usize = 3;
pub const FILTER_PEAK: usize = 4;
pub const FILTER_MORPH: usize = 5;
pub const FILTER_COMB: usize = 6;
pub const FILTER_RESAMPLE: usize = 7;
pub const FILTER_DISPERSION: usize = 8;

// Same division table as Str8 Delay / Space Echo / Phaser-Flanger so the UI
// labels match.
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

// ── Parameter slots ──
const STATE_ENABLED: usize = 0;
const STATE_DRIVE: usize = 1; // dB, -12..36
const STATE_TONE: usize = 2; // -1..1
const STATE_TONE_FREQ: usize = 3; // Hz
const STATE_TONE_MODE: usize = 4; // 0 tilt, 1 shelf
const STATE_ROUTING: usize = 5; // 0..6
const STATE_BLEND: usize = 6; // 0..1 (parallel crossfade / M-S balance)
const STATE_XOVER_LOW: usize = 7; // Hz
const STATE_XOVER_HIGH: usize = 8; // Hz
const STATE_FB_MODE: usize = 9; // 0 time, 1 note
const STATE_FB_TIME: usize = 10; // ms
const STATE_FB_DIV: usize = 11; // SYNC_BEATS index
const STATE_FB_AMOUNT: usize = 12; // 0..1
const STATE_FB_INVERT: usize = 13;
const STATE_FB_DUCK: usize = 14;
const STATE_FB_FREQ: usize = 15; // Hz (bandpass center)
const STATE_FB_WIDTH: usize = 16; // octaves
const STATE_COMPRESS: usize = 17; // 0..1
const STATE_SC_HPF: usize = 18;
const STATE_OUTPUT: usize = 19; // dB
const STATE_MIX: usize = 20; // 0..1
const STATE_SAMPLE_RATE: usize = 21;
const STATE_BPM: usize = 22;

// Per-stage parameter block.
const STATE_STAGE_BASE: usize = 23;
const STAGE_STRIDE: usize = 8;
const STAGE_SHAPER: usize = 0;
const STAGE_AMOUNT: usize = 1;
const STAGE_BIAS: usize = 2;
const STAGE_LEVEL: usize = 3; // dB
const STAGE_FILTER: usize = 4;
const STAGE_FREQ: usize = 5; // Hz
const STAGE_RES: usize = 6; // 0..1
const STAGE_PRE: usize = 7;

// Host-modulation depth slots (4 modulator slots × 4 targets).
const STATE_MOD_DRIVE_DEPTH_1: usize = 47; // dB
const STATE_MOD_TONE_DEPTH_1: usize = 51;
const STATE_MOD_FB_AMOUNT_DEPTH_1: usize = 55;
const STATE_MOD_MIX_DEPTH_1: usize = 59;

// ── Runtime state ──
const STATE_SM_DRIVE: usize = 63; // linear amp
const STATE_SM_TONE_LO: usize = 64; // low-leg gain (amp)
const STATE_SM_TONE_HI: usize = 65; // high-leg gain (amp)
const STATE_SM_OUTPUT: usize = 66; // linear amp
const STATE_SM_MIX: usize = 67;
const STATE_SM_BLEND: usize = 68;
const STATE_SM_FB_AMOUNT: usize = 69; // signed (invert folds in)
const STATE_SM_FB_DELAY: usize = 70; // samples
const STATE_SM_COMPRESS: usize = 71;
const STATE_SM_FB_HP_COEF: usize = 72;
const STATE_SM_FB_LP_COEF: usize = 73;
// Per-stage smoothers.
const STATE_SM_STAGE_BASE: usize = 74;
const SM_STAGE_STRIDE: usize = 6;
const SM_STAGE_GAIN: usize = 0; // pre-shaper drive (linear)
const SM_STAGE_BIAS: usize = 1;
const SM_STAGE_LEVEL: usize = 2; // linear amp
const SM_STAGE_ENGAGE: usize = 3; // shaper dry/wet (null at amount 0)
const SM_STAGE_G: usize = 4; // SVF g
const SM_STAGE_K: usize = 5; // SVF damping 1/Q

// Display meters (state tail contract with the UI).
pub const STATE_METER_PRE: usize = 92; // 3 × (min, max) of driven signal, linear
pub const STATE_METER_POST_DB: usize = 98; // 3 × post-stage out dB
const STATE_TONE_LP_L: usize = 101;
const STATE_TONE_LP_R: usize = 102;
const STATE_FB_WPOS: usize = 103;
const STATE_DUCK_ENV: usize = 104;
const STATE_COMP_ENV: usize = 105;
const STATE_SC_Z_L: usize = 106;
const STATE_SC_Z_R: usize = 107;
const STATE_FB_BP_Z: usize = 108; // hp_l, lp_l, hp_r, lp_r
const STATE_CFG_ROUTING: usize = 112;
const STATE_CFG_SHAPER: usize = 113; // 3
const STATE_CFG_FILTER: usize = 116; // 3
const STATE_CFG_PRE: usize = 119; // 3
const STATE_XOVER_Z: usize = 122; // 2 ch × 20
                                  // Per-stage-per-channel runtime blocks (index = stage * 2 + channel).
const STATE_STAGE_RT: usize = 162;
const STAGE_RT_STRIDE: usize = 64;
const RT_SVF_IC1: usize = 0;
const RT_SVF_IC2: usize = 1;
const RT_AP_BASE: usize = 2; // 4 × (x1, y1) dispersion allpasses
const RT_HOLD_VAL: usize = 10;
const RT_HOLD_PHASE: usize = 11;
const RT_DC_X: usize = 12;
const RT_DC_Y: usize = 13;
const RT_RNG: usize = 14;
const RT_SHARD_HOLD: usize = 15;
const RT_SHARD_COUNT: usize = 16;
const RT_COMB_POS: usize = 17;
const RT_UP_RING: usize = 18; // 16
const RT_DOWN_RING: usize = 34; // 16
const RT_OS_POS: usize = 50;

const STATE_FB_BUF_L: usize = STATE_STAGE_RT + 6 * STAGE_RT_STRIDE;
const STATE_FB_BUF_R: usize = STATE_FB_BUF_L + FB_BUF_LEN;
const STATE_COMB_BUF: usize = STATE_FB_BUF_R + FB_BUF_LEN; // 6 × COMB_BUF_LEN

pub const ROAR_STATE_SIZE: usize = STATE_COMB_BUF + 6 * COMB_BUF_LEN;

pub const ROAR_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const ROAR_PARAM_DRIVE: u64 = STATE_DRIVE as u64;
pub const ROAR_PARAM_TONE: u64 = STATE_TONE as u64;
pub const ROAR_PARAM_TONE_FREQ: u64 = STATE_TONE_FREQ as u64;
pub const ROAR_PARAM_TONE_MODE: u64 = STATE_TONE_MODE as u64;
pub const ROAR_PARAM_ROUTING: u64 = STATE_ROUTING as u64;
pub const ROAR_PARAM_BLEND: u64 = STATE_BLEND as u64;
pub const ROAR_PARAM_XOVER_LOW: u64 = STATE_XOVER_LOW as u64;
pub const ROAR_PARAM_XOVER_HIGH: u64 = STATE_XOVER_HIGH as u64;
pub const ROAR_PARAM_FB_MODE: u64 = STATE_FB_MODE as u64;
pub const ROAR_PARAM_FB_TIME: u64 = STATE_FB_TIME as u64;
pub const ROAR_PARAM_FB_DIV: u64 = STATE_FB_DIV as u64;
pub const ROAR_PARAM_FB_AMOUNT: u64 = STATE_FB_AMOUNT as u64;
pub const ROAR_PARAM_FB_INVERT: u64 = STATE_FB_INVERT as u64;
pub const ROAR_PARAM_FB_DUCK: u64 = STATE_FB_DUCK as u64;
pub const ROAR_PARAM_FB_FREQ: u64 = STATE_FB_FREQ as u64;
pub const ROAR_PARAM_FB_WIDTH: u64 = STATE_FB_WIDTH as u64;
pub const ROAR_PARAM_COMPRESS: u64 = STATE_COMPRESS as u64;
pub const ROAR_PARAM_SC_HPF: u64 = STATE_SC_HPF as u64;
pub const ROAR_PARAM_OUTPUT: u64 = STATE_OUTPUT as u64;
pub const ROAR_PARAM_MIX: u64 = STATE_MIX as u64;
pub const ROAR_PARAM_BPM: u64 = STATE_BPM as u64;
pub const ROAR_PARAM_MOD_DRIVE_DEPTH_1: u64 = STATE_MOD_DRIVE_DEPTH_1 as u64;
pub const ROAR_PARAM_MOD_TONE_DEPTH_1: u64 = STATE_MOD_TONE_DEPTH_1 as u64;
pub const ROAR_PARAM_MOD_FB_AMOUNT_DEPTH_1: u64 = STATE_MOD_FB_AMOUNT_DEPTH_1 as u64;
pub const ROAR_PARAM_MOD_MIX_DEPTH_1: u64 = STATE_MOD_MIX_DEPTH_1 as u64;

#[derive(Clone, Copy)]
pub enum RoarStageField {
    Shaper = STAGE_SHAPER as isize,
    Amount = STAGE_AMOUNT as isize,
    Bias = STAGE_BIAS as isize,
    Level = STAGE_LEVEL as isize,
    Filter = STAGE_FILTER as isize,
    Freq = STAGE_FREQ as isize,
    Res = STAGE_RES as isize,
    Pre = STAGE_PRE as isize,
}

pub const fn roar_stage_param(stage: usize, field: RoarStageField) -> u64 {
    (STATE_STAGE_BASE + stage * STAGE_STRIDE + field as usize) as u64
}

// Amount 0..1 maps to 1×..64× pre-shaper gain (exp law).
const AMOUNT_GAIN_OCT: f32 = 6.0;
// Loop gain ceiling: the tanh soft limiter in the loop keeps full feedback
// screaming instead of diverging.
const FB_AMOUNT_MAX: f32 = 0.98;
// Stage DC blocker pole (~10 Hz).
const DC_BLOCK_HZ: f32 = 10.0;
// Meter ballistics (amplitude domain, like ott.rs).
const METER_ATTACK_MS: f32 = 5.0;
const METER_RELEASE_MS: f32 = 250.0;
// One-knob compressor: fixed ratio, program-material ballistics.
const COMP_RATIO: f32 = 4.0;
const COMP_ATTACK_MS: f32 = 5.0;
const COMP_RELEASE_MS: f32 = 120.0;
const COMP_MAX_THR_DB: f32 = -30.0;
const SC_HPF_HZ: f32 = 120.0;

// 11-tap halfband lowpass for the 2× oversampled shaper path. Even taps are
// the polyphase interpolator; the run is short enough to convolve directly on
// the zero-stuffed stream.
const HALFBAND: [f32; 11] = [
    0.006028, 0.0, -0.051070, 0.0, 0.297044, 0.5, 0.297044, 0.0, -0.051070, 0.0, 0.006028,
];
const OS_RING_MASK: usize = 15;

#[inline]
fn db_to_amp(db: f32) -> f32 {
    (10.0_f32).powf(db / 20.0)
}

#[inline]
fn amp_to_db(amp: f32) -> f32 {
    20.0 * amp.max(1.0e-9).log10()
}

#[inline]
fn one_pole_coef(freq: f32, sr: f32) -> f32 {
    1.0 - (-std::f32::consts::TAU * freq / sr.max(1.0)).exp()
}

#[inline]
fn time_coef(ms: f32, sr: f32) -> f32 {
    1.0 - (-1.0 / (ms.max(0.01) * 0.001 * sr.max(1.0))).exp()
}

#[inline]
fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

#[inline]
fn finite_clamp(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    finite_or(value, fallback).clamp(min, max)
}

/// Deterministic 24-bit LCG noise, exactly representable in f32 state (same
/// generator as phaser_flanger's doubler drift).
#[inline]
fn next_noise(state: &mut f32) -> f32 {
    const MASK: u32 = (1 << 24) - 1;
    let seed = (*state as u32) & MASK;
    let next = seed.wrapping_mul(1_140_671_485).wrapping_add(12_820_163) & MASK;
    *state = next as f32;
    next as f32 * (2.0 / MASK as f32) - 1.0
}

#[inline]
fn soft_sine(x: f32) -> f32 {
    x.clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2)
        .sin()
}

/// Stateless transfer curve for one shaper. `amount` is the stage Amount
/// (0..1) for curves that use it as an extra shape control. Every curve is
/// normalized so the small-signal slope at the origin is ~1 (the rectifiers
/// are the deliberate exceptions). The `roar-shaper` UI widget dual-maintains
/// these formulas in its fragment shader — keep the two in sync.
pub fn shaper_transfer(shaper: usize, amount: f32, x: f32) -> f32 {
    let a = amount.clamp(0.0, 1.0);
    match shaper {
        SHAPER_DIGITAL_CLIP => x.clamp(-1.0, 1.0),
        SHAPER_BIT_CRUSHER => {
            let levels = 64.0 + (2.0 - 64.0) * a;
            (x * levels).round() / levels
        }
        SHAPER_DIODE => {
            // Asymmetric: exponential knee past the forward drop on the
            // positive half, softer tanh on the negative half.
            if x >= 0.0 {
                let t = 0.35;
                if x <= t {
                    x
                } else {
                    t + (1.0 - (-(x - t) * 3.0).exp()) / 3.0
                }
            } else {
                1.2 * (x / 1.2).tanh()
            }
        }
        SHAPER_TUBE => {
            // Second-harmonic warmth; the DC created on asymmetric material
            // is removed by the stage DC blocker. Clamp keeps the even term
            // monotonic for hot negative excursions.
            let u = x.max(-2.4);
            (u + 0.2 * u * u).tanh()
        }
        SHAPER_HALF_WAVE => 2.0 * x.max(0.0),
        SHAPER_FULL_WAVE => x.abs(),
        SHAPER_POLYNOMIAL => {
            let cheb = (3.0 * x - 4.0 * x * x * x) / 3.0;
            ((1.0 - a) * x + a * cheb).clamp(-1.0, 1.0)
        }
        SHAPER_FRACTAL => {
            let mut y = x;
            for k in [1.9_f32, 1.5, 1.2] {
                y = (k * y).sin() / k;
            }
            y
        }
        SHAPER_TRI_FOLD => {
            let t = ((x + 1.0) * 0.25).rem_euclid(1.0);
            1.0 - 4.0 * (t - 0.5).abs()
        }
        SHAPER_NOISE | SHAPER_SHARDS => soft_sine(x),
        _ => soft_sine(x),
    }
}

/// Runtime shaper: the stateless transfer plus the stateful glitch/noise
/// layers, ticked once per (oversampled) sample.
#[inline]
unsafe fn shaper_sample(shaper: usize, amount: f32, x: f32, rt: *mut f32) -> f32 {
    let base = shaper_transfer(shaper, amount, x);
    match shaper {
        SHAPER_NOISE => {
            let mut rng = *rt.add(RT_RNG);
            if rng == 0.0 {
                rng = 0x51_7c_c1 as f32;
            }
            let n = next_noise(&mut rng);
            *rt.add(RT_RNG) = rng;
            base + amount * 0.5 * n * x.abs().min(2.0)
        }
        SHAPER_SHARDS => {
            // Sample-hold discontinuities: every short segment the output
            // freezes to a coarsely quantized copy of the signal, gated by
            // deterministic noise so the glitches track Amount.
            let mut count = *rt.add(RT_SHARD_COUNT);
            count -= 1.0;
            if count <= 0.0 {
                let mut rng = *rt.add(RT_RNG);
                if rng == 0.0 {
                    rng = 0xa3_42_19 as f32;
                }
                let n = next_noise(&mut rng);
                *rt.add(RT_RNG) = rng;
                let gate = if n > 1.0 - 1.6 * amount { 1.0 } else { 0.0 };
                *rt.add(RT_SHARD_HOLD) = gate * (base * 4.0).round() * 0.25;
                count = 24.0 + (n * 20.0).abs();
            }
            *rt.add(RT_SHARD_COUNT) = count;
            let hold = *rt.add(RT_SHARD_HOLD);
            if hold != 0.0 {
                base + amount * (hold - base)
            } else {
                base
            }
        }
        _ => base,
    }
}

/// Res 0..1 → filter Q 0.5..~12 (log law).
#[inline]
pub fn res_to_q(res: f32) -> f32 {
    0.5 * (24.0_f32).powf(res.clamp(0.0, 1.0))
}

/// Everything the per-sample stage kernel needs, refreshed at the control
/// tick.
#[derive(Clone, Copy, Default)]
struct StageCtx {
    shaper: usize,
    filter: usize,
    pre: bool,
    freq: f32,
    res: f32,
    amount: f32,
    // Dispersion allpass coefficients (spread by res).
    ap_coef: [f32; 4],
    comb_delay: f32,
    hold_interval: f32,
}

/// One channel of one stage's filter, selected by type. `rt` points at the
/// stage-channel runtime block; comb lines live in their own arena.
#[inline]
unsafe fn stage_filter_sample(
    ctx: &StageCtx,
    g: f32,
    k: f32,
    x: f32,
    rt: *mut f32,
    comb: *mut f32,
) -> f32 {
    match ctx.filter {
        FILTER_COMB => {
            let pos = (*rt.add(RT_COMB_POS)) as usize % COMB_BUF_LEN;
            let delay = (ctx.comb_delay as usize).clamp(2, COMB_BUF_LEN - 2);
            let read = *comb.add((pos + COMB_BUF_LEN - delay) % COMB_BUF_LEN);
            let fb = ctx.res * 0.9;
            let w = x + fb * read;
            *comb.add(pos) = w;
            *rt.add(RT_COMB_POS) = ((pos + 1) % COMB_BUF_LEN) as f32;
            0.5 * (w + read)
        }
        FILTER_RESAMPLE => {
            if ctx.hold_interval <= 1.001 {
                return x;
            }
            let mut phase = *rt.add(RT_HOLD_PHASE) + 1.0;
            if phase >= ctx.hold_interval {
                phase -= ctx.hold_interval;
                let regen = ctx.res * 0.85;
                *rt.add(RT_HOLD_VAL) = (x - regen * *rt.add(RT_HOLD_VAL)).clamp(-4.0, 4.0);
            }
            *rt.add(RT_HOLD_PHASE) = phase;
            *rt.add(RT_HOLD_VAL)
        }
        FILTER_DISPERSION => {
            let mut y = x;
            for section in 0..4 {
                let c = ctx.ap_coef[section];
                let z = rt.add(RT_AP_BASE + section * 2);
                let x1 = *z;
                let y1 = *z.add(1);
                let out = c * (y - y1) + x1;
                *z = y;
                *z.add(1) = out;
                y = out;
            }
            y
        }
        _ => {
            // SVF core (Simper), mode-selected output.
            let ic1 = *rt.add(RT_SVF_IC1);
            let ic2 = *rt.add(RT_SVF_IC2);
            let a1 = 1.0 / (1.0 + g * (g + k));
            let a2 = g * a1;
            let a3 = g * a2;
            let v3 = x - ic2;
            let v1 = a1 * ic1 + a2 * v3;
            let v2 = ic2 + a2 * ic1 + a3 * v3;
            *rt.add(RT_SVF_IC1) = 2.0 * v1 - ic1;
            *rt.add(RT_SVF_IC2) = 2.0 * v2 - ic2;
            let lp = v2;
            let bp = v1;
            let hp = x - k * v1 - v2;
            match ctx.filter {
                FILTER_BP => bp,
                FILTER_HP => hp,
                FILTER_NOTCH => lp + hp,
                FILTER_PEAK => lp - hp,
                // Fixed BP-leaning morph position (flagged for iteration).
                FILTER_MORPH => 0.3 * lp + 0.4 * k * bp + 0.3 * hp,
                _ => lp,
            }
        }
    }
}

/// Full per-channel stage kernel: [pre filter] -> drive -> bias -> shaper
/// (2× oversampled) -> DC block -> [post filter] -> level. Returns the stage
/// output and records the pre-shaper drive-region extremes for the meters.
#[allow(clippy::too_many_arguments)]
#[inline]
unsafe fn stage_sample(
    ctx: &StageCtx,
    s: *mut f32,
    stage: usize,
    ch: usize,
    x: f32,
    dc_r: f32,
    extremes: &mut [f32; 2],
) -> f32 {
    let block = stage * 2 + ch;
    let rt = s.add(STATE_STAGE_RT + block * STAGE_RT_STRIDE);
    let comb = s.add(STATE_COMB_BUF + block * COMB_BUF_LEN);
    let sm = s.add(STATE_SM_STAGE_BASE + stage * SM_STAGE_STRIDE);
    let gain = *sm.add(SM_STAGE_GAIN);
    let bias = *sm.add(SM_STAGE_BIAS);
    let level = *sm.add(SM_STAGE_LEVEL);
    let engage = *sm.add(SM_STAGE_ENGAGE);
    let g = *sm.add(SM_STAGE_G);
    let k = *sm.add(SM_STAGE_K);

    let mut y = x;
    if ctx.pre {
        y = stage_filter_sample(ctx, g, k, y, rt, comb);
    }
    let filtered = y;

    let driven = filtered * gain + bias;
    if driven < extremes[0] {
        extremes[0] = driven;
    }
    if driven > extremes[1] {
        extremes[1] = driven;
    }

    let shaped = if ctx.shaper == SHAPER_BIT_CRUSHER {
        // Intentionally aliased: quantizers run at base rate.
        shaper_sample(ctx.shaper, ctx.amount, driven, rt)
    } else {
        // 2× oversample: zero-stuff + halfband up, shape both subsamples,
        // halfband down + decimate.
        let up = rt.add(RT_UP_RING);
        let down = rt.add(RT_DOWN_RING);
        let mut pos = (*rt.add(RT_OS_POS)) as usize & OS_RING_MASK;
        let mut out = 0.0;
        for (sub, input) in [(0usize, driven), (1usize, 0.0)] {
            *up.add(pos) = input;
            let mut u = 0.0;
            for (tap, &h) in HALFBAND.iter().enumerate() {
                u += h * *up.add((pos + 16 - tap) & OS_RING_MASK);
            }
            let w = shaper_sample(ctx.shaper, ctx.amount, 2.0 * u, rt);
            *down.add(pos) = w;
            if sub == 1 {
                let mut d = 0.0;
                for (tap, &h) in HALFBAND.iter().enumerate() {
                    d += h * *down.add((pos + 16 - tap) & OS_RING_MASK);
                }
                out = d;
            }
            pos = (pos + 1) & OS_RING_MASK;
        }
        *rt.add(RT_OS_POS) = pos as f32;
        out
    };

    // One-pole DC blocker (removes bias and rectifier/even-harmonic DC).
    let dc_y = shaped - *rt.add(RT_DC_X) + dc_r * *rt.add(RT_DC_Y);
    *rt.add(RT_DC_X) = shaped;
    *rt.add(RT_DC_Y) = dc_y;

    // Amount 0 keeps the stage null-clean: crossfade the whole nonlinear leg
    // back to the filtered signal.
    y = filtered + engage * (dc_y - filtered);

    if !ctx.pre {
        y = stage_filter_sample(ctx, g, k, y, rt, comb);
    }
    y * level
}

unsafe extern "C" fn roar_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    std::ptr::write_bytes(s, 0, ROAR_STATE_SIZE);
    *s.add(STATE_ENABLED) = 1.0;
    *s.add(STATE_DRIVE) = 0.0;
    *s.add(STATE_TONE) = 0.0;
    *s.add(STATE_TONE_FREQ) = 180.0;
    *s.add(STATE_TONE_MODE) = 0.0;
    *s.add(STATE_ROUTING) = ROUTING_SINGLE as f32;
    *s.add(STATE_BLEND) = 0.5;
    *s.add(STATE_XOVER_LOW) = 200.0;
    *s.add(STATE_XOVER_HIGH) = 2000.0;
    *s.add(STATE_FB_MODE) = 0.0;
    *s.add(STATE_FB_TIME) = 18.2;
    *s.add(STATE_FB_DIV) = 3.0; // "1/8"
    *s.add(STATE_FB_AMOUNT) = 0.0;
    *s.add(STATE_FB_FREQ) = 1000.0;
    *s.add(STATE_FB_WIDTH) = 8.0;
    *s.add(STATE_COMPRESS) = 0.0;
    *s.add(STATE_OUTPUT) = 0.0;
    *s.add(STATE_MIX) = 1.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    *s.add(STATE_BPM) = 120.0;
    for stage in 0..NUM_STAGES {
        let base = STATE_STAGE_BASE + stage * STAGE_STRIDE;
        *s.add(base + STAGE_SHAPER) = SHAPER_SOFT_SINE as f32;
        *s.add(base + STAGE_AMOUNT) = 0.0;
        *s.add(base + STAGE_BIAS) = 0.0;
        *s.add(base + STAGE_LEVEL) = 0.0;
        *s.add(base + STAGE_FILTER) = FILTER_LP as f32;
        *s.add(base + STAGE_FREQ) = 16000.0;
        *s.add(base + STAGE_RES) = 0.1;
        *s.add(base + STAGE_PRE) = 0.0;
        let sm = s.add(STATE_SM_STAGE_BASE + stage * SM_STAGE_STRIDE);
        *sm.add(SM_STAGE_GAIN) = 1.0;
        *sm.add(SM_STAGE_LEVEL) = 1.0;
    }
    *s.add(STATE_SM_DRIVE) = 1.0;
    *s.add(STATE_SM_TONE_LO) = 1.0;
    *s.add(STATE_SM_TONE_HI) = 1.0;
    *s.add(STATE_SM_OUTPUT) = 1.0;
    *s.add(STATE_SM_MIX) = 1.0;
    *s.add(STATE_SM_BLEND) = 0.5;
    *s.add(STATE_SM_FB_DELAY) = 0.0182 * sample_rate as f32;
    *s.add(STATE_CFG_ROUTING) = -1.0;
    for stage in 0..NUM_STAGES {
        *s.add(STATE_CFG_SHAPER + stage) = -1.0;
        *s.add(STATE_CFG_FILTER + stage) = -1.0;
        *s.add(STATE_CFG_PRE + stage) = -1.0;
    }
    for stage in 0..NUM_STAGES {
        *s.add(STATE_METER_POST_DB + stage) = -80.0;
    }
}

/// 4-point Hermite read of the feedback line.
#[inline]
unsafe fn fb_line_read(buf: *const f32, wpos: usize, delay: f32) -> f32 {
    let d = delay.clamp(2.0, (FB_BUF_LEN - 4) as f32);
    let read = wpos as f32 - d + FB_BUF_LEN as f32;
    let base = read.floor();
    let frac = read - base;
    let i0 = (base as usize + FB_BUF_LEN - 1) & FB_BUF_MASK;
    let xm1 = *buf.add(i0);
    let x0 = *buf.add((i0 + 1) & FB_BUF_MASK);
    let x1 = *buf.add((i0 + 2) & FB_BUF_MASK);
    let x2 = *buf.add((i0 + 3) & FB_BUF_MASK);
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * frac + c2) * frac + c1) * frac + x0
}

unsafe extern "C" fn roar_process(
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
    let out0 = *out.add(0);
    let out1 = *out.add(1);

    if *s.add(STATE_ENABLED) <= 0.5 {
        std::ptr::copy_nonoverlapping(in0 as *const f32, out0, nf);
        std::ptr::copy_nonoverlapping(in1 as *const f32, out1, nf);
        return;
    }

    let sr = finite_clamp(*s.add(STATE_SAMPLE_RATE), 1_000.0, 384_000.0, 48_000.0);
    let routing = finite_clamp(*s.add(STATE_ROUTING), 0.0, 6.0, 0.0).round() as usize;
    let tone_mode = finite_clamp(*s.add(STATE_TONE_MODE), 0.0, 1.0, 0.0).round() as usize;
    let drive_db_knob = finite_clamp(*s.add(STATE_DRIVE), -12.0, 36.0, 0.0);
    let tone_knob = finite_clamp(*s.add(STATE_TONE), -1.0, 1.0, 0.0);
    let tone_freq = finite_clamp(*s.add(STATE_TONE_FREQ), 50.0, 18_000.0, 180.0);
    let blend_target = finite_clamp(*s.add(STATE_BLEND), 0.0, 1.0, 0.5);
    let xover_low = finite_clamp(*s.add(STATE_XOVER_LOW), 40.0, 1_000.0, 200.0);
    let xover_high =
        finite_clamp(*s.add(STATE_XOVER_HIGH), 500.0, 10_000.0, 2_000.0).max(xover_low * 1.2);
    let fb_amount_knob = finite_clamp(*s.add(STATE_FB_AMOUNT), 0.0, 1.0, 0.0);
    let fb_sign = if *s.add(STATE_FB_INVERT) > 0.5 {
        -1.0
    } else {
        1.0
    };
    let fb_duck = *s.add(STATE_FB_DUCK) > 0.5;
    let fb_freq = finite_clamp(*s.add(STATE_FB_FREQ), 30.0, 18_000.0, 1_000.0);
    let fb_width = finite_clamp(*s.add(STATE_FB_WIDTH), 0.5, 9.0, 8.0);
    let compress_knob = finite_clamp(*s.add(STATE_COMPRESS), 0.0, 1.0, 0.0);
    let sc_hpf = *s.add(STATE_SC_HPF) > 0.5;
    let output_target = db_to_amp(finite_clamp(*s.add(STATE_OUTPUT), -24.0, 24.0, 0.0));
    let mix_knob = finite_clamp(*s.add(STATE_MIX), 0.0, 1.0, 1.0);

    // Feedback delay target in samples (Time or synced Note division).
    let fb_delay_target = if *s.add(STATE_FB_MODE) > 0.5 {
        let idx = (finite_clamp(*s.add(STATE_FB_DIV), 0.0, 10.0, 3.0).round() as usize)
            .min(SYNC_BEATS.len() - 1);
        let bpm = finite_clamp(*s.add(STATE_BPM), 20.0, 999.0, 120.0);
        SYNC_BEATS[idx] * 60.0 / bpm * sr
    } else {
        finite_clamp(*s.add(STATE_FB_TIME), 0.5, 1_000.0, 18.2) * 0.001 * sr
    }
    .clamp(2.0, (FB_BUF_LEN - 8) as f32);

    // Feedback bandpass: two one-pole skirts placed half the width above and
    // below the center frequency.
    let half_w = fb_width * 0.5;
    let fb_hp_coef_target =
        one_pole_coef((fb_freq * 0.5_f32.powf(half_w)).clamp(10.0, sr * 0.45), sr);
    let fb_lp_coef_target =
        one_pole_coef((fb_freq * 2.0_f32.powf(half_w)).clamp(10.0, sr * 0.45), sr);

    let drive_mod_amt = [
        finite_clamp(*s.add(STATE_MOD_DRIVE_DEPTH_1), -24.0, 24.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_DRIVE_DEPTH_1 + 1), -24.0, 24.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_DRIVE_DEPTH_1 + 2), -24.0, 24.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_DRIVE_DEPTH_1 + 3), -24.0, 24.0, 0.0),
    ];
    let tone_mod_amt = [
        finite_clamp(*s.add(STATE_MOD_TONE_DEPTH_1), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_TONE_DEPTH_1 + 1), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_TONE_DEPTH_1 + 2), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_TONE_DEPTH_1 + 3), -1.0, 1.0, 0.0),
    ];
    let fb_amount_mod_amt = [
        finite_clamp(*s.add(STATE_MOD_FB_AMOUNT_DEPTH_1), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_FB_AMOUNT_DEPTH_1 + 1), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_FB_AMOUNT_DEPTH_1 + 2), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_FB_AMOUNT_DEPTH_1 + 3), -1.0, 1.0, 0.0),
    ];
    let mix_mod_amt = [
        finite_clamp(*s.add(STATE_MOD_MIX_DEPTH_1), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_MIX_DEPTH_1 + 1), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_MIX_DEPTH_1 + 2), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_MIX_DEPTH_1 + 3), -1.0, 1.0, 0.0),
    ];
    let mods_active = drive_mod_amt.iter().any(|d| *d != 0.0)
        || tone_mod_amt.iter().any(|d| *d != 0.0)
        || fb_amount_mod_amt.iter().any(|d| *d != 0.0)
        || mix_mod_amt.iter().any(|d| *d != 0.0);

    // Stage control contexts (types + coefficient targets).
    let mut stage_ctx = [StageCtx::default(); NUM_STAGES];
    let mut stage_gain_target = [1.0_f32; NUM_STAGES];
    let mut stage_bias_target = [0.0_f32; NUM_STAGES];
    let mut stage_level_target = [1.0_f32; NUM_STAGES];
    let mut stage_engage_target = [0.0_f32; NUM_STAGES];
    let mut stage_g_target = [0.0_f32; NUM_STAGES];
    let mut stage_k_target = [1.0_f32; NUM_STAGES];
    for stage in 0..NUM_STAGES {
        let base = STATE_STAGE_BASE + stage * STAGE_STRIDE;
        let shaper = finite_clamp(
            *s.add(base + STAGE_SHAPER),
            0.0,
            (NUM_SHAPERS - 1) as f32,
            0.0,
        )
        .round() as usize;
        let filter = finite_clamp(
            *s.add(base + STAGE_FILTER),
            0.0,
            (NUM_FILTERS - 1) as f32,
            0.0,
        )
        .round() as usize;
        let pre = *s.add(base + STAGE_PRE) > 0.5;
        let amount = finite_clamp(*s.add(base + STAGE_AMOUNT), 0.0, 1.0, 0.0);
        let freq = finite_clamp(*s.add(base + STAGE_FREQ), 20.0, 16_000.0, 16_000.0);
        let res = finite_clamp(*s.add(base + STAGE_RES), 0.0, 1.0, 0.1);
        let mut ap_coef = [0.0_f32; 4];
        for (section, coef) in ap_coef.iter_mut().enumerate() {
            let off = (section as f32 - 1.5) * res;
            let f = (freq * off.exp2()).clamp(20.0, sr * 0.45);
            let t = (std::f32::consts::PI * f / sr).tan();
            *coef = (t - 1.0) / (t + 1.0);
        }
        stage_ctx[stage] = StageCtx {
            shaper,
            filter,
            pre,
            freq,
            res,
            amount,
            ap_coef,
            comb_delay: (sr / freq).clamp(2.0, (COMB_BUF_LEN - 2) as f32),
            hold_interval: if freq >= 15_900.0 { 1.0 } else { sr / freq },
        };
        stage_gain_target[stage] = (amount * AMOUNT_GAIN_OCT).exp2();
        stage_bias_target[stage] = finite_clamp(*s.add(base + STAGE_BIAS), -1.0, 1.0, 0.0);
        stage_level_target[stage] =
            db_to_amp(finite_clamp(*s.add(base + STAGE_LEVEL), -24.0, 24.0, 0.0));
        stage_engage_target[stage] = (amount * 50.0).clamp(0.0, 1.0);
        stage_g_target[stage] = (std::f32::consts::PI * (freq / sr).clamp(1.0e-5, 0.45)).tan();
        stage_k_target[stage] = 1.0 / res_to_q(res);

        // Reset a stage's runtime when its topology-affecting selections
        // change: stale filter/oversampler histories from another type would
        // otherwise leak into the new configuration.
        let cfg_changed = (*s.add(STATE_CFG_SHAPER + stage) - shaper as f32).abs() > 0.5
            || (*s.add(STATE_CFG_FILTER + stage) - filter as f32).abs() > 0.5
            || (*s.add(STATE_CFG_PRE + stage) - (pre as u8) as f32).abs() > 0.5;
        if cfg_changed {
            for ch in 0..2 {
                let block = stage * 2 + ch;
                std::ptr::write_bytes(
                    s.add(STATE_STAGE_RT + block * STAGE_RT_STRIDE),
                    0,
                    STAGE_RT_STRIDE,
                );
                std::ptr::write_bytes(
                    s.add(STATE_COMB_BUF + block * COMB_BUF_LEN),
                    0,
                    COMB_BUF_LEN,
                );
            }
            *s.add(STATE_CFG_SHAPER + stage) = shaper as f32;
            *s.add(STATE_CFG_FILTER + stage) = filter as f32;
            *s.add(STATE_CFG_PRE + stage) = (pre as u8) as f32;
        }
    }
    if (*s.add(STATE_CFG_ROUTING) - routing as f32).abs() > 0.5 {
        // Routing swaps what feeds each stage; clear the shared feedback and
        // crossover memories so the new topology starts clean.
        std::ptr::write_bytes(s.add(STATE_FB_BUF_L), 0, FB_BUF_LEN);
        std::ptr::write_bytes(s.add(STATE_FB_BUF_R), 0, FB_BUF_LEN);
        std::ptr::write_bytes(s.add(STATE_XOVER_Z), 0, 40);
        std::ptr::write_bytes(s.add(STATE_FB_BP_Z), 0, 4);
        *s.add(STATE_CFG_ROUTING) = routing as f32;
    }

    let xover_coefs = [
        butterworth(xover_low, sr, 0),
        butterworth(xover_low, sr, 1),
        butterworth(xover_high, sr, 0),
        butterworth(xover_high, sr, 1),
        butterworth(xover_high, sr, 2),
    ];

    let knob_coef = one_pole_coef(30.0, sr);
    let tone_lp_coef = one_pole_coef(tone_freq, sr);
    let dc_r = (-std::f32::consts::TAU * DC_BLOCK_HZ / sr).exp();
    let sc_coef = one_pole_coef(SC_HPF_HZ, sr);
    let meter_attack = time_coef(METER_ATTACK_MS, sr);
    let meter_release = time_coef(METER_RELEASE_MS, sr);
    let duck_attack = time_coef(5.0, sr);
    let duck_release = time_coef(120.0, sr);
    let comp_attack = time_coef(COMP_ATTACK_MS, sr);
    let comp_release = time_coef(COMP_RELEASE_MS, sr);

    let fb_buf_l = s.add(STATE_FB_BUF_L);
    let fb_buf_r = s.add(STATE_FB_BUF_R);
    let xover_z0 = s.add(STATE_XOVER_Z);
    let xover_z1 = s.add(STATE_XOVER_Z + 20);

    let mut sm_drive = finite_clamp(*s.add(STATE_SM_DRIVE), 0.0, 128.0, 1.0);
    let mut sm_tone_lo = finite_clamp(*s.add(STATE_SM_TONE_LO), 0.0, 8.0, 1.0);
    let mut sm_tone_hi = finite_clamp(*s.add(STATE_SM_TONE_HI), 0.0, 8.0, 1.0);
    let mut sm_output = finite_clamp(*s.add(STATE_SM_OUTPUT), 0.0, 32.0, output_target);
    let mut sm_mix = finite_clamp(*s.add(STATE_SM_MIX), 0.0, 1.0, mix_knob);
    let mut sm_blend = finite_clamp(*s.add(STATE_SM_BLEND), 0.0, 1.0, blend_target);
    let mut sm_fb_amount = finite_clamp(*s.add(STATE_SM_FB_AMOUNT), -1.0, 1.0, 0.0);
    let mut sm_fb_delay = finite_clamp(
        *s.add(STATE_SM_FB_DELAY),
        2.0,
        (FB_BUF_LEN - 8) as f32,
        fb_delay_target,
    );
    let mut sm_compress = finite_clamp(*s.add(STATE_SM_COMPRESS), 0.0, 1.0, compress_knob);
    let mut sm_fb_hp = finite_clamp(*s.add(STATE_SM_FB_HP_COEF), 0.0, 1.0, fb_hp_coef_target);
    let mut sm_fb_lp = finite_clamp(*s.add(STATE_SM_FB_LP_COEF), 0.0, 1.0, fb_lp_coef_target);
    let mut tone_lp_l = finite_or(*s.add(STATE_TONE_LP_L), 0.0);
    let mut tone_lp_r = finite_or(*s.add(STATE_TONE_LP_R), 0.0);
    let mut fb_wpos = (*s.add(STATE_FB_WPOS)) as usize & FB_BUF_MASK;
    let mut duck_env = finite_clamp(*s.add(STATE_DUCK_ENV), 0.0, 16.0, 0.0);
    let mut comp_env = finite_clamp(*s.add(STATE_COMP_ENV), 0.0, 1.0e6, 0.0);
    let mut sc_z_l = finite_or(*s.add(STATE_SC_Z_L), 0.0);
    let mut sc_z_r = finite_or(*s.add(STATE_SC_Z_R), 0.0);
    let mut fb_bp_z = [
        finite_or(*s.add(STATE_FB_BP_Z), 0.0),
        finite_or(*s.add(STATE_FB_BP_Z + 1), 0.0),
        finite_or(*s.add(STATE_FB_BP_Z + 2), 0.0),
        finite_or(*s.add(STATE_FB_BP_Z + 3), 0.0),
    ];
    let mut meter_pre = [0.0_f32; 6];
    for (slot, value) in meter_pre.iter_mut().enumerate() {
        *value = finite_clamp(*s.add(STATE_METER_PRE + slot), -1.0e4, 1.0e4, 0.0);
    }
    let mut meter_post = [0.0_f32; NUM_STAGES];
    for (stage, value) in meter_post.iter_mut().enumerate() {
        *value = db_to_amp(finite_clamp(
            *s.add(STATE_METER_POST_DB + stage),
            -80.0,
            40.0,
            -80.0,
        ));
    }

    // Tone leg gains (recomputed when the knob moves; smoothed as amps).
    let tone_targets = |tone: f32| -> (f32, f32) {
        let t = tone.clamp(-1.0, 1.0);
        if tone_mode == 1 {
            // Shelf: single low-shelf cut/boost, highs untouched.
            (db_to_amp(6.0 * t), 1.0)
        } else {
            // Tilt around the pivot: ±6 dB shelves at the extremes.
            (db_to_amp(-6.0 * t), db_to_amp(6.0 * t))
        }
    };

    for i in 0..nf {
        let dry_l = *in0.add(i);
        let dry_r = *in1.add(i);

        let mut drive_mod = 0.0_f32;
        let mut tone_mod = 0.0_f32;
        let mut fb_amount_mod = 0.0_f32;
        let mut mix_mod = 0.0_f32;
        if mods_active {
            for slot in 0..4 {
                let m = finite_or(*mod_inputs[slot].add(i), 0.0);
                drive_mod += m * drive_mod_amt[slot];
                tone_mod += m * tone_mod_amt[slot];
                fb_amount_mod += m * fb_amount_mod_amt[slot];
                mix_mod += m * mix_mod_amt[slot];
            }
        }

        let drive_target = db_to_amp((drive_db_knob + drive_mod).clamp(-12.0, 36.0));
        let (tone_lo_target, tone_hi_target) = tone_targets(tone_knob + tone_mod);
        sm_drive += knob_coef * (drive_target - sm_drive);
        sm_tone_lo += knob_coef * (tone_lo_target - sm_tone_lo);
        sm_tone_hi += knob_coef * (tone_hi_target - sm_tone_hi);
        sm_output += knob_coef * (output_target - sm_output);
        sm_mix += knob_coef * ((mix_knob + mix_mod).clamp(0.0, 1.0) - sm_mix);
        sm_blend += knob_coef * (blend_target - sm_blend);
        sm_fb_amount += knob_coef
            * ((fb_amount_knob + fb_amount_mod).clamp(0.0, 1.0) * FB_AMOUNT_MAX * fb_sign
                - sm_fb_amount);
        sm_fb_delay += knob_coef * (fb_delay_target - sm_fb_delay);
        sm_compress += knob_coef * (compress_knob - sm_compress);
        sm_fb_hp += knob_coef * (fb_hp_coef_target - sm_fb_hp);
        sm_fb_lp += knob_coef * (fb_lp_coef_target - sm_fb_lp);
        for stage in 0..NUM_STAGES {
            let sm = s.add(STATE_SM_STAGE_BASE + stage * SM_STAGE_STRIDE);
            *sm.add(SM_STAGE_GAIN) +=
                knob_coef * (stage_gain_target[stage] - *sm.add(SM_STAGE_GAIN));
            *sm.add(SM_STAGE_BIAS) +=
                knob_coef * (stage_bias_target[stage] - *sm.add(SM_STAGE_BIAS));
            *sm.add(SM_STAGE_LEVEL) +=
                knob_coef * (stage_level_target[stage] - *sm.add(SM_STAGE_LEVEL));
            *sm.add(SM_STAGE_ENGAGE) +=
                knob_coef * (stage_engage_target[stage] - *sm.add(SM_STAGE_ENGAGE));
            *sm.add(SM_STAGE_G) += knob_coef * (stage_g_target[stage] - *sm.add(SM_STAGE_G));
            *sm.add(SM_STAGE_K) += knob_coef * (stage_k_target[stage] - *sm.add(SM_STAGE_K));
        }

        // Input section: Drive then Tone (dry tap already captured).
        let mut x_l = dry_l * sm_drive;
        let mut x_r = dry_r * sm_drive;
        tone_lp_l += tone_lp_coef * (x_l - tone_lp_l);
        tone_lp_r += tone_lp_coef * (x_r - tone_lp_r);
        x_l = sm_tone_lo * tone_lp_l + sm_tone_hi * (x_l - tone_lp_l);
        x_r = sm_tone_lo * tone_lp_r + sm_tone_hi * (x_r - tone_lp_r);

        let mut pre_extremes = [[0.0_f32; 2]; NUM_STAGES]; // (min, max) per stage
        for extremes in pre_extremes.iter_mut() {
            extremes[0] = f32::INFINITY;
            extremes[1] = f32::NEG_INFINITY;
        }
        let mut stage_out_peak = [0.0_f32; NUM_STAGES];

        macro_rules! run_stage {
            ($stage:expr, $ch:expr, $x:expr) => {{
                let stage = $stage;
                let y = stage_sample(
                    &stage_ctx[stage],
                    s,
                    stage,
                    $ch,
                    $x,
                    dc_r,
                    &mut pre_extremes[stage],
                );
                if y.abs() > stage_out_peak[stage] {
                    stage_out_peak[stage] = y.abs();
                }
                y
            }};
        }

        let (mut wet_l, mut wet_r) = match routing {
            ROUTING_SERIAL => {
                let a_l = run_stage!(0, 0, x_l);
                let a_r = run_stage!(0, 1, x_r);
                (run_stage!(1, 0, a_l), run_stage!(1, 1, a_r))
            }
            ROUTING_PARALLEL => {
                let a_l = run_stage!(0, 0, x_l);
                let a_r = run_stage!(0, 1, x_r);
                let b_l = run_stage!(1, 0, x_l);
                let b_r = run_stage!(1, 1, x_r);
                let theta = sm_blend * std::f32::consts::FRAC_PI_2;
                let (g_b, g_a) = theta.sin_cos();
                (a_l * g_a + b_l * g_b, a_r * g_a + b_r * g_b)
            }
            ROUTING_MULTIBAND => {
                let (lo_l, mid_l, hi_l) = split_bands(x_l, &xover_coefs, xover_z0);
                let (lo_r, mid_r, hi_r) = split_bands(x_r, &xover_coefs, xover_z1);
                (
                    run_stage!(0, 0, lo_l) + run_stage!(1, 0, mid_l) + run_stage!(2, 0, hi_l),
                    run_stage!(0, 1, lo_r) + run_stage!(1, 1, mid_r) + run_stage!(2, 1, hi_r),
                )
            }
            ROUTING_MID_SIDE => {
                let m = (x_l + x_r) * std::f32::consts::FRAC_1_SQRT_2;
                let side = (x_l - x_r) * std::f32::consts::FRAC_1_SQRT_2;
                let m_out = run_stage!(0, 0, m);
                let s_out = run_stage!(1, 0, side);
                // Blend = M/S balance: 0.5 unity, toward 0 emphasizes Mid,
                // toward 1 emphasizes Side.
                let g_m = (2.0 * (1.0 - sm_blend)).min(1.0);
                let g_s = (2.0 * sm_blend).min(1.0);
                let m_b = m_out * g_m;
                let s_b = s_out * g_s;
                (
                    (m_b + s_b) * std::f32::consts::FRAC_1_SQRT_2,
                    (m_b - s_b) * std::f32::consts::FRAC_1_SQRT_2,
                )
            }
            ROUTING_FEEDBACK | ROUTING_DELAY => {
                // Loop: S1 -> delay -> S2 -> bandpass -> tanh -> ±amount
                // (duckable) -> back into S1's input.
                let ret_l = fb_line_read(fb_buf_l, fb_wpos, sm_fb_delay);
                let ret_r = fb_line_read(fb_buf_r, fb_wpos, sm_fb_delay);
                let r2_l = run_stage!(1, 0, ret_l);
                let r2_r = run_stage!(1, 1, ret_r);
                // Bandpass: one-pole HP skirt then one-pole LP skirt.
                fb_bp_z[0] += sm_fb_hp * (r2_l - fb_bp_z[0]);
                let hp_l = r2_l - fb_bp_z[0];
                fb_bp_z[1] += sm_fb_lp * (hp_l - fb_bp_z[1]);
                fb_bp_z[2] += sm_fb_hp * (r2_r - fb_bp_z[2]);
                let hp_r = r2_r - fb_bp_z[2];
                fb_bp_z[3] += sm_fb_lp * (hp_r - fb_bp_z[3]);
                let cond_l = fb_bp_z[1].tanh();
                let cond_r = fb_bp_z[3].tanh();

                let in_peak = dry_l.abs().max(dry_r.abs());
                let duck_coef = if in_peak > duck_env {
                    duck_attack
                } else {
                    duck_release
                };
                duck_env += duck_coef * (in_peak - duck_env);
                let duck_gain = if fb_duck {
                    (1.0 - 2.5 * duck_env).clamp(0.0, 1.0)
                } else {
                    1.0
                };

                let inj = sm_fb_amount * duck_gain;
                let y_l = run_stage!(0, 0, x_l + cond_l * inj);
                let y_r = run_stage!(0, 1, x_r + cond_r * inj);
                *fb_buf_l.add(fb_wpos) = y_l;
                *fb_buf_r.add(fb_wpos) = y_r;
                if routing == ROUTING_DELAY {
                    // Audible echoes: the conditioned delay-line return joins
                    // the output as well as the loop.
                    (y_l + cond_l, y_r + cond_r)
                } else {
                    (y_l, y_r)
                }
            }
            _ => (run_stage!(0, 0, x_l), run_stage!(0, 1, x_r)),
        };
        fb_wpos = (fb_wpos + 1) & FB_BUF_MASK;

        // One-knob compressor on the wet path (peak detector, optional
        // 120 Hz sidechain HPF so bass stops pumping it).
        if sm_compress > 1.0e-3 {
            let (det_l, det_r) = if sc_hpf {
                sc_z_l += sc_coef * (wet_l - sc_z_l);
                sc_z_r += sc_coef * (wet_r - sc_z_r);
                (wet_l - sc_z_l, wet_r - sc_z_r)
            } else {
                (wet_l, wet_r)
            };
            let det = det_l.abs().max(det_r.abs());
            let coef = if det > comp_env {
                comp_attack
            } else {
                comp_release
            };
            comp_env += coef * (det - comp_env);
            let level_db = amp_to_db(comp_env);
            let thr_db = COMP_MAX_THR_DB * sm_compress;
            let over = level_db - thr_db;
            let gr_db = if over > 0.0 {
                -over * (1.0 - 1.0 / COMP_RATIO)
            } else {
                0.0
            };
            let makeup_db = -thr_db * (1.0 - 1.0 / COMP_RATIO) * 0.5;
            let g = db_to_amp(gr_db + makeup_db);
            wet_l *= g;
            wet_r *= g;
        }

        wet_l *= sm_output;
        wet_r *= sm_output;

        let g_wet = (sm_mix * std::f32::consts::FRAC_PI_2).sin();
        let g_dry = (sm_mix * std::f32::consts::FRAC_PI_2).cos();
        *out0.add(i) = dry_l * g_dry + wet_l * g_wet;
        *out1.add(i) = dry_r * g_dry + wet_r * g_wet;

        // Meter ballistics: pre-shaper extremes decay toward 0 (the region
        // collapses at silence), post levels are peak envelopes.
        for stage in 0..NUM_STAGES {
            let (mn, mx) = (pre_extremes[stage][0], pre_extremes[stage][1]);
            let (mn, mx) = if mn.is_finite() { (mn, mx) } else { (0.0, 0.0) };
            let slot_min = &mut meter_pre[stage * 2];
            let coef = if mn < *slot_min {
                meter_attack
            } else {
                meter_release
            };
            *slot_min += coef * (mn - *slot_min);
            let slot_max = &mut meter_pre[stage * 2 + 1];
            let coef = if mx > *slot_max {
                meter_attack
            } else {
                meter_release
            };
            *slot_max += coef * (mx - *slot_max);
            let post = &mut meter_post[stage];
            let coef = if stage_out_peak[stage] > *post {
                meter_attack
            } else {
                meter_release
            };
            *post += coef * (stage_out_peak[stage] - *post);
        }
    }

    *s.add(STATE_SM_DRIVE) = sm_drive;
    *s.add(STATE_SM_TONE_LO) = sm_tone_lo;
    *s.add(STATE_SM_TONE_HI) = sm_tone_hi;
    *s.add(STATE_SM_OUTPUT) = sm_output;
    *s.add(STATE_SM_MIX) = sm_mix;
    *s.add(STATE_SM_BLEND) = sm_blend;
    *s.add(STATE_SM_FB_AMOUNT) = sm_fb_amount;
    *s.add(STATE_SM_FB_DELAY) = sm_fb_delay;
    *s.add(STATE_SM_COMPRESS) = sm_compress;
    *s.add(STATE_SM_FB_HP_COEF) = sm_fb_hp;
    *s.add(STATE_SM_FB_LP_COEF) = sm_fb_lp;
    *s.add(STATE_TONE_LP_L) = tone_lp_l;
    *s.add(STATE_TONE_LP_R) = tone_lp_r;
    *s.add(STATE_FB_WPOS) = fb_wpos as f32;
    *s.add(STATE_DUCK_ENV) = duck_env;
    *s.add(STATE_COMP_ENV) = comp_env;
    *s.add(STATE_SC_Z_L) = sc_z_l;
    *s.add(STATE_SC_Z_R) = sc_z_r;
    for (slot, value) in fb_bp_z.iter().enumerate() {
        *s.add(STATE_FB_BP_Z + slot) = *value;
    }
    for (slot, value) in meter_pre.iter().enumerate() {
        *s.add(STATE_METER_PRE + slot) = finite_or(*value, 0.0);
    }
    for (stage, value) in meter_post.iter().enumerate() {
        *s.add(STATE_METER_POST_DB + stage) = amp_to_db(finite_or(*value, 0.0)).max(-80.0);
    }
}

pub fn roar_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(roar_process),
        init: Some(roar_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: usize = 48000;
    const BLOCK: usize = 128;

    fn make_state() -> Vec<f32> {
        let mut state = vec![0.0_f32; ROAR_STATE_SIZE];
        unsafe {
            roar_init(
                state.as_mut_ptr() as *mut c_void,
                SR as c_int,
                BLOCK as c_int,
                std::ptr::null(),
            );
        }
        state
    }

    fn render(
        state: &mut [f32],
        input: impl Fn(usize) -> (f32, f32),
        n: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut out_all_l = vec![0.0_f32; n];
        let mut out_all_r = vec![0.0_f32; n];
        let mut in_l = vec![0.0_f32; BLOCK];
        let mut in_r = vec![0.0_f32; BLOCK];
        let mut mods = vec![0.0_f32; BLOCK];
        let mut out_l = vec![0.0_f32; BLOCK];
        let mut out_r = vec![0.0_f32; BLOCK];
        let mut pos = 0;
        while pos < n {
            let frames = BLOCK.min(n - pos);
            for i in 0..frames {
                let (l, r) = input(pos + i);
                in_l[i] = l;
                in_r[i] = r;
            }
            let inputs = [
                in_l.as_mut_ptr(),
                in_r.as_mut_ptr(),
                mods.as_mut_ptr(),
                mods.as_mut_ptr(),
                mods.as_mut_ptr(),
                mods.as_mut_ptr(),
            ];
            let outputs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
            unsafe {
                roar_process(
                    inputs.as_ptr(),
                    outputs.as_ptr(),
                    frames as c_int,
                    state.as_mut_ptr() as *mut c_void,
                    std::ptr::null_mut(),
                );
            }
            out_all_l[pos..pos + frames].copy_from_slice(&out_l[..frames]);
            out_all_r[pos..pos + frames].copy_from_slice(&out_r[..frames]);
            pos += frames;
        }
        (out_all_l, out_all_r)
    }

    fn sine(freq: f32, amp: f32) -> impl Fn(usize) -> (f32, f32) {
        move |i| {
            let v = amp * (std::f32::consts::TAU * freq * i as f32 / SR as f32).sin();
            (v, v)
        }
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0_f32, |m, &x| m.max(x.abs()))
    }

    fn rms(buf: &[f32]) -> f32 {
        (buf.iter().map(|v| v * v).sum::<f32>() / buf.len().max(1) as f32).sqrt()
    }

    fn set_stage(state: &mut [f32], stage: usize, field: RoarStageField, value: f32) {
        state[roar_stage_param(stage, field) as usize] = value;
    }

    #[test]
    fn bypass_is_transparent() {
        let mut state = make_state();
        state[STATE_ENABLED] = 0.0;
        let src: Vec<f32> = (0..4096).map(|i| sine(220.0, 0.5)(i).0).collect();
        let (l, _) = render(&mut state, sine(220.0, 0.5), 4096);
        assert_eq!(l, src);
    }

    #[test]
    fn default_settings_pass_audio_nearly_unchanged() {
        // Amount 0 everywhere: the shaper leg is crossfaded out and the
        // default 16 kHz LP is nearly transparent at 1 kHz.
        let mut state = make_state();
        let n = SR;
        let (l, _) = render(&mut state, sine(1000.0, 0.5), n);
        let p = peak(&l[n / 2..]);
        assert!((p - 0.5).abs() < 0.05, "expected near-unity pass, peak {p}");
    }

    #[test]
    fn shaper_curves_have_unit_slope_at_origin() {
        // The rectifiers deliberately break the rule (slopes 2/0 and ±1).
        for shaper in [
            SHAPER_SOFT_SINE,
            SHAPER_DIGITAL_CLIP,
            SHAPER_DIODE,
            SHAPER_TUBE,
            SHAPER_POLYNOMIAL,
            SHAPER_FRACTAL,
            SHAPER_TRI_FOLD,
            SHAPER_NOISE,
            SHAPER_SHARDS,
        ] {
            for amount in [0.0, 0.5, 1.0] {
                let h = 1.0e-3;
                let slope = (shaper_transfer(shaper, amount, h)
                    - shaper_transfer(shaper, amount, -h))
                    / (2.0 * h);
                assert!(
                    (slope - 1.0).abs() < 0.15,
                    "shaper {shaper} amount {amount}: origin slope {slope}"
                );
            }
        }
    }

    #[test]
    fn shaper_curves_stay_bounded() {
        for shaper in 0..NUM_SHAPERS {
            for amount in [0.0, 0.5, 1.0] {
                for step in -400..=400 {
                    let x = step as f32 * 0.02; // -8..8
                    let y = shaper_transfer(shaper, amount, x);
                    assert!(
                        y.is_finite() && y.abs() <= 16.0,
                        "shaper {shaper} amount {amount} x {x} -> {y}"
                    );
                }
            }
        }
    }

    #[test]
    fn amount_drives_saturation_but_output_stays_bounded() {
        let mut state = make_state();
        set_stage(&mut state, 0, RoarStageField::Amount, 1.0);
        let n = SR;
        let (l, _) = render(&mut state, sine(220.0, 0.5), n);
        let p = peak(&l[n / 2..]);
        assert!(p <= 1.5, "soft-sine full amount should stay bounded: {p}");
        assert!(p > 0.5, "saturated output should not collapse: {p}");
    }

    #[test]
    fn half_wave_dc_is_blocked() {
        let mut state = make_state();
        set_stage(
            &mut state,
            0,
            RoarStageField::Shaper,
            SHAPER_HALF_WAVE as f32,
        );
        set_stage(&mut state, 0, RoarStageField::Amount, 0.6);
        let n = 2 * SR;
        let (l, _) = render(&mut state, sine(220.0, 0.4), n);
        let tail = &l[n - SR / 2..];
        let mean = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean.abs() < 0.02, "rectifier DC should be blocked: {mean}");
    }

    #[test]
    fn serial_routing_applies_both_stages() {
        let run = |routing: f32, s2_level: f32| {
            let mut state = make_state();
            state[STATE_ROUTING] = routing;
            set_stage(&mut state, 0, RoarStageField::Amount, 0.4);
            set_stage(&mut state, 1, RoarStageField::Amount, 0.4);
            set_stage(&mut state, 1, RoarStageField::Level, s2_level);
            let n = SR;
            let (l, _) = render(&mut state, sine(220.0, 0.25), n);
            rms(&l[n / 2..])
        };
        let single = run(ROUTING_SINGLE as f32, -24.0);
        let serial = run(ROUTING_SERIAL as f32, -24.0);
        assert!(
            serial < single * 0.25,
            "stage 2 level should only act in serial routing: single {single}, serial {serial}"
        );
    }

    #[test]
    fn parallel_blend_crossfades_stage_outputs() {
        let run = |blend: f32| {
            let mut state = make_state();
            state[STATE_ROUTING] = ROUTING_PARALLEL as f32;
            state[STATE_BLEND] = blend;
            set_stage(&mut state, 1, RoarStageField::Level, -24.0);
            let n = SR;
            let (l, _) = render(&mut state, sine(1000.0, 0.4), n);
            rms(&l[n / 2..])
        };
        let stage1_only = run(0.0);
        let stage2_only = run(1.0);
        assert!(
            stage2_only < stage1_only * 0.25,
            "blend 1 should isolate the trimmed stage 2: {stage1_only} vs {stage2_only}"
        );
    }

    #[test]
    fn multiband_sums_flat_at_zero_amount() {
        let mut state = make_state();
        state[STATE_ROUTING] = ROUTING_MULTIBAND as f32;
        for freq in [60.0, 250.0, 1000.0, 4000.0, 10000.0] {
            let mut state = state.clone();
            let n = 16384;
            let (l, _) = render(&mut state, sine(freq, 0.5), n);
            let p = peak(&l[n / 2..]);
            assert!(
                (p - 0.5).abs() < 0.06,
                "multiband not flat at {freq} Hz: peak {p}"
            );
        }
    }

    #[test]
    fn mid_side_leaves_mono_side_stage_silent() {
        // Mono input has no side signal, so cranking the side stage must not
        // change the output.
        let run = |side_amount: f32| {
            let mut state = make_state();
            state[STATE_ROUTING] = ROUTING_MID_SIDE as f32;
            set_stage(&mut state, 1, RoarStageField::Amount, side_amount);
            let n = SR;
            let (l, _) = render(&mut state, sine(500.0, 0.4), n);
            rms(&l[n / 2..])
        };
        let quiet = run(0.0);
        let cranked = run(1.0);
        assert!(
            (quiet - cranked).abs() < quiet * 0.02 + 1.0e-4,
            "side stage acted on mono input: {quiet} vs {cranked}"
        );
    }

    #[test]
    fn mid_side_decode_is_transparent_at_defaults() {
        let mut state = make_state();
        state[STATE_ROUTING] = ROUTING_MID_SIDE as f32;
        let n = SR;
        let input = |i: usize| {
            let l = 0.4 * (std::f32::consts::TAU * 300.0 * i as f32 / SR as f32).sin();
            let r = 0.3 * (std::f32::consts::TAU * 900.0 * i as f32 / SR as f32).sin();
            (l, r)
        };
        let (l, r) = render(&mut state, input, n);
        let (lp, rp) = (peak(&l[n / 2..]), peak(&r[n / 2..]));
        assert!((lp - 0.4).abs() < 0.05, "left decode drifted: {lp}");
        assert!((rp - 0.3).abs() < 0.05, "right decode drifted: {rp}");
    }

    #[test]
    fn feedback_at_full_amount_stays_bounded() {
        let mut state = make_state();
        state[STATE_ROUTING] = ROUTING_FEEDBACK as f32;
        state[STATE_FB_AMOUNT] = 1.0;
        state[STATE_FB_TIME] = 12.0;
        set_stage(&mut state, 0, RoarStageField::Amount, 0.8);
        set_stage(&mut state, 1, RoarStageField::Amount, 0.5);
        let n = 2 * SR;
        let input = |i: usize| {
            let v = if i % (SR / 2) < 64 { 0.9 } else { 0.0 };
            (v, v)
        };
        let (l, _) = render(&mut state, input, n);
        let p = peak(&l);
        assert!(p.is_finite() && p < 10.0, "feedback loop blew up: {p}");
        assert!(
            rms(&l[n - SR / 4..]) > 1.0e-4,
            "full feedback should keep ringing"
        );
    }

    #[test]
    fn delay_routing_produces_audible_echo() {
        let mut state = make_state();
        state[STATE_ROUTING] = ROUTING_DELAY as f32;
        state[STATE_FB_AMOUNT] = 0.4;
        state[STATE_FB_TIME] = 250.0;
        set_stage(&mut state, 0, RoarStageField::Amount, 0.2);
        let n = SR;
        let input = |i: usize| {
            let v = if i < 64 { 0.8 } else { 0.0 };
            (v, v)
        };
        let (l, _) = render(&mut state, input, n);
        // The echo lands ~250 ms after the burst.
        let echo_at = (0.25 * SR as f32) as usize;
        let echo = peak(&l[echo_at..echo_at + 4096]);
        assert!(echo > 0.01, "delay routing should produce an echo: {echo}");
    }

    #[test]
    fn compress_reduces_crest_and_raises_quiet_material() {
        let run = |compress: f32| {
            let mut state = make_state();
            state[STATE_COMPRESS] = compress;
            let n = SR;
            let (l, _) = render(&mut state, sine(1000.0, db_to_amp(-20.0)), n);
            rms(&l[n / 2..])
        };
        let dry = run(0.0);
        let squashed = run(1.0);
        assert!(
            squashed > dry * 1.5,
            "compress makeup should lift a -20 dB signal: {dry} vs {squashed}"
        );
    }

    #[test]
    fn meters_track_drive_region_and_stage_output() {
        let mut state = make_state();
        set_stage(&mut state, 0, RoarStageField::Amount, 0.5);
        set_stage(&mut state, 0, RoarStageField::Bias, 0.25);
        let _ = render(&mut state, sine(500.0, 0.4), SR);
        let pre_min = state[STATE_METER_PRE];
        let pre_max = state[STATE_METER_PRE + 1];
        assert!(
            pre_max > 0.5,
            "drive-region max should track the driven signal: {pre_max}"
        );
        assert!(
            pre_min < -0.2,
            "drive-region min should go negative: {pre_min}"
        );
        assert!(
            pre_max + pre_min > 0.05,
            "positive bias should skew the region: [{pre_min}, {pre_max}]"
        );
        let post = state[STATE_METER_POST_DB];
        assert!(post > -20.0, "stage output meter should be live: {post}");
        let idle = state[STATE_METER_POST_DB + 2];
        assert!(idle < -60.0, "unused stage 3 should stay quiet: {idle}");
    }

    #[test]
    fn stage_filter_lp_darkens_and_comb_survives() {
        let run = |filter: usize, freq: f32| {
            let mut state = make_state();
            set_stage(&mut state, 0, RoarStageField::Filter, filter as f32);
            set_stage(&mut state, 0, RoarStageField::Freq, freq);
            let n = SR;
            let (l, _) = render(&mut state, sine(4000.0, 0.4), n);
            rms(&l[n / 2..])
        };
        let open = run(FILTER_LP, 16000.0);
        let dark = run(FILTER_LP, 300.0);
        assert!(
            dark < open * 0.1,
            "300 Hz LP should crush a 4 kHz sine: {open} vs {dark}"
        );
        let comb = run(FILTER_COMB, 1000.0);
        assert!(
            comb.is_finite() && comb > 1.0e-3,
            "comb output died: {comb}"
        );
        let resampled = run(FILTER_RESAMPLE, 2000.0);
        assert!(
            resampled.is_finite() && resampled > 1.0e-3,
            "resample output died: {resampled}"
        );
        let dispersed = run(FILTER_DISPERSION, 1000.0);
        assert!(
            (dispersed - open).abs() < open * 0.35,
            "dispersion is allpass, magnitude should survive: {open} vs {dispersed}"
        );
    }

    #[test]
    fn note_mode_uses_bpm_for_the_loop_time() {
        let mut state = make_state();
        state[STATE_ROUTING] = ROUTING_DELAY as f32;
        state[STATE_FB_MODE] = 1.0;
        state[STATE_FB_DIV] = 6.0; // "1/4" = one beat
        state[STATE_BPM] = 120.0; // 500 ms
        state[STATE_FB_AMOUNT] = 0.4;
        set_stage(&mut state, 0, RoarStageField::Amount, 0.2);
        // Pre-seed the delay smoother so the echo position is exact.
        state[STATE_SM_FB_DELAY] = 0.5 * SR as f32;
        let n = SR;
        let input = |i: usize| {
            let v = if i < 64 { 0.8 } else { 0.0 };
            (v, v)
        };
        let (l, _) = render(&mut state, input, n);
        let echo_at = SR / 2;
        let echo = peak(&l[echo_at..echo_at + 2048]);
        let before = peak(&l[SR / 4..SR / 4 + 2048]);
        assert!(
            echo > before * 4.0 + 1.0e-3,
            "synced echo should land at 500 ms: before {before}, echo {echo}"
        );
    }

    #[test]
    fn dry_wet_zero_returns_the_dry_input() {
        let mut state = make_state();
        state[STATE_MIX] = 0.0;
        state[STATE_SM_MIX] = 0.0;
        state[STATE_DRIVE] = 24.0;
        set_stage(&mut state, 0, RoarStageField::Amount, 1.0);
        let n = SR / 2;
        let (l, _) = render(&mut state, sine(700.0, 0.3), n);
        let p = peak(&l[n / 2..]);
        assert!((p - 0.3).abs() < 0.01, "dry path should be untouched: {p}");
    }
}
