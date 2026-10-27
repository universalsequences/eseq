use std::path::Path;

use super::*;

const STEEL: Material = Material { density: 7850.0, young: 2.0e11, poisson: 0.3, grain: None };

fn bar_spec(length: f64, side: f64, element: f64, block: Option<Region>, modes: usize) -> SolidSpec {
    SolidSpec {
        name: None,
        solid: Solid::Box { size: [length, side, side], center: [0.0; 3] },
        resolution: Resolution::ElementSize(element),
        material: STEEL,
        block,
        modes,
        mesh: MeshMode::Auto,
    }
}

/// sqrt(EI / (ρ A L⁴)) for a square section of side b.
fn beam_scale(length: f64, side: f64) -> f64 {
    side / (length * length) * (STEEL.young / (12.0 * STEEL.density)).sqrt()
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs()
}

#[test]
fn free_free_bar_bending_follows_euler_bernoulli() {
    // 0.6 × 0.02 × 0.02 m steel bar, 60 × 2 × 2 elements (L/b = 30). The
    // square section makes each bending mode a degenerate pair.
    let (length, side) = (0.6, 0.02);
    let d = solve_detailed(&bar_spec(length, side, 0.01, None, 6), Path::new("."), &SolveProgress::default()).unwrap();
    let f = &d.solid.freqs;
    assert_eq!(d.stats.rigid_modes_deflated, 6);
    for pair in [0, 2, 4] {
        assert!(rel(f[pair + 1], f[pair]) < 1e-6, "pair {pair}: {f:?}");
    }
    let beta = [4.730_040_7, 7.853_204_6, 10.995_607_8];
    let expected: Vec<f64> =
        beta.iter().map(|b| b * b / std::f64::consts::TAU * beam_scale(length, side)).collect();
    let ratios = [f[2] / f[0], f[4] / f[0]];
    let eb_ratios = [expected[1] / expected[0], expected[2] / expected[0]];
    eprintln!(
        "free-free bar: f = {:.2} {:.2} {:.2} Hz, Euler-Bernoulli {:.2} {:.2} {:.2}; ratios {:.4} {:.4} vs {:.4} {:.4}",
        f[0], f[2], f[4], expected[0], expected[1], expected[2], ratios[0], ratios[1], eb_ratios[0], eb_ratios[1]
    );
    // Shear deformation and rotary inertia (absent from Euler–Bernoulli)
    // lower the higher modes slightly at L/b = 30.
    assert!(rel(f[0], expected[0]) < 0.01, "f1 {} vs {}", f[0], expected[0]);
    assert!(rel(ratios[0], eb_ratios[0]) < 0.015, "{ratios:?} vs {eb_ratios:?}");
    assert!(rel(ratios[1], eb_ratios[1]) < 0.03, "{ratios:?} vs {eb_ratios:?}");
}

#[test]
fn cantilever_first_frequency_matches_beam_theory() {
    // 0.3 × 0.01 × 0.01 m, 60 × 2 × 2 elements, the x = −L/2 face clamped.
    let (length, side) = (0.3, 0.01);
    let block = Region::Box { min: [-1.0, -1.0, -1.0], max: [-0.5 * length, 1.0, 1.0] };
    let d = solve_detailed(&bar_spec(length, side, 0.005, Some(block), 4), Path::new("."), &SolveProgress::default()).unwrap();
    let f = &d.solid.freqs;
    assert_eq!(d.stats.blocked_nodes, 9);
    assert_eq!(d.stats.rigid_modes_deflated, 0);
    let expected = 3.516_015 / std::f64::consts::TAU * beam_scale(length, side);
    eprintln!("cantilever: f1 = {:.3} Hz vs {:.3} Hz ({:+.3}%)", f[0], expected, 100.0 * (f[0] / expected - 1.0));
    assert!(rel(f[0], expected) < 0.01, "{} vs {}", f[0], expected);
    assert!(rel(f[1], f[0]) < 1e-6);
}

