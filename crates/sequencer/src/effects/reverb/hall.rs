//! Hall tank — Lexicon 224 Concert Hall topology: 2 input diffusers → a
//! single loop of 4 sections (allpass → allpass → modulated delay), damping
//! applied once per loop pass, decay gain distributed per section, L/R
//! outputs pulled from two disjoint 4-tap sets. Modulation is a per-section
//! random walk (Griesinger "spin"/"wander") with excursions up to 30 ms —
//! what keeps very long decays from going metallic. Ported from Multiverb's
//! hall branch (`effects/multiverb.rs`) minus the era grit and its 1-pole
//! damp/bass filters, which the shared feedback shelves replace.

use super::tank_common::*;
use super::ShelfCoefs;

// Disjoint L/R tap sets from the four section delays (samples @ 29,761 Hz).
const TAPS_L: [(usize, usize, f32); 4] = [
    (RB_H_DEL, 601, 1.0),
    (RB_H_DEL + 1, 211, -1.0),
    (RB_H_DEL + 2, 887, 1.0),
    (RB_H_DEL + 3, 359, -1.0),
];
const TAPS_R: [(usize, usize, f32); 4] = [
    (RB_H_DEL, 313, 1.0),
    (RB_H_DEL + 1, 719, 1.0),
    (RB_H_DEL + 2, 197, -1.0),
    (RB_H_DEL + 3, 1123, -1.0),
];

/// Per-allpass base gains for the loop sections (Costello: 224 allpass gains
/// run high, ~0.6-0.72); scaled by the diffusion knob.
const AP_G: [f32; 8] = [0.62, 0.70, 0.66, 0.72, 0.60, 0.68, 0.64, 0.71];

/// The 224's modulation was never a clean LFO: this much of the section
/// wander is random walk even at the default shape.
const WALK_SHAPE: f32 = 0.35;

pub(super) struct HallParams {
    /// Per-section gain = decay_g^0.25 (distributed round the loop).
    pub sect_g: f32,
    pub in_g: f32,
    pub dscale: f32,
    /// Max section-delay excursion in samples (before the per-delay clamp).
    pub exc_samps: f32,
    pub walk_coef: f32,
    pub shelves: ShelfCoefs,
}

#[derive(Default, Clone, Copy)]
pub(super) struct HallState {
    pub walk: [f32; 4],
    pub walk_tgt: [f32; 4],
    pub shelf: [f32; 2],
}

pub(super) const STATE_SLOTS: usize = 10;

impl HallState {
    pub unsafe fn load(s: *const f32, base: usize) -> Self {
        let mut st = Self::default();
        for k in 0..4 {
            st.walk[k] = *s.add(base + k);
            st.walk_tgt[k] = *s.add(base + 4 + k);
        }
        st.shelf = [*s.add(base + 8), *s.add(base + 9)];
        st
    }

    pub unsafe fn store(&self, s: *mut f32, base: usize) {
        for k in 0..4 {
            *s.add(base + k) = super::flush(self.walk[k]);
            *s.add(base + 4 + k) = self.walk_tgt[k];
        }
        *s.add(base + 8) = super::flush(self.shelf[0]);
        *s.add(base + 9) = super::flush(self.shelf[1]);
    }
}

/// One sample: mono tank input → (wet_l, wet_r). `q_before/q_after` are the
/// LFO quarter-cycle indices around this sample's phase advance.
#[inline(always)]
pub(super) unsafe fn process_sample(
    rings: &mut Rings,
    st: &mut HallState,
    p: &HallParams,
    x_in: f32,
    sm_scale: f32,
    lfo_phase: f32,
    q_before: usize,
    q_after: usize,
    wrapped: bool,
    rng: &mut u32,
) -> (f32, f32) {
    // Per-section random walks retargeted round-robin, one per quarter of
    // the LFO cycle, blended with quadrature LFO taps.
    if q_after != q_before || wrapped {
        st.walk_tgt[q_after] = xorshift32(rng);
    }
    let mut mods = [0.0f32; 4];
    for k in 0..4 {
        st.walk[k] += p.walk_coef * (st.walk_tgt[k] - st.walk[k]);
        let ph = (lfo_phase + k as f32 * 0.25) * std::f32::consts::TAU;
        mods[k] = (1.0 - WALK_SHAPE) * ph.sin() + WALK_SHAPE * st.walk[k];
    }

    let mut x = x_in;
    x = rings.allpass(
        RB_H_DIFF1,
        x,
        p.in_g,
        BASE_LENS[RB_H_DIFF1] as f32 * sm_scale,
    );
    x = rings.allpass(
        RB_H_DIFF2,
        x,
        p.in_g,
        BASE_LENS[RB_H_DIFF2] as f32 * sm_scale,
    );

    // Loop feedback = section-4 delay output through the once-per-loop
    // damping shelves.
    let len4 = BASE_LENS[RB_H_DEL + 3] as f32 * sm_scale;
    let exc4 = p.exc_samps.min(len4 * 0.7);
    let fb_raw = rings.read_frac(RB_H_DEL + 3, len4 + exc4 * mods[3]);
    let fb = p.shelves.apply(fb_raw, &mut st.shelf) * p.sect_g;

    let mut t = clamp_node(x + fb);
    for k in 0..4 {
        let g1 = (AP_G[2 * k] * p.dscale).min(0.75);
        let g2 = (AP_G[2 * k + 1] * p.dscale).min(0.75);
        t = rings.allpass(
            RB_H_AP + 2 * k,
            t,
            g1,
            BASE_LENS[RB_H_AP + 2 * k] as f32 * sm_scale,
        );
        t = rings.allpass(
            RB_H_AP + 2 * k + 1,
            t,
            g2,
            BASE_LENS[RB_H_AP + 2 * k + 1] as f32 * sm_scale,
        );
        if k < 3 {
            let len = BASE_LENS[RB_H_DEL + k] as f32 * sm_scale;
            let exc = p.exc_samps.min(len * 0.7);
            let d = rings.read_frac(RB_H_DEL + k, len + exc * mods[k]);
            rings.write(RB_H_DEL + k, clamp_node(t));
            t = d * p.sect_g;
        } else {
            rings.write(RB_H_DEL + 3, clamp_node(t));
        }
    }

    let mut y_l = 0.0f32;
    for &(buf, off, sign) in TAPS_L.iter() {
        y_l += sign * rings.read_frac(buf, off as f32 * sm_scale);
    }
    let mut y_r = 0.0f32;
    for &(buf, off, sign) in TAPS_R.iter() {
        y_r += sign * rings.read_frac(buf, off as f32 * sm_scale);
    }
    (y_l * 0.5, y_r * 0.5)
}
