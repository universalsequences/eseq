//! Dispersive spring reverb (Parker/Välimäki-style) used by the Space Echo.
//!
//! Structure per tank:
//!
//! ```text
//! input -> drip soft-sat -> 1-pole HP -> biquad LP
//!       -> 2 series Schroeder diffusion allpasses
//!       -> 3 parallel dispersive loops:
//!            [delay] -> [M first-order allpasses] -> output tap
//!                       feedback path: damping LP + low/high shelf split -> gain -> delay
//!       -> weighted loop sum + short feedback-comb HF path
//! ```
//!
//! The cascade of identical first-order allpasses gives the frequency-
//! dependent group delay that smears each loop round trip into the spring's
//! "boing" chirp. All parameters are in physical units (Hz / seconds / linear
//! gain) and coefficients are derived per sample rate, so the tank behaves
//! identically at 44.1/48/96 kHz.
//!
//! State lives in a caller-provided flat `&mut [f32]` of `SPRING_TANK_STATE_LEN`
//! floats so the same code runs inside the space-echo node's flat state array
//! and inside the offline tuner bin (`src/bin/spring_tune.rs`).

/// Parallel dispersive loops per tank.
pub const SPRING_LOOPS: usize = 3;
/// Maximum allpasses per loop the state layout supports; the active count is
/// a parameter (`SpringParams::ap_per_loop`) so the tuner can sweep it.
pub const SPRING_AP_MAX: usize = 128;
/// Maximum allpass stretch factor (z^-1 → z^-K). K places the dispersion
/// peak at fs/(2K) — the spring transition frequency — so highs arrive later
/// than lows within each chirp, like a real spring.
pub const SPRING_K_MAX: usize = 8;

const LOOP_BUF_LEN: usize = 16384; // ≤ ~150 ms net loop delay at 96 kHz
const DIFF_BUF_LEN: usize = 1024;
const HF_BUF_LEN: usize = 1024;

// Scalar layout within a tank's state slice.
const SC_AP_Z: usize = 0; // LOOPS × AP_MAX × K_MAX stretched-allpass states
const SC_DAMP: usize = SC_AP_Z + SPRING_LOOPS * SPRING_AP_MAX * SPRING_K_MAX;
const SC_SHELF: usize = SC_DAMP + SPRING_LOOPS; // per-loop shelf crossover LP
const SC_HP_LP: usize = SC_SHELF + SPRING_LOOPS;
const SC_HP_LP2: usize = SC_HP_LP + 1;
const SC_IN_LP_Z1: usize = SC_HP_LP2 + 1;
const SC_IN_LP_Z2: usize = SC_IN_LP_Z1 + 1;
const SC_IN_LP_Z3: usize = SC_IN_LP_Z2 + 1;
const SC_IN_LP_Z4: usize = SC_IN_LP_Z3 + 1;
const SC_HF_LP: usize = SC_IN_LP_Z4 + 1;
const SC_WPOS: usize = SC_HF_LP + 1;
const SC_AP_PHASE: usize = SC_WPOS + 1; // cycles 0..K-1 (K need not divide the wpos wrap)
const SC_HF_IN_LP: usize = SC_AP_PHASE + 1; // HF-path input highpass state
const SCALARS: usize = SC_AP_PHASE + 4; // + spares

const BUF_LOOP0: usize = SCALARS;
const BUF_DIFF0: usize = BUF_LOOP0 + SPRING_LOOPS * LOOP_BUF_LEN;
const BUF_HF: usize = BUF_DIFF0 + 2 * DIFF_BUF_LEN;

/// Total f32 slots one tank needs.
pub const SPRING_TANK_STATE_LEN: usize = BUF_HF + HF_BUF_LEN;

// Shared write counter wrap: divisible by every (power-of-two) buffer length
// and exactly representable in f32.
const WPOS_WRAP: usize = 8_388_608;