#[test]
fn free_cube_drops_rigid_modes_and_modes_are_mass_normalized() {
    let spec = SolidSpec {
        name: None,
        solid: Solid::Box { size: [0.1; 3], center: [0.0; 3] },
        resolution: Resolution::Cells(5),
        material: STEEL,
        block: None,
        modes: 10,
        mesh: MeshMode::Auto,
    };
    let d = solve_detailed(&spec, Path::new("."), &SolveProgress::default()).unwrap();
    assert_eq!(d.stats.rigid_modes_deflated, 6);
    assert_eq!(d.solid.freqs.len(), 10);
    // A 10 cm steel cube's lowest elastic mode is near 20 kHz; anything
    // near zero would be a rigid motion leaking through.
    assert!(d.solid.freqs[0] > 10_000.0, "{:?}", d.solid.freqs);
    for w in d.solid.freqs.windows(2) {
        assert!(w[0] <= w[1]);
    }
    let m_dot = |a: &[f64], b: &[f64]| eigen::m_dot(&d.dof_mass, a, b);
    for k in 0..d.vectors.len() {
        for l in 0..d.vectors.len() {
            let g = m_dot(&d.vectors[k], &d.vectors[l]);
            let target = if k == l { 1.0 } else { 0.0 };
            assert!((g - target).abs() < 1e-8, "φ{k}ᵀMφ{l} = {g}");
        }
        // K φ = λ M φ.
        let kphi = d.stiffness.mul(&d.vectors[k]);
        let lambda = d.lambdas[k];
        let mut residual = 0.0;
        let mut scale = 0.0;
        for i in 0..kphi.len() {
            let mphi = lambda * d.dof_mass[i] * d.vectors[k][i];
            residual += (kphi[i] - mphi).powi(2);
            scale += mphi * mphi;
        }
        assert!((residual / scale).sqrt() < 1e-5, "mode {k} residual {}", (residual / scale).sqrt());
    }
    let mass = 7850.0 * 1e-3;
    assert!(rel(d.solid.mass, mass) < 1e-12);
}

#[test]
fn obj_cube_voxelizes_to_the_box_primitive() {
    let dir = tempfile::tempdir().unwrap();
    let mut text = String::new();
    for i in 0..8 {
        let c = |bit: usize| if i >> bit & 1 == 1 { 0.05 } else { -0.05 };
        text.push_str(&format!("v {} {} {}\n", c(0), c(1), c(2)));
    }
    // Two triangles per face, diagonals included, outward winding.
    for f in [
        [1, 3, 4, 2],
        [5, 6, 8, 7],
        [1, 2, 6, 5],
        [3, 7, 8, 4],
        [1, 5, 7, 3],
        [2, 4, 8, 6],
    ] {
        text.push_str(&format!("f {} {} {}\nf {} {} {}\n", f[0], f[1], f[2], f[0], f[2], f[3]));
    }
    std::fs::write(dir.path().join("cube.obj"), text).unwrap();
    let spec = |solid| SolidSpec {
        name: None,
        solid,
        resolution: Resolution::Cells(6),
        material: STEEL,
        block: None,
        modes: 4,
        mesh: MeshMode::Auto,
    };
    let from_obj = mesh::build_mesh(
        &spec(Solid::Obj { path: "cube.obj".into(), scale: 1.0, offset: [0.0; 3] }),
        dir.path(),
    )
    .unwrap();
    let from_box =
        mesh::build_mesh(&spec(Solid::Box { size: [0.1; 3], center: [0.0; 3] }), dir.path()).unwrap();
    assert_eq!(from_obj.elements.len(), 216);
    assert_eq!(from_obj.nodes, from_box.nodes);
    assert_eq!(from_obj.elements, from_box.elements);
}

