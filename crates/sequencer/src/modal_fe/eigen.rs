//! Eigen solvers: a dense symmetric one (Householder tridiagonalization then
//! implicit QL, after the EISPACK tred2 / tql2 pair) and shift-invert block
//! Lanczos for the generalized problem K φ = λ M φ with diagonal M.
//!
//! The Lanczos run is Ruhe's band variant: p start vectors, each processed
//! vector spawning one new basis vector, the projected matrix banded with
//! bandwidth p. Blocks find repeated eigenvalues (the sin/cos pairs of
//! anything axisymmetric, the triples of a cube) that a single-vector run
//! only finds by rounding luck. Every new vector is fully orthogonalized
//! (classical Gram–Schmidt, repeated when the DGKS test asks for it)
//! against the whole basis and the deflation vectors, all in the M-inner
//! product.

use super::skyline::dot;

/// Eigenvalues ascending and eigenvectors (column k of the row-major
/// `vectors` belongs to `values[k]`) of the symmetric n×n row-major `a`.
pub(crate) fn symmetric_eigen(a: &[f64], n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut v = a.to_vec();
    let mut d = vec![0.0; n];
    let mut e = vec![0.0; n];
    if n == 0 {
        return (d, v);
    }
    tred2(&mut v, &mut d, &mut e, n);
    tql2(&mut v, &mut d, &mut e, n);
    (d, v)
}

fn tred2(v: &mut [f64], d: &mut [f64], e: &mut [f64], n: usize) {
    let at = |r: usize, c: usize| r * n + c;
    for j in 0..n {
        d[j] = v[at(n - 1, j)];
    }
    for i in (1..n).rev() {
        let mut scale = 0.0;
        let mut h = 0.0;
        for k in 0..i {
            scale += d[k].abs();
        }
        if scale == 0.0 {
            e[i] = d[i - 1];
            for j in 0..i {
                d[j] = v[at(i - 1, j)];
                v[at(i, j)] = 0.0;
                v[at(j, i)] = 0.0;
            }
        } else {
            for k in 0..i {
                d[k] /= scale;
                h += d[k] * d[k];
            }
            let mut f = d[i - 1];
            let mut g = h.sqrt();
            if f > 0.0 {
                g = -g;
            }
            e[i] = scale * g;
            h -= f * g;
            d[i - 1] = f - g;
            for j in 0..i {
                e[j] = 0.0;
            }
            for j in 0..i {
                f = d[j];
                v[at(j, i)] = f;
                g = e[j] + v[at(j, j)] * f;
                for k in j + 1..i {
                    g += v[at(k, j)] * d[k];
                    e[k] += v[at(k, j)] * f;
                }
                e[j] = g;
            }
            f = 0.0;
            for j in 0..i {
                e[j] /= h;
                f += e[j] * d[j];
            }
            let hh = f / (h + h);
            for j in 0..i {
                e[j] -= hh * d[j];
            }
            for j in 0..i {
                f = d[j];
                g = e[j];
                for k in j..i {
                    v[at(k, j)] -= f * e[k] + g * d[k];
                }
                d[j] = v[at(i - 1, j)];
                v[at(i, j)] = 0.0;
            }
        }
        d[i] = h;
    }
    // Accumulate the transformations.
    for i in 0..n - 1 {
        v[at(n - 1, i)] = v[at(i, i)];
        v[at(i, i)] = 1.0;
        let h = d[i + 1];
        if h != 0.0 {
            for k in 0..=i {
                d[k] = v[at(k, i + 1)] / h;
            }
            for j in 0..=i {
                let mut g = 0.0;
                for k in 0..=i {
                    g += v[at(k, i + 1)] * v[at(k, j)];
                }
                for k in 0..=i {
                    v[at(k, j)] -= g * d[k];
                }
            }
        }
        for k in 0..=i {
            v[at(k, i + 1)] = 0.0;
        }
    }
    for j in 0..n {
        d[j] = v[at(n - 1, j)];
        v[at(n - 1, j)] = 0.0;
    }
    v[at(n - 1, n - 1)] = 1.0;
    e[0] = 0.0;
}

