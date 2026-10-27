//! The hex8 element: linear elasticity (isotropic, or orthotropic along a
//! wood's grain), 2×2×2 Gauss, with Wilson's
//! nine incompatible modes in Taylor's patch-test-passing form (QM6 / the
//! element Abaqus calls C3D8I), statically condensed.
//!
//! Plain trilinear hexes lock in bending: a thin bar or a bell wall one
//! element thick comes out several times too stiff. The incompatible modes
//! (bubble displacements 1 − ξ², 1 − η², 1 − ζ² per component) add the
//! missing quadratic bending field; with their derivatives taken from the
//! Jacobian at the element center (scaled by det J₀ / det J) the element
//! still reproduces constant strain exactly, so it converges. Rectangular
//! elements become exact in pure bending, which is what a voxel mesh and a
//! one-layer wall need. Cost: a 9×9 solve per element.
//!
//! Mass is lumped by row sum (ρ ∫ Nₐ dV per node). A diagonal M keeps every
//! M-inner product and the shift (K − σM) trivial, and for these meshes its
//! error (frequencies slightly low) partly cancels the residual stiffness of
//! the element (slightly high).

const CORNERS: [[f64; 3]; 8] = [
    [-1.0, -1.0, -1.0],
    [1.0, -1.0, -1.0],
    [1.0, 1.0, -1.0],
    [-1.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [1.0, -1.0, 1.0],
    [1.0, 1.0, 1.0],
    [-1.0, 1.0, 1.0],
];

/// Isotropic elasticity matrix in Voigt order (xx, yy, zz, xy, yz, zx) with
/// engineering shear strains.
pub(crate) fn elasticity(young: f64, poisson: f64) -> [[f64; 6]; 6] {
    let lambda = young * poisson / ((1.0 + poisson) * (1.0 - 2.0 * poisson));
    let mu = young / (2.0 * (1.0 + poisson));
    let mut d = [[0.0; 6]; 6];
    for i in 0..3 {
        for j in 0..3 {
            d[i][j] = lambda;
        }
        d[i][i] = lambda + 2.0 * mu;
        d[i + 3][i + 3] = mu;
    }
    d
}

fn shape_derivatives(xi: [f64; 3]) -> [[f64; 3]; 8] {
    let mut dn = [[0.0; 3]; 8];
    for (a, c) in CORNERS.iter().enumerate() {
        let f = [1.0 + c[0] * xi[0], 1.0 + c[1] * xi[1], 1.0 + c[2] * xi[2]];
        dn[a] = [c[0] * f[1] * f[2] / 8.0, f[0] * c[1] * f[2] / 8.0, f[0] * f[1] * c[2] / 8.0];
    }
    dn
}

/// J[i][j] = ∂xᵢ/∂ξⱼ.
fn jacobian(x: &[[f64; 3]; 8], dn: &[[f64; 3]; 8]) -> [[f64; 3]; 3] {
    let mut j = [[0.0; 3]; 3];
    for a in 0..8 {
        for r in 0..3 {
            for c in 0..3 {
                j[r][c] += x[a][r] * dn[a][c];
            }
        }
    }
    j
}

fn det3(m: &[[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

fn inverse3(m: &[[f64; 3]; 3], det: f64) -> [[f64; 3]; 3] {
    let mut inv = [[0.0; 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            // Cofactor of (c, r), transposed into (r, c).
            let (r1, r2) = ((c + 1) % 3, (c + 2) % 3);
            let (c1, c2) = ((r + 1) % 3, (r + 2) % 3);
            inv[r][c] = (m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1]) / det;
        }
    }
    inv
}

pub(crate) fn center_jacobian_det(x: &[[f64; 3]; 8]) -> f64 {
    det3(&jacobian(x, &shape_derivatives([0.0; 3])))
}

/// Physical gradients ∂/∂xᵢ = Σⱼ (J⁻¹)ⱼᵢ ∂/∂ξⱼ.
fn physical(inv: &[[f64; 3]; 3], d: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| inv[0][i] * d[0] + inv[1][i] * d[1] + inv[2][i] * d[2])
}

/// A material's elasticity matrix (Voigt order as [`elasticity`]):
/// isotropic, or orthotropic along its grain.
pub(crate) fn material_elasticity(m: &super::spec::Material) -> [[f64; 6]; 6] {
    match &m.grain {
        None => elasticity(m.young, m.poisson),
        Some(grain) => orthotropic(m.young, grain),
    }
}

/// The normal-strain compliance (3×3, global axes) of a grained material.
fn normal_compliance(young_l: f64, grain: &super::spec::Grain) -> [[f64; 3]; 3] {
    let e = [young_l, grain.young_r, grain.young_t];
    // nu[i][j]: contraction along j under stress along i (local L R T)
    let mut nu = [[0.0; 3]; 3];
    nu[0][1] = grain.poisson_lr;
    nu[0][2] = grain.poisson_lt;
    nu[1][2] = grain.poisson_rt;
    // reciprocity: nu_ji / E_j = nu_ij / E_i
    for (i, j) in [(0, 1), (0, 2), (1, 2)] {
        nu[j][i] = nu[i][j] * e[j] / e[i];
    }
    let axes = grain.axes();
    let mut s = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            s[axes[i]][axes[j]] = if i == j { 1.0 / e[i] } else { -nu[i][j] / e[i] };
        }
    }
    s
}

