//! Hex8 meshes: voxelized solids, body-fitted (mapped) solids of revolution,
//! and the exterior surface of either.
//!
//! Node order inside an element is the usual isoparametric one: corners at
//! ξ ∈ {−1, 1}³ listed as (−,−,−) (+,−,−) (+,+,−) (−,+,−), then the same four
//! with ζ = +1. Elements are stored with a positive Jacobian.

use std::collections::HashMap;
use std::path::Path;

use super::obj::TriMesh;
use super::spec::{axis_frame, dist, from_polar, to_rz, Axis, MeshMode, Profile, Resolution, Segments, Solid, SolidSpec};

pub(crate) struct HexMesh {
    pub nodes: Vec<[f64; 3]>,
    pub elements: Vec<[u32; 8]>,
    /// Voxel cells dropped because they were not face-connected to the
    /// largest piece (edge or corner contacts would be hinges).
    pub dropped_cells: usize,
}

/// The six faces of a hex as corner cycles.
const HEX_FACES: [[usize; 4]; 6] =
    [[0, 1, 2, 3], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]];

pub(crate) fn build_mesh(spec: &SolidSpec, base_dir: &Path) -> Result<HexMesh, String> {
    let mappable = mapped_profile(&spec.solid);
    match (spec.mesh, mappable) {
        (MeshMode::Mapped, None) => {
            Err("mesh \"mapped\" needs a lone revolve (with a wall), tube, shell or cylinder".into())
        }
        (MeshMode::Auto | MeshMode::Mapped, Some((profile, center, axis, segments))) => {
            let e = profile.extent();
            let largest = (2.0 * e[1]).max(e[3] - e[2]);
            revolve_mesh(&profile, center, axis, segments, element_size(spec.resolution, largest))
        }
        _ => voxel_mesh(spec, base_dir),
    }
}

fn element_size(resolution: Resolution, largest_extent: f64) -> f64 {
    match resolution {
        Resolution::Cells(n) => largest_extent / n as f64,
        Resolution::ElementSize(h) => h,
    }
}