#[test]
fn mapped_tube_has_degenerate_ovaling_pairs() {
    // A thin ring wall one element thick, revolved with the seam merged:
    // the n = 2 ovaling modes must come out as an exact sin / cos pair.
    let spec = SolidSpec {
        name: None,
        solid: Solid::Revolve {
            profile: vec![vec![0.0475, -0.02], vec![0.0475, 0.02]],
            thickness: Some(0.005),
            center: [0.0; 3],
            axis: Axis::Z,
            segments: Segments { along: Some(4), around: Some(32), through: Some(1) },
        },
        resolution: Resolution::Cells(16),
        material: STEEL,
        block: None,
        modes: 4,
        mesh: MeshMode::Auto,
    };
    let (solid, stats) = solve(&spec, Path::new(".")).unwrap();
    assert_eq!(stats.nodes, 5 * 2 * 32);
    assert_eq!(stats.rigid_modes_deflated, 6);
    let f = &solid.freqs;
    assert!(rel(f[1], f[0]) < 1e-6, "{f:?}");
    // Thin ring in-plane n = 2: f = (1/2π) · n(n²−1)/√(n²+1) · √(EI/(ρA R⁴)).
    let (r, t) = (0.0475f64, 0.005f64);
    let ring = 6.0 / 5f64.sqrt() / std::f64::consts::TAU * (t / (r * r)) * (STEEL.young / (12.0 * STEEL.density)).sqrt();
    eprintln!("tube ovaling: {:.1} Hz vs thin ring {:.1} Hz", f[0], ring);
    assert!(rel(f[0], ring) < 0.08, "{} vs {}", f[0], ring);
}

#[test]
fn shell_poles_collapse_into_wedges() {
    let spec = SolidSpec {
        name: None,
        solid: Solid::Shell { outer_radius: 0.1, inner_radius: 0.095, center: [0.0; 3] },
        resolution: Resolution::Cells(8),
        material: STEEL,
        block: None,
        modes: 6,
        mesh: MeshMode::Auto,
    };
    let (solid, stats) = solve(&spec, Path::new(".")).unwrap();
    assert_eq!(stats.rigid_modes_deflated, 6);
    assert!(solid.freqs[0] > 100.0, "{:?}", solid.freqs);
    // Surface output is consistent: unit normals, indices in range.
    let p = solid.point_count();
    assert!(solid.triangles.iter().all(|&i| (i as usize) < p));
    for n in solid.normals.chunks(3) {
        let len = mesh::dot([0, 1, 2].map(|i| n[i] as f64), [0, 1, 2].map(|i| n[i] as f64)).sqrt();
        assert!((len - 1.0).abs() < 1e-5);
    }
    assert_eq!(solid.shapes.len(), p * solid.mode_count());
}

