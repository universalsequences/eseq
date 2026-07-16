use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

// Multiverb — multi-mode vintage reverb (spec: docs/reverb-modes-spec.md).
//
// Plate (phase 1): Dattorro, "Effect Design Part 1: Reverberator and Other
// Filters" (JAES 45(9), 1997) — predelay → bandwidth LP → 4 series input
// diffusers → cross-coupled figure-eight tank with modulated decay-diffusion
// allpasses and fixed output tap tables.
//
// Hall (phase 2): Lexicon 224 Concert Hall topology — 2 input diffusers →
// a single loop of 4 sections (allpass → allpass → modulated delay), damping
// LP + bass shelf applied once per loop pass, decay gain distributed per
// section, L/R outputs pulled from two disjoint 4-tap sets. Modulation is a
// per-section random walk (Griesinger "spin"/"wander") with excursions up to
// 30 ms — the thing that keeps very long decays from going metallic and
// audibly detunes the tail.
//
// Quad (phase 3): Alesis Quadraverb topology (Costello) — 4 allpass loops,
// each 2 allpasses + 1 delay in a feedback loop, CASCADED: the end of each
// loop feeds the next loop's input. LF/HF decay shelves inside every loop,
// output taps summed from all four loops. 1988 16-bit DSP character: the era
// knob is biased high here (word-length grit + truncated interpolation on
// reads), and it runs essentially unmodulated by default. Dense, smeared,
// slightly metallic is correct for this box — don't over-smooth it.
//
// Mod (phase 4): Freeverb (Jezar at Dreampoint, standard tunings @44.1 kHz)
// — per channel 8 parallel lowpass-feedback combs → 4 series allpasses,
// right channel +23 samples (stereospread) — except the four allpasses per
// channel get interpolated, LFO-modulated delay reads at detuned rates
// (0.71/0.93/1.17/1.39 × mod rate), depth to ±48 samples, mod shape blending
// LFO → random walk. Shallow = chorusing shimmer, deep = seasick
// pitch-smeared tails. At mod depth 0 the mod path is exactly zero — it
// nulls against an unmodulated render regardless of rate/shape.
//
// Mode switching drops the old tail: fade wet to zero over ~25 ms, clear the
// incoming mode's tank, fade back in (locked decision — matches hardware
// program-change behavior; only the active mode's tank runs per sample).

// ── Modes ──

pub const MODE_PLATE: f32 = 0.0;
pub const MODE_HALL: f32 = 1.0;
pub const MODE_QUAD: f32 = 2.0;
pub const MODE_MOD: f32 = 3.0;

// ── Param state slots (descriptor writes raw f32 here) ──

const ST_MODE: usize = 0;
const ST_DECAY: usize = 1;
const ST_SIZE: usize = 2;
const ST_PREDELAY_MS: usize = 3;
const ST_DAMP: usize = 4;
const ST_BASS: usize = 5;
const ST_DIFFUSION: usize = 6;
const ST_MOD_RATE: usize = 7;
const ST_MOD_DEPTH: usize = 8;
const ST_MOD_SHAPE: usize = 9;
const ST_ERA: usize = 10;
const ST_WIDTH: usize = 11;
const ST_MIX: usize = 12;
const ST_ENABLED: usize = 13;

// Host-modulation depths. Descriptor order is append-only, so these appear
// after `enabled` in EffectDescriptor even though their state slots sit next
// to the base parameters. Four fixed modulator inputs can independently drive
// each destination.
const ST_MOD_DECAY_DEPTH_1: usize = 14;
const ST_MOD_SIZE_DEPTH_1: usize = 18;
const ST_MOD_DEPTH_DEPTH_1: usize = 22;
const ST_MOD_MIX_DEPTH_1: usize = 26;

// ── Runtime state slots ──

const ST_SAMPLE_RATE: usize = 30;
const ST_SM_SCALE: usize = 31; // smoothed tank delay scale (fs ratio × size)
const ST_SM_PREDELAY: usize = 32; // smoothed predelay in samples
const ST_LFO_PHASE: usize = 33;
const ST_RNG: usize = 34; // xorshift32 state stored as bits
const ST_BW_LP: usize = 35;
const ST_ACTIVE_MODE: usize = 36; // currently-running tank; -1 = adopt param
const ST_FADE: usize = 37; // wet fade gain for mode switching

// Plate runtime.
const ST_P_WALK_L: usize = 38;
const ST_P_WALK_TGT_L: usize = 39;
const ST_P_WALK_R: usize = 40;
const ST_P_WALK_TGT_R: usize = 41;
const ST_P_DAMP_L: usize = 42;
const ST_P_DAMP_R: usize = 43;
const ST_P_BASS_L: usize = 44;
const ST_P_BASS_R: usize = 45;

// Hall runtime.
const ST_H_DAMP: usize = 46;
const ST_H_BASS: usize = 47;
const ST_H_WALK: usize = 48; // 4 per-section random walks
const ST_H_WALK_TGT: usize = 52; // 4 walk targets

// Quad runtime (per-loop damping/bass shelves + shallow mod walks).
const ST_Q_DAMP: usize = 56; // 4
const ST_Q_BASS: usize = 60; // 4
const ST_Q_WALK: usize = 64; // 4
const ST_Q_WALK_TGT: usize = 68; // 4

// Error-feedback accumulators for the era word-length quantizers (carrying
// the quantization error into the next sample keeps the grit but removes the
// systematic energy loss that otherwise strangles long tails, and kills
// limit cycles).
const ST_P_QERR_L: usize = 72;
const ST_P_QERR_R: usize = 73;
const ST_H_QERR: usize = 74;
const ST_Q_QERR: usize = 75; // 4

// Quad per-loop DC blockers: four cascaded feedback loops multiply their
// subsonic gains (each ≈ 1/(1−g)), so without these a click train pumps
// enormous <30 Hz energy into the tail, especially at small sizes.
const ST_Q_DC: usize = 79; // 4

// Mod (Freeverb) runtime: per-comb damping LP + LF-decay shelf (8 combs × 2
// channels), 4 detuned allpass LFO phases and 4 random walks.
const ST_M_DAMP: usize = 83; // 16: L combs 0-7, then R combs 0-7
const ST_M_BASS: usize = 99; // 16
const ST_M_LFO: usize = 115; // 4
const ST_M_WALK: usize = 119; // 4
const ST_M_WALK_TGT: usize = 123; // 4
const ST_M_QERR: usize = 127; // 16 comb-loop error-feedback accumulators

const ST_WRITE_IDX: usize = 143; // NBUFS write positions

// ── Delay buffers ──

const NBUFS: usize = 63;

const BUF_PREDELAY: usize = 0;
const BUF_DIFF1: usize = 1;
const BUF_DIFF2: usize = 2;
const BUF_DIFF3: usize = 3;
const BUF_DIFF4: usize = 4;
const BUF_AP_L1: usize = 5;
const BUF_DEL_L1: usize = 6;
const BUF_AP_L2: usize = 7;
const BUF_DEL_L2: usize = 8;
const BUF_AP_R1: usize = 9;
const BUF_DEL_R1: usize = 10;
const BUF_AP_R2: usize = 11;
const BUF_DEL_R2: usize = 12;
// Hall: 2 input diffusers, 4 sections × (AP, AP) then 4 section delays.
const BUF_H_DIFF1: usize = 13;
const BUF_H_DIFF2: usize = 14;
const BUF_H_AP: usize = 15; // 15..23: section k allpasses at +2k, +2k+1
const BUF_H_DEL: usize = 23; // 23..27: section k delay at +k
                             // Quad: 4 cascaded loops, each 2 allpasses + 1 delay.
const BUF_Q_AP: usize = 27; // 27..35: loop k allpasses at +2k, +2k+1
const BUF_Q_DEL: usize = 35; // 35..39: loop k delay at +k
                             // Mod (Freeverb): 8 combs + 4 allpasses per channel. NOTE: these base
                             // lengths are samples at 44,100 Hz (Freeverb's reference), not 29,761.
const BUF_M_COMB_L: usize = 39; // 39..47
const BUF_M_COMB_R: usize = 47; // 47..55
const BUF_M_AP_L: usize = 55; // 55..59 (modulated)
const BUF_M_AP_R: usize = 59; // 59..63 (modulated)

const PLATE_BUFS: std::ops::Range<usize> = BUF_DIFF1..BUF_H_DIFF1;
const HALL_BUFS: std::ops::Range<usize> = BUF_H_DIFF1..BUF_Q_AP;
const QUAD_BUFS: std::ops::Range<usize> = BUF_Q_AP..BUF_M_COMB_L;
const MOD_BUFS: std::ops::Range<usize> = BUF_M_COMB_L..NBUFS;

/// Dattorro reference sample rate; all base lengths below are samples at it
/// (the hall lengths use the same convention so one smoothed scale serves
/// every tank).
const DATTORRO_FS: f32 = 29_761.0;

/// Base delay lengths at 29,761 Hz (predelay slot unused — fixed capacity).
const BASE_LENS: [usize; NBUFS] = [
    0, // predelay
    142, 107, 379, 277, // plate input diffusers
    672, 4453, 1800, 3720, // plate left tank: AP1 (modulated), del1, AP2, del2
    908, 4217, 2656, 3163, // plate right tank: AP1 (modulated), del1, AP2, del2
    131, 239, // hall input diffusers
    229, 411, 331, 527, 433, 359, 283, 617, // hall section APs (4 × 2)
    743, 907, 1123, 1429, // hall section delays (modulated, deep)
    181, 293, 379, 251, 467, 331, 283, 541, // quad loop APs (4 × 2)
    997, 1373, 1777, 2251, // quad loop delays (shallow mod)
    // Mod (Freeverb) — samples at 44,100 Hz:
    1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617, // left combs
    1139, 1211, 1300, 1379, 1445, 1514, 1580, 1640, // right combs (+23)
    556, 441, 341, 225, // left allpasses (modulated)
    579, 464, 364, 248, // right allpasses (+23, modulated)
];