/// All tank parameters, in physical units. Tuned offline against
/// `spring_reverb_impulse.wav` by `scripts/spring_tune.py` (Nelder-Mead).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpringParams {
    /// Active first-order allpasses per loop (≤ `SPRING_AP_MAX`).
    pub ap_per_loop: usize,
    /// Low-frequency round-trip time of each loop, seconds.
    pub d_loop: [f32; SPRING_LOOPS],
    /// Output tap weight of each loop.
    pub w_loop: [f32; SPRING_LOOPS],
    /// Low-band decay time, seconds.
    pub t60: f32,
    /// Allpass-chain group delay at DC, seconds (sets the chirp/dispersion).
    pub t_dc: f32,
    /// Dispersion-peak (spring transition) frequency, Hz → stretch factor
    /// K = round(fs / (2·f_peak)).
    pub f_peak: f32,
    /// In-loop damping lowpass, Hz.
    pub f_damp: f32,
    /// In-loop shelf crossover, Hz: below it the loop decays with `t60`,
    /// above it the per-pass gain is additionally scaled by `g_shelf`.
    pub f_shelf: f32,
    /// Per-pass gain multiplier above `f_shelf` (≤ 1 → mids die faster).
    pub g_shelf: f32,
    /// Input highpass, Hz.
    pub f_hp: f32,
    /// Input lowpass (spring transition frequency), Hz.
    pub f_lp: f32,
    /// Diffuser allpass coefficient.
    pub c_df: f32,
    /// First diffuser delay, seconds (second is 2.6×).
    pub d_df1: f32,
    /// HF path comb delay, seconds.
    pub d_hf: f32,
    /// HF path decay time, seconds.
    pub t60_hf: f32,
    /// HF path damping lowpass, Hz.
    pub f_hf_damp: f32,
    /// HF path output mix.
    pub g_hf: f32,
}

impl SpringParams {
    /// Hand seed before tuning; replaced by the optimized constants below.
    pub fn seed() -> Self {
        SpringParams {
            ap_per_loop: 80,
            d_loop: [0.0545, 0.0754, 0.0976],
            w_loop: [1.0, 0.8, 0.65],
            t60: 4.5,
            t_dc: 0.002,
            f_peak: 4300.0,
            f_damp: 2800.0,
            f_shelf: 250.0,
            g_shelf: 0.92,
            f_hp: 80.0,
            f_lp: 4500.0,
            c_df: 0.55,
            d_df1: 0.0047,
            d_hf: 0.0023,
            t60_hf: 0.5,
            f_hf_damp: 4000.0,
            g_hf: 0.15,
        }
    }

    /// Parameters fitted to `spring_reverb_impulse.wav` with
    /// `scripts/spring_tune.py` (staged Nelder-Mead: decay/EQ, then
    /// dispersion with an allpass-count sweep, then a joint multi-start
    /// polish). Final residuals vs the reference: EDC 0.59 dB RMS,
    /// 1/6-oct spectrum 4.6 dB, echo density 0.15, chirp ridge 14.4 ms.
    pub fn re201() -> Self {
        SpringParams {
            ap_per_loop: 80,
            d_loop: [0.057_982_56, 0.080_675_68, 0.097_750_0],
            w_loop: [1.0, 0.946_145_3, 0.597_101_9],
            t60: 53.055_11,
            t_dc: 0.001_956_374_6,
            f_peak: 4_692.831,
            f_damp: 897.749_5,
            f_shelf: 921.917_7,
            g_shelf: 0.884_591_3,
            f_hp: 166.949_92,
            f_lp: 2_216.071_5,
            c_df: 0.568_544,
            d_df1: 0.006_556_414,
            d_hf: 0.0023,
            t60_hf: 0.785_690_2,
            f_hf_damp: 3_596.918,
            g_hf: 0.216_239_3,
        }
    }

