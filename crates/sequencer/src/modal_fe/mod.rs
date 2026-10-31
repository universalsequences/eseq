//! Finite-element modal solids (docs/modalys-gap-analysis.md §4, bead
//! eseq-k0m2.27): mesh a 3-D elastic solid, compute its lowest vibration
//! modes, and export them for a modal synthesizer to play like a bell, bar
//! or cymbal, the way Modalys' finite-element objects work.
//!
//! Pipeline, all offline and dependency-free:
//!
//! 1. [`SolidSpec`] → hex8 mesh ([`mesh`]): voxels for primitives, CSG and
//!    .obj imports; a body-fitted mapped mesh for a lone solid of
//!    revolution (bells, bowls, cymbals, tubes, shells, cylinders), where
//!    voxels could not follow a thin curved wall.
//! 2. Stiffness from hex8 elements with incompatible modes (no shear
//!    locking in thin bending), row-sum lumped mass ([`element`]).
//! 3. K φ = λ M φ by shift-invert block Lanczos ([`eigen`]) on K − σM,
//!    factored by envelope Cholesky in reverse Cuthill–McKee order
//!    ([`skyline`]). σ is a small negative shift so a free body stays
//!    factorizable; its six rigid-body modes per free piece are built
//!    analytically and deflated, so they never enter the result.
//! 4. The exterior surface as triangles, with each mode's displacement
//!    along the vertex normals, mass-normalized (φᵀ M φ = 1).
//!
//! The result is written as the "eseq-modes" v1 JSON document
//! ([`ModalSolid::to_json`]).

mod eigen;
mod element;
pub(crate) mod mesh;
pub mod net;
mod obj;
mod skyline;
mod spec;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub use spec::{Axis, Grain, Material, MeshMode, Region, Resolution, Segments, Solid, SolidSpec};

/// A solid's elastic modes over its boundary surface. Arrays are flat:
/// `points` and `normals` are P×3, `triangles` T×3, and `shapes` P×N
/// point-major (`shapes[p * N + k]` is mode k's displacement at point p
/// along its normal, for the mass-normalized mode).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModalSolid {
    /// Mode frequencies in Hz, ascending, rigid-body modes excluded.
    pub freqs: Vec<f64>,
    pub points: Vec<f32>,
    pub normals: Vec<f32>,
    pub triangles: Vec<u32>,
    pub shapes: Vec<f32>,
    /// Total mass in kg.
    pub mass: f64,
}

/// Mesh and solver figures for one solve.
#[derive(Clone, Debug, Default)]
pub struct SolveStats {
    pub nodes: usize,
    pub elements: usize,
    pub blocked_nodes: usize,
    /// The mesh node nearest the block region's centre (a preview's, for
    /// an editor to move a block that holds nothing onto the shape).
    pub block_nearest: Option<[f64; 3]>,
    pub dofs: usize,
    /// Stored entries of the factored envelope.
    pub envelope: usize,
    pub rigid_modes_deflated: usize,
    pub dropped_cells: usize,
    /// Operator applications in the Lanczos run, and how many Ritz pairs
    /// (lowest first, counted before mechanism filtering, capped at the
    /// modes wanted) met its residual tolerance.
    pub lanczos_steps: usize,
    pub converged: usize,
    pub mesh_time: Duration,
    pub assemble_time: Duration,
    pub factor_time: Duration,
    pub eigen_time: Duration,
    pub total_time: Duration,
}

pub const MODES_FORMAT: &str = "eseq-modes";
pub const MODES_VERSION: u32 = 1;

/// The document: header, the solid's fields inline, then the spec.
#[derive(Serialize, Deserialize)]
struct ModesFile<M, S> {
    format: String,
    version: u32,
    #[serde(flatten)]
    solid: M,
    spec: S,
}

impl ModalSolid {
    pub fn mode_count(&self) -> usize {
        self.freqs.len()
    }

    pub fn point_count(&self) -> usize {
        self.points.len() / 3
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles.len() / 3
    }

    /// The "eseq-modes" v1 document, `spec` echoed in canonical form.
    pub fn to_json(&self, spec: &SolidSpec) -> String {
        let file = ModesFile {
            format: MODES_FORMAT.to_string(),
            version: MODES_VERSION,
            solid: self,
            spec,
        };
        serde_json::to_string(&file).expect("modal solids serialize")
    }

