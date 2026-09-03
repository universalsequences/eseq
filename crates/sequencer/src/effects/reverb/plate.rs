//! Plate tank — Dattorro, "Effect Design Part 1: Reverberator and Other
//! Filters" (JAES 45(9), 1997): 4 series input diffusers → cross-coupled
//! figure-eight tank with modulated decay-diffusion allpasses and fixed
//! output tap tables. Ported from Multiverb's plate branch
//! (`effects/multiverb.rs`) minus the era grit and its 1-pole damp/bass
//! filters, which the shared feedback shelves replace.

use super::tank_common::*;
use super::ShelfCoefs;

// ── Output tap tables (samples @ 29,761 Hz) ──

const TAPS_L: [(usize, usize, f32); 7] = [
    (RB_DEL_R1, 266, 1.0),
    (RB_DEL_R1, 2974, 1.0),
    (RB_AP_R2, 1913, -1.0),
    (RB_DEL_R2, 1996, 1.0),
    (RB_DEL_L1, 1990, -1.0),
    (RB_AP_L2, 187, -1.0),
    (RB_DEL_L2, 1066, -1.0),
];

const TAPS_R: [(usize, usize, f32); 7] = [
    (RB_DEL_L1, 353, 1.0),
    (RB_DEL_L1, 3627, 1.0),
    (RB_AP_L2, 1228, -1.0),
    (RB_DEL_L2, 2673, 1.0),
    (RB_DEL_R1, 2111, -1.0),
    (RB_AP_R2, 335, -1.0),
    (RB_DEL_R2, 121, -1.0),
];

/// Per-block derived parameters.
pub(super) struct PlateParams {
    pub decay_g: f32,
    pub ap1_g: f32,
    pub ap2_g: f32,
    pub in_g1: f32,
    pub in_g2: f32,
    /// Max AP1 modulation excursion in samples (before the per-delay clamp).
    pub exc_samps: f32,
    pub walk_coef: f32,
    pub shelves: ShelfCoefs,
}

/// Runtime state carried across blocks (mirrors the `ST_P_*` slots).
#[derive(Default, Clone, Copy)]
pub(super) struct PlateState {
    pub walk_l: f32,
    pub walk_tgt_l: f32,
    pub walk_r: f32,
    pub walk_tgt_r: f32,
    pub shelf_l: [f32; 2],
    pub shelf_r: [f32; 2],
}

pub(super) const STATE_SLOTS: usize = 8;

impl PlateState {
    pub unsafe fn load(s: *const f32, base: usize) -> Self {
        Self {
            walk_l: *s.add(base),
            walk_tgt_l: *s.add(base + 1),
            walk_r: *s.add(base + 2),
            walk_tgt_r: *s.add(base + 3),
            shelf_l: [*s.add(base + 4), *s.add(base + 5)],
            shelf_r: [*s.add(base + 6), *s.add(base + 7)],
        }
    }

    pub unsafe fn store(&self, s: *mut f32, base: usize) {
        *s.add(base) = super::flush(self.walk_l);
        *s.add(base + 1) = self.walk_tgt_l;
        *s.add(base + 2) = super::flush(self.walk_r);
        *s.add(base + 3) = self.walk_tgt_r;
        *s.add(base + 4) = super::flush(self.shelf_l[0]);
        *s.add(base + 5) = super::flush(self.shelf_l[1]);
        *s.add(base + 6) = super::flush(self.shelf_r[0]);
        *s.add(base + 7) = super::flush(self.shelf_r[1]);
    }
}