fn tql2(v: &mut [f64], d: &mut [f64], e: &mut [f64], n: usize) {
    for i in 1..n {
        e[i - 1] = e[i];
    }
    e[n - 1] = 0.0;
    let mut f = 0.0;
    let mut tst1: f64 = 0.0;
    let eps = f64::EPSILON;
    for l in 0..n {
        tst1 = tst1.max(d[l].abs() + e[l].abs());
        let mut m = l;
        while m < n {
            if e[m].abs() <= eps * tst1 {
                break;
            }
            m += 1;
        }
        if m > l {
            loop {
                let mut g = d[l];
                let mut p = (d[l + 1] - g) / (2.0 * e[l]);
                let mut r = p.hypot(1.0);
                if p < 0.0 {
                    r = -r;
                }
                d[l] = e[l] / (p + r);
                d[l + 1] = e[l] * (p + r);
                let dl1 = d[l + 1];
                let mut h = g - d[l];
                for di in d.iter_mut().take(n).skip(l + 2) {
                    *di -= h;
                }
                f += h;
                p = d[m];
                let mut c = 1.0;
                let mut c2 = c;
                let mut c3 = c;
                let el1 = e[l + 1];
                let mut s = 0.0;
                let mut s2 = 0.0;
                for i in (l..m).rev() {
                    c3 = c2;
                    c2 = c;
                    s2 = s;
                    g = c * e[i];
                    h = c * p;
                    r = p.hypot(e[i]);
                    e[i + 1] = s * r;
                    s = e[i] / r;
                    c = p / r;
                    p = c * d[i] - s * g;
                    d[i + 1] = h + s * (c * g + s * d[i]);
                    for k in 0..n {
                        let h = v[k * n + i + 1];
                        v[k * n + i + 1] = s * v[k * n + i] + c * h;
                        v[k * n + i] = c * v[k * n + i] - s * h;
                    }
                }
                p = -s * s2 * c3 * el1 * e[l] / dl1;
                e[l] = s * p;
                d[l] = c * p;
                if e[l].abs() <= eps * tst1 {
                    break;
                }
            }
        }
        d[l] += f;
        e[l] = 0.0;
    }
    // Sort ascending, carrying the vectors.
    for i in 0..n.saturating_sub(1) {
        let mut k = i;
        for j in i + 1..n {
            if d[j] < d[k] {
                k = j;
            }
        }
        if k != i {
            d.swap(i, k);
            for r in 0..n {
                v.swap(r * n + i, r * n + k);
            }
        }
    }
}

pub(crate) struct LanczosResult {
    /// θ = 1 / (λ − σ), largest first.
    pub thetas: Vec<f64>,
    /// M-orthonormal Ritz vectors, matching `thetas`.
    pub vectors: Vec<Vec<f64>>,
    /// How many of the wanted pairs met the residual tolerance.
    pub converged: usize,
    /// Operator applications (solves).
    pub steps: usize,
}

/// Block size. 4 covers the multiplicities voxel and axisymmetric meshes
/// produce (pairs; triples for cubic symmetry).
const BLOCK: usize = 4;
/// Ritz pairs count as converged when ‖A x − θ x‖_M ≤ TOL · θ.
const TOL: f64 = 1e-7;

