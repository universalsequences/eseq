//! Builtin `Reverb` — three tanks behind one stereo insert.
//!
//! * `galaxy` — the original Airwindows Galactic port (`galaxy.rs`). Bit-exact
//!   with the pre-multi-mode builtin at the appended params' defaults; the
//!   golden-hash test below is the proof, so 59 saved projects keep their sound.
//! * `plate` — Dattorro figure-eight plate (`plate.rs`).
//! * `hall` — Lexicon-224-style four-section loop (`hall.rs`).
//!
//! Only the active tank runs. Around it, shared stages modelled on Ableton's
//! Reverb: stereo predelay → input lo-cut/hi-cut → tank (with hi/lo damping
//! shelves inside its feedback) → wet chorus → stereo width → dry/wet. Every
//! shared stage has an exact-bypass default so galaxy stays identical.
//!
//! Mode switching drops the old tail: fade wet to zero over ~25 ms, clear the
//! incoming tank at the block boundary, fade back in. Plate and hall were
//! ported from `effects/multiverb.rs`, which is retired but untouched so
//! saved Multiverb slots render exactly as before.

mod galaxy;
mod hall;
mod plate;
mod tank_common;

use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};
use tank_common::*;

// ── Modes ──

pub const MODE_GALAXY: f32 = 0.0;
pub const MODE_PLATE: f32 = 1.0;
pub const MODE_HALL: f32 = 2.0;

// ── Param state slots (descriptor writes raw f32 here) ──
// Slots 0-4 predate the multi-mode rewrite; the rest are appended. Node
// indices are free to move (projects persist by descriptor index), but the
// descriptor order in `EffectDescriptor::builtin_reverb_insert` is not.

const ST_REPLACE: usize = 0;
const ST_BRIGHT: usize = 1;
const ST_DETUNE: usize = 2; // fixed 0.1, not exposed
const ST_BIGNESS: usize = 3;
const ST_MIX: usize = 4;
const ST_MODE: usize = 5;
const ST_PREDELAY_MS: usize = 6;
const ST_DECAY: usize = 7;
const ST_DIFFUSION: usize = 8;
const ST_HI_SHELF_FREQ: usize = 9;
const ST_HI_SHELF_GAIN: usize = 10; // dB, <= 0
const ST_LO_SHELF_FREQ: usize = 11;
const ST_LO_SHELF_GAIN: usize = 12; // dB, <= 0
const ST_IN_LOCUT: usize = 13; // Hz; at IN_LOCUT_OFF the stage is skipped
const ST_IN_HICUT: usize = 14; // Hz; at IN_HICUT_OFF the stage is skipped
const ST_WIDTH: usize = 15;
const ST_CHORUS_AMT: usize = 16;
const ST_CHORUS_RATE: usize = 17;
const ST_MOD_DEPTH: usize = 18;
const ST_ENABLED: usize = 19;

// ── Shared runtime slots ──

const ST_SAMPLE_RATE: usize = 20;
const ST_ACTIVE_MODE: usize = 21; // currently-running tank; -1 = adopt param
const ST_FADE: usize = 22; // wet fade gain for mode switching
const ST_SM_PREDELAY: usize = 23; // smoothed predelay in samples
const ST_SM_SCALE: usize = 24; // smoothed tank delay scale (fs ratio × size)
const ST_LFO_PHASE: usize = 25;
const ST_RNG: usize = 26; // xorshift32 state stored as bits
const ST_IN_HP: usize = 27; // 4: L stage 1, L stage 2, R stage 1, R stage 2
const ST_IN_LP: usize = 31; // 4
const ST_CHORUS_PHASE: usize = 35;
// Appended after the fact (descriptor indices 19-20); they live in the spare
// shared slots so the tank layouts did not move.
const ST_WET_GAIN_DB: usize = 36;
const ST_MIX_LAW: usize = 37; // 0 = Galactic cube law (legacy), 1 = linear crossfade

const GALAXY_RT: usize = 40;
const PLATE_RT: usize = GALAXY_RT + galaxy::RUNTIME_SLOTS;
const HALL_RT: usize = PLATE_RT + plate::STATE_SLOTS;
const ST_WRITE_IDX: usize = HALL_RT + hall::STATE_SLOTS;
const ST_BUFS: usize = ST_WRITE_IDX + NRING;
const GALAXY_BUF_BASE: usize = ST_BUFS + total_ring_floats();

pub const REVERB_STATE_SIZE: usize = GALAXY_BUF_BASE + galaxy::total_buf_floats();

// Public param indices for the descriptor.
pub const REVERB_PARAM_REPLACE: u64 = ST_REPLACE as u64;
pub const REVERB_PARAM_BRIGHT: u64 = ST_BRIGHT as u64;
pub const REVERB_PARAM_SIZE: u64 = ST_BIGNESS as u64;
pub const REVERB_PARAM_MIX: u64 = ST_MIX as u64;
pub const REVERB_PARAM_MODE: u64 = ST_MODE as u64;
pub const REVERB_PARAM_PREDELAY_MS: u64 = ST_PREDELAY_MS as u64;
pub const REVERB_PARAM_DECAY: u64 = ST_DECAY as u64;
pub const REVERB_PARAM_DIFFUSION: u64 = ST_DIFFUSION as u64;
pub const REVERB_PARAM_HI_SHELF_FREQ: u64 = ST_HI_SHELF_FREQ as u64;
pub const REVERB_PARAM_HI_SHELF_GAIN: u64 = ST_HI_SHELF_GAIN as u64;
pub const REVERB_PARAM_LO_SHELF_FREQ: u64 = ST_LO_SHELF_FREQ as u64;
pub const REVERB_PARAM_LO_SHELF_GAIN: u64 = ST_LO_SHELF_GAIN as u64;
pub const REVERB_PARAM_IN_LOCUT: u64 = ST_IN_LOCUT as u64;
pub const REVERB_PARAM_IN_HICUT: u64 = ST_IN_HICUT as u64;
pub const REVERB_PARAM_WIDTH: u64 = ST_WIDTH as u64;
pub const REVERB_PARAM_CHORUS_AMT: u64 = ST_CHORUS_AMT as u64;
pub const REVERB_PARAM_CHORUS_RATE: u64 = ST_CHORUS_RATE as u64;
pub const REVERB_PARAM_MOD_DEPTH: u64 = ST_MOD_DEPTH as u64;
pub const REVERB_PARAM_ENABLED: u64 = ST_ENABLED as u64;
pub const REVERB_PARAM_WET_GAIN_DB: u64 = ST_WET_GAIN_DB as u64;
pub const REVERB_PARAM_MIX_LAW: u64 = ST_MIX_LAW as u64;