/// Orthotropic elasticity: the inverse of the compliance, its axes laid
/// along the grain's (a permutation of x y z, so shear stays diagonal).
fn orthotropic(young_l: f64, grain: &super::spec::Grain) -> [[f64; 6]; 6] {
    let s = normal_compliance(young_l, grain);
    let det = det3(&s);
    let c = inverse3(&s, det);
    let mut d = [[0.0; 6]; 6];
    for i in 0..3 {
        for j in 0..3 {
            d[i][j] = c[i][j];
        }
    }
    // Voigt shear slots: 3 xy, 4 yz, 5 zx
    let slot = |a: usize, b: usize| match (a.min(b), a.max(b)) {
        (0, 1) => 3,
        (1, 2) => 4,
        _ => 5,
    };
    let [l, r, t] = grain.axes();
    d[slot(l, r)][slot(l, r)] = grain.shear_lr;
    d[slot(l, t)][slot(l, t)] = grain.shear_lt;
    d[slot(r, t)][slot(r, t)] = grain.shear_rt;
    d
}

/// Whether a material's normal compliance is positive definite (its
/// leading minors are), as a stable elastic material's must be.
pub(crate) fn compliance_is_positive(m: &super::spec::Material) -> bool {
    let Some(grain) = &m.grain else { return true };
    let s = normal_compliance(m.young, grain);
    s[0][0] > 0.0 && s[0][0] * s[1][1] - s[0][1] * s[1][0] > 0.0 && det3(&s) > 0.0
}

/// Strain-displacement columns for one vector DOF set with gradient g.
fn b_columns(g: [f64; 3]) -> [[f64; 6]; 3] {
    [
        [g[0], 0.0, 0.0, g[1], 0.0, g[2]],
        [0.0, g[1], 0.0, g[0], g[2], 0.0],
        [0.0, 0.0, g[2], 0.0, g[1], g[0]],
    ]
}