    /// Fitted to `impulses/prepared/king-tubby.wav` (Grampian spring, the
    /// King Tubby dub tank) with `scripts/spring_tune.py` staged Nelder-Mead
    /// from the `re201()` start, then a hand "boing" pass: the optimizer's
    /// centroid-based ridge metric under-rewards distinct chirp arcs, so
    /// `t_dc`/`ap_per_loop` were pushed up (chirp sweep ~7 ms → ~35 ms) and
    /// the diffusers backed off (c_df, d_df1) until the spectrogram shows the
    /// reference's repeating rising arcs — the audible spring boing.
    /// Brighter and much shorter than the RE-201 tank (flat to ~5 kHz,
    /// −20 dB by 1.0 s). Residuals: EDC 1.29 dB RMS, 1/6-oct spectrum
    /// 2.6 dB, echo density 0.14.
    pub fn king_tubby() -> Self {
        SpringParams {
            ap_per_loop: 128,
            d_loop: [0.049_275_733, 0.072_572_678, 0.099_002_319],
            w_loop: [1.0, 0.945_277_6, 0.603_259_06],
            t60: 12.674_776,
            t_dc: 0.020,
            f_peak: 5_655.696_7,
            f_damp: 2_247.093,
            f_shelf: 336.457_84,
            g_shelf: 0.950_482_36,
            f_hp: 360.119,
            f_lp: 5_045.882,
            c_df: 0.25,
            d_df1: 0.0015,
            d_hf: 0.0023,
            // Short HF-comb ring: at the fitted 0.84 s it reads as a pitched
            // note on transients rather than shimmer.
            t60_hf: 0.3,
            f_hf_damp: 4_718.846_7,
            g_hf: 0.265_802_4,
        }
    }
}

impl SpringParams {
    /// One-knob "spring tension" macro, 0..1 with 0.5 = these params exactly.
    /// Tighter spring = faster wave speed: every time-like parameter shrinks
    /// and every frequency-like parameter rises by the same physical factor
    /// (0.5×–2× across the throw), so any position is still a plausible
    /// spring — low tension a big dark dub tank, high a tight amp pan.
    pub fn with_tension(mut self, tension: f32) -> Self {
        let t = tension.clamp(0.0, 1.0);
        let s = (2.0f32).powf(2.0 * (t - 0.5));
        // K = round(fs/2·f_peak) quantizes coarsely above ~7.5 kHz, so the
        // transition frequency stops scaling before the rest does.
        let s_peak = s.min(1.6);
        for d in self.d_loop.iter_mut() {
            *d /= s;
        }
        self.f_peak *= s_peak;
        self.f_lp *= s;
        self.f_damp *= s;
        self.f_shelf *= s;
        self.t_dc /= s;
        self.d_df1 /= s;
        // d_hf deliberately NOT scaled: the HF comb's fundamental is faintly
        // audible on transients, and having the tension knob repitch it reads
        // as a synthetic tone rather than a spring.
        // Loose springs ring a touch longer.
        self.t60 *= s.powf(-0.4);
        self
    }
}

#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

fn lowpass_biquad(freq: f32, sr: f32) -> Biquad {
    let omega = std::f32::consts::TAU * freq.clamp(20.0, sr * 0.49) / sr.max(1.0);
    let sin = omega.sin();
    let cos = omega.cos();
    let alpha = sin * std::f32::consts::FRAC_1_SQRT_2;
    let a0 = 1.0 + alpha;
    Biquad {
        b0: (1.0 - cos) * 0.5 / a0,
        b1: (1.0 - cos) / a0,
        b2: (1.0 - cos) * 0.5 / a0,
        a1: (-2.0 * cos) / a0,
        a2: (1.0 - alpha) / a0,
    }
}

#[inline]
fn one_pole_coef(freq: f32, sr: f32) -> f32 {
    1.0 - (-std::f32::consts::TAU * freq / sr.max(1.0)).exp()
}

/// Per-sample-rate derived coefficients. Build once per block.
#[derive(Clone)]
pub struct SpringCoeffs {
    m: usize,
    k: usize,
    ap_a: f32,
    loop_delay: [usize; SPRING_LOOPS],
    loop_gain: [f32; SPRING_LOOPS],
    w_loop: [f32; SPRING_LOOPS],
    damp_coef: f32,
    shelf_coef: f32,
    g_shelf: f32,
    hp_coef: f32,
    in_lp: Biquad,
    c_df: f32,
    df_delay: [usize; 2],
    hf_delay: usize,
    hf_fb: f32,
    hf_damp_coef: f32,
    hf_in_hp_coef: f32,
    g_hf: f32,
}