pub const MIX_LAW_CUBE: f32 = 0.0;
pub const MIX_LAW_LINEAR: f32 = 1.0;
pub const WET_GAIN_MIN_DB: f32 = -12.0;
pub const WET_GAIN_MAX_DB: f32 = 18.0;

/// Input cut ranges; the stage is skipped at the "open" end so the default
/// render is untouched.
pub const IN_LOCUT_OFF: f32 = 20.0;
pub const IN_LOCUT_MAX: f32 = 2_000.0;
pub const IN_HICUT_MIN: f32 = 1_000.0;
pub const IN_HICUT_OFF: f32 = 20_000.0;
pub const HI_SHELF_FREQ_MIN: f32 = 1_000.0;
pub const HI_SHELF_FREQ_MAX: f32 = 16_000.0;
pub const LO_SHELF_FREQ_MIN: f32 = 40.0;
pub const LO_SHELF_FREQ_MAX: f32 = 1_000.0;
pub const SHELF_GAIN_MIN_DB: f32 = -18.0;
pub const CHORUS_RATE_MIN: f32 = 0.05;
pub const CHORUS_RATE_MAX: f32 = 8.0;

/// Exact-bypass values for the appended descriptor params (indices 5..=20,
/// descriptor order: mode, predelay, decay, diffusion, hi shelf freq/gain,
/// lo shelf freq/gain, in lo cut, in hi cut, stereo, chorus amount, chorus
/// rate, mod depth, wet gain, mix law). A Reverb slot saved before the
/// multi-mode fold carries only its five original params; on load it gets
/// these instead of the descriptor defaults, so galaxy mode renders it
/// exactly as before (including the original cube dry/wet law).
pub const LEGACY_BYPASS_DEFAULTS: [f32; 16] = [
    MODE_GALAXY,
    0.0,
    0.55,
    0.7,
    6_000.0,
    0.0,
    250.0,
    0.0,
    IN_LOCUT_OFF,
    IN_HICUT_OFF,
    1.0,
    0.0,
    0.7,
    0.15,
    0.0,
    MIX_LAW_CUBE,
];

/// Size changes alter every tank delay; keep a short glide (~4 ms).
const SIZE_SMOOTH_CUTOFF_HZ: f32 = 40.0;
/// Chorus: up to this much modulated delay on the wet path at full amount.
const CHORUS_MAX_MS: f32 = 6.0;

// ── Helpers ──

#[inline(always)]
fn clamp4(x: f32) -> f32 {
    x.clamp(-4.0, 4.0)
}

#[inline(always)]
fn clamp1(x: f32) -> f32 {
    x.clamp(-1.0, 1.0)
}

#[inline(always)]
fn flush(x: f32) -> f32 {
    if x.abs() < 1.0e-18 || !x.is_finite() {
        0.0
    } else {
        x
    }
}

/// Cut-only hi/lo damping shelves for a feedback path. Each stage is a
/// convex blend of the signal and its own 1-pole split, so with both gains in
/// (0, 1] the loop gain never exceeds the tank's own decay — unconditionally
/// stable. At 0 dB both stages are `x + 0.0 * t`, i.e. exact identity.
#[derive(Clone, Copy)]
pub(crate) struct ShelfCoefs {
    c_hi: f32,
    g_hi_m1: f32,
    c_lo: f32,
    g_lo_m1: f32,
}

impl ShelfCoefs {
    fn new(hi_freq: f32, hi_db: f32, lo_freq: f32, lo_db: f32, fs: f32) -> Self {
        Self {
            c_hi: one_pole_coef(hi_freq, fs),
            g_hi_m1: 10.0_f32.powf(hi_db.min(0.0) / 20.0) - 1.0,
            c_lo: one_pole_coef(lo_freq, fs),
            g_lo_m1: 10.0_f32.powf(lo_db.min(0.0) / 20.0) - 1.0,
        }
    }

    fn is_flat(&self) -> bool {
        self.g_hi_m1 == 0.0 && self.g_lo_m1 == 0.0
    }

    #[inline(always)]
    /// `st` = [hi-split lowpass, lo-split lowpass] states.
    fn apply(&self, x: f32, st: &mut [f32; 2]) -> f32 {
        st[0] += self.c_hi * (x - st[0]);
        let y = x + self.g_hi_m1 * (x - st[0]);
        st[1] += self.c_lo * (y - st[1]);
        y + self.g_lo_m1 * st[1]
    }
}

// ── Init ──

