//! The modes of a circular Kirchhoff plate (Leissa 1969), for eseq.modal's
//! `circ-plate`: every root of the clamped or free frequency equation in
//! increasing lambda, with each mode's radial shape tabulated from centre to
//! edge. Run once when an instrument is generated.

use eseqlisp::vm::{bessel_i, bessel_j};

/// One mode: lambda = a (rho h w^2 / D)^(1/4), its nodal diameters `m`, and
/// R(r) at `samples` radii from 0 to 1, scaled so R(r) cos(m theta) has mean
/// square 1/2 over the plate (the string's unit modal mass).
#[derive(Clone, Debug, PartialEq)]
pub struct PlateMode {
    pub lambda: f64,
    pub m: i32,
    pub radial: Vec<f64>,
}

/// How the plate's edge is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlateEdge {
    /// W = W' = 0.
    Clamped,
    /// No bending moment and no shear; the modes depend on Poisson's ratio.
    Free,
}

fn jd(m: i32, x: f64) -> f64 {
    (bessel_j(m - 1, x) - bessel_j(m + 1, x)) / 2.0
}

fn id(m: i32, x: f64) -> f64 {
    (bessel_i(m - 1, x) + bessel_i(m + 1, x)) / 2.0
}

/// The edge's bending moment (free) or displacement (clamped) from the J
/// and the I part of W = J_m(l r) + C I_m(l r) at r = 1, using the Bessel
/// equations to remove the second derivatives. The free-edge shear pair is
/// in `free_shear`.
fn edge_terms(edge: PlateEdge, m: i32, nu: f64, l: f64) -> (f64, f64) {
    let (k, mm) = (1.0 - nu, (m * m) as f64);
    match edge {
        PlateEdge::Clamped => (bessel_j(m, l), bessel_i(m, l)),
        // M ∝ W'' + nu (W' - m^2 W): J'' = -J'/x - (1 - m^2/x^2) J and
        // I'' = -I'/x + (1 + m^2/x^2) I.
        PlateEdge::Free => (
            -(l * l * bessel_j(m, l) + k * (l * jd(m, l) - mm * bessel_j(m, l))),
            l * l * bessel_i(m, l) - k * (l * id(m, l) - mm * bessel_i(m, l)),
        ),
    }
}

fn free_shear(m: i32, nu: f64, l: f64) -> (f64, f64) {
    let (k, mm) = (1.0 - nu, (m * m) as f64);
    (
        l * l * l * jd(m, l) + k * mm * (l * jd(m, l) - bessel_j(m, l)),
        l * l * l * id(m, l) - k * mm * (l * id(m, l) - bessel_i(m, l)),
    )
}

/// The frequency equation: zero at each mode.
fn edge_equation(edge: PlateEdge, m: i32, nu: f64, l: f64) -> f64 {
    match edge {
        // J_m I_m+1 + I_m J_m+1 = 0 (from W = W' = 0)
        PlateEdge::Clamped => bessel_j(m, l) * bessel_i(m + 1, l) + bessel_i(m, l) * bessel_j(m + 1, l),
        // Leissa's form, A / B = vJ / vI with A = -mJ the J part's moment
        // with its sign flipped
        PlateEdge::Free => {
            let (mj, mi) = edge_terms(edge, m, nu, l);
            let (vj, vi) = free_shear(m, nu, l);
            -mj * vi - vj * mi
        }
    }
}

/// C in W = J_m(l r) + C I_m(l r): the ratio that zeroes the edge term.
fn edge_ratio(edge: PlateEdge, m: i32, nu: f64, l: f64) -> f64 {
    let (tj, ti) = edge_terms(edge, m, nu, l);
    -tj / ti
}

/// Roots of `f` in (lo, hi): a scan, then bisection of each sign change.
fn roots(f: impl Fn(f64) -> f64, lo: f64, hi: f64, step: f64) -> Vec<f64> {
    let mut found = Vec::new();
    let (mut a, mut fa) = (lo, f(lo));
    while a < hi {
        let b = (a + step).min(hi);
        let fb = f(b);
        if fa * fb < 0.0 {
            let (mut x0, mut x1, mut f0) = (a, b, fa);
            for _ in 0..60 {
                let mid = 0.5 * (x0 + x1);
                let fm = f(mid);
                if f0 * fm <= 0.0 {
                    x1 = mid;
                } else {
                    (x0, f0) = (mid, fm);
                }
            }
            found.push(0.5 * (x0 + x1));
        }
        (a, fa) = (b, fb);
    }
    found
}