    /// Parse an "eseq-modes" v1 document; the echoed spec comes back raw.
    pub fn from_json(text: &str) -> Result<(Self, serde_json::Value), String> {
        let file: ModesFile<ModalSolid, serde_json::Value> =
            serde_json::from_str(text).map_err(|error| error.to_string())?;
        if file.format != MODES_FORMAT || file.version != MODES_VERSION {
            return Err(format!("not an {MODES_FORMAT} v{MODES_VERSION} document"));
        }
        let solid = file.solid;
        let (p, n) = (solid.point_count(), solid.mode_count());
        if solid.normals.len() != 3 * p || solid.shapes.len() != p * n || solid.triangles.len() % 3 != 0
        {
            return Err("eseq-modes arrays have inconsistent lengths".into());
        }
        Ok((solid, file.spec))
    }
}

/// Stable hex SHA-256 of the canonical spec JSON, for caching solves.
pub fn spec_hash(spec: &SolidSpec) -> String {
    let canonical = serde_json::to_string(spec).expect("specs serialize");
    crate::sample_manifest::sha256_hex(canonical.as_bytes())
}

/// Shift as a fraction of the largest diagonal K/M ratio (≈ the top of the
/// spectrum). Far below any elastic eigenvalue, yet large enough that the
/// near-singular pivots of a free body stay well above rounding.
const SHIFT: f64 = 1e-10;

/// Solve a spec; relative .obj paths resolve against `base_dir`.
pub fn solve(spec: &SolidSpec, base_dir: &Path) -> Result<(ModalSolid, SolveStats), String> {
    solve_watched(spec, base_dir, &SolveProgress::default())
}

/// [`solve`], reporting its progress to (and stopping with [`CANCELLED`]
/// when cancelled through) `progress`, which another thread may watch.
pub fn solve_watched(spec: &SolidSpec, base_dir: &Path, progress: &SolveProgress) -> Result<(ModalSolid, SolveStats), String> {
    solve_detailed(spec, base_dir, progress).map(|d| (d.solid, d.stats))
}

/// Most bytes the stiffness envelope (and so its factor) may take: a
/// quarter of the machine's memory, leaving room for the rest of the solve
/// and the app (2 GB where the memory can't be read). A 0.35 m bar at
/// 2.5 mm elements needs 1 GB; the hollow-body guitar's body 4.9 GB; a
/// chunky solid at the lab's finer details tens of GB.
fn max_envelope_bytes() -> f64 {
    #[cfg(unix)]
    {
        // SAFETY: sysconf only reads system configuration.
        let (pages, page) = unsafe { (libc::sysconf(libc::_SC_PHYS_PAGES), libc::sysconf(libc::_SC_PAGESIZE)) };
        if pages > 0 && page > 0 {
            return pages as f64 * page as f64 / 4.0;
        }
    }
    2e9
}

/// The error a cancelled solve returns.
pub const CANCELLED: &str = "solve cancelled";

/// Where a solve is, shared with whoever watches it: its phase, how far
/// through the whole solve it is, and a flag that cancels it.
#[derive(Default)]
pub struct SolveProgress {
    cancelled: std::sync::atomic::AtomicBool,
    /// The phase (a [`SolvePhase`] as u8) and the whole solve's progress in
    /// millionths.
    phase: std::sync::atomic::AtomicU8,
    done: std::sync::atomic::AtomicU32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolvePhase {
    Mesh,
    Assemble,
    Factor,
    Modes,
}

impl SolvePhase {
    /// Each phase's share of a solve's time, as fractions of the whole
    /// (measured on voxel bars and the bell: factoring and the mode search
    /// take nearly all of it).
    fn span(self) -> (f64, f64) {
        match self {
            SolvePhase::Mesh => (0.0, 0.02),
            SolvePhase::Assemble => (0.02, 0.08),
            SolvePhase::Factor => (0.08, 0.5),
            SolvePhase::Modes => (0.5, 1.0),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SolvePhase::Mesh => "meshing",
            SolvePhase::Assemble => "assembling",
            SolvePhase::Factor => "factoring",
            SolvePhase::Modes => "finding modes",
        }
    }
}

impl SolveProgress {
    pub fn cancel(&self) {
        self.cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// `fraction` (0..1) of the way through `phase`.
    pub fn report(&self, phase: SolvePhase, fraction: f64) {
        let (from, to) = phase.span();
        let whole = from + (to - from) * fraction.clamp(0.0, 1.0);
        self.phase.store(phase as u8, std::sync::atomic::Ordering::Relaxed);
        self.done.store((whole * 1e6) as u32, std::sync::atomic::Ordering::Relaxed);
    }

    /// The phase and how far through the whole solve (0..1).
    pub fn snapshot(&self) -> (SolvePhase, f64) {
        let phase = match self.phase.load(std::sync::atomic::Ordering::Relaxed) {
            0 => SolvePhase::Mesh,
            1 => SolvePhase::Assemble,
            2 => SolvePhase::Factor,
            _ => SolvePhase::Modes,
        };
        (phase, self.done.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6)
    }

    /// A step callback for `phase`: reports how far through it is and
    /// answers whether to go on.
    fn watch(&self, phase: SolvePhase) -> impl Fn(f64) -> bool + '_ {
        move |fraction| {
            self.report(phase, fraction);
            !self.is_cancelled()
        }
    }