/// The `wanted` largest eigenvalues θ of A = (K − σM)⁻¹ M, i.e. the λ
/// nearest σ. `solve` overwrites b with (K − σM)⁻¹ b; `deflate` are
/// M-orthonormal vectors (rigid-body modes) kept out of the basis.
/// `progress` hears an estimate of how far through it is after each step and
/// stops it early (with what it has) when it answers false.
pub(crate) fn shift_invert_lanczos(
    mass: &[f64],
    solve: &dyn Fn(&mut [f64]),
    deflate: &[Vec<f64>],
    wanted: usize,
    progress: &dyn Fn(f64) -> bool,
) -> LanczosResult {
    let n = mass.len();
    let room = n.saturating_sub(deflate.len());
    let wanted = wanted.min(room);
    let p = BLOCK.min(room).max(1);
    let max_basis = room.min(4 * wanted + 25 * p + 100);
    let mut rng = 0x9e37_79b9_7f4a_7c15u64;
    let mut basis: Vec<Vec<f64>> = Vec::with_capacity(max_basis);
    // t[j] holds column j of the projected matrix: coefficients of A v_j.
    let mut t: Vec<Vec<f64>> = Vec::new();
    let m_dot = |a: &[f64], b: &[f64]| m_dot(mass, a, b);
    // Classical Gram–Schmidt against deflation + basis, repeated once when
    // the first pass cancelled most of the vector (the DGKS test); returns
    // the basis coefficients and the remaining M-norm.
    let orthogonalize = |w: &mut Vec<f64>, basis: &[Vec<f64>], coefficients: &mut Vec<f64>| -> f64 {
        let mut mw = vec![0.0; w.len()];
        coefficients.clear();
        coefficients.resize(basis.len(), 0.0);
        let mut norm = m_dot(w, w).sqrt();
        for _ in 0..2 {
            for i in 0..w.len() {
                mw[i] = mass[i] * w[i];
            }
            for d in deflate {
                axpy(-dot(d, &mw), d, w);
            }
            for (v, acc) in basis.iter().zip(coefficients.iter_mut()) {
                let c = dot(v, &mw);
                axpy(-c, v, w);
                *acc += c;
            }
            let after = m_dot(w, w).sqrt();
            let enough = after > 0.7 * norm;
            norm = after;
            if enough {
                break;
            }
        }
        norm
    };
    let mut random_unit = |basis: &[Vec<f64>], scratch: &mut Vec<f64>| -> Option<Vec<f64>> {
        for _ in 0..4 {
            let mut w: Vec<f64> = (0..n)
                .map(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    (rng >> 11) as f64 / (1u64 << 53) as f64 - 0.5
                })
                .collect();
            let before = m_dot(&w, &w).sqrt();
            let norm = orthogonalize(&mut w, basis, scratch);
            if norm > 1e-6 * before {
                w.iter_mut().for_each(|x| *x /= norm);
                return Some(w);
            }
        }
        None
    };
    let mut scratch = Vec::new();
    for _ in 0..p {
        match random_unit(&basis, &mut scratch) {
            Some(v) => basis.push(v),
            None => break,
        }
    }

    let mut processed = 0;
    let mut next_check = wanted + p;
    let mut result = None;
    while processed < basis.len() {
        // A full basis has no row left for the next column's remainder, so
        // its residual would go uncounted and overstate `converged`. Stop
        // here instead; when the basis spans the whole space the remainder
        // is roundoff and the run may finish.
        if basis.len() >= max_basis && max_basis < room {
            break;
        }
        let j = processed;
        let mut w: Vec<f64> = (0..n).map(|i| mass[i] * basis[j][i]).collect();
        solve(&mut w);
        let before = m_dot(&w, &w).sqrt();
        let mut column = Vec::new();
        let norm = orthogonalize(&mut w, &basis, &mut column);
        if basis.len() < max_basis {
            if norm > 1e-10 * before {
                w.iter_mut().for_each(|x| *x /= norm);
                column.push(norm);
                basis.push(w);
            } else if let Some(v) = random_unit(&basis, &mut scratch) {
                // Invariant subspace found: restart the block direction.
                column.push(0.0);
                basis.push(v);
            }
        }
        t.push(column);
        processed += 1;
        // a run usually needs about three steps per wanted mode
        if !progress((processed as f64 / (3 * wanted + 2 * p) as f64).min(0.99)) {
            break;
        }

        let exhausted = processed == basis.len();
        if processed >= next_check || exhausted {
            next_check = processed + p.max(processed / 8);
            let ritz = ritz(&t, processed, basis.len(), wanted);
            let done = ritz.converged >= wanted || exhausted || processed + p > max_basis;
            if done {
                result = Some(ritz);
                break;
            }
        }
    }
    let ritz = result.unwrap_or_else(|| ritz(&t, processed, basis.len(), wanted));
    let vectors = ritz
        .vectors
        .iter()
        .map(|y| {
            let mut x = vec![0.0; n];
            for (v, &c) in basis.iter().zip(y) {
                axpy(c, v, &mut x);
            }
            let norm = m_dot(&x, &x).sqrt();
            x.iter_mut().for_each(|v| *v /= norm);
            x
        })
        .collect();
    LanczosResult { thetas: ritz.thetas, vectors, converged: ritz.converged, steps: processed }
}

struct Ritz {
    thetas: Vec<f64>,
    vectors: Vec<Vec<f64>>,
    converged: usize,
}