/// Condensed 24×24 stiffness (row-major, DOF = 3·corner + component) and the
/// lumped nodal masses of one element.
pub(crate) fn hex8(
    x: &[[f64; 3]; 8],
    d: &[[f64; 6]; 6],
    density: f64,
) -> Result<([f64; 576], [f64; 8]), String> {
    let j0 = jacobian(x, &shape_derivatives([0.0; 3]));
    let det0 = det3(&j0);
    if !(det0 > 0.0) {
        return Err("inverted or degenerate element".into());
    }
    let inv0 = inverse3(&j0, det0);
    let g = 1.0 / 3f64.sqrt();
    let mut kcc = [0.0; 576];
    let mut kci = [0.0; 24 * 9];
    let mut kii = [0.0; 81];
    let mut mass = [0.0; 8];
    for gp in 0..8 {
        let xi = [CORNERS[gp][0] * g, CORNERS[gp][1] * g, CORNERS[gp][2] * g];
        let dn = shape_derivatives(xi);
        let jac = jacobian(x, &dn);
        let det = det3(&jac);
        if !(det > 0.0) {
            return Err("element Jacobian is not positive at a Gauss point".into());
        }
        let inv = inverse3(&jac, det);
        // Compatible B (24 columns of 6) and incompatible B at J₀ (9).
        let mut bc = [[0.0; 6]; 24];
        for a in 0..8 {
            let cols = b_columns(physical(&inv, dn[a]));
            bc[3 * a..3 * a + 3].copy_from_slice(&cols);
        }
        let mut bi = [[0.0; 6]; 9];
        for q in 0..3 {
            let mut dp = [0.0; 3];
            dp[q] = -2.0 * xi[q];
            let cols = b_columns(physical(&inv0, dp));
            bi[3 * q..3 * q + 3].copy_from_slice(&cols);
        }
        let dbc: Vec<[f64; 6]> = bc.iter().map(|col| mat6_vec(d, col)).collect();
        let dbi: Vec<[f64; 6]> = bi.iter().map(|col| mat6_vec(d, col)).collect();
        for r in 0..24 {
            for c in r..24 {
                let v = dot6(&bc[r], &dbc[c]) * det;
                kcc[r * 24 + c] += v;
            }
            for c in 0..9 {
                kci[r * 9 + c] += dot6(&bc[r], &dbi[c]) * det0;
            }
        }
        for r in 0..9 {
            for c in 0..9 {
                kii[r * 9 + c] += dot6(&bi[r], &dbi[c]) * det0 * det0 / det;
            }
        }
        for (a, c) in CORNERS.iter().enumerate() {
            let n = (1.0 + c[0] * xi[0]) * (1.0 + c[1] * xi[1]) * (1.0 + c[2] * xi[2]) / 8.0;
            mass[a] += density * n * det;
        }
    }
    for r in 0..24 {
        for c in 0..r {
            kcc[r * 24 + c] = kcc[c * 24 + r];
        }
    }
    // K = Kcc − Kci Kii⁻¹ Kciᵀ.
    let chol = cholesky9(&kii).ok_or("incompatible-mode block is not positive definite")?;
    for r in 0..24 {
        let mut row = [0.0; 9];
        row.copy_from_slice(&kci[r * 9..r * 9 + 9]);
        let solved = solve9(&chol, row);
        for c in r..24 {
            let v: f64 = (0..9).map(|q| solved[q] * kci[c * 9 + q]).sum();
            kcc[r * 24 + c] -= v;
            if c != r {
                kcc[c * 24 + r] -= v;
            }
        }
    }
    Ok((kcc, mass))
}

fn mat6_vec(d: &[[f64; 6]; 6], v: &[f64; 6]) -> [f64; 6] {
    [0, 1, 2, 3, 4, 5].map(|r| dot6(&d[r], v))
}

fn dot6(a: &[f64; 6], b: &[f64; 6]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3] + a[4] * b[4] + a[5] * b[5]
}

fn cholesky9(a: &[f64; 81]) -> Option<[f64; 81]> {
    let mut l = [0.0; 81];
    for i in 0..9 {
        for j in 0..=i {
            let mut s = a[i * 9 + j];
            for k in 0..j {
                s -= l[i * 9 + k] * l[j * 9 + k];
            }
            if i == j {
                if !(s > 0.0) {
                    return None;
                }
                l[i * 9 + i] = s.sqrt();
            } else {
                l[i * 9 + j] = s / l[j * 9 + j];
            }
        }
    }
    Some(l)
}