fn mapped_profile(solid: &Solid) -> Option<(Profile, [f64; 3], Axis, Segments)> {
    match solid {
        Solid::Revolve { profile, thickness, center, axis, segments } => {
            let profile = Profile::new(profile, *thickness);
            profile.walls.is_some().then_some((profile, *center, *axis, *segments))
        }
        Solid::Tube { outer_radius, inner_radius, height, center, axis } => Some((
            Profile::tube(*outer_radius, *inner_radius, *height),
            *center,
            *axis,
            Segments::default(),
        )),
        Solid::Shell { outer_radius, inner_radius, center } => Some((
            Profile::shell(*outer_radius, *inner_radius),
            *center,
            Axis::Z,
            Segments::default(),
        )),
        Solid::Cylinder { radius, height, center, axis } => {
            Some((Profile::cylinder(*radius, *height), *center, *axis, Segments::default()))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Voxels

struct Bounds {
    lo: [f64; 3],
    hi: [f64; 3],
}

impl Bounds {
    fn around(center: [f64; 3], half: [f64; 3]) -> Self {
        Bounds {
            lo: [center[0] - half[0], center[1] - half[1], center[2] - half[2]],
            hi: [center[0] + half[0], center[1] + half[1], center[2] + half[2]],
        }
    }

    fn union(self, other: Bounds) -> Bounds {
        Bounds {
            lo: [0, 1, 2].map(|i| self.lo[i].min(other.lo[i])),
            hi: [0, 1, 2].map(|i| self.hi[i].max(other.hi[i])),
        }
    }
}

fn axial_half(axis: Axis, radial: f64, axial: f64) -> [f64; 3] {
    let mut half = [radial; 3];
    half[axis_frame(axis)[2]] = axial;
    half
}

/// Each .obj leaf of a solid tree, loaded once.
type Objs = HashMap<*const Solid, TriMesh>;

fn load_objs(solid: &Solid, base_dir: &Path, objs: &mut Objs) -> Result<(), String> {
    match solid {
        Solid::Union(parts) | Solid::Difference(parts) => {
            parts.iter().try_for_each(|part| load_objs(part, base_dir, objs))
        }
        Solid::Obj { path, scale, offset } => {
            objs.insert(solid, TriMesh::load(&base_dir.join(path), *scale, *offset)?);
            Ok(())
        }
        _ => Ok(()),
    }
}

fn bounds(solid: &Solid, objs: &Objs) -> Bounds {
    match solid {
        Solid::Box { size, center } => Bounds::around(*center, size.map(|s| 0.5 * s)),
        Solid::Cylinder { radius, height, center, axis } => {
            Bounds::around(*center, axial_half(*axis, *radius, 0.5 * height))
        }
        Solid::Tube { outer_radius, height, center, axis, .. } => {
            Bounds::around(*center, axial_half(*axis, *outer_radius, 0.5 * height))
        }
        Solid::Sphere { radius, center } => Bounds::around(*center, [*radius; 3]),
        Solid::Shell { outer_radius, center, .. } => Bounds::around(*center, [*outer_radius; 3]),
        Solid::Revolve { profile, thickness, center, axis, .. } => {
            let e = Profile::new(profile, *thickness).extent();
            let [_, _, c] = axis_frame(*axis);
            let mut b = Bounds::around(*center, axial_half(*axis, e[1], 0.0));
            b.lo[c] = center[c] + e[2];
            b.hi[c] = center[c] + e[3];
            b
        }
        Solid::Union(parts) => {
            let mut parts = parts.iter();
            let mut b = bounds(parts.next().expect("validated non-empty"), objs);
            for part in parts {
                b = b.union(bounds(part, objs));
            }
            b
        }
        Solid::Difference(parts) => bounds(&parts[0], objs),
        Solid::Obj { .. } => {
            let (lo, hi) = bounding_box(&objs[&(solid as *const Solid)].vertices);
            Bounds { lo, hi }
        }
    }
}

struct Grid {
    lo: [f64; 3],
    h: [f64; 3],
    n: [usize; 3],
}

impl Grid {
    fn center(&self, i: usize, j: usize, k: usize) -> [f64; 3] {
        [
            self.lo[0] + (i as f64 + 0.5) * self.h[0],
            self.lo[1] + (j as f64 + 0.5) * self.h[1],
            self.lo[2] + (k as f64 + 0.5) * self.h[2],
        ]
    }

    fn cell(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.n[1] + j) * self.n[0] + i
    }

    fn len(&self) -> usize {
        self.n[0] * self.n[1] * self.n[2]
    }
}

/// Cells whose centers lie inside the solid.
fn occupancy(solid: &Solid, grid: &Grid, objs: &Objs) -> Vec<bool> {
    let point_test = |inside: &dyn Fn([f64; 3]) -> bool| {
        let mut cells = vec![false; grid.len()];
        for k in 0..grid.n[2] {
            for j in 0..grid.n[1] {
                for i in 0..grid.n[0] {
                    cells[grid.cell(i, j, k)] = inside(grid.center(i, j, k));
                }
            }
        }
        cells
    };
    match solid {
        Solid::Box { size, center } => point_test(&|p| {
            (0..3).all(|i| (p[i] - center[i]).abs() <= 0.5 * size[i])
        }),
        Solid::Cylinder { radius, height, center, axis } => point_test(&|p| {
            let (r, z) = to_rz(p, *center, *axis);
            r <= *radius && z.abs() <= 0.5 * height
        }),
        Solid::Tube { outer_radius, inner_radius, height, center, axis } => point_test(&|p| {
            let (r, z) = to_rz(p, *center, *axis);
            r <= *outer_radius && r >= *inner_radius && z.abs() <= 0.5 * height
        }),
        Solid::Sphere { radius, center } => {
            point_test(&|p| dist(p, *center) <= *radius)
        }
        Solid::Shell { outer_radius, inner_radius, center } => point_test(&|p| {
            let d = dist(p, *center);
            d <= *outer_radius && d >= *inner_radius
        }),
        Solid::Revolve { profile, thickness, center, axis, .. } => {
            let profile = Profile::new(profile, *thickness);
            point_test(&|p| {
                let (r, z) = to_rz(p, *center, *axis);
                profile.contains(r, z)
            })
        }
        Solid::Union(parts) => {
            let mut cells = vec![false; grid.len()];
            for part in parts {
                for (cell, inside) in cells.iter_mut().zip(occupancy(part, grid, objs)) {
                    *cell |= inside;
                }
            }
            cells
        }
        Solid::Difference(parts) => {
            let mut cells = occupancy(&parts[0], grid, objs);
            for part in &parts[1..] {
                for (cell, inside) in cells.iter_mut().zip(occupancy(part, grid, objs)) {
                    *cell &= !inside;
                }
            }
            cells
        }
        Solid::Obj { .. } => {
            let mesh = &objs[&(solid as *const Solid)];
            let mut cells = vec![false; grid.len()];
            for k in 0..grid.n[2] {
                for j in 0..grid.n[1] {
                    let c = grid.center(0, j, k);
                    let hits = mesh.row_crossings(c[1], c[2]);
                    let mut crossed = 0;
                    for i in 0..grid.n[0] {
                        let x = grid.center(i, j, k)[0];
                        while crossed < hits.len() && hits[crossed] < x {
                            crossed += 1;
                        }
                        cells[grid.cell(i, j, k)] = crossed % 2 == 1;
                    }
                }
            }
            cells
        }
    }
}

fn voxel_mesh(spec: &SolidSpec, base_dir: &Path) -> Result<HexMesh, String> {
    let mut objs = Objs::new();
    load_objs(&spec.solid, base_dir, &mut objs)?;
    let b = bounds(&spec.solid, &objs);
    let extent = [0, 1, 2].map(|i| b.hi[i] - b.lo[i]);
    let largest = extent.iter().cloned().fold(0.0, f64::max);
    if !(largest > 0.0) {
        return Err("solid has an empty bounding box".into());
    }
    let h = element_size(spec.resolution, largest);
    // Cells per axis fit the bounding box exactly (cells are boxes of about
    // h on a side), so a box primitive keeps its exact dimensions.
    let n = extent.map(|e| ((e / h).round() as usize).max(1));
    if n.iter().product::<usize>() > 8_000_000 {
        return Err(format!("voxel grid {}x{}x{} is too large; coarsen the resolution", n[0], n[1], n[2]));
    }
    let grid = Grid { lo: b.lo, h: [0, 1, 2].map(|i| extent[i] / n[i] as f64), n };
    let mut cells = occupancy(&spec.solid, &grid, &objs);
    let total = cells.iter().filter(|&&c| c).count();
    if total == 0 {
        return Err("the solid covers no voxel cells; refine the resolution".into());
    }
    keep_largest_face_component(&mut cells, &grid);
    let kept = cells.iter().filter(|&&c| c).count();

    let nn = [n[0] + 1, n[1] + 1, n[2] + 1];
    let grid_node = |i: usize, j: usize, k: usize| (k * nn[1] + j) * nn[0] + i;
    let mut node_index = vec![u32::MAX; nn[0] * nn[1] * nn[2]];
    let mut nodes = Vec::new();
    let mut elements = Vec::with_capacity(kept);
    for k in 0..n[2] {
        for j in 0..n[1] {
            for i in 0..n[0] {
                if !cells[grid.cell(i, j, k)] {
                    continue;
                }
                let corners = [
                    (i, j, k),
                    (i + 1, j, k),
                    (i + 1, j + 1, k),
                    (i, j + 1, k),
                    (i, j, k + 1),
                    (i + 1, j, k + 1),
                    (i + 1, j + 1, k + 1),
                    (i, j + 1, k + 1),
                ];
                elements.push(corners.map(|(a, b, c)| {
                    let slot = &mut node_index[grid_node(a, b, c)];
                    if *slot == u32::MAX {
                        *slot = nodes.len() as u32;
                        nodes.push([
                            grid.lo[0] + a as f64 * grid.h[0],
                            grid.lo[1] + b as f64 * grid.h[1],
                            grid.lo[2] + c as f64 * grid.h[2],
                        ]);
                    }
                    *slot
                }));
            }
        }
    }
    Ok(HexMesh { nodes, elements, dropped_cells: total - kept })
}

fn keep_largest_face_component(cells: &mut [bool], grid: &Grid) {
    let mut label = vec![u32::MAX; cells.len()];
    let mut sizes = Vec::new();
    let mut stack = Vec::new();
    let [nx, ny, nz] = grid.n;
    for start in 0..cells.len() {
        if !cells[start] || label[start] != u32::MAX {
            continue;
        }
        let id = sizes.len() as u32;
        let mut size = 0;
        label[start] = id;
        stack.push(start);
        while let Some(c) = stack.pop() {
            size += 1;
            let (i, j, k) = (c % nx, (c / nx) % ny, c / (nx * ny));
            let mut visit = |ii: usize, jj: usize, kk: usize| {
                let d = grid.cell(ii, jj, kk);
                if cells[d] && label[d] == u32::MAX {
                    label[d] = id;
                    stack.push(d);
                }
            };
            if i > 0 {
                visit(i - 1, j, k);
            }
            if i + 1 < nx {
                visit(i + 1, j, k);
            }
            if j > 0 {
                visit(i, j - 1, k);
            }
            if j + 1 < ny {
                visit(i, j + 1, k);
            }
            if k > 0 {
                visit(i, j, k - 1);
            }
            if k + 1 < nz {
                visit(i, j, k + 1);
            }
        }
        sizes.push(size);
    }
    let largest = (0..sizes.len()).max_by_key(|&i| sizes[i]).unwrap_or(0) as u32;
    for (cell, l) in cells.iter_mut().zip(label) {
        *cell = *cell && l == largest;
    }
}

// ---------------------------------------------------------------------------
// Mapped solids of revolution

/// A body-fitted mesh of a revolved wall: `along` elements following the
/// profile's arc length, `through` across the wall, `around` the axis.
/// Nodes on the axis are shared by every angle, which turns the elements
/// touching the axis into wedges (a collapsed hex face, still a valid hex8).
fn revolve_mesh(
    profile: &Profile,
    center: [f64; 3],
    axis: Axis,
    segments: Segments,
    h: f64,
) -> Result<HexMesh, String> {
    let walls = profile.walls.as_ref().expect("mapped profiles have walls");
    // Arc length along the mid-surface.
    let mut arc = vec![0.0];
    for w in profile.points.windows(2) {
        let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
        arc.push(arc.last().unwrap() + d);
    }
    let length = *arc.last().unwrap();
    if !(length > 0.0) {
        return Err("revolve profile has zero length".into());
    }
    let extent = profile.extent();
    let wall_max = walls.iter().cloned().fold(0.0, f64::max);
    let along = segments.along.unwrap_or(((length / h).ceil() as usize).max(2));
    let through = segments.through.unwrap_or(((wall_max / h).round() as usize).max(1));
    let around = segments.around.unwrap_or_else(|| {
        let n = (std::f64::consts::TAU * extent[1] / h).ceil() as usize;
        (n.div_ceil(4) * 4).max(8)
    });
    if along == 0 || through == 0 || around < 3 {
        return Err("revolve segments must be positive (around >= 3)".into());
    }

    // Resample the profile at uniform arc length (mid point and wall); the
    // wall normal comes from neighboring samples in the ring loop below.
    let sample = |s: f64| -> ([f64; 2], f64) {
        let i = arc.partition_point(|&a| a <= s).clamp(1, arc.len() - 1);
        let span = arc[i] - arc[i - 1];
        let t = if span > 0.0 { ((s - arc[i - 1]) / span).clamp(0.0, 1.0) } else { 0.0 };
        let (a, b) = (profile.points[i - 1], profile.points[i]);
        ([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])], walls[i - 1] + t * (walls[i] - walls[i - 1]))
    };
    let mids: Vec<([f64; 2], f64)> =
        (0..=along).map(|s| sample(length * s as f64 / along as f64)).collect();
    let scale = extent[1].max(extent[3] - extent[2]);
    let on_axis = 1e-9 * scale;
    let mut ring: Vec<[f64; 2]> = Vec::with_capacity((along + 1) * (through + 1));
    for s in 0..=along {
        let prev = mids[s.saturating_sub(1)].0;
        let next = mids[(s + 1).min(along)].0;
        let tangent = [next[0] - prev[0], next[1] - prev[1]];
        let norm = (tangent[0] * tangent[0] + tangent[1] * tangent[1]).sqrt();
        let normal = [tangent[1] / norm, -tangent[0] / norm];
        let (mid, wall) = mids[s];
        for l in 0..=through {
            let offset = wall * (l as f64 / through as f64 - 0.5);
            let r = mid[0] + normal[0] * offset;
            ring.push([if r < on_axis { 0.0 } else { r }, mid[1] + normal[1] * offset]);
        }
    }

    let mut nodes = Vec::new();
    let mut index = vec![0u32; ring.len() * around];
    for (q, &[r, z]) in ring.iter().enumerate() {
        if r == 0.0 {
            let id = nodes.len() as u32;
            nodes.push(from_polar(0.0, 0.0, z, center, axis));
            index[q * around..(q + 1) * around].fill(id);
        } else {
            for a in 0..around {
                index[q * around + a] = nodes.len() as u32;
                let theta = std::f64::consts::TAU * a as f64 / around as f64;
                nodes.push(from_polar(r, theta, z, center, axis));
            }
        }
    }
    let node = |s: usize, l: usize, a: usize| index[(s * (through + 1) + l) * around + a % around];
    let mut elements = Vec::with_capacity(along * through * around);
    for s in 0..along {
        for l in 0..through {
            for a in 0..around {
                let mut e = [
                    node(s, l, a),
                    node(s + 1, l, a),
                    node(s + 1, l + 1, a),
                    node(s, l + 1, a),
                    node(s, l, a + 1),
                    node(s + 1, l, a + 1),
                    node(s + 1, l + 1, a + 1),
                    node(s, l + 1, a + 1),
                ];
                let mut distinct = e.to_vec();
                distinct.sort_unstable();
                distinct.dedup();
                if distinct.len() < 6 {
                    continue;
                }
                let corners = e.map(|n| nodes[n as usize]);
                let det = super::element::center_jacobian_det(&corners);
                if det.abs() <= 1e-12 * h * h * h {
                    continue;
                }
                if det < 0.0 {
                    e = [e[4], e[5], e[6], e[7], e[0], e[1], e[2], e[3]];
                }
                elements.push(e);
            }
        }
    }
    if elements.is_empty() {
        return Err("revolve produced no elements".into());
    }
    Ok(HexMesh { nodes, elements, dropped_cells: 0 })
}