#[test]
fn modes_json_round_trips_and_hash_is_stable() {
    let spec = SolidSpec::from_json(
        r#"{"solid": {"box": {"size": [0.1, 0.02, 0.02]}}, "resolution": {"cells": 10},
            "material": {"density": 7850, "young": 2e11, "poisson": 0.3}, "modes": 3}"#,
    )
    .unwrap();
    let (solid, _) = solve(&spec, Path::new(".")).unwrap();
    let text = solid.to_json(&spec);
    assert!(text.starts_with(r#"{"format":"eseq-modes","version":1,"freqs":["#));
    let (back, echoed) = ModalSolid::from_json(&text).unwrap();
    assert_eq!(back.triangles, solid.triangles);
    assert_eq!(back.shapes, solid.shapes);
    assert_eq!(serde_json::from_value::<SolidSpec>(echoed).unwrap(), spec);
    let hash = spec_hash(&spec);
    assert_eq!(hash.len(), 64);
    let reparsed = SolidSpec::from_json(&serde_json::to_string(&spec).unwrap()).unwrap();
    assert_eq!(spec_hash(&reparsed), hash);
}

/// Oversized requests come back as errors before anything is allocated:
/// segments or a cell size that would overflow the element count, and more
/// modes than a solve takes.
#[test]
fn oversized_meshes_and_mode_counts_are_refused() {
    let revolve = |segments: Segments, resolution: Resolution| SolidSpec {
        name: None,
        solid: Solid::Revolve {
            profile: vec![vec![0.0475, -0.02], vec![0.0475, 0.02]],
            thickness: Some(0.005),
            center: [0.0; 3],
            axis: Axis::Z,
            segments,
        },
        resolution,
        material: STEEL,
        block: None,
        modes: 4,
        mesh: MeshMode::Auto,
    };
    let huge = Segments { along: Some(4), around: Some(usize::MAX / 2), through: Some(4) };
    let error = mesh::build_mesh(&revolve(huge, Resolution::Cells(16)), Path::new(".")).err().unwrap();
    assert!(error.contains("too large"), "{error}");
    let fine = revolve(Segments::default(), Resolution::ElementSize(1e-300));
    let error = mesh::build_mesh(&fine, Path::new(".")).err().unwrap();
    assert!(error.contains("too large"), "{error}");

    let mut voxels = bar_spec(1.0, 1.0, 0.1, None, 4);
    voxels.resolution = Resolution::CellSize([1e-300, 1e-300, 1.0]);
    let error = mesh::build_mesh(&voxels, Path::new(".")).err().unwrap();
    assert!(error.contains("too large"), "{error}");

    // a chunky cube meshes within the element cap, but its factor would
    // not fit: refused before anything is assembled
    let mut cube = bar_spec(0.1, 0.1, 0.01, None, 4);
    cube.resolution = Resolution::Cells(32);
    let error = solve_detailed(&cube, Path::new("."), &SolveProgress::default()).err().unwrap();
    assert!(error.contains("too fine"), "{error}");

    let many = bar_spec(0.1, 0.02, 0.01, None, spec::MAX_MODES + 1);
    assert!(many.validate().is_err());
    assert!(bar_spec(0.1, 0.02, 0.01, None, spec::MAX_MODES).validate().is_ok());
}

/// A tube with a hole through both walls keeps its mapped mesh, less the
/// elements the hole covers (voxels could not follow a 2.5 mm wall), and
/// still solves, its modes moved a little by the missing metal.
#[test]
fn a_drilled_tube_is_its_mapped_mesh_carved() {
    let tube = Solid::Tube { outer_radius: 0.016, inner_radius: 0.0135, height: 0.3, center: [0.0; 3], axis: Axis::Z };
    let spec = |solid: Solid| SolidSpec {
        name: None,
        solid,
        resolution: Resolution::Cells(60),
        material: STEEL,
        block: None,
        modes: 6,
        mesh: MeshMode::Auto,
    };
    let whole = spec(tube.clone());
    let drilled = spec(Solid::Difference(vec![
        tube,
        Solid::Cylinder { radius: 0.007, height: 0.05, center: [0.0, 0.0, 0.08], axis: Axis::X },
    ]));
    let full = mesh::build_mesh(&whole, Path::new(".")).unwrap();
    let carved = mesh::build_mesh(&drilled, Path::new(".")).unwrap();
    let removed = full.elements.len() - carved.elements.len();
    assert!(removed > 0 && removed * 20 < full.elements.len(), "{removed} of {}", full.elements.len());
    assert_eq!(carved.dropped_cells, 0);
    let (a, _) = solve(&whole, Path::new(".")).unwrap();
    let (b, _) = solve(&drilled, Path::new(".")).unwrap();
    assert_eq!(b.freqs.len(), 6);
    assert!(a.freqs[0] != b.freqs[0] && rel(b.freqs[0], a.freqs[0]) < 0.05, "{:?} vs {:?}", a.freqs, b.freqs);
}

/// A watched solve reports its phases as it goes and finishes at the mode
/// search; cancelled mid-way, it stops promptly with CANCELLED.
#[test]
fn a_watched_solve_reports_progress_and_stops_when_cancelled() {
    let small = bar_spec(0.3, 0.02, 0.01, None, 4);
    let progress = SolveProgress::default();
    solve_watched(&small, Path::new("."), &progress).unwrap();
    let (phase, done) = progress.snapshot();
    assert_eq!(phase, SolvePhase::Modes);
    assert!(done > 0.5, "{done}");

    // a solve that takes seconds, cancelled once it is factoring or later
    let big = bar_spec(0.35, 0.04, 0.0025, None, 24);
    let progress = std::sync::Arc::new(SolveProgress::default());
    let watched = progress.clone();
    let solving = std::thread::spawn(move || solve_watched(&big, Path::new("."), &watched).map(|_| ()));
    let start = std::time::Instant::now();
    while progress.snapshot().0 == SolvePhase::Mesh || progress.snapshot().0 == SolvePhase::Assemble {
        assert!(start.elapsed() < std::time::Duration::from_secs(60), "never reached factoring");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let cancelled_at = std::time::Instant::now();
    progress.cancel();
    let outcome = solving.join().unwrap();
    assert_eq!(outcome, Err(CANCELLED.to_string()));
    assert!(cancelled_at.elapsed() < std::time::Duration::from_secs(2), "{:?}", cancelled_at.elapsed());
}

/// Sugar maple's constants (Wood Handbook ratios), the grain along `along`.
fn maple(along: Axis, across: Axis) -> Material {
    let e = 1.2e10;
    Material {
        density: 650.0,
        young: e,
        poisson: 0.35,
        grain: Some(Grain {
            along,
            across,
            young_r: 0.132 * e,
            young_t: 0.065 * e,
            shear_lr: 0.111 * e,
            shear_lt: 0.063 * e,
            shear_rt: 0.021 * e,
            poisson_lr: 0.424,
            poisson_lt: 0.476,
            poisson_rt: 0.774,
        }),
    }
}

#[test]
fn a_wooden_bar_bends_by_the_modulus_along_its_length() {
    // 0.3 × 0.03 × 0.01 m: its first mode bends about y, through z, with
    // E along x. Grain along x: E_L. Grain along y (R along z, so T along
    // x): E_T, about 1/15 as stiff, so ~sqrt(1/0.065) = 3.9 times lower.
    let spec = |material: Material| SolidSpec {
        name: None,
        solid: Solid::Box { size: [0.3, 0.03, 0.01], center: [0.0; 3] },
        resolution: Resolution::ElementSize(0.005),
        material,
        block: None,
        modes: 2,
        mesh: MeshMode::Auto,
    };
    let solve = |material| solve_detailed(&spec(material), Path::new("."), &SolveProgress::default()).unwrap().solid.freqs[0];
    let along = solve(maple(Axis::X, Axis::Y));
    let across = solve(maple(Axis::Y, Axis::Z));
    let isotropic = solve(Material { grain: None, ..maple(Axis::X, Axis::Y) });
    eprintln!("maple bar: grain along {along:.1} Hz, across {across:.1} Hz, isotropic {isotropic:.1} Hz");
    // along the grain it is nearly the isotropic bar (shear is softer)
    assert!(along < isotropic && along > 0.9 * isotropic, "{along} vs {isotropic}");
    let ratio = along / across;
    assert!(ratio > 3.3 && ratio < 4.2, "{ratio}");
    // a grain whose ratios are unstable is refused
    let mut bad = maple(Axis::X, Axis::Y);
    bad.grain.as_mut().unwrap().poisson_rt = 3.0;
    assert!(spec(bad).validate().is_err());
}

#[test]
fn flat_cells_through_a_thin_bar_match_fine_cubes() {
    // 0.3 × 0.03 × 0.01 m steel: 4 layers through its thickness either way,
    // as 10 mm × 10 mm × 2.5 mm cells or 2.5 mm cubes (16 times as many).
    let spec = |resolution| SolidSpec {
        name: None,
        solid: Solid::Box { size: [0.3, 0.03, 0.01], center: [0.0; 3] },
        resolution,
        material: STEEL,
        block: None,
        modes: 3,
        mesh: MeshMode::Voxel,
    };
    let solve = |resolution| solve_detailed(&spec(resolution), Path::new("."), &SolveProgress::default()).unwrap();
    let flat = solve(Resolution::CellSize([0.01, 0.01, 0.0025]));
    let cubes = solve(Resolution::ElementSize(0.0025));
    assert_eq!(flat.stats.elements * 16, cubes.stats.elements);
    for k in 0..3 {
        assert!(rel(flat.solid.freqs[k], cubes.solid.freqs[k]) < 0.02, "{:?} vs {:?}", flat.solid.freqs, cubes.solid.freqs);
    }
    let bad = spec(Resolution::CellSize([0.01, 0.0, 0.001]));
    assert!(bad.validate().is_err());
}