/// One sample: mono tank input → (wet_l, wet_r).
#[inline(always)]
pub(super) unsafe fn process_sample(
    rings: &mut Rings,
    st: &mut PlateState,
    p: &PlateParams,
    x_in: f32,
    sm_scale: f32,
    lfo_phase: f32,
    wrapped: bool,
    rng: &mut u32,
) -> (f32, f32) {
    // LFO plus a slow random walk retargeted once per LFO cycle (subtle
    // "spin" so long tails never lock into a static comb).
    if wrapped {
        st.walk_tgt_l = xorshift32(rng);
        st.walk_tgt_r = xorshift32(rng);
    }
    st.walk_l += p.walk_coef * (st.walk_tgt_l - st.walk_l);
    st.walk_r += p.walk_coef * (st.walk_tgt_r - st.walk_r);
    let ph = lfo_phase * std::f32::consts::TAU;
    let mod_l = ph.sin();
    let mod_r = (ph + std::f32::consts::FRAC_PI_2).sin();

    let mut x = x_in;
    x = rings.allpass(RB_DIFF1, x, p.in_g1, BASE_LENS[RB_DIFF1] as f32 * sm_scale);
    x = rings.allpass(RB_DIFF2, x, p.in_g1, BASE_LENS[RB_DIFF2] as f32 * sm_scale);
    x = rings.allpass(RB_DIFF3, x, p.in_g2, BASE_LENS[RB_DIFF3] as f32 * sm_scale);
    x = rings.allpass(RB_DIFF4, x, p.in_g2, BASE_LENS[RB_DIFF4] as f32 * sm_scale);
    let diffused = x;

    // Cross-coupling: each branch input takes the other branch's final
    // delay output (read before this sample's write).
    let cross_r = rings.read_frac(RB_DEL_R2, BASE_LENS[RB_DEL_R2] as f32 * sm_scale);
    let cross_l = rings.read_frac(RB_DEL_L2, BASE_LENS[RB_DEL_L2] as f32 * sm_scale);

    // Excursion can't exceed a fraction of the (possibly tiny) delay it
    // modulates, or deep mod at small sizes pins reads at the clamp.
    let exc_l = p.exc_samps.min(BASE_LENS[RB_AP_L1] as f32 * sm_scale * 0.7);
    let exc_r = p.exc_samps.min(BASE_LENS[RB_AP_R1] as f32 * sm_scale * 0.7);

    // Left branch.
    let mut tl = clamp_node(diffused + cross_r * p.decay_g);
    tl = rings.allpass(
        RB_AP_L1,
        tl,
        p.ap1_g,
        BASE_LENS[RB_AP_L1] as f32 * sm_scale + exc_l * mod_l,
    );
    let del_l1_out = rings.read_frac(RB_DEL_L1, BASE_LENS[RB_DEL_L1] as f32 * sm_scale);
    rings.write(RB_DEL_L1, tl);
    let mut dl = p.shelves.apply(del_l1_out, &mut st.shelf_l) * p.decay_g;
    dl = rings.allpass(RB_AP_L2, dl, p.ap2_g, BASE_LENS[RB_AP_L2] as f32 * sm_scale);
    rings.write(RB_DEL_L2, clamp_node(dl));

    // Right branch.
    let mut tr = clamp_node(diffused + cross_l * p.decay_g);
    tr = rings.allpass(
        RB_AP_R1,
        tr,
        p.ap1_g,
        BASE_LENS[RB_AP_R1] as f32 * sm_scale + exc_r * mod_r,
    );
    let del_r1_out = rings.read_frac(RB_DEL_R1, BASE_LENS[RB_DEL_R1] as f32 * sm_scale);
    rings.write(RB_DEL_R1, tr);
    let mut dr = p.shelves.apply(del_r1_out, &mut st.shelf_r) * p.decay_g;
    dr = rings.allpass(RB_AP_R2, dr, p.ap2_g, BASE_LENS[RB_AP_R2] as f32 * sm_scale);
    rings.write(RB_DEL_R2, clamp_node(dr));

    // Output taps.
    let mut y_l = 0.0f32;
    for &(buf, off, sign) in TAPS_L.iter() {
        y_l += sign * rings.read_frac(buf, off as f32 * sm_scale);
    }
    let mut y_r = 0.0f32;
    for &(buf, off, sign) in TAPS_R.iter() {
        y_r += sign * rings.read_frac(buf, off as f32 * sm_scale);
    }
    (y_l * 0.6, y_r * 0.6)
}