// ---------------------------------------------------------------------------
// Surface

pub(crate) struct Surface {
    /// Mesh node index of each surface point.
    pub nodes: Vec<u32>,
    pub normals: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

/// Exterior faces (faces belonging to exactly one element) as outward
/// triangles over a compact point set, with area-weighted vertex normals.
pub(crate) fn surface(mesh: &HexMesh) -> Surface {
    let mut faces: HashMap<[u32; 4], (u32, u8, u32)> = HashMap::new();
    for (e, element) in mesh.elements.iter().enumerate() {
        for (f, face) in HEX_FACES.iter().enumerate() {
            let mut key = face.map(|c| element[c]);
            key.sort_unstable();
            faces.entry(key).and_modify(|v| v.2 += 1).or_insert((e as u32, f as u8, 1));
        }
    }
    let mut exterior: Vec<(u32, u8)> =
        faces.values().filter(|v| v.2 == 1).map(|v| (v.0, v.1)).collect();
    exterior.sort_unstable();

    let mut point_of = vec![u32::MAX; mesh.nodes.len()];
    let mut nodes = Vec::new();
    let mut triangles = Vec::new();
    for (e, f) in exterior {
        let element = &mesh.elements[e as usize];
        let quad = HEX_FACES[f as usize].map(|c| element[c]);
        let p = quad.map(|n| mesh.nodes[n as usize]);
        let centroid = average(&element.map(|n| mesh.nodes[n as usize]));
        let face_center = average(&p);
        let normal = cross(sub(p[2], p[0]), sub(p[3], p[1]));
        let outward = dot(normal, sub(face_center, centroid)) >= 0.0;
        let tris = if outward {
            [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]]
        } else {
            [[quad[0], quad[2], quad[1]], [quad[0], quad[3], quad[2]]]
        };
        for tri in tris {
            if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
                continue;
            }
            triangles.push(tri.map(|n| {
                let slot = &mut point_of[n as usize];
                if *slot == u32::MAX {
                    *slot = nodes.len() as u32;
                    nodes.push(n);
                }
                *slot
            }));
        }
    }
    let mut normals = vec![[0.0; 3]; nodes.len()];
    for tri in &triangles {
        let p = tri.map(|i| mesh.nodes[nodes[i as usize] as usize]);
        let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        for &i in tri {
            let acc = &mut normals[i as usize];
            for c in 0..3 {
                acc[c] += n[c];
            }
        }
    }
    for n in &mut normals {
        let len = dot(*n, *n).sqrt();
        if len > 0.0 {
            *n = n.map(|c| c / len);
        }
    }
    Surface { nodes, normals, triangles }
}

fn average<const N: usize>(p: &[[f64; 3]; N]) -> [f64; 3] {
    let mut c = [0.0; 3];
    for q in p {
        for i in 0..3 {
            c[i] += q[i] / N as f64;
        }
    }
    c
}

pub(crate) fn bounding_box(points: &[[f64; 3]]) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in points {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    (lo, hi)
}

pub(crate) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub(crate) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