/// Freeverb reference sample rate for the mod-mode buffers; the shared
/// smoothed scale (fs / 29,761 × size) is converted with this ratio.
const FREEVERB_FS: f32 = 44_100.0;
const MOD_SCALE_RATIO: f32 = DATTORRO_FS / FREEVERB_FS;

/// Per-allpass LFO rate detune factors × mod rate (mod mode).
const M_AP_RATES: [f32; 4] = [0.71, 0.93, 1.17, 1.39];

/// Quad loop round-trip times in reference seconds (loop AP1 + AP2 + delay
/// base lengths / 29,761) — used to derive per-loop feedback gains so every
/// loop decays at the same rate.
const Q_LOOP_T: [f32; 4] = [
    (181 + 293 + 997) as f32 / DATTORRO_FS,
    (379 + 251 + 1373) as f32 / DATTORRO_FS,
    (467 + 331 + 1777) as f32 / DATTORRO_FS,
    (283 + 541 + 2251) as f32 / DATTORRO_FS,
];

/// 250 ms at 96 kHz plus interpolation headroom.
const PREDELAY_CAP: usize = 24_064;

/// Max modulation excursion of the hall section delays: 30 ms at 96 kHz.
const HALL_EXC_CAP: usize = 2_880;

/// Capacity per buffer: base × 6.5 covers fs up to 96 kHz (×3.226) at max
/// size (×2.0). Plate buffers get +544 headroom (3 ms max mod excursion at
/// 96 kHz = 288, plus interpolation and rounding margin); the hall section
/// delays are modulated far deeper (up to 30 ms) and get +HALL_EXC_CAP+64.
const fn buf_cap(i: usize) -> usize {
    if i == BUF_PREDELAY {
        PREDELAY_CAP
    } else if i >= BUF_H_DEL && i < BUF_Q_AP {
        BASE_LENS[i] * 13 / 2 + HALL_EXC_CAP + 64
    } else if i >= BUF_M_COMB_L {
        // 44.1 kHz reference: max scale is 96k/44.1k × size 2.0 ≈ 4.36; +544
        // covers the ±48-sample (scaled ≈ ±105) allpass mod excursion.
        BASE_LENS[i] * 9 / 2 + 544
    } else {
        BASE_LENS[i] * 13 / 2 + 544
    }
}

const fn buf_offsets() -> [usize; NBUFS] {
    let mut offsets = [0usize; NBUFS];
    let mut offset = ST_BUFS;
    let mut i = 0;
    while i < NBUFS {
        offsets[i] = offset;
        offset += buf_cap(i);
        i += 1;
    }
    offsets
}

const ST_BUFS: usize = ST_WRITE_IDX + NBUFS;

const fn total_buf_floats() -> usize {
    let mut total = 0;
    let mut i = 0;
    while i < NBUFS {
        total += buf_cap(i);
        i += 1;
    }
    total
}

const BUF_OFFSETS: [usize; NBUFS] = buf_offsets();

pub const MULTIVERB_STATE_SIZE: usize = ST_BUFS + total_buf_floats();

// Public param indices for the descriptor.
pub const MULTIVERB_PARAM_MODE: u64 = ST_MODE as u64;
pub const MULTIVERB_PARAM_DECAY: u64 = ST_DECAY as u64;
pub const MULTIVERB_PARAM_SIZE: u64 = ST_SIZE as u64;
pub const MULTIVERB_PARAM_PREDELAY_MS: u64 = ST_PREDELAY_MS as u64;
pub const MULTIVERB_PARAM_DAMP: u64 = ST_DAMP as u64;
pub const MULTIVERB_PARAM_BASS: u64 = ST_BASS as u64;
pub const MULTIVERB_PARAM_DIFFUSION: u64 = ST_DIFFUSION as u64;
pub const MULTIVERB_PARAM_MOD_RATE: u64 = ST_MOD_RATE as u64;
pub const MULTIVERB_PARAM_MOD_DEPTH: u64 = ST_MOD_DEPTH as u64;
pub const MULTIVERB_PARAM_MOD_SHAPE: u64 = ST_MOD_SHAPE as u64;
pub const MULTIVERB_PARAM_ERA: u64 = ST_ERA as u64;
pub const MULTIVERB_PARAM_WIDTH: u64 = ST_WIDTH as u64;
pub const MULTIVERB_PARAM_MIX: u64 = ST_MIX as u64;
pub const MULTIVERB_PARAM_ENABLED: u64 = ST_ENABLED as u64;
pub const MULTIVERB_PARAM_MOD_DECAY_DEPTH_1: u64 = ST_MOD_DECAY_DEPTH_1 as u64;
pub const MULTIVERB_PARAM_MOD_DECAY_DEPTH_2: u64 = ST_MOD_DECAY_DEPTH_1 as u64 + 1;
pub const MULTIVERB_PARAM_MOD_DECAY_DEPTH_3: u64 = ST_MOD_DECAY_DEPTH_1 as u64 + 2;
pub const MULTIVERB_PARAM_MOD_DECAY_DEPTH_4: u64 = ST_MOD_DECAY_DEPTH_1 as u64 + 3;
pub const MULTIVERB_PARAM_MOD_SIZE_DEPTH_1: u64 = ST_MOD_SIZE_DEPTH_1 as u64;
pub const MULTIVERB_PARAM_MOD_SIZE_DEPTH_2: u64 = ST_MOD_SIZE_DEPTH_1 as u64 + 1;
pub const MULTIVERB_PARAM_MOD_SIZE_DEPTH_3: u64 = ST_MOD_SIZE_DEPTH_1 as u64 + 2;
pub const MULTIVERB_PARAM_MOD_SIZE_DEPTH_4: u64 = ST_MOD_SIZE_DEPTH_1 as u64 + 3;
pub const MULTIVERB_PARAM_MOD_DEPTH_DEPTH_1: u64 = ST_MOD_DEPTH_DEPTH_1 as u64;
pub const MULTIVERB_PARAM_MOD_DEPTH_DEPTH_2: u64 = ST_MOD_DEPTH_DEPTH_1 as u64 + 1;
pub const MULTIVERB_PARAM_MOD_DEPTH_DEPTH_3: u64 = ST_MOD_DEPTH_DEPTH_1 as u64 + 2;
pub const MULTIVERB_PARAM_MOD_DEPTH_DEPTH_4: u64 = ST_MOD_DEPTH_DEPTH_1 as u64 + 3;
pub const MULTIVERB_PARAM_MOD_MIX_DEPTH_1: u64 = ST_MOD_MIX_DEPTH_1 as u64;
pub const MULTIVERB_PARAM_MOD_MIX_DEPTH_2: u64 = ST_MOD_MIX_DEPTH_1 as u64 + 1;
pub const MULTIVERB_PARAM_MOD_MIX_DEPTH_3: u64 = ST_MOD_MIX_DEPTH_1 as u64 + 2;
pub const MULTIVERB_PARAM_MOD_MIX_DEPTH_4: u64 = ST_MOD_MIX_DEPTH_1 as u64 + 3;

// ── Dattorro plate output tap tables (samples @ 29,761 Hz) ──
// yL/yR are ±0.6-weighted reads from the tank delays and AP2 buffers.

const TAPS_L: [(usize, f32); 7] = [
    (266, 1.0),   // right del1
    (2974, 1.0),  // right del1
    (1913, -1.0), // right AP2
    (1996, 1.0),  // right del2
    (1990, -1.0), // left del1
    (187, -1.0),  // left AP2
    (1066, -1.0), // left del2
];
const TAPS_L_BUFS: [usize; 7] = [
    BUF_DEL_R1, BUF_DEL_R1, BUF_AP_R2, BUF_DEL_R2, BUF_DEL_L1, BUF_AP_L2, BUF_DEL_L2,
];

const TAPS_R: [(usize, f32); 7] = [
    (353, 1.0),   // left del1
    (3627, 1.0),  // left del1
    (1228, -1.0), // left AP2
    (2673, 1.0),  // left del2
    (2111, -1.0), // right del1
    (335, -1.0),  // right AP2
    (121, -1.0),  // right del2
];
const TAPS_R_BUFS: [usize; 7] = [
    BUF_DEL_L1, BUF_DEL_L1, BUF_AP_L2, BUF_DEL_L2, BUF_DEL_R1, BUF_AP_R2, BUF_DEL_R2,
];

// ── Hall tap tables (samples @ 29,761 Hz, disjoint L/R sets from the four
// section delays — real decorrelation, not widened mono) ──

const H_TAPS_L: [(usize, usize, f32); 4] = [
    (BUF_H_DEL, 601, 1.0),
    (BUF_H_DEL + 1, 211, -1.0),
    (BUF_H_DEL + 2, 887, 1.0),
    (BUF_H_DEL + 3, 359, -1.0),
];
const H_TAPS_R: [(usize, usize, f32); 4] = [
    (BUF_H_DEL, 313, 1.0),
    (BUF_H_DEL + 1, 719, 1.0),
    (BUF_H_DEL + 2, 197, -1.0),
    (BUF_H_DEL + 3, 1123, -1.0),
];