    fn check(&self) -> Result<(), String> {
        if self.is_cancelled() { Err(CANCELLED.to_string()) } else { Ok(()) }
    }
}

/// A solve plus the full-mesh data tests check it against.
pub(crate) struct Detailed {
    pub solid: ModalSolid,
    pub stats: SolveStats,
    /// Per free DOF (3·rank + component).
    pub dof_mass: Vec<f64>,
    pub lambdas: Vec<f64>,
    /// Mass-normalized modes over the free DOFs.
    pub vectors: Vec<Vec<f64>>,
    /// Unshifted K (cloned only in test builds).
    #[cfg(test)]
    pub stiffness: skyline::Envelope,
}

pub(crate) fn solve_detailed(spec: &SolidSpec, base_dir: &Path, progress: &SolveProgress) -> Result<Detailed, String> {
    let start = Instant::now();
    spec.validate()?;
    progress.report(SolvePhase::Mesh, 0.0);
    let mut stats = SolveStats::default();
    let mesh = mesh::build_mesh(spec, base_dir)?;
    progress.check()?;
    progress.report(SolvePhase::Assemble, 0.0);
    stats.nodes = mesh.nodes.len();
    stats.elements = mesh.elements.len();
    stats.dropped_cells = mesh.dropped_cells;
    stats.mesh_time = start.elapsed();

    let assemble_start = Instant::now();
    let n_nodes = mesh.nodes.len();
    let extent = bounding_extent(&mesh.nodes);
    let blocked = blocked_nodes(spec, &mesh.nodes, extent);
    stats.blocked_nodes = blocked.iter().filter(|&&b| b).count();
    if spec.block.is_some() && stats.blocked_nodes == 0 {
        return Err("the block region contains no mesh node".into());
    }
    if stats.blocked_nodes == n_nodes {
        return Err("the block region clamps every node".into());
    }

    // Free nodes in reverse Cuthill–McKee order; DOF = 3·rank + component.
    let mut adjacency: Vec<Vec<u32>> = vec![Vec::new(); n_nodes];
    for element in &mesh.elements {
        for &a in element {
            for &b in element {
                if a != b && !blocked[a as usize] && !blocked[b as usize] {
                    adjacency[a as usize].push(b);
                }
            }
        }
    }
    for list in &mut adjacency {
        list.sort_unstable();
        list.dedup();
    }
    let free_nodes: Vec<u32> = (0..n_nodes as u32).filter(|&v| !blocked[v as usize]).collect();
    let mut local = vec![u32::MAX; n_nodes];
    for (i, &v) in free_nodes.iter().enumerate() {
        local[v as usize] = i as u32;
    }
    let local_adjacency: Vec<Vec<u32>> = free_nodes
        .iter()
        .map(|&v| adjacency[v as usize].iter().map(|&w| local[w as usize]).collect())
        .collect();
    drop(adjacency);
    let order = skyline::reverse_cuthill_mckee(&local_adjacency);
    let mut rank = vec![u32::MAX; n_nodes];
    for (r, &l) in order.iter().enumerate() {
        rank[free_nodes[l as usize] as usize] = r as u32;
    }
    let dofs = 3 * free_nodes.len();
    stats.dofs = dofs;

    let mut first: Vec<usize> = (0..dofs).collect();
    for element in &mesh.elements {
        let low = element.iter().filter_map(|&n| free_rank(&rank, n)).min();
        if let Some(low) = low {
            for &n in element {
                if let Some(r) = free_rank(&rank, n) {
                    for c in 0..3 {
                        first[3 * r + c] = first[3 * r + c].min(3 * low);
                    }
                }
            }
        }
    }
    // The factor's envelope grows about as the fifth power of the
    // resolution on a chunky solid; refuse one the app could not hold
    // rather than abort on the allocation.
    let envelope = skyline::envelope_size(&first) as f64 * 8.0;
    let max = max_envelope_bytes();
    if envelope > max {
        return Err(format!(
            "this solid is too fine to solve: its stiffness envelope needs {:.1} GB, and this machine allows {:.1} GB; coarsen its resolution",
            envelope * 1e-9,
            max * 1e-9,
        ));
    }
    let d = element::material_elasticity(&spec.material);
    let (mut matrix, node_mass) = assemble(&mesh, &rank, &first, &d, spec.material.density, progress)?;
    stats.envelope = matrix.stored();
    let mut dof_mass = vec![0.0; dofs];
    for v in 0..n_nodes {
        if let Some(r) = free_rank(&rank, v as u32) {
            dof_mass[3 * r..3 * r + 3].fill(node_mass[v]);
        }
    }
    let lambda_scale = (0..dofs).map(|i| matrix.diagonal(i) / dof_mass[i]).fold(0.0, f64::max);

    // Rigid-body modes of every free (unblocked) connected piece.
    let deflate = rigid_modes(&mesh, &blocked, &rank, &node_mass, &dof_mass);
    stats.rigid_modes_deflated = deflate.len();
    stats.assemble_time = assemble_start.elapsed();
    progress.check()?;

    let factor_start = Instant::now();
    #[cfg(test)]
    let stiffness = matrix.clone();
    // A shift too small for rounding makes a free body's near-null pivots
    // fail; widen it (reassembling, as the failed factor overwrote K) until
    // the factorization goes through.
    let mut sigma = -SHIFT * lambda_scale;
    let factor = loop {
        for i in 0..dofs {
            matrix.add(i, i, -sigma * dof_mass[i]);
        }
        match matrix.factor(&progress.watch(SolvePhase::Factor)) {
            Ok(()) => break matrix,
            Err(error) if progress.is_cancelled() => return Err(error),
            Err(_) if sigma > -1e-4 * lambda_scale => {
                sigma *= 100.0;
                matrix = assemble(&mesh, &rank, &first, &d, spec.material.density, progress)?.0;
            }
            Err(error) => return Err(error),
        }
    };
    progress.check()?;
    stats.factor_time = factor_start.elapsed();

    let eigen_start = Instant::now();
    // A block region that pins too little leaves mechanisms (zero modes
    // not deflated); ask for a few extra and filter them below.
    let spare = if spec.block.is_some() { 6 } else { 0 };
    let solve_op = |b: &mut [f64]| factor.solve(b);
    let result =
        eigen::shift_invert_lanczos(&dof_mass, &solve_op, &deflate, spec.modes.saturating_add(spare), &progress.watch(SolvePhase::Modes));
    progress.check()?;
    stats.lanczos_steps = result.steps;
    stats.converged = result.converged.min(spec.modes);
    let mut modes: Vec<(f64, Vec<f64>)> = result
        .thetas
        .iter()
        .zip(result.vectors)
        .map(|(&theta, vector)| (sigma + 1.0 / theta, vector))
        .filter(|(lambda, _)| *lambda > 1e-4 * sigma.abs())
        .collect();
    modes.sort_by(|a, b| a.0.total_cmp(&b.0));
    modes.truncate(spec.modes);
    stats.eigen_time = eigen_start.elapsed();

    // Surface, normal displacements, and the sign convention: each mode's
    // largest nodal displacement component is positive.
    let surface = mesh::surface(&mesh);
    let n_modes = modes.len();
    let mut shapes = vec![0f32; surface.nodes.len() * n_modes];
    for (k, (_, vector)) in modes.iter_mut().enumerate() {
        let peak = vector.iter().cloned().fold(0.0f64, |m, v| if v.abs() > m.abs() { v } else { m });
        if peak < 0.0 {
            vector.iter_mut().for_each(|v| *v = -*v);
        }
        for (p, (&node, normal)) in surface.nodes.iter().zip(&surface.normals).enumerate() {
            if let Some(r) = free_rank(&rank, node) {
                let u = [vector[3 * r], vector[3 * r + 1], vector[3 * r + 2]];
                shapes[p * n_modes + k] = mesh::dot(u, *normal) as f32;
            }
        }
    }
    let solid = ModalSolid {
        freqs: modes.iter().map(|(l, _)| l.max(0.0).sqrt() / std::f64::consts::TAU).collect(),
        shapes,
        mass: node_mass.iter().sum(),
        ..surface_solid(&mesh, &surface)
    };
    stats.total_time = start.elapsed();
    Ok(Detailed {
        solid,
        stats,
        dof_mass,
        lambdas: modes.iter().map(|(l, _)| *l).collect(),
        vectors: modes.into_iter().map(|(_, v)| v).collect(),
        #[cfg(test)]
        stiffness,
    })
}

/// A spec meshed but not solved, for a shape editor's live view: the mesh
/// takes milliseconds where the solve takes seconds. The surface is the one
/// a solve would return, with no modes (`freqs` and `shapes` empty); `mass`
/// is the density times the elements' volume.
pub fn preview(spec: &SolidSpec, base_dir: &Path) -> Result<(ModalSolid, SolveStats), String> {
    let start = Instant::now();
    spec.validate()?;
    let mesh = mesh::build_mesh(spec, base_dir)?;
    let volume: f64 = mesh
        .elements
        .iter()
        .map(|element| 8.0 * element::center_jacobian_det(&element.map(|n| mesh.nodes[n as usize])))
        .sum();
    let solid = ModalSolid { mass: spec.material.density * volume, ..surface_solid(&mesh, &mesh::surface(&mesh)) };
    let extent = bounding_extent(&mesh.nodes);
    let stats = SolveStats {
        nodes: mesh.nodes.len(),
        elements: mesh.elements.len(),
        blocked_nodes: blocked_nodes(spec, &mesh.nodes, extent).iter().filter(|&&b| b).count(),
        block_nearest: spec.block.and_then(|region| {
            let c = region.center();
            mesh.nodes.iter().min_by(|a, b| spec::dist(**a, c).total_cmp(&spec::dist(**b, c))).copied()
        }),
        dropped_cells: mesh.dropped_cells,
        mesh_time: start.elapsed(),
        total_time: start.elapsed(),
        ..SolveStats::default()
    };
    Ok((solid, stats))
}

/// Which of `nodes` the spec's block region clamps (none without one).
fn blocked_nodes(spec: &SolidSpec, nodes: &[[f64; 3]], extent: f64) -> Vec<bool> {
    match &spec.block {
        Some(region) => nodes.iter().map(|&p| region.contains(p, 1e-9 * extent)).collect(),
        None => vec![false; nodes.len()],
    }
}

/// The surface of `mesh` as a solid with no modes.
fn surface_solid(mesh: &mesh::HexMesh, surface: &mesh::Surface) -> ModalSolid {
    ModalSolid {
        freqs: Vec::new(),
        points: surface.nodes.iter().flat_map(|&n| mesh.nodes[n as usize].map(|c| c as f32)).collect(),
        normals: surface.normals.iter().flat_map(|n| n.map(|c| c as f32)).collect(),
        triangles: surface.triangles.iter().flatten().copied().collect(),
        shapes: Vec::new(),
        mass: 0.0,
    }
}

/// Load a spec file and solve it, resolving .obj paths beside the spec.
pub fn solve_file(path: &Path) -> Result<(SolidSpec, ModalSolid, SolveStats), String> {
    let spec = SolidSpec::load(path)?;
    let base = path.parent().unwrap_or(Path::new("."));
    let (solid, stats) = solve(&spec, base)?;
    Ok((spec, solid, stats))
}

/// K in envelope storage over the free DOFs, and the lumped node masses.
/// Consecutive elements with the same shape (every cell of a voxel mesh)
/// reuse one element matrix.
fn assemble(
    mesh: &mesh::HexMesh,
    rank: &[u32],
    first: &[usize],
    d: &[[f64; 6]; 6],
    density: f64,
    progress: &SolveProgress,
) -> Result<(skyline::Envelope, Vec<f64>), String> {
    let mut matrix = skyline::Envelope::new(first.to_vec());
    let mut node_mass = vec![0.0; mesh.nodes.len()];
    let mut cached: Option<([u64; 24], [f64; 576], [f64; 8])> = None;
    for (e, element) in mesh.elements.iter().enumerate() {
        if e % 1024 == 0 {
            progress.check()?;
            progress.report(SolvePhase::Assemble, e as f64 / mesh.elements.len() as f64);
        }
        let corners = element.map(|n| mesh.nodes[n as usize]);
        let mut shape = [0u64; 24];
        for a in 0..8 {
            for c in 0..3 {
                shape[3 * a + c] = (corners[a][c] - corners[0][c]).to_bits();
            }
        }
        if cached.as_ref().map_or(true, |(key, _, _)| *key != shape) {
            let (k, m) = element::hex8(&corners, d, density)?;
            cached = Some((shape, k, m));
        }
        let (_, k, m) = cached.as_ref().expect("just computed");
        for a in 0..8 {
            node_mass[element[a] as usize] += m[a];
            let Some(ra) = free_rank(rank, element[a]) else { continue };
            for b in 0..8 {
                let Some(rb) = free_rank(rank, element[b]) else { continue };
                if rb > ra {
                    continue;
                }
                for ca in 0..3 {
                    for cb in 0..3 {
                        let (i, j) = (3 * ra + ca, 3 * rb + cb);
                        if j <= i {
                            matrix.add(i, j, k[(3 * a + ca) * 24 + 3 * b + cb]);
                        }
                    }
                }
            }
        }
    }
    Ok((matrix, node_mass))
}

fn free_rank(rank: &[u32], node: u32) -> Option<usize> {
    let r = rank[node as usize];
    (r != u32::MAX).then_some(r as usize)
}

fn bounding_extent(nodes: &[[f64; 3]]) -> f64 {
    let (lo, hi) = mesh::bounding_box(nodes);
    (0..3).map(|i| hi[i] - lo[i]).fold(0.0, f64::max)
}

/// Translations and rotations about the center of mass of every connected
/// piece with no blocked node, M-orthonormalized.
fn rigid_modes(
    mesh: &mesh::HexMesh,
    blocked: &[bool],
    rank: &[u32],
    node_mass: &[f64],
    dof_mass: &[f64],
) -> Vec<Vec<f64>> {
    let n = mesh.nodes.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn find(parent: &mut [u32], mut v: u32) -> u32 {
        while parent[v as usize] != v {
            parent[v as usize] = parent[parent[v as usize] as usize];
            v = parent[v as usize];
        }
        v
    }
    for element in &mesh.elements {
        for &b in &element[1..] {
            let (ra, rb) = (find(&mut parent, element[0]), find(&mut parent, b));
            if ra != rb {
                parent[ra as usize] = rb;
            }
        }
    }
    let mut pieces: std::collections::BTreeMap<u32, Vec<usize>> = Default::default();
    for v in 0..n {
        let root = find(&mut parent, v as u32);
        pieces.entry(root).or_default().push(v);
    }
    let mut modes: Vec<Vec<f64>> = Vec::new();
    for nodes in pieces.values() {
        if nodes.len() < 2 || nodes.iter().any(|&v| blocked[v]) {
            continue;
        }
        let total: f64 = nodes.iter().map(|&v| node_mass[v]).sum();
        let mut c = [0.0; 3];
        for &v in nodes {
            for i in 0..3 {
                c[i] += node_mass[v] * mesh.nodes[v][i] / total;
            }
        }
        for kind in 0..6 {
            let mut x = vec![0.0; dof_mass.len()];
            for &v in nodes {
                let r = rank[v] as usize;
                let p = mesh::sub(mesh.nodes[v], c);
                let u = match kind {
                    0..=2 => {
                        let mut u = [0.0; 3];
                        u[kind] = 1.0;
                        u
                    }
                    3 => [0.0, -p[2], p[1]],
                    4 => [p[2], 0.0, -p[0]],
                    _ => [-p[1], p[0], 0.0],
                };
                x[3 * r..3 * r + 3].copy_from_slice(&u);
            }
            for _ in 0..2 {
                for prior in &modes {
                    let s = eigen::m_dot(dof_mass, prior, &x);
                    eigen::axpy(-s, prior, &mut x);
                }
            }
            let norm = eigen::m_dot(dof_mass, &x, &x).sqrt();
            // A piece too thin to rotate about some axis (a line of nodes)
            // has fewer than six independent rigid motions.
            if norm > 1e-9 * total.sqrt() * (1.0 + mesh::dot(c, c).sqrt()) {
                x.iter_mut().for_each(|v| *v /= norm);
                modes.push(x);
            }
        }
    }
    modes
}