fn solve9(l: &[f64; 81], mut b: [f64; 9]) -> [f64; 9] {
    for i in 0..9 {
        for k in 0..i {
            b[i] -= l[i * 9 + k] * b[k];
        }
        b[i] /= l[i * 9 + i];
    }
    for i in (0..9).rev() {
        for k in i + 1..9 {
            b[i] -= l[k * 9 + i] * b[k];
        }
        b[i] /= l[i * 9 + i];
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modal_fe::spec::Axis;

    fn skewed_hex() -> [[f64; 3]; 8] {
        let mut x = CORNERS.map(|c| [0.5 * (c[0] + 1.0) * 0.3, 0.5 * (c[1] + 1.0) * 0.1, 0.5 * (c[2] + 1.0) * 0.05]);
        x[6][0] += 0.04;
        x[2][1] -= 0.01;
        x[5][2] += 0.012;
        x
    }

    fn apply(k: &[f64; 576], u: &[f64; 24]) -> [f64; 24] {
        let mut f = [0.0; 24];
        for r in 0..24 {
            f[r] = (0..24).map(|c| k[r * 24 + c] * u[c]).sum();
        }
        f
    }

    fn maple_grain(along: Axis, across: Axis) -> crate::modal_fe::spec::Material {
        crate::modal_fe::spec::Material {
            density: 700.0,
            young: 1.26e10,
            poisson: 0.35,
            grain: Some(crate::modal_fe::spec::Grain {
                along,
                across,
                young_r: 1.66e9,
                young_t: 0.82e9,
                shear_lr: 1.4e9,
                shear_lt: 0.79e9,
                shear_rt: 0.26e9,
                poisson_lr: 0.424,
                poisson_lt: 0.476,
                poisson_rt: 0.774,
            }),
        }
    }

    #[test]
    fn a_grained_material_is_stiff_along_its_grain_and_soft_across() {
        let m = maple_grain(Axis::X, Axis::Y);
        assert!(compliance_is_positive(&m));
        let d = material_elasticity(&m);
        // uniaxial stress along each axis: strain = S e_i, so E_i = 1 / S_ii
        let s = normal_compliance(m.young, m.grain.as_ref().unwrap());
        assert!((1.0 / s[0][0] - 1.26e10).abs() < 1.0);
        assert!((1.0 / s[1][1] - 1.66e9).abs() < 1.0);
        assert!((1.0 / s[2][2] - 0.82e9).abs() < 1.0);
        // D inverts S on the normal block, and is symmetric
        for i in 0..3 {
            for j in 0..3 {
                let product: f64 = (0..3).map(|k| d[i][k] * s[k][j]).sum();
                assert!((product - if i == j { 1.0 } else { 0.0 }).abs() < 1e-9, "{i}{j} {product}");
                assert!((d[i][j] - d[j][i]).abs() < 1e-3 * d[i][i]);
            }
        }
        assert_eq!((d[3][3], d[4][4], d[5][5]), (1.4e9, 0.26e9, 0.79e9));
        // along z, across x: L is z, R x, T y
        let d = material_elasticity(&maple_grain(Axis::Z, Axis::X));
        assert_eq!((d[3][3], d[4][4], d[5][5]), (0.26e9, 0.79e9, 1.4e9));
        let mut unstable = maple_grain(Axis::X, Axis::Y);
        unstable.grain.as_mut().unwrap().poisson_rt = 3.0;
        assert!(!compliance_is_positive(&unstable));
    }

    #[test]
    fn rigid_motions_cost_no_energy_on_a_distorted_hex() {
        let x = skewed_hex();
        let (k, mass) = hex8(&x, &elasticity(2e11, 0.3), 7800.0).unwrap();
        let scale = k.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let rotation = |p: [f64; 3]| [-p[1], p[0], 0.0];
        let mut u = [0.0; 24];
        for a in 0..8 {
            u[3 * a..3 * a + 3].copy_from_slice(&rotation(x[a]));
        }
        let f = apply(&k, &u);
        assert!(f.iter().all(|v| v.abs() < 1e-9 * scale), "{f:?}");
        let volume_mass: f64 = mass.iter().sum();
        assert!(volume_mass > 0.0);
    }

    #[test]
    fn constant_strain_passes_the_patch_test_on_a_distorted_hex() {
        // Under a linear displacement field the incompatible modes must not
        // respond (Taylor's J₀ form): the condensed element's forces equal
        // ∫ Bᵀ σ dV of the plain compatible element.
        let x = skewed_hex();
        let d = elasticity(1e9, 0.25);
        let (k, _) = hex8(&x, &d, 1000.0).unwrap();
        let grad = [[0.001, 0.0004, 0.0], [0.0, 0.0, 0.0002], [-0.0003, 0.0, 0.0]];
        let mut u = [0.0; 24];
        for a in 0..8 {
            for c in 0..3 {
                u[3 * a + c] = (0..3).map(|j| grad[c][j] * x[a][j]).sum();
            }
        }
        let strain = [
            grad[0][0],
            grad[1][1],
            grad[2][2],
            grad[0][1] + grad[1][0],
            grad[1][2] + grad[2][1],
            grad[2][0] + grad[0][2],
        ];
        let stress = mat6_vec(&d, &strain);
        let g = 1.0 / 3f64.sqrt();
        let mut expected = [0.0; 24];
        for gp in 0..8 {
            let xi = CORNERS[gp].map(|c| c * g);
            let dn = shape_derivatives(xi);
            let jac = jacobian(&x, &dn);
            let det = det3(&jac);
            let inv = inverse3(&jac, det);
            for a in 0..8 {
                let cols = b_columns(physical(&inv, dn[a]));
                for c in 0..3 {
                    expected[3 * a + c] += dot6(&cols[c], &stress) * det;
                }
            }
        }
        let f = apply(&k, &u);
        let scale = expected.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        for i in 0..24 {
            assert!((f[i] - expected[i]).abs() < 1e-9 * scale, "dof {i}: {} vs {}", f[i], expected[i]);
        }
    }
}