unsafe extern "C" fn reverb_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    for i in 0..REVERB_STATE_SIZE {
        *s.add(i) = 0.0;
    }

    // Param defaults (descriptor defaults mirror these — the "factory" voicing
    // for a freshly added Reverb). Pre-mode saved slots are instead filled
    // with `LEGACY_BYPASS_DEFAULTS`, see the descriptor sync.
    *s.add(ST_REPLACE) = 0.23;
    *s.add(ST_BRIGHT) = 0.88;
    *s.add(ST_DETUNE) = 0.1;
    *s.add(ST_BIGNESS) = 0.51;
    *s.add(ST_MIX) = 0.67;
    *s.add(ST_MODE) = MODE_GALAXY;
    *s.add(ST_PREDELAY_MS) = 0.0;
    *s.add(ST_DECAY) = 0.55;
    *s.add(ST_DIFFUSION) = 0.7;
    *s.add(ST_HI_SHELF_FREQ) = 1_128.0;
    *s.add(ST_HI_SHELF_GAIN) = -11.5;
    *s.add(ST_LO_SHELF_FREQ) = 130.0;
    *s.add(ST_LO_SHELF_GAIN) = -9.7;
    *s.add(ST_IN_LOCUT) = 103.0;
    *s.add(ST_IN_HICUT) = 1_892.0;
    *s.add(ST_WIDTH) = 1.0;
    *s.add(ST_CHORUS_AMT) = 0.26;
    *s.add(ST_CHORUS_RATE) = 0.7;
    *s.add(ST_MOD_DEPTH) = 0.15;
    *s.add(ST_ENABLED) = 1.0;
    *s.add(ST_WET_GAIN_DB) = 6.0;
    *s.add(ST_MIX_LAW) = MIX_LAW_LINEAR;

    let fs = sample_rate as f32;
    *s.add(ST_SAMPLE_RATE) = fs;
    *s.add(ST_SM_SCALE) = fs / DATTORRO_FS;
    *s.add(ST_SM_PREDELAY) = 1.0;
    *s.add(ST_RNG) = f32::from_bits(0x9e3779b9);
    // Adopt whatever mode the first process() sees — descriptors and tests
    // set params after init, and that first choice must not trigger a fade.
    *s.add(ST_ACTIVE_MODE) = -1.0;
    *s.add(ST_FADE) = 1.0;

    galaxy::seed(s);
}

// ── Process ──