/// Per-allpass base gains for the hall loop sections (Costello: 224 allpass
/// gains run high, ~0.6-0.72); scaled by the diffusion knob.
const H_AP_G: [f32; 8] = [0.62, 0.70, 0.66, 0.72, 0.60, 0.68, 0.64, 0.71];

// ── Quad tap tables (samples @ 29,761 Hz, disjoint L/R sets summed from all
// four cascaded loops' delays) ──

const Q_TAPS_L: [(usize, usize, f32); 4] = [
    (BUF_Q_DEL, 313, 1.0),
    (BUF_Q_DEL + 1, 937, -1.0),
    (BUF_Q_DEL + 2, 353, 1.0),
    (BUF_Q_DEL + 3, 1567, -1.0),
];
const Q_TAPS_R: [(usize, usize, f32); 4] = [
    (BUF_Q_DEL, 739, 1.0),
    (BUF_Q_DEL + 1, 211, 1.0),
    (BUF_Q_DEL + 2, 1361, -1.0),
    (BUF_Q_DEL + 3, 733, -1.0),
];

/// Quad loop allpass base gains; scaled by the diffusion knob (the shared
/// param table routes diffusion into the loop APs for this mode).
const Q_AP_G: [f32; 8] = [0.63, 0.68, 0.61, 0.70, 0.66, 0.60, 0.69, 0.64];

// ── Helpers ──

#[inline(always)]
fn clamp_node(x: f32) -> f32 {
    x.clamp(-4.0, 4.0)
}

#[inline(always)]
fn flush(x: f32) -> f32 {
    if x.abs() < 1.0e-18 || !x.is_finite() {
        0.0
    } else {
        x
    }
}

#[inline(always)]
fn modulation_signal(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[inline(always)]
fn modulation_depth(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

#[inline(always)]
fn tank_size_scale(size: f32) -> f32 {
    if size >= 0.5 {
        4.0_f32.powf(size - 0.5)
    } else {
        0.03_f32.powf(1.0 - 2.0 * size)
    }
}

#[inline(always)]
fn one_pole_coef(cutoff_hz: f32, sample_rate: f32) -> f32 {
    let c = 1.0 - (-std::f32::consts::TAU * cutoff_hz / sample_rate).exp();
    c.clamp(0.0, 1.0)
}

#[inline(always)]
fn xorshift32(state: &mut u32) -> f32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    // [-1, 1)
    (x as f32) * (2.0 / 4_294_967_296.0) - 1.0
}

// ── Init ──

unsafe extern "C" fn multiverb_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    for i in 0..MULTIVERB_STATE_SIZE {
        *s.add(i) = 0.0;
    }

    // Param defaults (descriptor defaults mirror these).
    *s.add(ST_MODE) = MODE_PLATE;
    *s.add(ST_DECAY) = 0.55;
    *s.add(ST_SIZE) = 0.5;
    *s.add(ST_PREDELAY_MS) = 0.0;
    *s.add(ST_DAMP) = 0.35;
    *s.add(ST_BASS) = 0.5;
    *s.add(ST_DIFFUSION) = 0.7;
    *s.add(ST_MOD_RATE) = 0.7;
    *s.add(ST_MOD_DEPTH) = 0.15;
    *s.add(ST_MOD_SHAPE) = 0.0;
    *s.add(ST_ERA) = 0.15;
    *s.add(ST_WIDTH) = 1.0;
    *s.add(ST_MIX) = 0.35;
    *s.add(ST_ENABLED) = 1.0;

    let fs = sample_rate as f32;
    *s.add(ST_SAMPLE_RATE) = fs;
    // Pre-seed the smoothers so short renders don't spend the first block
    // gliding from zero.
    *s.add(ST_SM_SCALE) = (fs / DATTORRO_FS) * 1.0;
    *s.add(ST_SM_PREDELAY) = 1.0;
    *s.add(ST_RNG) = f32::from_bits(0x9e3779b9);
    // Adopt whatever mode the first process() sees — descriptors and tests
    // set params after init, and that first choice must not trigger a fade.
    *s.add(ST_ACTIVE_MODE) = -1.0;
    *s.add(ST_FADE) = 1.0;
}

// ── Process ──

