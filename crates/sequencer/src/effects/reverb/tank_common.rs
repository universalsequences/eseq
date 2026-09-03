//! Ring-buffer plumbing shared by the plate and hall tanks (ported from
//! `effects/multiverb.rs`, which stays untouched so retired Multiverb slots
//! keep rendering exactly as saved).

use super::{flush, ST_BUFS};

/// Ring buffers (fractional-read delay lines). Galaxy keeps its own
/// count/length buffers after these.
pub(super) const NRING: usize = 30;

pub(super) const RB_PREDELAY_L: usize = 0;
pub(super) const RB_PREDELAY_R: usize = 1;
pub(super) const RB_DIFF1: usize = 2;
pub(super) const RB_DIFF2: usize = 3;
pub(super) const RB_DIFF3: usize = 4;
pub(super) const RB_DIFF4: usize = 5;
pub(super) const RB_AP_L1: usize = 6;
pub(super) const RB_DEL_L1: usize = 7;
pub(super) const RB_AP_L2: usize = 8;
pub(super) const RB_DEL_L2: usize = 9;
pub(super) const RB_AP_R1: usize = 10;
pub(super) const RB_DEL_R1: usize = 11;
pub(super) const RB_AP_R2: usize = 12;
pub(super) const RB_DEL_R2: usize = 13;
pub(super) const RB_H_DIFF1: usize = 14;
pub(super) const RB_H_DIFF2: usize = 15;
pub(super) const RB_H_AP: usize = 16; // 16..24: section k allpasses at +2k, +2k+1
pub(super) const RB_H_DEL: usize = 24; // 24..28: section k delay at +k
pub(super) const RB_CHORUS_L: usize = 28;
pub(super) const RB_CHORUS_R: usize = 29;

pub(super) const PLATE_BUFS: std::ops::Range<usize> = RB_DIFF1..RB_H_DIFF1;
pub(super) const HALL_BUFS: std::ops::Range<usize> = RB_H_DIFF1..RB_CHORUS_L;

/// Dattorro reference sample rate; all tank base lengths are samples at it.
pub(super) const DATTORRO_FS: f32 = 29_761.0;

/// Base delay lengths at 29,761 Hz (predelay/chorus slots unused — fixed capacity).
pub(super) const BASE_LENS: [usize; NRING] = [
    0, 0, // predelay L/R
    142, 107, 379, 277, // plate input diffusers
    672, 4453, 1800, 3720, // plate left tank: AP1 (modulated), del1, AP2, del2
    908, 4217, 2656, 3163, // plate right tank: AP1 (modulated), del1, AP2, del2
    131, 239, // hall input diffusers
    229, 411, 331, 527, 433, 359, 283, 617, // hall section APs (4 × 2)
    743, 907, 1123, 1429, // hall section delays (modulated, deep)
    0, 0, // chorus L/R
];

/// 250 ms at 96 kHz plus interpolation headroom.
pub(super) const PREDELAY_CAP: usize = 24_064;
/// Chorus: 8 ms base + 6 ms excursion at 96 kHz, plus headroom.
pub(super) const CHORUS_CAP: usize = 2_048;
/// Max modulation excursion of the hall section delays: 30 ms at 96 kHz.
pub(super) const HALL_EXC_CAP: usize = 2_880;

/// Capacity per ring: base × 6.5 covers fs up to 96 kHz (×3.226) at max size
/// (×2.0). Plate buffers get +544 headroom (3 ms max mod excursion at 96 kHz
/// plus margin); the hall section delays are modulated far deeper.
pub(super) const fn ring_cap(i: usize) -> usize {
    if i == RB_PREDELAY_L || i == RB_PREDELAY_R {
        PREDELAY_CAP
    } else if i == RB_CHORUS_L || i == RB_CHORUS_R {
        CHORUS_CAP
    } else if i >= RB_H_DEL && i < RB_CHORUS_L {
        BASE_LENS[i] * 13 / 2 + HALL_EXC_CAP + 64
    } else {
        BASE_LENS[i] * 13 / 2 + 544
    }
}

const fn ring_offsets() -> [usize; NRING] {
    let mut offsets = [0usize; NRING];
    let mut offset = ST_BUFS;
    let mut i = 0;
    while i < NRING {
        offsets[i] = offset;
        offset += ring_cap(i);
        i += 1;
    }
    offsets
}

pub(super) const RING_OFFSETS: [usize; NRING] = ring_offsets();

pub(super) const fn total_ring_floats() -> usize {
    let mut total = 0;
    let mut i = 0;
    while i < NRING {
        total += ring_cap(i);
        i += 1;
    }
    total
}

#[inline(always)]
pub(super) fn clamp_node(x: f32) -> f32 {
    x.clamp(-4.0, 4.0)
}

#[inline(always)]
pub(super) fn tank_size_scale(size: f32) -> f32 {
    if size >= 0.5 {
        4.0_f32.powf(size - 0.5)
    } else {
        0.03_f32.powf(1.0 - 2.0 * size)
    }
}

#[inline(always)]
pub(super) fn one_pole_coef(cutoff_hz: f32, sample_rate: f32) -> f32 {
    let c = 1.0 - (-std::f32::consts::TAU * cutoff_hz / sample_rate).exp();
    c.clamp(0.0, 1.0)
}

#[inline(always)]
pub(super) fn xorshift32(state: &mut u32) -> f32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    // [-1, 1)
    (x as f32) * (2.0 / 4_294_967_296.0) - 1.0
}

/// Ring-buffer cursor set for one block: the shared state pointer plus the
/// per-ring write positions (loaded once per block, stored at block end).
pub(super) struct Rings {
    pub s: *mut f32,
    pub wpos: [usize; NRING],
}

impl Rings {
    /// Read a fractional delay `delay` samples behind the write head (which
    /// has not been written this sample yet, so delay 1.0 = last sample).
    #[inline(always)]
    pub unsafe fn read_frac(&self, buf: usize, delay: f32) -> f32 {
        let cap = ring_cap(buf);
        let ofs = RING_OFFSETS[buf];
        let d = delay.clamp(1.0, (cap - 2) as f32);
        let di = d as usize;
        let frac = d - di as f32;
        let i0 = (self.wpos[buf] + cap - di) % cap;
        let i1 = (i0 + cap - 1) % cap;
        let v0 = *self.s.add(ofs + i0);
        let v1 = *self.s.add(ofs + i1);
        v0 + (v1 - v0) * frac
    }

    #[inline(always)]
    pub unsafe fn write(&mut self, buf: usize, value: f32) {
        *self.s.add(RING_OFFSETS[buf] + self.wpos[buf]) = flush(value);
        self.wpos[buf] = (self.wpos[buf] + 1) % ring_cap(buf);
    }

    /// Allpass with feedback-around-delay: u = x + g·d, y = d − g·u.
    #[inline(always)]
    pub unsafe fn allpass(&mut self, buf: usize, x: f32, g: f32, delay: f32) -> f32 {
        let d = self.read_frac(buf, delay);
        let u = clamp_node(x + g * d);
        self.write(buf, u);
        d - g * u
    }

    pub unsafe fn clear(&mut self, bufs: std::ops::Range<usize>) {
        for b in bufs {
            std::ptr::write_bytes(self.s.add(RING_OFFSETS[b]), 0, ring_cap(b));
            self.wpos[b] = 0;
        }
    }
}