unsafe extern "C" fn reverb_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let nf = nframes as usize;

    let in_l = *inp.add(0);
    let in_r = *inp.add(1);
    let out_l = *out.add(0);
    let out_r = *out.add(1);

    if *s.add(ST_ENABLED) <= 0.5 {
        std::ptr::copy_nonoverlapping(in_l as *const f32, out_l, nf);
        std::ptr::copy_nonoverlapping(in_r as *const f32, out_r, nf);
        return;
    }

    let mut fs = *s.add(ST_SAMPLE_RATE);
    if !(8000.0..=192_000.0).contains(&fs) {
        fs = 44_100.0;
    }

    // ── Read params (per block) ──
    let replace = *s.add(ST_REPLACE);
    let bright = *s.add(ST_BRIGHT);
    let detune = *s.add(ST_DETUNE);
    let bigness = *s.add(ST_BIGNESS);
    let mix = (*s.add(ST_MIX)).clamp(0.0, 1.0);
    let desired_mode = (*s.add(ST_MODE)).round().clamp(0.0, 2.0) as i32;
    let predelay_ms = (*s.add(ST_PREDELAY_MS)).clamp(0.0, 250.0);
    let decay = (*s.add(ST_DECAY)).clamp(0.0, 1.0);
    let diffusion = (*s.add(ST_DIFFUSION)).clamp(0.0, 1.0);
    let hi_shelf_freq = (*s.add(ST_HI_SHELF_FREQ)).clamp(HI_SHELF_FREQ_MIN, HI_SHELF_FREQ_MAX);
    let hi_shelf_db = (*s.add(ST_HI_SHELF_GAIN)).clamp(SHELF_GAIN_MIN_DB, 0.0);
    let lo_shelf_freq = (*s.add(ST_LO_SHELF_FREQ)).clamp(LO_SHELF_FREQ_MIN, LO_SHELF_FREQ_MAX);
    let lo_shelf_db = (*s.add(ST_LO_SHELF_GAIN)).clamp(SHELF_GAIN_MIN_DB, 0.0);
    let in_locut = (*s.add(ST_IN_LOCUT)).clamp(IN_LOCUT_OFF, IN_LOCUT_MAX);
    let in_hicut = (*s.add(ST_IN_HICUT)).clamp(IN_HICUT_MIN, IN_HICUT_OFF);
    let width = (*s.add(ST_WIDTH)).clamp(0.0, 1.0);
    let chorus_amt = (*s.add(ST_CHORUS_AMT)).clamp(0.0, 1.0);
    let chorus_rate = (*s.add(ST_CHORUS_RATE)).clamp(CHORUS_RATE_MIN, CHORUS_RATE_MAX);
    let mod_depth = (*s.add(ST_MOD_DEPTH)).clamp(0.0, 1.0);
    let wet_gain_db = (*s.add(ST_WET_GAIN_DB)).clamp(WET_GAIN_MIN_DB, WET_GAIN_MAX_DB);
    let cube_law = (*s.add(ST_MIX_LAW)).round() <= 0.5;

    // ── Derived (per block) ──
    // Exact-bypass gates: these compare against the descriptor's own range
    // ends, so a default slot never touches the audio.
    let locut_on = in_locut > IN_LOCUT_OFF + 0.5;
    let hicut_on = in_hicut < IN_HICUT_OFF - 0.5;
    let hp_coef = one_pole_coef(in_locut, fs);
    let lp_coef = one_pole_coef(in_hicut, fs);
    let width_on = width != 1.0;
    let chorus_on = chorus_amt > 0.0;
    let predelay_target = (predelay_ms * 0.001 * fs).max(1.0);
    let predelay_coef = one_pole_coef(8.0, fs);
    let scale_coef = one_pole_coef(SIZE_SMOOTH_CUTOFF_HZ, fs);
    let shelves = ShelfCoefs::new(hi_shelf_freq, hi_shelf_db, lo_shelf_freq, lo_shelf_db, fs);

    let lfo_inc = chorus_rate / fs;
    let fade_step = 1.0 / (0.025 * fs);
    let chorus_span = CHORUS_MAX_MS * 0.001 * fs * chorus_amt;

    // ── Load shared mutable state ──
    let mut active_mode = *s.add(ST_ACTIVE_MODE) as i32;
    let mut fade = (*s.add(ST_FADE)).clamp(0.0, 1.0);
    if active_mode < 0 {
        active_mode = desired_mode;
        fade = 1.0;
    }
    let mut sm_predelay = *s.add(ST_SM_PREDELAY);
    let mut sm_scale = *s.add(ST_SM_SCALE);
    if sm_scale <= 0.0 {
        sm_scale = (fs / DATTORRO_FS) * tank_size_scale(bigness);
    }
    let mut lfo_phase = *s.add(ST_LFO_PHASE);
    let mut rng = (*s.add(ST_RNG)).to_bits();
    if rng == 0 {
        rng = 0x9e3779b9;
    }
    let mut hp = [0.0f32; 4];
    let mut lp = [0.0f32; 4];
    for k in 0..4 {
        hp[k] = *s.add(ST_IN_HP + k);
        lp[k] = *s.add(ST_IN_LP + k);
    }
    let mut chorus_phase = *s.add(ST_CHORUS_PHASE);

    let mut rings = Rings {
        s,
        wpos: [0usize; NRING],
    };
    for b in 0..NRING {
        let w = *s.add(ST_WRITE_IDX + b);
        rings.wpos[b] = (w.max(0.0) as usize) % ring_cap(b);
    }

    // ── Front end: predelay → input cuts, written in place to `out` ──
    // (`out` doubles as the tank input; the dry signal for the mix is
    // always the untouched `in`.)
    let predelay_on = predelay_ms > 0.0 || sm_predelay > 1.001;
    for i in 0..nf {
        let mut l = *in_l.add(i);
        let mut r = *in_r.add(i);
        if predelay_on {
            sm_predelay += predelay_coef * (predelay_target - sm_predelay);
            rings.write(RB_PREDELAY_L, l);
            rings.write(RB_PREDELAY_R, r);
            l = rings.read_frac(RB_PREDELAY_L, sm_predelay);
            r = rings.read_frac(RB_PREDELAY_R, sm_predelay);
        }
        if locut_on {
            hp[0] += hp_coef * (l - hp[0]);
            l -= hp[0];
            hp[1] += hp_coef * (l - hp[1]);
            l -= hp[1];
            hp[2] += hp_coef * (r - hp[2]);
            r -= hp[2];
            hp[3] += hp_coef * (r - hp[3]);
            r -= hp[3];
        }
        if hicut_on {
            lp[0] += lp_coef * (l - lp[0]);
            lp[1] += lp_coef * (lp[0] - lp[1]);
            l = lp[1];
            lp[2] += lp_coef * (r - lp[2]);
            lp[3] += lp_coef * (lp[2] - lp[3]);
            r = lp[3];
        }
        *out_l.add(i) = l;
        *out_r.add(i) = r;
    }
    if !predelay_on {
        sm_predelay = 1.0;
    }

    // ── Tank: `out` (tank input) → `out` (raw wet) ──
    match active_mode {
        1 | 2 => {
            let target_scale = (fs / DATTORRO_FS) * tank_size_scale(bigness);
            let decay_g = (0.25 + 0.75 * decay.powf(1.4)).min(1.0);
            let walk_coef = one_pole_coef((chorus_rate * 0.5).max(0.05), fs);
            if active_mode == 1 {
                // Diffusion drives the tank allpasses, not just the input
                // diffusers — knob at 0.7 = exact paper gains.
                let dscale = (diffusion / 0.7).min(1.13);
                let p = plate::PlateParams {
                    decay_g,
                    ap1_g: diffusion.min(0.9),
                    ap2_g: ((decay + 0.15).clamp(0.25, 0.50) * (0.4 + 0.857 * diffusion)).min(0.6),
                    in_g1: (0.75 * dscale).min(0.85),
                    in_g2: (0.625 * dscale).min(0.85),
                    // Up to 3 ms; hardware sat nearer 0.5 ms.
                    exc_samps: mod_depth.powf(1.5) * 0.003 * fs,
                    walk_coef,
                    shelves,
                };
                let mut st = plate::PlateState::load(s, PLATE_RT);
                for i in 0..nf {
                    sm_scale += scale_coef * (target_scale - sm_scale);
                    lfo_phase += lfo_inc;
                    let mut wrapped = false;
                    if lfo_phase >= 1.0 {
                        lfo_phase -= 1.0;
                        wrapped = true;
                    }
                    let x = (*out_l.add(i) + *out_r.add(i)) * 0.5;
                    let (wl, wr) = plate::process_sample(
                        &mut rings, &mut st, &p, x, sm_scale, lfo_phase, wrapped, &mut rng,
                    );
                    *out_l.add(i) = wl;
                    *out_r.add(i) = wr;
                }
                st.store(s, PLATE_RT);
            } else {
                let dscale = (diffusion / 0.7).min(1.13);
                let p = hall::HallParams {
                    sect_g: decay_g.powf(0.25),
                    in_g: (0.75 * dscale).min(0.85),
                    dscale: (diffusion / 0.7).min(1.04),
                    // Steeper curve than the plate: the default depth should
                    // breathe, the top opens to the full 30 ms 224 wander.
                    exc_samps: (mod_depth.powi(2) * 0.030 * fs).min(HALL_EXC_CAP as f32),
                    walk_coef: one_pole_coef((chorus_rate * 0.35).max(0.05), fs),
                    shelves,
                };
                let mut st = hall::HallState::load(s, HALL_RT);
                for i in 0..nf {
                    sm_scale += scale_coef * (target_scale - sm_scale);
                    let q_before = ((lfo_phase * 4.0) as usize).min(3);
                    lfo_phase += lfo_inc;
                    let mut wrapped = false;
                    if lfo_phase >= 1.0 {
                        lfo_phase -= 1.0;
                        wrapped = true;
                    }
                    let q_after = ((lfo_phase * 4.0) as usize).min(3);
                    let x = (*out_l.add(i) + *out_r.add(i)) * 0.5;
                    let (wl, wr) = hall::process_sample(
                        &mut rings, &mut st, &p, x, sm_scale, lfo_phase, q_before, q_after,
                        wrapped, &mut rng,
                    );
                    *out_l.add(i) = wl;
                    *out_r.add(i) = wr;
                }
                st.store(s, HALL_RT);
            }
        }
        _ => {
            // Galaxy's loop runs sub-sampled; design its shelves at that rate.
            let overallscale = fs / 44100.0;
            let cycle_end = (overallscale.floor() as i32).clamp(1, 4) * 4;
            let galaxy_shelves = if shelves.is_flat() {
                None
            } else {
                Some(ShelfCoefs::new(
                    hi_shelf_freq,
                    hi_shelf_db,
                    lo_shelf_freq,
                    lo_shelf_db,
                    fs / cycle_end as f32,
                ))
            };
            let p = galaxy::GalaxyParams {
                sample_rate: fs,
                replace,
                bright,
                detune,
                bigness,
                shelves: galaxy_shelves,
            };
            galaxy::process_block(s, out_l, out_r, out_l, out_r, nf, &p);
            // Keep the size glide primed so a switch into plate/hall starts
            // at the right scale instead of gliding from wherever it was.
            sm_scale = (fs / DATTORRO_FS) * tank_size_scale(bigness);
        }
    }

    // ── Back end: chorus → width → makeup → fade → mix with the untouched dry ──
    // Galaxy's cube mix law and ±1 clamp are part of the legacy identity (old
    // slots load with `mix law` = cube); everything else is a plain linear
    // crossfade (dry −6 dB at 50%, like Ableton's Dry/Wet — equal power left
    // the dry audibly untouched until far up the knob). The Galactic tank
    // runs well under unity, so `wet gain` is the makeup that lets 100% wet
    // meter like the dry.
    let galaxy_mix = active_mode == 0 && cube_law;
    let g_wet = 1.0 - (1.0 - mix) * (1.0 - mix) * (1.0 - mix);
    let g_dry = 1.0 - g_wet;
    let lin_dry = 1.0 - mix;
    let makeup_on = wet_gain_db != 0.0;
    let makeup = 10.0_f32.powf(wet_gain_db / 20.0);
    for i in 0..nf {
        let mut wl = *out_l.add(i);
        let mut wr = *out_r.add(i);

        if chorus_on {
            // Pitch-wobble the tail (no comb: the delayed read replaces the
            // wet rather than mixing with it). Delay grows continuously from
            // 1 sample at amount 0⁺, quadrature L/R for width.
            rings.write(RB_CHORUS_L, wl);
            rings.write(RB_CHORUS_R, wr);
            chorus_phase += lfo_inc;
            if chorus_phase >= 1.0 {
                chorus_phase -= 1.0;
            }
            let ph = chorus_phase * std::f32::consts::TAU;
            let dl = 1.0 + chorus_span * (0.5 + 0.5 * ph.sin());
            let dr = 1.0 + chorus_span * (0.5 + 0.5 * ph.cos());
            wl = rings.read_frac(RB_CHORUS_L, dl);
            wr = rings.read_frac(RB_CHORUS_R, dr);
        }

        if width_on {
            let mid = (wl + wr) * 0.5;
            let side = (wl - wr) * 0.5 * width;
            wl = mid + side;
            wr = mid - side;
        }
        if makeup_on {
            wl *= makeup;
            wr *= makeup;
        }

        // Mode-switch wet fade: ramp down while a switch is pending, ramp
        // back up once the swap (at block end) has happened.
        if desired_mode != active_mode {
            fade = (fade - fade_step).max(0.0);
        } else if fade < 1.0 {
            fade = (fade + fade_step).min(1.0);
        }

        let dry_l = *in_l.add(i);
        let dry_r = *in_r.add(i);
        if galaxy_mix {
            let w = g_wet * fade;
            *out_l.add(i) = clamp1(wl * w + dry_l * g_dry);
            *out_r.add(i) = clamp1(wr * w + dry_r * g_dry);
        } else {
            let fg = fade * mix;
            *out_l.add(i) = clamp4(dry_l * lin_dry + wl * fg);
            *out_r.add(i) = clamp4(dry_r * lin_dry + wr * fg);
        }
    }

    // ── Mode swap at the block boundary once the wet has faded out ──
    if desired_mode != active_mode && fade <= 0.0 {
        active_mode = desired_mode;
        match active_mode {
            1 => {
                rings.clear(PLATE_BUFS);
                plate::PlateState::default().store(s, PLATE_RT);
            }
            2 => {
                rings.clear(HALL_BUFS);
                hall::HallState::default().store(s, HALL_RT);
            }
            _ => galaxy::clear(s),
        }
    }

    // ── Store shared state ──
    *s.add(ST_ACTIVE_MODE) = active_mode as f32;
    *s.add(ST_FADE) = fade;
    *s.add(ST_SM_PREDELAY) = sm_predelay;
    *s.add(ST_SM_SCALE) = sm_scale;
    *s.add(ST_LFO_PHASE) = lfo_phase;
    *s.add(ST_RNG) = f32::from_bits(rng);
    for k in 0..4 {
        *s.add(ST_IN_HP + k) = flush(hp[k]);
        *s.add(ST_IN_LP + k) = flush(lp[k]);
    }
    *s.add(ST_CHORUS_PHASE) = chorus_phase;
    for b in 0..NRING {
        *s.add(ST_WRITE_IDX + b) = rings.wpos[b] as f32;
    }
}