/// Rayleigh–Ritz on the first m processed vectors. Column j of `t` holds
/// v_iᵀ M A v_j for i < len(column); rows m.. are the residual coupling.
fn ritz(t: &[Vec<f64>], m: usize, basis_len: usize, wanted: usize) -> Ritz {
    let entry = |i: usize, j: usize| t[j].get(i).copied().unwrap_or(0.0);
    let mut a = vec![0.0; m * m];
    for i in 0..m {
        for j in 0..m {
            a[i * m + j] = 0.5 * (entry(i, j) + entry(j, i));
        }
    }
    let (values, vectors) = symmetric_eigen(&a, m);
    let take = wanted.min(m);
    let mut thetas = Vec::with_capacity(take);
    let mut ys = Vec::with_capacity(take);
    let mut converged = 0;
    let mut counting = true;
    for k in (m - take..m).rev() {
        let y: Vec<f64> = (0..m).map(|r| vectors[r * m + k]).collect();
        let residual: f64 = (m..basis_len)
            .map(|i| {
                let s: f64 = (0..m).map(|j| entry(i, j) * y[j]).sum();
                s * s
            })
            .sum::<f64>()
            .sqrt();
        if counting && residual <= TOL * values[k].abs() {
            converged += 1;
        } else {
            counting = false;
        }
        thetas.push(values[k]);
        ys.push(y);
    }
    Ritz { thetas, vectors: ys, converged }
}

/// The M-inner product aᵀ M b for diagonal M.
pub(crate) fn m_dot(mass: &[f64], a: &[f64], b: &[f64]) -> f64 {
    let mut s = 0.0;
    for i in 0..a.len() {
        s += mass[i] * a[i] * b[i];
    }
    s
}

pub(crate) fn axpy(a: f64, x: &[f64], y: &mut [f64]) {
    for (yi, xi) in y.iter_mut().zip(x) {
        *yi += a * xi;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_eigen_diagonalizes_a_symmetric_matrix() {
        let n = 7;
        let mut a = vec![0.0; n * n];
        for i in 0..n {
            for j in 0..=i {
                let v = ((i * 7 + j * 3) as f64).sin() + if i == j { 2.0 } else { 0.0 };
                a[i * n + j] = v;
                a[j * n + i] = v;
            }
        }
        let (values, vectors) = symmetric_eigen(&a, n);
        for w in values.windows(2) {
            assert!(w[0] <= w[1]);
        }
        for k in 0..n {
            for r in 0..n {
                let av: f64 = (0..n).map(|c| a[r * n + c] * vectors[c * n + k]).sum();
                assert!((av - values[k] * vectors[r * n + k]).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn lanczos_finds_the_lowest_eigenvalues_with_multiplicity() {
        // Diagonal K with a repeated low eigenvalue, mass not identity.
        let n = 60;
        let mass: Vec<f64> = (0..n).map(|i| 1.0 + (i % 3) as f64).collect();
        let lambda: Vec<f64> = (0..n).map(|i| if i < 3 { 1.0 } else { 2.0 + i as f64 }).collect();
        let k: Vec<f64> = (0..n).map(|i| lambda[i] * mass[i]).collect();
        let solve = |b: &mut [f64]| {
            for i in 0..b.len() {
                b[i] /= k[i];
            }
        };
        let result = shift_invert_lanczos(&mass, &solve, &[], 5, &|_| true);
        let mut found: Vec<f64> = result.thetas.iter().map(|t| 1.0 / t).collect();
        found.sort_by(|a, b| a.total_cmp(b));
        let mut expected = lambda.clone();
        expected.sort_by(|a, b| a.total_cmp(b));
        for (f, e) in found.iter().zip(&expected) {
            assert!((f - e).abs() < 1e-9 * e, "{found:?} vs {expected:?}");
        }
        assert_eq!(result.converged, 5);
    }

    #[test]
    fn a_run_that_fills_its_basis_counts_only_true_convergence() {
        // A tightly clustered spectrum the basis budget cannot resolve: every
        // pair counted as converged must really meet the residual tolerance.
        let n = 600;
        let mass = vec![1.0; n];
        let lambda: Vec<f64> = (0..n).map(|i| 1.0 + 1e-6 * i as f64).collect();
        let solve = |b: &mut [f64]| {
            for i in 0..b.len() {
                b[i] /= lambda[i];
            }
        };
        let wanted = 20;
        let result = shift_invert_lanczos(&mass, &solve, &[], wanted, &|_| true);
        assert!(result.converged < wanted, "the cluster should not resolve: {}", result.converged);
        for (theta, x) in result.thetas.iter().zip(&result.vectors).take(result.converged) {
            let residual: f64 = (0..n).map(|i| (x[i] / lambda[i] - theta * x[i]).powi(2)).sum::<f64>().sqrt();
            assert!(residual <= 10.0 * TOL * theta.abs(), "residual {residual} for theta {theta}");
        }
    }
}