unsafe extern "C" fn multiverb_process(
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

    let mod_inputs = [*inp.add(2), *inp.add(3), *inp.add(4), *inp.add(5)];

    let mut fs = *s.add(ST_SAMPLE_RATE);
    if !(8000.0..=192_000.0).contains(&fs) {
        fs = 44_100.0;
    }

    // ── Read params (per block) ──
    let mode = (*s.add(ST_MODE)).round().clamp(0.0, 3.0) as i32;
    let decay = (*s.add(ST_DECAY)).clamp(0.0, 1.0);
    let size = (*s.add(ST_SIZE)).clamp(0.0, 1.0);
    let predelay_ms = (*s.add(ST_PREDELAY_MS)).clamp(0.0, 250.0);
    let damp = (*s.add(ST_DAMP)).clamp(0.0, 1.0);
    let bass = (*s.add(ST_BASS)).clamp(0.0, 1.0);
    let diffusion = (*s.add(ST_DIFFUSION)).clamp(0.0, 1.0);
    let mod_rate = (*s.add(ST_MOD_RATE)).clamp(0.05, 8.0);
    let mod_depth = (*s.add(ST_MOD_DEPTH)).clamp(0.0, 1.0);
    let mod_shape = (*s.add(ST_MOD_SHAPE)).clamp(0.0, 1.0);
    let era = (*s.add(ST_ERA)).clamp(0.0, 1.0);
    let width = (*s.add(ST_WIDTH)).clamp(0.0, 1.0);
    let mix = (*s.add(ST_MIX)).clamp(0.0, 1.0);

    let mod_decay_depths = [
        modulation_depth(*s.add(ST_MOD_DECAY_DEPTH_1)),
        modulation_depth(*s.add(ST_MOD_DECAY_DEPTH_1 + 1)),
        modulation_depth(*s.add(ST_MOD_DECAY_DEPTH_1 + 2)),
        modulation_depth(*s.add(ST_MOD_DECAY_DEPTH_1 + 3)),
    ];
    let mod_size_depths = [
        modulation_depth(*s.add(ST_MOD_SIZE_DEPTH_1)),
        modulation_depth(*s.add(ST_MOD_SIZE_DEPTH_1 + 1)),
        modulation_depth(*s.add(ST_MOD_SIZE_DEPTH_1 + 2)),
        modulation_depth(*s.add(ST_MOD_SIZE_DEPTH_1 + 3)),
    ];
    let mod_depth_depths = [
        modulation_depth(*s.add(ST_MOD_DEPTH_DEPTH_1)),
        modulation_depth(*s.add(ST_MOD_DEPTH_DEPTH_1 + 1)),
        modulation_depth(*s.add(ST_MOD_DEPTH_DEPTH_1 + 2)),
        modulation_depth(*s.add(ST_MOD_DEPTH_DEPTH_1 + 3)),
    ];
    let mod_mix_depths = [
        modulation_depth(*s.add(ST_MOD_MIX_DEPTH_1)),
        modulation_depth(*s.add(ST_MOD_MIX_DEPTH_1 + 1)),
        modulation_depth(*s.add(ST_MOD_MIX_DEPTH_1 + 2)),
        modulation_depth(*s.add(ST_MOD_MIX_DEPTH_1 + 3)),
    ];

    let desired_mode = mode;

    // ── Derived (per block) ──
    let target_predelay = (predelay_ms * 0.001 * fs).max(1.0);
    let scale_coef = one_pole_coef(3.0, fs); // ~50 ms glide; sweeps pitch-smear
    let predelay_coef = one_pole_coef(8.0, fs);

    // Diffusion drives the tank allpasses, not just the input diffusers —
    // knob at 0.7 = exact paper gains (decay diffusion 1 = 0.70); at 0 the
    // tank degenerates into discrete echoes, at 1 it's maximally smeared.
    let ap1_g = diffusion.min(0.9);
    // Input diffusion gains; knob at 0.7 = exact paper values.
    let dscale = (diffusion / 0.7).min(1.13);
    let in_g1 = (0.75 * dscale).min(0.85);
    let in_g2 = (0.625 * dscale).min(0.85);

    // Damping LP inside the loop; era pulls it down too (the 224's dull top
    // end compounds every pass around the tank), and darkens the input LP.
    let damp_cutoff = (19_000.0 * (1.0 - damp).powi(2) + 900.0) * (1.0 - 0.68 * era);
    let damp_coef = one_pole_coef(damp_cutoff, fs);
    let bw_coef = one_pole_coef(21_000.0 * (1.0 - 0.82 * era) + 1000.0, fs);
    // Bass = LF decay multiplier via a low shelf on the loop (0.5 = neutral).
    let bass_split_coef = one_pole_coef(250.0, fs);
    let bass_gain = 4.0_f32.powf(bass - 0.5);

    let lfo_inc = mod_rate / fs;
    let walk_coef = one_pole_coef((mod_rate * 0.5).max(0.05), fs);

    // Hall modulation already leans random at shape 0 (the 224's modulation
    // was never a clean LFO).
    let h_dscale = (diffusion / 0.7).min(1.04);
    let h_shape = 0.35 + 0.65 * mod_shape;
    let h_walk_coef = one_pole_coef((mod_rate * 0.35).max(0.05), fs);

    // Quad: per-loop feedback gains matched so all four cascaded loops decay
    // at the same rate, calibrated against the plate's RT60-per-knob feel
    // (plate branch time ≈ 0.358 s reference; Q_CAL trims for the plate's
    // extra tap/diffuser losses). decay 1.0 still freezes.
    const Q_CAL: f32 = 3.5;
    // Quad reads the era knob biased high — a 1988 16-bit box is never
    // clean — which drags its in-loop damping down with it.
    let q_era = 0.35 + 0.65 * era;
    let q_quant_scale = 65_536.0 * 0.5f32.powf(q_era * 3.0);
    let q_quant_inv = 1.0 / q_quant_scale;
    let q_damp_coef = one_pole_coef(
        (19_000.0 * (1.0 - damp).powi(2) + 900.0) * (1.0 - 0.68 * q_era),
        fs,
    );
    let q_ap_scale = (diffusion / 0.7).min(1.05);
    let q_dc_coef = one_pole_coef(12.0, fs);

    // Mod (Freeverb): per-comb feedback gains from the shared decay feel
    // (M_CAL calibrated by render against the plate's RT60-per-knob curve);
    // allpass gains ride the diffusion knob around Freeverb's stock 0.5.
    const M_CAL: f32 = 1.9;
    let m_ap_g = (0.5 * (diffusion / 0.7)).min(0.6);
    let m_lfo_inc = [
        M_AP_RATES[0] * mod_rate / fs,
        M_AP_RATES[1] * mod_rate / fs,
        M_AP_RATES[2] * mod_rate / fs,
        M_AP_RATES[3] * mod_rate / fs,
    ];

    // Era grit: coarse-quantized interpolation (the 224 "halo") plus word-
    // length truncation inside the loop — 16-bit-ish at era 0⁺ down to
    // ~13-bit at era 1, re-applied every pass so it accumulates.
    let era_quant = era;
    let quant_scale = 65_536.0 * 0.5f32.powf(era * 3.0);
    let quant_inv = 1.0 / quant_scale;

    let side_g = width;

    // Mode-switch wet fade: 25 ms out, clear the incoming tank, 25 ms in.
    let fade_step = 1.0 / (0.025 * fs);

    // ── Load mutable state ──
    let mut sm_scale = *s.add(ST_SM_SCALE);
    let mut sm_predelay = *s.add(ST_SM_PREDELAY);
    let mut lfo_phase = *s.add(ST_LFO_PHASE);
    let mut rng = (*s.add(ST_RNG)).to_bits();
    if rng == 0 {
        rng = 0x9e3779b9;
    }
    let mut bw_lp = *s.add(ST_BW_LP);
    let mut active_mode = *s.add(ST_ACTIVE_MODE) as i32;
    let mut fade = (*s.add(ST_FADE)).clamp(0.0, 1.0);
    if active_mode < 0 {
        // First process after init: adopt the param directly, buffers are
        // already clear and there is no old tail to fade.
        active_mode = desired_mode;
        fade = 1.0;
    }

    let mut p_walk_l = *s.add(ST_P_WALK_L);
    let mut p_walk_tgt_l = *s.add(ST_P_WALK_TGT_L);
    let mut p_walk_r = *s.add(ST_P_WALK_R);
    let mut p_walk_tgt_r = *s.add(ST_P_WALK_TGT_R);
    let mut p_damp_l = *s.add(ST_P_DAMP_L);
    let mut p_damp_r = *s.add(ST_P_DAMP_R);
    let mut p_bass_l = *s.add(ST_P_BASS_L);
    let mut p_bass_r = *s.add(ST_P_BASS_R);

    let mut h_damp = *s.add(ST_H_DAMP);
    let mut h_bass = *s.add(ST_H_BASS);
    let mut h_walk = [0.0f32; 4];
    let mut h_walk_tgt = [0.0f32; 4];
    let mut q_damp = [0.0f32; 4];
    let mut q_bass = [0.0f32; 4];
    let mut q_walk = [0.0f32; 4];
    let mut q_walk_tgt = [0.0f32; 4];
    let mut p_qerr_l = *s.add(ST_P_QERR_L);
    let mut p_qerr_r = *s.add(ST_P_QERR_R);
    let mut h_qerr = *s.add(ST_H_QERR);
    let mut q_qerr = [0.0f32; 4];
    let mut q_dc = [0.0f32; 4];
    let mut m_lfo = [0.0f32; 4];
    let mut m_walk = [0.0f32; 4];
    let mut m_walk_tgt = [0.0f32; 4];
    for k in 0..4 {
        q_qerr[k] = *s.add(ST_Q_QERR + k);
        q_dc[k] = *s.add(ST_Q_DC + k);
        m_lfo[k] = *s.add(ST_M_LFO + k);
        m_walk[k] = *s.add(ST_M_WALK + k);
        m_walk_tgt[k] = *s.add(ST_M_WALK_TGT + k);
    }
    let mut m_damp = [0.0f32; 16];
    let mut m_bass = [0.0f32; 16];
    let mut m_qerr = [0.0f32; 16];
    for k in 0..16 {
        m_damp[k] = *s.add(ST_M_DAMP + k);
        m_bass[k] = *s.add(ST_M_BASS + k);
        m_qerr[k] = *s.add(ST_M_QERR + k);
    }
    for k in 0..4 {
        h_walk[k] = *s.add(ST_H_WALK + k);
        h_walk_tgt[k] = *s.add(ST_H_WALK_TGT + k);
        q_damp[k] = *s.add(ST_Q_DAMP + k);
        q_bass[k] = *s.add(ST_Q_BASS + k);
        q_walk[k] = *s.add(ST_Q_WALK + k);
        q_walk_tgt[k] = *s.add(ST_Q_WALK_TGT + k);
    }

    let mut wpos = [0usize; NBUFS];
    for b in 0..NBUFS {
        let w = *s.add(ST_WRITE_IDX + b);
        wpos[b] = (w.max(0.0) as usize) % buf_cap(b);
    }

    if sm_scale <= 0.0 {
        sm_scale = (fs / DATTORRO_FS) * tank_size_scale(size);
    }

    // Interpolation-fraction quantization amount for read_frac!; assigned
    // per sample because quad hears the era knob biased high (declared here,
    // before the macro definitions, so the macro body can see it).
    let mut era_interp;

    // Read a fractional delay `delay` samples behind the write head (which
    // has not been written this sample yet, so delay 1.0 = last sample).
    // `era` quantizes the interpolation fraction to 1/32 steps (224 "halo").
    macro_rules! read_frac {
        ($buf:expr, $delay:expr) => {{
            let cap = buf_cap($buf);
            let ofs = BUF_OFFSETS[$buf];
            let d = ($delay).clamp(1.0, (cap - 2) as f32);
            let di = d as usize;
            let mut frac = d - di as f32;
            if era_interp > 0.0 {
                let fq = (frac * 32.0).floor() * (1.0 / 32.0);
                frac += era_interp * (fq - frac);
            }
            let i0 = (wpos[$buf] + cap - di) % cap;
            let i1 = (i0 + cap - 1) % cap;
            let v0 = *s.add(ofs + i0);
            let v1 = *s.add(ofs + i1);
            v0 + (v1 - v0) * frac
        }};
    }

    macro_rules! write_buf {
        ($buf:expr, $val:expr) => {{
            let value = $val;
            *s.add(BUF_OFFSETS[$buf] + wpos[$buf]) = flush(value);
            wpos[$buf] = (wpos[$buf] + 1) % buf_cap($buf);
        }};
    }

    // Allpass with feedback-around-delay: u = x + g·d, y = d − g·u.
    macro_rules! allpass {
        ($buf:expr, $x:expr, $g:expr, $delay:expr) => {{
            let d = read_frac!($buf, $delay);
            let u = clamp_node($x + $g * d);
            write_buf!($buf, u);
            d - $g * u
        }};
    }

    for i in 0..nf {
        let dry_l = *in_l.add(i);
        let dry_r = *in_r.add(i);

        let mut decay_mod = 0.0f32;
        let mut size_mod = 0.0f32;
        let mut depth_mod = 0.0f32;
        let mut mix_mod = 0.0f32;
        for slot in 0..4 {
            let signal = modulation_signal(*mod_inputs[slot].add(i));
            decay_mod += signal * mod_decay_depths[slot];
            size_mod += signal * mod_size_depths[slot];
            depth_mod += signal * mod_depth_depths[slot];
            mix_mod += signal * mod_mix_depths[slot];
        }
        let sample_decay = (decay + decay_mod).clamp(0.0, 1.0);
        let sample_size = (size + size_mod).clamp(0.0, 1.0);
        let sample_mod_depth = (mod_depth + depth_mod).clamp(0.0, 1.0);
        let sample_mix = (mix + mix_mod).clamp(0.0, 1.0);

        // Size's upper half spans reference tuning to 2×. Below 0.5 it
        // dives exponentially to 0.03×, turning every mode into a playable
        // few-millisecond comb/resonator bank. The existing glide makes both
        // automation and host modulation pitch-smear instead of zipper.
        let target_scale = (fs / DATTORRO_FS) * tank_size_scale(sample_size);
        sm_scale += scale_coef * (target_scale - sm_scale);
        sm_predelay += predelay_coef * (target_predelay - sm_predelay);

        // Shared decay feel, evaluated after host modulation so all four
        // modes respond consistently to the same target slot.
        let decay_g = (0.25 + 0.75 * sample_decay.powf(1.4)).min(1.0);

        // ── Mode switch: fade wet out, swap + clear, fade back in ──
        if desired_mode != active_mode {
            fade -= fade_step;
            if fade <= 0.0 {
                fade = 0.0;
                active_mode = desired_mode;
                let bufs = match active_mode {
                    1 => HALL_BUFS,
                    2 => QUAD_BUFS,
                    3 => MOD_BUFS,
                    _ => PLATE_BUFS,
                };
                for b in bufs {
                    std::ptr::write_bytes(s.add(BUF_OFFSETS[b]), 0, buf_cap(b));
                    wpos[b] = 0;
                }
                // The filter states live in locals during the block — zero
                // the incoming mode's copies (they're written back to the
                // state slots at block end).
                match active_mode {
                    1 => {
                        h_damp = 0.0;
                        h_bass = 0.0;
                        h_qerr = 0.0;
                    }
                    2 => {
                        q_damp = [0.0; 4];
                        q_bass = [0.0; 4];
                        q_qerr = [0.0; 4];
                        q_dc = [0.0; 4];
                    }
                    3 => {
                        m_damp = [0.0; 16];
                        m_bass = [0.0; 16];
                        m_qerr = [0.0; 16];
                    }
                    _ => {
                        p_damp_l = 0.0;
                        p_damp_r = 0.0;
                        p_bass_l = 0.0;
                        p_bass_r = 0.0;
                        p_qerr_l = 0.0;
                        p_qerr_r = 0.0;
                    }
                }
            }
        } else if fade < 1.0 {
            fade = (fade + fade_step).min(1.0);
        }

        // ── Shared modulator clock ──
        let q_before = ((lfo_phase * 4.0) as usize).min(3);
        lfo_phase += lfo_inc;
        let mut wrapped = false;
        if lfo_phase >= 1.0 {
            lfo_phase -= 1.0;
            wrapped = true;
        }
        let q_after = ((lfo_phase * 4.0) as usize).min(3);

        // read_frac! picks this up: quad hears the era knob biased high
        // (its reads go truncated/grainy sooner).
        era_interp = if active_mode == 2 { q_era } else { era };

        // ── Input: mono sum → predelay → bandwidth LP ──
        write_buf!(BUF_PREDELAY, (dry_l + dry_r) * 0.5);
        let pd = read_frac!(BUF_PREDELAY, sm_predelay);
        bw_lp += bw_coef * (pd - bw_lp);

        let (wet_l, wet_r) = if active_mode == 1 {
            // ── Hall: 2 diffusers → loop of 4 sections, filters once/loop ──
            let h_sect_g = decay_g.powf(0.25);
            let h_lo_g = bass_gain.min(0.9985 / decay_g.max(0.25));
            // Keep the shared default depth subtle: Concert Hall should
            // breathe, not constantly bend obvious source pitch. The steeper
            // curve still opens to the full 30 ms wander at the top for the
            // deliberately detuned 224-style effect.
            let h_exc_samps = (sample_mod_depth.powi(2) * 0.030 * fs).min(HALL_EXC_CAP as f32);

            // Per-section random walks retargeted round-robin, one per
            // quarter of the LFO cycle, blended with quadrature LFO taps.
            if q_after != q_before || wrapped {
                h_walk_tgt[q_after] = xorshift32(&mut rng);
            }
            let mut mods = [0.0f32; 4];
            for k in 0..4 {
                h_walk[k] += h_walk_coef * (h_walk_tgt[k] - h_walk[k]);
                let ph = (lfo_phase + k as f32 * 0.25) * std::f32::consts::TAU;
                mods[k] = (1.0 - h_shape) * ph.sin() + h_shape * h_walk[k];
            }

            let mut x = bw_lp;
            x = allpass!(
                BUF_H_DIFF1,
                x,
                in_g1,
                BASE_LENS[BUF_H_DIFF1] as f32 * sm_scale
            );
            x = allpass!(
                BUF_H_DIFF2,
                x,
                in_g1,
                BASE_LENS[BUF_H_DIFF2] as f32 * sm_scale
            );

            // Loop feedback = section-4 delay output, then the once-per-loop
            // damping LP, bass shelf and era word-length truncation.
            let len4 = BASE_LENS[BUF_H_DEL + 3] as f32 * sm_scale;
            let exc4 = h_exc_samps.min(len4 * 0.7);
            let fb_raw = read_frac!(BUF_H_DEL + 3, len4 + exc4 * mods[3]);
            h_damp += damp_coef * (fb_raw - h_damp);
            h_bass += bass_split_coef * (h_damp - h_bass);
            let mut fb = (h_damp - h_bass) * h_sect_g + h_bass * (h_lo_g * h_sect_g).min(0.9985);
            if era_quant > 0.0 {
                let t = fb + h_qerr;
                let q = (t * quant_scale).trunc() * quant_inv;
                h_qerr = t - q;
                fb += era_quant * (q - fb);
            }

            let mut t = clamp_node(x + fb);
            for k in 0..4 {
                let g1 = (H_AP_G[2 * k] * h_dscale).min(0.75);
                let g2 = (H_AP_G[2 * k + 1] * h_dscale).min(0.75);
                t = allpass!(
                    BUF_H_AP + 2 * k,
                    t,
                    g1,
                    BASE_LENS[BUF_H_AP + 2 * k] as f32 * sm_scale
                );
                t = allpass!(
                    BUF_H_AP + 2 * k + 1,
                    t,
                    g2,
                    BASE_LENS[BUF_H_AP + 2 * k + 1] as f32 * sm_scale
                );
                if k < 3 {
                    let len = BASE_LENS[BUF_H_DEL + k] as f32 * sm_scale;
                    let exc = h_exc_samps.min(len * 0.7);
                    let d = read_frac!(BUF_H_DEL + k, len + exc * mods[k]);
                    write_buf!(BUF_H_DEL + k, clamp_node(t));
                    t = d * h_sect_g;
                } else {
                    write_buf!(BUF_H_DEL + 3, clamp_node(t));
                }
            }

            let mut y_l = 0.0f32;
            for &(buf, off, sign) in H_TAPS_L.iter() {
                y_l += sign * read_frac!(buf, off as f32 * sm_scale);
            }
            let mut y_r = 0.0f32;
            for &(buf, off, sign) in H_TAPS_R.iter() {
                y_r += sign * read_frac!(buf, off as f32 * sm_scale);
            }
            (y_l * 0.5, y_r * 0.5)
        } else if active_mode == 2 {
            // ── Quad: 4 cascaded allpass loops (2 APs + delay each) ──
            // Shallow modulation only; the Quadraverb move is modulating
            // decay/size from outside (mod-target slots, phase 5).
            let q_exc_samps = sample_mod_depth.powf(1.5) * 0.002 * fs;
            if q_after != q_before || wrapped {
                q_walk_tgt[q_after] = xorshift32(&mut rng);
            }
            let mut y_l = 0.0f32;
            let mut y_r = 0.0f32;
            let mut t_in = bw_lp;
            for k in 0..4 {
                q_walk[k] += walk_coef * (q_walk_tgt[k] - q_walk[k]);
                let ph = (lfo_phase + k as f32 * 0.25) * std::f32::consts::TAU;
                let m = (1.0 - mod_shape) * ph.sin() + mod_shape * q_walk[k];

                let len = BASE_LENS[BUF_Q_DEL + k] as f32 * sm_scale;
                let exc = q_exc_samps.min(len * 0.7);
                let d_out = read_frac!(BUF_Q_DEL + k, len + exc * m);

                // Per-loop HF damping LP + LF decay shelf on the feedback,
                // then 16-bit-ish word-length truncation every pass.
                q_damp[k] += q_damp_coef * (d_out - q_damp[k]);
                q_bass[k] += bass_split_coef * (q_damp[k] - q_bass[k]);
                let g = decay_g.powf(Q_CAL * Q_LOOP_T[k] / 0.358).min(1.0);
                let mut fb = (q_damp[k] - q_bass[k]) * g + q_bass[k] * (bass_gain * g).min(0.9985);
                // DC blocker (~12 Hz) — cascaded loops multiply subsonic gain.
                q_dc[k] += q_dc_coef * (fb - q_dc[k]);
                fb -= q_dc[k];
                let qt = fb + q_qerr[k];
                let q = (qt * q_quant_scale).trunc() * q_quant_inv;
                q_qerr[k] = qt - q;
                fb += q_era * (q - fb);

                let mut t = clamp_node(t_in + fb);
                let g1 = (Q_AP_G[2 * k] * q_ap_scale).min(0.72);
                let g2 = (Q_AP_G[2 * k + 1] * q_ap_scale).min(0.72);
                t = allpass!(
                    BUF_Q_AP + 2 * k,
                    t,
                    g1,
                    BASE_LENS[BUF_Q_AP + 2 * k] as f32 * sm_scale
                );
                t = allpass!(
                    BUF_Q_AP + 2 * k + 1,
                    t,
                    g2,
                    BASE_LENS[BUF_Q_AP + 2 * k + 1] as f32 * sm_scale
                );
                write_buf!(BUF_Q_DEL + k, clamp_node(t));

                // Cascade: the end of this loop feeds the next loop's input.
                t_in = d_out;
            }
            for &(buf, off, sign) in Q_TAPS_L.iter() {
                y_l += sign * read_frac!(buf, off as f32 * sm_scale);
            }
            for &(buf, off, sign) in Q_TAPS_R.iter() {
                y_r += sign * read_frac!(buf, off as f32 * sm_scale);
            }
            (y_l * 0.45, y_r * 0.45)
        } else if active_mode == 3 {
            // ── Mod: Freeverb combs → modulated series allpasses ──
            let m_scale = sm_scale * MOD_SCALE_RATIO;
            // ±48 samples at 44.1 kHz, scaled to the running rate.
            let m_exc_samps = sample_mod_depth * 48.0 * (fs / FREEVERB_FS);

            // Per-allpass detuned LFOs + random walks. At mod depth 0 the
            // excursion is exactly zero, so none of this touches the audio
            // path (the null-test guarantee).
            let mut mods_l = [0.0f32; 4];
            let mut mods_r = [0.0f32; 4];
            for k in 0..4 {
                m_lfo[k] += m_lfo_inc[k];
                if m_lfo[k] >= 1.0 {
                    m_lfo[k] -= 1.0;
                    m_walk_tgt[k] = xorshift32(&mut rng);
                }
                m_walk[k] += walk_coef * (m_walk_tgt[k] - m_walk[k]);
                let ph = m_lfo[k] * std::f32::consts::TAU;
                mods_l[k] = (1.0 - mod_shape) * ph.sin() + mod_shape * m_walk[k];
                mods_r[k] = (1.0 - mod_shape) * ph.cos() - mod_shape * m_walk[k];
            }

            // 8 parallel lowpass-feedback combs per channel, fed the same
            // mono input; decorrelation comes from the +23 stereospread.
            let x = bw_lp * 0.25;
            let mut sum_l = 0.0f32;
            let mut sum_r = 0.0f32;
            for k in 0..8 {
                for (ch, (comb0, sum)) in [(BUF_M_COMB_L, &mut sum_l), (BUF_M_COMB_R, &mut sum_r)]
                    .into_iter()
                    .enumerate()
                {
                    let buf = comb0 + k;
                    let f = ch * 8 + k;
                    let out = read_frac!(buf, BASE_LENS[buf] as f32 * m_scale);
                    *sum += out;
                    // Damp LP + LF-decay shelf inside the comb feedback.
                    m_damp[f] += damp_coef * (out - m_damp[f]);
                    m_bass[f] += bass_split_coef * (m_damp[f] - m_bass[f]);
                    let t_k = BASE_LENS[BUF_M_COMB_L + k] as f32 / FREEVERB_FS;
                    let g = decay_g.powf(M_CAL * t_k / 0.358).min(1.0);
                    let mut fb =
                        (m_damp[f] - m_bass[f]) * g + m_bass[f] * (bass_gain * g).min(0.9995);
                    // Word-length loss belongs inside every recursive comb
                    // pass. Error feedback preserves long-tail energy while
                    // retaining the era-dependent grain and avoiding limit
                    // cycles, matching the other three Multiverb tanks.
                    if era_quant > 0.0 {
                        let t = fb + m_qerr[f];
                        let q = (t * quant_scale).trunc() * quant_inv;
                        m_qerr[f] = t - q;
                        fb += era_quant * (q - fb);
                    }
                    write_buf!(buf, clamp_node(x + fb));
                }
            }

            // 4 series allpasses per channel with interpolated modulated
            // reads — the instrument part of this mode.
            let mut y_l = sum_l;
            let mut y_r = sum_r;
            for k in 0..4 {
                let len_l = BASE_LENS[BUF_M_AP_L + k] as f32 * m_scale;
                let exc_l = m_exc_samps.min(len_l * 0.7);
                y_l = allpass!(BUF_M_AP_L + k, y_l, m_ap_g, len_l + exc_l * mods_l[k]);
                let len_r = BASE_LENS[BUF_M_AP_R + k] as f32 * m_scale;
                let exc_r = m_exc_samps.min(len_r * 0.7);
                y_r = allpass!(BUF_M_AP_R + k, y_r, m_ap_g, len_r + exc_r * mods_r[k]);
            }
            (y_l * 0.35, y_r * 0.35)
        } else {
            // ── Plate: 4 diffusers → cross-coupled figure-eight tank ──
            let ap2_g =
                ((sample_decay + 0.15).clamp(0.25, 0.50) * (0.4 + 0.857 * diffusion)).min(0.6);
            // Up to 3 ms; hardware sat nearer 0.5 ms, while the top of the
            // shared knob is deliberately capable of seasick modulation.
            let exc_samps = sample_mod_depth.powf(1.5) * 0.003 * fs;
            if wrapped {
                p_walk_tgt_l = xorshift32(&mut rng);
                p_walk_tgt_r = xorshift32(&mut rng);
            }
            p_walk_l += walk_coef * (p_walk_tgt_l - p_walk_l);
            p_walk_r += walk_coef * (p_walk_tgt_r - p_walk_r);
            let ph = lfo_phase * std::f32::consts::TAU;
            let mod_l = (1.0 - mod_shape) * ph.sin() + mod_shape * p_walk_l;
            let mod_r =
                (1.0 - mod_shape) * (ph + std::f32::consts::FRAC_PI_2).sin() + mod_shape * p_walk_r;

            let mut x = bw_lp;
            x = allpass!(BUF_DIFF1, x, in_g1, BASE_LENS[BUF_DIFF1] as f32 * sm_scale);
            x = allpass!(BUF_DIFF2, x, in_g1, BASE_LENS[BUF_DIFF2] as f32 * sm_scale);
            x = allpass!(BUF_DIFF3, x, in_g2, BASE_LENS[BUF_DIFF3] as f32 * sm_scale);
            x = allpass!(BUF_DIFF4, x, in_g2, BASE_LENS[BUF_DIFF4] as f32 * sm_scale);
            let diffused = x;

            // Cross-coupling: each branch input takes the other branch's
            // final delay output (read before this sample's write).
            let cross_r = read_frac!(BUF_DEL_R2, BASE_LENS[BUF_DEL_R2] as f32 * sm_scale);
            let cross_l = read_frac!(BUF_DEL_L2, BASE_LENS[BUF_DEL_L2] as f32 * sm_scale);

            // Excursion can't exceed a fraction of the (possibly tiny) delay
            // it modulates, or deep mod at small sizes pins reads at the
            // clamp.
            let exc_l = exc_samps.min(BASE_LENS[BUF_AP_L1] as f32 * sm_scale * 0.7);
            let exc_r = exc_samps.min(BASE_LENS[BUF_AP_R1] as f32 * sm_scale * 0.7);

            // Left branch.
            let mut tl = clamp_node(diffused + cross_r * decay_g);
            tl = allpass!(
                BUF_AP_L1,
                tl,
                ap1_g,
                BASE_LENS[BUF_AP_L1] as f32 * sm_scale + exc_l * mod_l
            );
            let del_l1_out = read_frac!(BUF_DEL_L1, BASE_LENS[BUF_DEL_L1] as f32 * sm_scale);
            write_buf!(BUF_DEL_L1, tl);
            p_damp_l += damp_coef * (del_l1_out - p_damp_l);
            p_bass_l += bass_split_coef * (p_damp_l - p_bass_l);
            let hi_l = p_damp_l - p_bass_l;
            let mut dl = hi_l * decay_g + p_bass_l * (bass_gain * decay_g).min(0.998);
            if era_quant > 0.0 {
                let t = dl + p_qerr_l;
                let q = (t * quant_scale).trunc() * quant_inv;
                p_qerr_l = t - q;
                dl += era_quant * (q - dl);
            }
            dl = allpass!(BUF_AP_L2, dl, ap2_g, BASE_LENS[BUF_AP_L2] as f32 * sm_scale);
            write_buf!(BUF_DEL_L2, clamp_node(dl));

            // Right branch.
            let mut tr = clamp_node(diffused + cross_l * decay_g);
            tr = allpass!(
                BUF_AP_R1,
                tr,
                ap1_g,
                BASE_LENS[BUF_AP_R1] as f32 * sm_scale + exc_r * mod_r
            );
            let del_r1_out = read_frac!(BUF_DEL_R1, BASE_LENS[BUF_DEL_R1] as f32 * sm_scale);
            write_buf!(BUF_DEL_R1, tr);
            p_damp_r += damp_coef * (del_r1_out - p_damp_r);
            p_bass_r += bass_split_coef * (p_damp_r - p_bass_r);
            let hi_r = p_damp_r - p_bass_r;
            let mut dr = hi_r * decay_g + p_bass_r * (bass_gain * decay_g).min(0.998);
            if era_quant > 0.0 {
                let t = dr + p_qerr_r;
                let q = (t * quant_scale).trunc() * quant_inv;
                p_qerr_r = t - q;
                dr += era_quant * (q - dr);
            }
            dr = allpass!(BUF_AP_R2, dr, ap2_g, BASE_LENS[BUF_AP_R2] as f32 * sm_scale);
            write_buf!(BUF_DEL_R2, clamp_node(dr));

            // Output taps.
            let mut y_l = 0.0f32;
            for (t, &(off, sign)) in TAPS_L.iter().enumerate() {
                y_l += sign * read_frac!(TAPS_L_BUFS[t], off as f32 * sm_scale);
            }
            let mut y_r = 0.0f32;
            for (t, &(off, sign)) in TAPS_R.iter().enumerate() {
                y_r += sign * read_frac!(TAPS_R_BUFS[t], off as f32 * sm_scale);
            }
            (y_l * 0.6, y_r * 0.6)
        };

        // Width via M/S, then the mode-switch fade on the wet path only.
        let mid = (wet_l + wet_r) * 0.5;
        let side = (wet_l - wet_r) * 0.5 * side_g;
        let dry_g = (1.0 - sample_mix * sample_mix).sqrt();
        let fg = fade * sample_mix;
        *out_l.add(i) = (dry_l * dry_g + (mid + side) * fg).clamp(-4.0, 4.0);
        *out_r.add(i) = (dry_r * dry_g + (mid - side) * fg).clamp(-4.0, 4.0);
    }

    // ── Store mutable state (flushing denormals in the recursive paths) ──
    *s.add(ST_SM_SCALE) = sm_scale;
    *s.add(ST_SM_PREDELAY) = sm_predelay;
    *s.add(ST_LFO_PHASE) = lfo_phase;
    *s.add(ST_RNG) = f32::from_bits(rng);
    *s.add(ST_BW_LP) = flush(bw_lp);
    *s.add(ST_ACTIVE_MODE) = active_mode as f32;
    *s.add(ST_FADE) = fade;
    *s.add(ST_P_WALK_L) = flush(p_walk_l);
    *s.add(ST_P_WALK_TGT_L) = p_walk_tgt_l;
    *s.add(ST_P_WALK_R) = flush(p_walk_r);
    *s.add(ST_P_WALK_TGT_R) = p_walk_tgt_r;
    *s.add(ST_P_DAMP_L) = flush(p_damp_l);
    *s.add(ST_P_DAMP_R) = flush(p_damp_r);
    *s.add(ST_P_BASS_L) = flush(p_bass_l);
    *s.add(ST_P_BASS_R) = flush(p_bass_r);
    *s.add(ST_H_DAMP) = flush(h_damp);
    *s.add(ST_H_BASS) = flush(h_bass);
    *s.add(ST_P_QERR_L) = flush(p_qerr_l);
    *s.add(ST_P_QERR_R) = flush(p_qerr_r);
    *s.add(ST_H_QERR) = flush(h_qerr);
    for k in 0..4 {
        *s.add(ST_H_WALK + k) = flush(h_walk[k]);
        *s.add(ST_H_WALK_TGT + k) = h_walk_tgt[k];
        *s.add(ST_Q_DAMP + k) = flush(q_damp[k]);
        *s.add(ST_Q_BASS + k) = flush(q_bass[k]);
        *s.add(ST_Q_WALK + k) = flush(q_walk[k]);
        *s.add(ST_Q_WALK_TGT + k) = q_walk_tgt[k];
        *s.add(ST_Q_QERR + k) = flush(q_qerr[k]);
        *s.add(ST_Q_DC + k) = flush(q_dc[k]);
        *s.add(ST_M_LFO + k) = m_lfo[k];
        *s.add(ST_M_WALK + k) = flush(m_walk[k]);
        *s.add(ST_M_WALK_TGT + k) = m_walk_tgt[k];
    }
    for k in 0..16 {
        *s.add(ST_M_DAMP + k) = flush(m_damp[k]);
        *s.add(ST_M_BASS + k) = flush(m_bass[k]);
        *s.add(ST_M_QERR + k) = flush(m_qerr[k]);
    }
    for b in 0..NBUFS {
        *s.add(ST_WRITE_IDX + b) = wpos[b] as f32;
    }
}