pub fn reverb_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(reverb_process),
        init: Some(reverb_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    /// Init, then apply the pre-mode base values and the legacy bypass set for
    /// the appended params — the state an old saved slot lands in.
    fn init_state(sample_rate: i32) -> Vec<f32> {
        let mut state = vec![0.0f32; REVERB_STATE_SIZE];
        unsafe {
            reverb_init(state.as_mut_ptr().cast(), sample_rate, 64, ptr::null());
        }
        state[ST_REPLACE] = 0.3;
        state[ST_BRIGHT] = 0.8;
        state[ST_BIGNESS] = 0.2;
        state[ST_MIX] = 0.35;
        let slots = [
            ST_MODE, ST_PREDELAY_MS, ST_DECAY, ST_DIFFUSION, ST_HI_SHELF_FREQ, ST_HI_SHELF_GAIN,
            ST_LO_SHELF_FREQ, ST_LO_SHELF_GAIN, ST_IN_LOCUT, ST_IN_HICUT, ST_WIDTH, ST_CHORUS_AMT,
            ST_CHORUS_RATE, ST_MOD_DEPTH, ST_WET_GAIN_DB, ST_MIX_LAW,
        ];
        for (slot, value) in slots.iter().zip(LEGACY_BYPASS_DEFAULTS) {
            state[*slot] = value;
        }
        state
    }

    #[test]
    fn wet_gain_is_makeup_on_the_wet_path_only() {
        let fs = 44_100;
        let render = |gain_db: f32, mix: f32| {
            let mut state = init_state(fs as i32);
            state[ST_MODE] = MODE_PLATE;
            state[ST_MIX] = mix;
            state[ST_WET_GAIN_DB] = gain_db;
            impulse_render(&mut state, 1.0, fs).0
        };
        let flat = energy(&render(0.0, 1.0));
        let up = energy(&render(6.0, 1.0));
        let ratio = up / flat;
        assert!((ratio - 3.98).abs() < 0.2, "+6 dB should ~4x wet energy, got {ratio}");
        // Fully dry: makeup must not touch the output.
        assert_eq!(render(12.0, 0.0), render(0.0, 0.0));
    }

    #[test]
    fn galaxy_mix_law_switches_between_cube_and_linear() {
        let fs = 44_100;
        let render = |law: f32| {
            let mut state = init_state(fs as i32);
            state[ST_MIX] = 0.5;
            state[ST_MIX_LAW] = law;
            let mut in_l = vec![0.5f32; 64];
            let mut in_r = vec![0.5f32; 64];
            process(&mut state, &mut in_l, &mut in_r).0[0]
        };
        // First sample is pure dry (the tank has not produced output yet):
        // cube law leaves 12.5% of it, linear 50%.
        let cube = render(MIX_LAW_CUBE);
        let linear = render(MIX_LAW_LINEAR);
        assert!((cube - 0.5 * 0.125).abs() < 1e-4, "cube dry gain: {cube}");
        assert!((linear - 0.25).abs() < 1e-4, "linear dry gain: {linear}");
    }

    #[test]
    fn factory_defaults_differ_from_legacy_bypass_and_stay_finite() {
        let mut state = vec![0.0f32; REVERB_STATE_SIZE];
        unsafe {
            reverb_init(state.as_mut_ptr().cast(), 44_100, 64, ptr::null());
        }
        assert_ne!(state[ST_IN_LOCUT], IN_LOCUT_OFF);
        let (out_l, out_r) = impulse_render(&mut state, 2.0, 44_100);
        assert!(out_l.iter().chain(out_r.iter()).all(|v| v.is_finite()));
        assert!(energy(&out_l) > 1.0e-4);
    }

    fn process(state: &mut [f32], in_l: &mut [f32], in_r: &mut [f32]) -> (Vec<f32>, Vec<f32>) {
        let nf = in_l.len();
        let inputs = [in_l.as_mut_ptr(), in_r.as_mut_ptr()];
        let mut out_l = vec![0.0f32; nf];
        let mut out_r = vec![0.0f32; nf];
        let outputs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        unsafe {
            reverb_process(
                inputs.as_ptr(),
                outputs.as_ptr(),
                nf as c_int,
                state.as_mut_ptr().cast(),
                ptr::null_mut(),
            );
        }
        (out_l, out_r)
    }

    /// Render an impulse then silence; returns (out_l, out_r) over `secs`.
    fn impulse_render(state: &mut [f32], secs: f32, fs: usize) -> (Vec<f32>, Vec<f32>) {
        let total = (secs * fs as f32) as usize;
        let mut acc_l = Vec::with_capacity(total);
        let mut acc_r = Vec::with_capacity(total);
        let block = 256;
        let mut first = true;
        let mut done = 0;
        while done < total {
            let n = block.min(total - done);
            let mut in_l = vec![0.0f32; n];
            let mut in_r = vec![0.0f32; n];
            if first {
                in_l[0] = 1.0;
                in_r[0] = 1.0;
                first = false;
            }
            let (ol, or) = process(state, &mut in_l, &mut in_r);
            acc_l.extend_from_slice(&ol);
            acc_r.extend_from_slice(&or);
            done += n;
        }
        (acc_l, acc_r)
    }

    fn energy(sig: &[f32]) -> f64 {
        sig.iter().map(|x| (*x as f64) * (*x as f64)).sum()
    }

    /// Deterministic 2 s render (LCG noise, 256-frame blocks) hashed bit-for-bit.
    fn golden_hash(state: &mut [f32]) -> u64 {
        let fs = 44_100usize;
        let total = fs * 2;
        let mut lcg: u32 = 0x1234_5678;
        let mut next = move || {
            lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((lcg >> 8) as f32 / 16_777_216.0) * 1.6 - 0.8
        };
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut done = 0;
        while done < total {
            let n = 256.min(total - done);
            let mut in_l: Vec<f32> = (0..n).map(|_| next()).collect();
            let mut in_r: Vec<f32> = (0..n).map(|_| next() * 0.5).collect();
            let (out_l, out_r) = process(state, &mut in_l, &mut in_r);
            for v in out_l.iter().chain(out_r.iter()) {
                hash ^= v.to_bits() as u64;
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            done += n;
        }
        hash
    }

    /// The hashes were captured from the pre-multi-mode `effects/reverb.rs`
    /// with the identical harness. Galaxy mode at the appended defaults must
    /// reproduce them exactly — that is what keeps old projects unchanged.
    #[test]
    fn galaxy_mode_matches_the_pre_multimode_reverb_bit_for_bit() {
        let mut state = init_state(44_100);
        state[ST_MIX] = 0.35;
        assert_eq!(golden_hash(&mut state), 0x9774_eefa_066f_9c75);

        let mut state = init_state(44_100);
        state[ST_BIGNESS] = 0.7;
        state[ST_BRIGHT] = 0.4;
        state[ST_REPLACE] = 0.6;
        state[ST_MIX] = 0.5;
        assert_eq!(golden_hash(&mut state), 0xe5eb_db84_3879_e643);
    }

    #[test]
    fn disabled_reverb_passes_stereo_inputs_independently() {
        let mut state = init_state(44_100);
        state[ST_ENABLED] = 0.0;
        let mut in_l = vec![0.25f32, -0.5, 0.75, -1.0];
        let mut in_r = vec![-0.125f32, 0.375, -0.625, 0.875];
        let (out_l, out_r) = process(&mut state, &mut in_l, &mut in_r);
        assert_eq!(out_l, in_l);
        assert_eq!(out_r, in_r);
    }

    #[test]
    fn every_mode_produces_a_decaying_stereo_tail() {
        let fs = 44_100;
        for mode in [MODE_GALAXY, MODE_PLATE, MODE_HALL] {
            let mut state = init_state(fs as i32);
            state[ST_MODE] = mode;
            state[ST_MIX] = 1.0;
            let (out_l, out_r) = impulse_render(&mut state, 3.0, fs);
            assert!(out_l.iter().chain(out_r.iter()).all(|v| v.is_finite()));
            let early = energy(&out_l[..fs / 2]);
            let late = energy(&out_l[fs * 2..fs * 5 / 2]);
            assert!(early > 1.0e-4, "mode {mode}: no early energy ({early})");
            assert!(
                late < early,
                "mode {mode}: tail did not decay ({early} -> {late})"
            );
            if mode != MODE_GALAXY {
                // Plate and hall pull L/R from disjoint tap sets.
                let diff: f64 = out_l
                    .iter()
                    .zip(&out_r)
                    .map(|(l, r)| ((*l - *r) as f64).powi(2))
                    .sum();
                assert!(diff > 1.0e-3, "mode {mode}: output is not stereo");
            }
        }
    }

    #[test]
    fn longer_decay_holds_more_late_energy_in_plate_and_hall() {
        let fs = 44_100;
        for mode in [MODE_PLATE, MODE_HALL] {
            let mut late = Vec::new();
            for decay in [0.2f32, 0.6, 0.95] {
                let mut state = init_state(fs as i32);
                state[ST_MODE] = mode;
                state[ST_MIX] = 1.0;
                state[ST_DECAY] = decay;
                let (out_l, _) = impulse_render(&mut state, 3.0, fs);
                late.push(energy(&out_l[fs * 2..fs * 3]));
            }
            assert!(
                late[0] < late[1] && late[1] < late[2],
                "mode {mode}: late energy not monotonic in decay: {late:?}"
            );
        }
    }

    #[test]
    fn mode_switch_fades_clears_and_stays_clean() {
        let fs = 44_100;
        let mut state = init_state(fs as i32);
        state[ST_MODE] = MODE_GALAXY;
        state[ST_MIX] = 1.0;
        state[ST_DECAY] = 0.9;
        let _ = impulse_render(&mut state, 0.5, fs);

        for next in [MODE_PLATE, MODE_HALL, MODE_GALAXY] {
            state[ST_MODE] = next;
            let mut peak = 0.0f32;
            let mut faded_to_zero = false;
            for _ in 0..40 {
                let mut in_l = vec![0.0f32; 256];
                let mut in_r = vec![0.0f32; 256];
                let (ol, or) = process(&mut state, &mut in_l, &mut in_r);
                for v in ol.iter().chain(or.iter()) {
                    assert!(v.is_finite());
                    peak = peak.max(v.abs());
                }
                if state[ST_FADE] == 0.0 || state[ST_ACTIVE_MODE] == next {
                    faded_to_zero = true;
                }
            }
            assert!(faded_to_zero, "switch to {next} never completed");
            assert_eq!(state[ST_ACTIVE_MODE], next);
            assert!(peak < 2.0, "switch to {next} spiked to {peak}");
            // The incoming tank starts from silence.
            let mut in_l = vec![0.0f32; 256];
            let mut in_r = vec![0.0f32; 256];
            let _ = process(&mut state, &mut in_l, &mut in_r);
        }
        // After the last switch the fade is back up and the tank is live.
        assert!(state[ST_FADE] > 0.0);
    }

    #[test]
    fn shelves_at_full_cut_stay_finite_and_darken_the_tail() {
        let fs = 44_100;
        for mode in [MODE_GALAXY, MODE_PLATE, MODE_HALL] {
            let flat = {
                let mut state = init_state(fs as i32);
                state[ST_MODE] = mode;
                state[ST_MIX] = 1.0;
                state[ST_DECAY] = 1.0;
                state[ST_REPLACE] = 0.2;
                impulse_render(&mut state, 4.0, fs).0
            };
            let cut = {
                let mut state = init_state(fs as i32);
                state[ST_MODE] = mode;
                state[ST_MIX] = 1.0;
                state[ST_DECAY] = 1.0;
                state[ST_REPLACE] = 0.2;
                state[ST_HI_SHELF_GAIN] = SHELF_GAIN_MIN_DB;
                state[ST_LO_SHELF_GAIN] = SHELF_GAIN_MIN_DB;
                state[ST_HI_SHELF_FREQ] = HI_SHELF_FREQ_MIN;
                state[ST_LO_SHELF_FREQ] = LO_SHELF_FREQ_MAX;
                impulse_render(&mut state, 4.0, fs).0
            };
            assert!(
                cut.iter().all(|v| v.is_finite()),
                "mode {mode}: shelves blew up"
            );
            let flat_late = energy(&flat[fs * 3..fs * 4]);
            let cut_late = energy(&cut[fs * 3..fs * 4]);
            assert!(
                cut_late < flat_late,
                "mode {mode}: full-cut shelves did not drain the tail ({flat_late} -> {cut_late})"
            );
        }
    }

    #[test]
    fn shared_stages_change_the_render_and_bypass_exactly_at_defaults() {
        let fs = 44_100;
        let baseline = {
            let mut state = init_state(fs as i32);
            state[ST_MODE] = MODE_PLATE;
            state[ST_MIX] = 0.5;
            golden_hash(&mut state)
        };
        // Same defaults again: deterministic.
        let again = {
            let mut state = init_state(fs as i32);
            state[ST_MODE] = MODE_PLATE;
            state[ST_MIX] = 0.5;
            golden_hash(&mut state)
        };
        assert_eq!(baseline, again);

        let tweaks: [(usize, f32); 7] = [
            (ST_IN_LOCUT, 400.0),
            (ST_IN_HICUT, 3_000.0),
            (ST_WIDTH, 0.3),
            (ST_CHORUS_AMT, 0.6),
            (ST_PREDELAY_MS, 40.0),
            (ST_HI_SHELF_GAIN, -9.0),
            (ST_MOD_DEPTH, 0.9),
        ];
        for (slot, value) in tweaks {
            let mut state = init_state(fs as i32);
            state[ST_MODE] = MODE_PLATE;
            state[ST_MIX] = 0.5;
            state[slot] = value;
            let h = golden_hash(&mut state);
            assert_ne!(h, baseline, "slot {slot} = {value} had no audible effect");
        }
    }

    #[test]
    fn predelay_delays_the_onset() {
        let fs = 44_100;
        let mut state = init_state(fs as i32);
        state[ST_MODE] = MODE_PLATE;
        state[ST_MIX] = 1.0;
        state[ST_PREDELAY_MS] = 100.0;
        // Let the predelay smoother settle before the impulse.
        let mut in_l = vec![0.0f32; fs];
        let mut in_r = vec![0.0f32; fs];
        let _ = process(&mut state, &mut in_l, &mut in_r);
        let (out_l, _) = impulse_render(&mut state, 0.5, fs);
        let before = energy(&out_l[..fs * 90 / 1000]);
        let after = energy(&out_l[fs * 100 / 1000..fs * 200 / 1000]);
        assert!(before < 1.0e-9, "energy before the predelay: {before}");
        assert!(after > 1.0e-5, "no energy after the predelay: {after}");
    }

    #[test]
    fn extreme_params_at_all_sample_rates_stay_finite() {
        for fs in [44_100usize, 48_000, 96_000] {
            for mode in [MODE_GALAXY, MODE_PLATE, MODE_HALL] {
                for size in [0.0f32, 1.0] {
                    let mut state = init_state(fs as i32);
                    state[ST_MODE] = mode;
                    state[ST_MIX] = 1.0;
                    state[ST_BIGNESS] = size;
                    state[ST_DECAY] = 1.0;
                    state[ST_DIFFUSION] = 1.0;
                    state[ST_MOD_DEPTH] = 1.0;
                    state[ST_CHORUS_AMT] = 1.0;
                    state[ST_CHORUS_RATE] = CHORUS_RATE_MAX;
                    state[ST_PREDELAY_MS] = 250.0;
                    state[ST_WIDTH] = 0.0;
                    state[ST_IN_LOCUT] = IN_LOCUT_MAX;
                    state[ST_IN_HICUT] = IN_HICUT_MIN;
                    let (out_l, out_r) = impulse_render(&mut state, 1.0, fs);
                    assert!(
                        out_l.iter().chain(out_r.iter()).all(|v| v.is_finite()),
                        "fs {fs} mode {mode} size {size}: non-finite output"
                    );
                }
            }
        }
    }

    #[test]
    fn state_layout_stays_inside_the_state_array() {
        assert!(ST_CHORUS_PHASE < GALAXY_RT);
        assert!(ST_WRITE_IDX + NRING == ST_BUFS);
        for b in 0..NRING {
            assert!(RING_OFFSETS[b] + ring_cap(b) <= GALAXY_BUF_BASE);
        }
        assert!(GALAXY_BUF_BASE + galaxy::total_buf_floats() == REVERB_STATE_SIZE);
        // Max size at 96 kHz plus the deepest modulation must fit every ring
        // (read_frac clamps, but a clamped read is a wrong read).
        let max_scale = (96_000.0 / DATTORRO_FS) * tank_size_scale(1.0);
        for b in RB_DIFF1..RB_CHORUS_L {
            let need = BASE_LENS[b] as f32 * max_scale
                + if (RB_H_DEL..RB_CHORUS_L).contains(&b) {
                    HALL_EXC_CAP as f32
                } else {
                    0.003 * 96_000.0
                };
            assert!(
                (need as usize) + 2 <= ring_cap(b),
                "ring {b} needs {need} but holds {}",
                ring_cap(b)
            );
        }
        assert!(
            ((1.0 + CHORUS_MAX_MS * 0.001 * 96_000.0) as usize) + 2 <= CHORUS_CAP,
            "chorus ring too small"
        );
        assert!(((250.0 * 0.001 * 96_000.0) as usize) + 2 <= PREDELAY_CAP);
    }
}