impl SpringCoeffs {
    pub fn new(p: &SpringParams, sr: f32) -> Self {
        let m = p.ap_per_loop.clamp(1, SPRING_AP_MAX);
        // Stretch factor puts the dispersion peak (group-delay maximum of a
        // z^-K allpass) at fs/(2K) ≈ the spring transition frequency.
        let k = ((sr / (2.0 * p.f_peak.clamp(800.0, 12_000.0))).round() as usize)
            .clamp(1, SPRING_K_MAX);
        // Each stretched allpass y = -a·x + x[-K] + a·y[-K] contributes a DC
        // group delay of K·(1+a)/(1-a) samples; solve the chain for `a` so
        // the total DC delay is t_dc seconds at any sample rate. a < 0 (the
        // physical spring regime) makes highs arrive later than lows.
        let t = (p.t_dc.max(1e-5) * sr).max(0.1) / k as f32;
        let mf = m as f32;
        let ap_a = ((t - mf) / (t + mf)).clamp(-0.98, 0.98);
        let chain_delay = (k as f32) * mf * (1.0 + ap_a) / (1.0 - ap_a);

        let mut loop_delay = [1usize; SPRING_LOOPS];
        let mut loop_gain = [0f32; SPRING_LOOPS];
        for k in 0..SPRING_LOOPS {
            let net = (p.d_loop[k] * sr - chain_delay).round();
            loop_delay[k] = (net as isize).clamp(1, (LOOP_BUF_LEN - 1) as isize) as usize;
            loop_gain[k] = (10.0f32)
                .powf(-3.0 * p.d_loop[k] / p.t60.max(0.05))
                .min(0.9995);
        }

        let df1 = ((p.d_df1 * sr).round() as isize).clamp(1, (DIFF_BUF_LEN - 1) as isize) as usize;
        let df2 =
            ((p.d_df1 * 2.6 * sr).round() as isize).clamp(1, (DIFF_BUF_LEN - 1) as isize) as usize;
        let hf_d = ((p.d_hf * sr).round() as isize).clamp(1, (HF_BUF_LEN - 1) as isize) as usize;
        let hf_fb = (10.0f32)
            .powf(-3.0 * p.d_hf / p.t60_hf.max(0.01))
            .min(0.9995);

        SpringCoeffs {
            m,
            k,
            ap_a,
            loop_delay,
            loop_gain,
            w_loop: p.w_loop,
            damp_coef: one_pole_coef(p.f_damp, sr),
            shelf_coef: one_pole_coef(p.f_shelf, sr),
            g_shelf: p.g_shelf.clamp(0.0, 1.0),
            hp_coef: one_pole_coef(p.f_hp, sr),
            in_lp: lowpass_biquad(p.f_lp, sr),
            c_df: p.c_df.clamp(-0.95, 0.95),
            df_delay: [df1, df2],
            hf_delay: hf_d,
            hf_fb,
            hf_damp_coef: one_pole_coef(p.f_hf_damp, sr),
            // HP the comb input above its own fundamental (1/d_hf).
            hf_in_hp_coef: one_pole_coef(1.6 / p.d_hf.max(1e-4), sr),
            g_hf: p.g_hf,
        }
    }
}