pub fn multiverb_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(multiverb_process),
        init: Some(multiverb_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    fn init_state(sample_rate: i32) -> Vec<f32> {
        let mut state = vec![0.0f32; MULTIVERB_STATE_SIZE];
        unsafe {
            multiverb_init(state.as_mut_ptr().cast(), sample_rate, 64, ptr::null());
        }
        state
    }

    fn process(state: &mut [f32], in_l: &mut [f32], in_r: &mut [f32]) -> (Vec<f32>, Vec<f32>) {
        let mut mods = std::array::from_fn(|_| vec![0.0f32; in_l.len()]);
        process_with_mod(state, in_l, in_r, &mut mods)
    }

    fn process_with_mod(
        state: &mut [f32],
        in_l: &mut [f32],
        in_r: &mut [f32],
        mods: &mut [Vec<f32>; 4],
    ) -> (Vec<f32>, Vec<f32>) {
        let nf = in_l.len();
        let [mod_1, mod_2, mod_3, mod_4] = mods;
        let inputs = [
            in_l.as_mut_ptr(),
            in_r.as_mut_ptr(),
            mod_1.as_mut_ptr(),
            mod_2.as_mut_ptr(),
            mod_3.as_mut_ptr(),
            mod_4.as_mut_ptr(),
        ];
        let mut out_l = vec![0.0f32; nf];
        let mut out_r = vec![0.0f32; nf];
        let outputs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        unsafe {
            multiverb_process(
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
        impulse_render_with_mod(state, secs, fs, [0.0; 4])
    }

    fn impulse_render_with_mod(
        state: &mut [f32],
        secs: f32,
        fs: usize,
        mod_values: [f32; 4],
    ) -> (Vec<f32>, Vec<f32>) {
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
            let mut mods = std::array::from_fn(|slot| vec![mod_values[slot]; n]);
            let (ol, or) = process_with_mod(state, &mut in_l, &mut in_r, &mut mods);
            acc_l.extend_from_slice(&ol);
            acc_r.extend_from_slice(&or);
            done += n;
        }
        (acc_l, acc_r)
    }

    fn energy(sig: &[f32]) -> f64 {
        sig.iter().map(|x| (*x as f64) * (*x as f64)).sum()
    }

    #[test]
    fn disabled_multiverb_passes_stereo_through() {
        let mut state = init_state(44_100);
        state[ST_ENABLED] = 0.0;
        let mut in_l = vec![0.25f32, -0.5, 0.75, -1.0];
        let mut in_r = vec![-0.125f32, 0.375, -0.625, 0.875];
        let (out_l, out_r) = process(&mut state, &mut in_l, &mut in_r);
        assert_eq!(out_l, in_l);
        assert_eq!(out_r, in_r);
    }

    #[test]
    fn impulse_produces_a_decaying_stereo_tail() {
        let fs = 44_100;
        for mode in [MODE_PLATE, MODE_HALL, MODE_QUAD, MODE_MOD] {
            let mut state = init_state(fs as i32);
            state[ST_MODE] = mode;
            state[ST_MIX] = 1.0;
            let (out_l, out_r) = impulse_render(&mut state, 3.0, fs);

            // Tail exists well after the impulse…
            let late = &out_l[fs..fs * 2];
            assert!(energy(late) > 1.0e-8, "no tail one second in (mode {mode})");
            // …decays…
            let early = energy(&out_l[fs / 4..fs / 2]);
            let later = energy(&out_l[fs * 2..fs * 5 / 2]);
            assert!(
                later < early,
                "tail did not decay (mode {mode}): {early} -> {later}"
            );
            // …and is decorrelated stereo, not dual mono.
            let dot: f64 = out_l[fs..fs * 2]
                .iter()
                .zip(&out_r[fs..fs * 2])
                .map(|(l, r)| (*l as f64) * (*r as f64))
                .sum();
            let corr = dot / (energy(&out_l[fs..fs * 2]) * energy(&out_r[fs..fs * 2])).sqrt();
            assert!(
                corr.abs() < 0.9,
                "L/R fully correlated (mode {mode}): {corr}"
            );
        }
    }

    #[test]
    fn longer_decay_setting_holds_more_late_energy() {
        let fs = 44_100;
        for mode in [MODE_PLATE, MODE_HALL, MODE_QUAD, MODE_MOD] {
            let mut energies = Vec::new();
            for decay in [0.2f32, 0.55, 0.9] {
                let mut state = init_state(fs as i32);
                state[ST_MODE] = mode;
                state[ST_MIX] = 1.0;
                state[ST_DECAY] = decay;
                let (out_l, _) = impulse_render(&mut state, 2.5, fs);
                energies.push(energy(&out_l[fs..fs * 2]));
            }
            assert!(
                energies[0] < energies[1] && energies[1] < energies[2],
                "late energy not monotonic in decay (mode {mode}): {energies:?}"
            );
        }
    }

    #[test]
    fn hall_echo_density_blooms_from_sparse_to_dense() {
        // Concert-hall signature: the impulse response starts sparse and
        // thickens as energy recirculates through the 8 loop allpasses.
        let fs = 44_100;
        let mut state = init_state(fs as i32);
        state[ST_MODE] = MODE_HALL;
        state[ST_MIX] = 1.0;
        state[ST_DECAY] = 0.7;
        let (out_l, _) = impulse_render(&mut state, 3.0, fs);

        // Density metric: fraction of samples in a window whose magnitude
        // exceeds 10% of the window peak — low for spikes-in-silence, high
        // for a solid noise-like tail.
        let density = |win: &[f32]| {
            let peak = win.iter().fold(0.0f32, |a, x| a.max(x.abs()));
            if peak <= 0.0 {
                return 0.0;
            }
            win.iter().filter(|x| x.abs() > 0.1 * peak).count() as f32 / win.len() as f32
        };
        let early = density(&out_l[fs * 3 / 100..fs * 13 / 100]); // 30-130 ms
        let late = density(&out_l[fs * 60 / 100..fs * 70 / 100]); // 600-700 ms
        assert!(
            late > early * 1.5,
            "hall density did not bloom: early {early} late {late}"
        );
    }

    #[test]
    fn depth_zero_nulls_hall_and_mod_against_unmodulated_reference_renders() {
        let fs = 44_100;
        for mode in [MODE_HALL, MODE_MOD] {
            let mut reference = init_state(fs as i32);
            reference[ST_MODE] = mode;
            reference[ST_MIX] = 1.0;
            reference[ST_MOD_DEPTH] = 0.0;
            reference[ST_MOD_RATE] = 0.05;
            reference[ST_MOD_SHAPE] = 0.0;

            let mut moving_modulators = reference.clone();
            moving_modulators[ST_MOD_RATE] = 8.0;
            moving_modulators[ST_MOD_SHAPE] = 1.0;

            let (reference_l, reference_r) = impulse_render(&mut reference, 2.0, fs);
            let (moving_l, moving_r) = impulse_render(&mut moving_modulators, 2.0, fs);
            let max_residual = reference_l
                .iter()
                .chain(&reference_r)
                .zip(moving_l.iter().chain(&moving_r))
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert_eq!(
                max_residual, 0.0,
                "depth-zero mode {mode} render did not null: residual {max_residual}"
            );
        }
    }

    #[test]
    fn all_host_modulation_targets_change_the_rendered_result() {
        let fs = 44_100;
        for (target_slot, depth) in [
            (ST_MOD_DECAY_DEPTH_1, 0.45),
            (ST_MOD_SIZE_DEPTH_1, 0.30),
            (ST_MOD_DEPTH_DEPTH_1, 0.75),
            (ST_MOD_MIX_DEPTH_1, -0.35),
        ] {
            let mut reference = init_state(fs as i32);
            reference[ST_MODE] = MODE_MOD;
            reference[ST_DECAY] = 0.35;
            reference[ST_SIZE] = 0.5;
            reference[ST_MOD_DEPTH] = 0.0;
            reference[ST_MIX] = 0.65;

            let mut modulated = reference.clone();
            modulated[target_slot] = depth;
            let (reference_l, reference_r) =
                impulse_render_with_mod(&mut reference, 1.5, fs, [1.0, 0.0, 0.0, 0.0]);
            let (modulated_l, modulated_r) =
                impulse_render_with_mod(&mut modulated, 1.5, fs, [1.0, 0.0, 0.0, 0.0]);
            let residual = reference_l
                .iter()
                .chain(&reference_r)
                .zip(modulated_l.iter().chain(&modulated_r))
                .map(|(a, b)| {
                    let d = (a - b) as f64;
                    d * d
                })
                .sum::<f64>();
            assert!(
                residual > 1.0e-7,
                "host modulation target at state slot {target_slot} had no audible effect"
            );
        }
    }

    #[test]
    fn minimum_size_turns_every_mode_into_a_short_resonator() {
        let fs = 44_100;
        for mode in [MODE_PLATE, MODE_HALL, MODE_QUAD, MODE_MOD] {
            let mut state = init_state(fs as i32);
            state[ST_MODE] = mode;
            state[ST_SIZE] = 0.0;
            state[ST_DECAY] = 0.7;
            state[ST_MIX] = 1.0;

            // Let the production size smoother reach the bottom before the
            // impulse; this tests the settled resonator tuning, not the
            // intentional pitch dive from the default size.
            let mut silence_l = vec![0.0f32; fs];
            let mut silence_r = vec![0.0f32; fs];
            let _ = process(&mut state, &mut silence_l, &mut silence_r);
            let (out_l, out_r) = impulse_render(&mut state, 0.35, fs);
            let tail = energy(&out_l[fs / 200..fs / 5]) + energy(&out_r[fs / 200..fs / 5]);
            assert!(
                tail > 1.0e-8,
                "minimum-size mode {mode} did not sustain a short resonant tail: {tail}"
            );
        }
    }

    #[test]
    fn mode_switch_fades_clears_and_stays_clean() {
        let fs = 44_100usize;
        let mut state = init_state(fs as i32);
        state[ST_MIX] = 1.0;
        state[ST_DECAY] = 0.9;
        // Excite the plate hard, then switch to hall mid-tail.
        let (_, _) = impulse_render(&mut state, 0.5, fs);
        state[ST_MODE] = MODE_HALL;
        let mut post = Vec::new();
        for _ in 0..(fs / 256) {
            let mut in_l = vec![0.0f32; 256];
            let mut in_r = vec![0.0f32; 256];
            let (ol, _) = process(&mut state, &mut in_l, &mut in_r);
            post.extend_from_slice(&ol);
        }
        assert!(post.iter().all(|x| x.is_finite()));
        // Old plate tail is dropped: 100 ms after the switch (fade complete)
        // the output must be essentially silent — the hall tank started
        // empty and no new input arrived.
        let after = &post[fs / 10..fs / 2];
        let peak = after.iter().fold(0.0f32, |a, x| a.max(x.abs()));
        assert!(peak < 1.0e-4, "old tail leaked through mode switch: {peak}");
        assert_eq!(state[ST_ACTIVE_MODE], 1.0);
        // And the fade must have been a ramp, not a cliff: successive-sample
        // deltas in the fade region stay small relative to the tail level.
        let pre_peak = post[..fs / 100].iter().fold(0.0f32, |a, x| a.max(x.abs()));
        let max_step = post[..fs / 20]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_step < (pre_peak * 0.5).max(1.0e-3),
            "mode switch clicked: step {max_step} vs tail peak {pre_peak}"
        );
    }

    #[test]
    fn all_modes_and_extreme_params_stay_finite() {
        let fs = 44_100;
        for mode in [MODE_PLATE, MODE_HALL, MODE_QUAD, MODE_MOD] {
            // size 0.0 = the degenerate comb-bank regime, 1.0 = max tank.
            for tank_size in [0.0f32, 1.0] {
                let mut state = init_state(fs as i32);
                state[ST_MODE] = mode;
                state[ST_MIX] = 1.0;
                state[ST_DECAY] = 1.0;
                state[ST_BASS] = 1.0;
                state[ST_MOD_DEPTH] = 1.0;
                state[ST_MOD_RATE] = 8.0;
                state[ST_MOD_SHAPE] = 1.0;
                state[ST_ERA] = 1.0;
                state[ST_SIZE] = tank_size;
                state[ST_PREDELAY_MS] = 250.0;
                let (out_l, out_r) = impulse_render(&mut state, 4.0, fs);
                assert!(
                    out_l.iter().chain(out_r.iter()).all(|x| x.is_finite()),
                    "non-finite output in mode {mode} size {tank_size}"
                );
                let peak = out_l.iter().fold(0.0f32, |a, x| a.max(x.abs()));
                assert!(
                    peak < 4.5,
                    "runaway output in mode {mode} size {tank_size}: peak {peak}"
                );
            }
        }
    }

    #[test]
    fn tail_survives_long_silence_without_denormal_blowup_and_stays_quiet() {
        let fs = 44_100;
        for mode in [MODE_PLATE, MODE_HALL, MODE_QUAD, MODE_MOD] {
            let mut state = init_state(fs as i32);
            state[ST_MODE] = mode;
            state[ST_MIX] = 1.0;
            state[ST_DECAY] = 0.4;
            let _ = impulse_render(&mut state, 30.0, fs);
            // After 30 s at a short decay the tail must be effectively silent
            // and every state slot finite.
            let mut in_l = vec![0.0f32; 256];
            let mut in_r = vec![0.0f32; 256];
            let (out_l, _) = process(&mut state, &mut in_l, &mut in_r);
            assert!(out_l.iter().all(|x| x.is_finite()));
            let peak = out_l.iter().fold(0.0f32, |a, x| a.max(x.abs()));
            assert!(peak < 1.0e-5, "tail failed to die (mode {mode}): {peak}");
            assert!(state.iter().all(|x| x.is_finite()));
            assert!(
                state[ST_BUFS..]
                    .iter()
                    .all(|x| *x == 0.0 || x.abs() >= f32::MIN_POSITIVE),
                "mode {mode} left denormals in recursive delay memory"
            );
        }
    }

    #[test]
    fn size_and_sample_rate_scaled_reads_stay_inside_buffer_capacity() {
        // Worst case: 96 kHz, size knob maxed, deepest modulation.
        let fs = 96_000.0f32;
        let scale = (fs / DATTORRO_FS) * 0.5 * 4.0f32;
        for b in 1..NBUFS {
            // Plate AP1s see up to 3 ms excursion; hall section delays are
            // modulated up to 30 ms but never beyond 0.7× their own scaled
            // length; quad loop delays get a shallow 2 ms wobble; everything
            // else is read at its scaled base length.
            let exc = if b == BUF_AP_L1 || b == BUF_AP_R1 {
                0.003 * fs
            } else if (BUF_H_DEL..BUF_Q_AP).contains(&b) {
                (0.030 * fs).min(BASE_LENS[b] as f32 * scale * 0.7)
            } else if (BUF_Q_DEL..BUF_M_COMB_L).contains(&b) {
                (0.002 * fs).min(BASE_LENS[b] as f32 * scale * 0.7)
            } else if b >= BUF_M_AP_L {
                let len = BASE_LENS[b] as f32 * (fs / 44_100.0);
                (48.0 * fs / 44_100.0).min(len * 0.7)
            } else {
                0.0
            };
            // Mod-mode buffers use Freeverb's 44.1 kHz reference.
            let eff_scale = if b >= BUF_M_COMB_L {
                (fs / 44_100.0) * 0.5 * 4.0
            } else {
                scale
            };
            let max_read = BASE_LENS[b] as f32 * eff_scale + exc;
            assert!(
                (max_read as usize) < buf_cap(b) - 2,
                "buffer {b} capacity too small: reads {max_read}, cap {}",
                buf_cap(b)
            );
        }
        assert!((250.0 * 0.001 * fs) < (PREDELAY_CAP - 2) as f32);
    }
}