/// The `count` lowest modes of a circular plate (a free plate's rigid
/// motions, at lambda 0, left out), sorted by lambda. Orders m are added
/// until one's first root lies above every mode kept, and the search widens
/// until `count` modes are found (lambda up to 120).
pub fn circ_plate_modes(edge: PlateEdge, nu: f64, count: usize, samples: usize) -> Vec<PlateMode> {
    let mut hi = 12.0;
    loop {
        let mut found: Vec<(f64, i32)> = Vec::new();
        for m in 0.. {
            let ls = roots(|l| edge_equation(edge, m, nu, l), 0.5, hi, 0.05);
            if ls.is_empty() {
                break;
            }
            found.extend(ls.into_iter().map(|l| (l, m)));
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        if found.len() >= count || hi >= 120.0 {
            found.truncate(count);
            return found
                .into_iter()
                .map(|(lambda, m)| PlateMode { lambda, m, radial: radial_table(edge, nu, m, lambda, samples) })
                .collect();
        }
        hi *= 1.5;
    }
}

/// R(r) at `samples` radii from 0 to 1, scaled to mean square 1/2 over the
/// disc: the disc mean of R^2 cos^2(m theta) is 2 integral R^2 r dr, times
/// 1/2 for m > 0. Simpson's rule on 400 intervals.
fn radial_table(edge: PlateEdge, nu: f64, m: i32, l: f64, samples: usize) -> Vec<f64> {
    let c = edge_ratio(edge, m, nu, l);
    let shape = |r: f64| bessel_j(m, l * r) + c * bessel_i(m, l * r);
    let n = 400;
    let integral: f64 = (0..=n)
        .map(|k| {
            let r = k as f64 / n as f64;
            let weight = if k == 0 || k == n { 1.0 } else if k % 2 == 1 { 4.0 } else { 2.0 };
            weight * shape(r).powi(2) * r
        })
        .sum::<f64>()
        / (3.0 * n as f64);
    let mean_square = 2.0 * integral * if m == 0 { 1.0 } else { 0.5 };
    let scale = (0.5 / mean_square).sqrt();
    (0..samples).map(|k| scale * shape(k as f64 / (samples - 1).max(1) as f64)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lambda_squared(edge: PlateEdge, nu: f64, count: usize) -> Vec<(i32, f64)> {
        circ_plate_modes(edge, nu, count, 2).iter().map(|mode| (mode.m, mode.lambda * mode.lambda)).collect()
    }

    #[test]
    fn matches_leissa() {
        let clamped = lambda_squared(PlateEdge::Clamped, 0.3, 6);
        for ((m, l2), (want_m, want)) in clamped.iter().zip([(0, 10.2158), (1, 21.260), (2, 34.877), (0, 39.771), (3, 51.030), (1, 60.829)]) {
            assert_eq!(*m, want_m);
            assert!((l2 - want).abs() < 2e-3, "clamped {m}: {l2}");
        }
        let free = lambda_squared(PlateEdge::Free, 0.33, 4);
        for ((m, l2), (want_m, want)) in free.iter().zip([(2, 5.262), (0, 9.068), (3, 12.244), (1, 20.513)]) {
            assert_eq!(*m, want_m);
            assert!((l2 - want).abs() < 2e-3, "free {m}: {l2}");
        }
    }

    /// A free edge has no bending moment: W'' + nu (W' - m^2 W) = 0 at r = 1.
    #[test]
    fn free_modes_have_no_edge_moment() {
        let nu = 0.3;
        for mode in circ_plate_modes(PlateEdge::Free, nu, 12, 2) {
            let (m, l) = (mode.m, mode.lambda);
            let c = edge_ratio(PlateEdge::Free, m, nu, l);
            let w = |r: f64| bessel_j(m, l * r) + c * bessel_i(m, l * r);
            let h = 1e-4;
            let (w0, w1, w2) = (w(1.0 - 2.0 * h), w(1.0 - h), w(1.0));
            let d1 = (3.0 * w2 - 4.0 * w1 + w0) / (2.0 * h);
            let d2 = (w2 - 2.0 * w1 + w0) / (h * h);
            let moment = d2 + nu * (d1 - (m * m) as f64 * w2);
            let scale = l * l * w2.abs().max(1.0);
            assert!(moment.abs() < 1e-3 * scale, "m {m} lambda {l}: moment {moment}");
        }
    }

    /// High orders are not left out: the 120 lowest free modes include
    /// orders past 15, sorted.
    #[test]
    fn keeps_every_order_below_the_last_mode() {
        let modes = circ_plate_modes(PlateEdge::Free, 0.33, 120, 2);
        assert_eq!(modes.len(), 120);
        assert!(modes.windows(2).all(|pair| pair[0].lambda <= pair[1].lambda));
        assert!(modes.iter().any(|mode| mode.m >= 16), "{:?}", modes.iter().map(|m| m.m).max());
    }

    #[test]
    fn radial_tables_have_unit_modal_mass() {
        for mode in circ_plate_modes(PlateEdge::Clamped, 0.3, 4, 401) {
            let integral: f64 = mode.radial.iter().enumerate()
                .map(|(k, r)| r * r * (k as f64 / 400.0))
                .sum::<f64>() / 400.0;
            let mean_square = 2.0 * integral * if mode.m == 0 { 1.0 } else { 0.5 };
            assert!((mean_square - 0.5).abs() < 0.01, "{mean_square}");
            assert!(mode.radial.last().unwrap().abs() < 1e-9, "clamped edge must not move");
        }
    }
}