/// Process one sample through one tank. `st` must be `SPRING_TANK_STATE_LEN`
/// floats, zero-initialized at node init.
#[inline]
pub fn spring_tank_process(input: f32, c: &SpringCoeffs, st: &mut [f32]) -> f32 {
    debug_assert!(st.len() >= SPRING_TANK_STATE_LEN);

    // Spring overload "drip" on hot transients.
    let x = input / (1.0 + 0.5 * input.abs());

    // Input band-limit: 2nd-order HP (real springs barely couple below ~100
    // Hz), 4th-order LP at the transition frequency.
    let hp = st[SC_HP_LP] + c.hp_coef * (x - st[SC_HP_LP]);
    st[SC_HP_LP] = hp;
    let x = x - hp;
    let hp = st[SC_HP_LP2] + c.hp_coef * (x - st[SC_HP_LP2]);
    st[SC_HP_LP2] = hp;
    let x = x - hp;
    // 4th-order input lowpass — the spring transition cliff is steep.
    let out = c.in_lp.b0 * x + st[SC_IN_LP_Z1];
    st[SC_IN_LP_Z1] = c.in_lp.b1 * x - c.in_lp.a1 * out + st[SC_IN_LP_Z2];
    st[SC_IN_LP_Z2] = c.in_lp.b2 * x - c.in_lp.a2 * out;
    let x = out;
    let out = c.in_lp.b0 * x + st[SC_IN_LP_Z3];
    st[SC_IN_LP_Z3] = c.in_lp.b1 * x - c.in_lp.a1 * out + st[SC_IN_LP_Z4];
    st[SC_IN_LP_Z4] = c.in_lp.b2 * x - c.in_lp.a2 * out;
    let mut x = out;

    let wpos = st[SC_WPOS] as usize;

    // Two series Schroeder diffusion allpasses.
    for (d, base) in [
        (c.df_delay[0], BUF_DIFF0),
        (c.df_delay[1], BUF_DIFF0 + DIFF_BUF_LEN),
    ] {
        let w = wpos % DIFF_BUF_LEN;
        let r = (w + DIFF_BUF_LEN - d) % DIFF_BUF_LEN;
        let delayed = st[base + r];
        let y = delayed - c.c_df * x;
        st[base + w] = x + c.c_df * y;
        x = y;
    }

    // Parallel dispersive loops. The stretched allpasses (z^-K) are realized
    // as K interleaved state slots per allpass, addressed by a shared phase.
    let phase = st[SC_AP_PHASE] as usize % c.k;
    let mut acc = 0.0f32;
    let lw = wpos % LOOP_BUF_LEN;
    for k in 0..SPRING_LOOPS {
        let base = BUF_LOOP0 + k * LOOP_BUF_LEN;
        let r = (lw + LOOP_BUF_LEN - c.loop_delay[k]) % LOOP_BUF_LEN;
        let mut v = st[base + r];
        let zs = SC_AP_Z + k * SPRING_AP_MAX * SPRING_K_MAX + phase;
        for z in st[zs..zs + c.m * SPRING_K_MAX]
            .iter_mut()
            .step_by(SPRING_K_MAX)
        {
            let y = -c.ap_a * v + *z;
            *z = v + c.ap_a * y;
            v = y;
        }
        // Feedback path: damping LP, then shelf split (lows keep t60, the band
        // above f_shelf gets an extra per-pass cut → non-exponential EDC).
        let damp = st[SC_DAMP + k] + c.damp_coef * (v - st[SC_DAMP + k]);
        st[SC_DAMP + k] = damp;
        let low = st[SC_SHELF + k] + c.shelf_coef * (damp - st[SC_SHELF + k]);
        st[SC_SHELF + k] = low;
        let fb = (low + c.g_shelf * (damp - low)) * c.loop_gain[k];
        st[base + lw] = x + fb;
        acc += c.w_loop[k] * v;
    }
    st[SC_AP_PHASE] = ((phase + 1) % c.k) as f32;

    // HF "shimmer" path: short damped feedback comb on the diffused input.
    // The comb's fundamental sits at 1/d_hf (~435 Hz), so its input is
    // highpassed above that — otherwise a kick's thump rings it as a clean
    // pitched note instead of transient sizzle.
    let hf_in_lp = st[SC_HF_IN_LP] + c.hf_in_hp_coef * (x - st[SC_HF_IN_LP]);
    st[SC_HF_IN_LP] = hf_in_lp;
    let hf_in = x - hf_in_lp;
    let hw = wpos % HF_BUF_LEN;
    let hr = (hw + HF_BUF_LEN - c.hf_delay) % HF_BUF_LEN;
    let hf_out = st[BUF_HF + hr];
    let hf_lp = st[SC_HF_LP] + c.hf_damp_coef * (hf_out - st[SC_HF_LP]);
    st[SC_HF_LP] = hf_lp;
    st[BUF_HF + hw] = hf_in + hf_lp * c.hf_fb;
    acc += c.g_hf * hf_out;

    st[SC_WPOS] = ((wpos + 1) % WPOS_WRAP) as f32;
    acc
}

/// Render a mono impulse response of one tank — used by the tuner bin and by
/// tests.
pub fn render_impulse(p: &SpringParams, sr: f32, seconds: f32, amp: f32) -> Vec<f32> {
    let n = (seconds * sr) as usize;
    let c = SpringCoeffs::new(p, sr);
    let mut st = vec![0.0f32; SPRING_TANK_STATE_LEN];
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let x = if i == 0 { amp } else { 0.0 };
        out.push(spring_tank_process(x, &c, &mut st));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edc_db(ir: &[f32]) -> Vec<f32> {
        let mut acc = 0.0f64;
        let mut rev: Vec<f64> = ir.iter().rev().map(|x| (*x as f64).powi(2)).collect();
        for v in rev.iter_mut() {
            acc += *v;
            *v = acc;
        }
        rev.reverse();
        let total = rev[0].max(1e-30);
        rev.iter()
            .map(|v| (10.0 * (v / total).log10()) as f32)
            .collect()
    }

    #[test]
    fn impulse_response_rings_and_decays() {
        let ir = render_impulse(&SpringParams::re201(), 44_100.0, 8.0, 0.25);
        assert!(ir.iter().all(|x| x.is_finite()));
        let edc = edc_db(&ir);
        // Matches the reference's decay scale: ~-29 dB EDC at 6.5 s (the
        // low band rings long, like the real spring's plateau).
        assert!(edc[(6.5 * 44_100.0) as usize] < -25.0);
        // The dominant arrival lands near the shortest loop delay, not at t=0
        // (the HF comb path may put low-level energy earlier, like a real
        // spring's fast transversal precursor).
        let p = SpringParams::re201();
        let peak = ir
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0;
        let expect = (p.d_loop[0].min(p.d_loop[1]).min(p.d_loop[2]) * 44_100.0) as usize;
        assert!(
            peak > expect / 2,
            "dominant energy arrives too early: {peak} vs loop {expect}"
        );
    }

    #[test]
    fn default_tension_is_the_tuned_fit_exactly() {
        let base = render_impulse(&SpringParams::re201(), 44_100.0, 1.0, 0.25);
        let half = render_impulse(
            &SpringParams::re201().with_tension(0.5),
            44_100.0,
            1.0,
            0.25,
        );
        assert_eq!(base, half, "tension 0.5 must be bit-identical to re201");
    }

    #[test]
    fn tension_extremes_stay_finite_and_shift_the_transit() {
        let lo = render_impulse(
            &SpringParams::re201().with_tension(0.0),
            44_100.0,
            2.0,
            0.25,
        );
        let hi = render_impulse(
            &SpringParams::re201().with_tension(1.0),
            44_100.0,
            2.0,
            0.25,
        );
        assert!(lo.iter().all(|x| x.is_finite()));
        assert!(hi.iter().all(|x| x.is_finite()));
        // Tight spring = shorter transit: the dominant arrival comes earlier.
        let peak = |ir: &[f32]| {
            ir.iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .unwrap()
                .0
        };
        assert!(
            peak(&hi) < peak(&lo),
            "high tension should arrive before low: {} vs {}",
            peak(&hi),
            peak(&lo)
        );
    }

    #[test]
    fn all_spring_type_presets_render_finite_and_distinct() {
        let presets = [SpringParams::re201(), SpringParams::king_tubby()];
        let irs: Vec<Vec<f32>> = presets
            .iter()
            .map(|p| render_impulse(p, 44_100.0, 2.0, 0.25))
            .collect();
        for (i, ir) in irs.iter().enumerate() {
            assert!(ir.iter().all(|x| x.is_finite()), "preset {i} not finite");
            assert!(ir.iter().any(|x| x.abs() > 1e-4), "preset {i} is silent");
        }
        for i in 0..irs.len() {
            for j in (i + 1)..irs.len() {
                let diff: f32 = irs[i].iter().zip(&irs[j]).map(|(a, b)| (a - b).abs()).sum();
                assert!(diff > 1e-2, "presets {i} and {j} render identically");
            }
        }
    }

    #[test]
    fn sample_rate_independent() {
        let p = SpringParams::re201();
        let a = render_impulse(&p, 44_100.0, 4.0, 0.25);
        let b = render_impulse(&p, 48_000.0, 4.0, 0.25);
        // First-arrival time in seconds agrees within 2 ms.
        let ta = a.iter().position(|x| x.abs() > 1e-3).unwrap() as f32 / 44_100.0;
        let tb = b.iter().position(|x| x.abs() > 1e-3).unwrap() as f32 / 48_000.0;
        assert!((ta - tb).abs() < 0.002, "first arrival {ta} vs {tb}");
        // EDC at 2 s agrees within 3 dB.
        let ea = edc_db(&a)[(2.0 * 44_100.0) as usize];
        let eb = edc_db(&b)[(2.0 * 48_000.0) as usize];
        assert!((ea - eb).abs() < 3.0, "EDC {ea} vs {eb}");
    }
}
