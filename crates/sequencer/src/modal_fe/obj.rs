//! Wavefront .obj import and inside testing for closed triangle meshes.
//!
//! Voxelization casts one ray per grid row along +x and counts crossings
//! (ray parity). The 2-D point-in-triangle test on the (y, z) projection is
//! made exact on shared edges and vertices by simulation of simplicity: the
//! ray origin is symbolically perturbed by (δ, δ²), so a ray through an edge
//! or vertex counts exactly one of the triangles around it. Cube faces split
//! along a diagonal therefore voxelize identically to the box primitive.

use std::path::Path;

pub(crate) struct TriMesh {
    pub vertices: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

impl TriMesh {
    pub fn load(path: &Path, scale: f64, offset: [f64; 3]) -> Result<Self, String> {
        let text =
            std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut mesh = parse_obj(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        for v in &mut mesh.vertices {
            for i in 0..3 {
                v[i] = v[i] * scale + offset[i];
            }
        }
        Ok(mesh)
    }

    /// Sorted x coordinates where the +x ray through (y, z) crosses the
    /// surface. Parity of crossings left of a point says whether it is inside.
    pub fn row_crossings(&self, y: f64, z: f64) -> Vec<f64> {
        let mut hits = Vec::new();
        for tri in &self.triangles {
            let [a, b, c] = tri.map(|i| self.vertices[i as usize]);
            let pa = [a[1], a[2]];
            let pb = [b[1], b[2]];
            let pc = [c[1], c[2]];
            let area = (pb[0] - pa[0]) * (pc[1] - pa[1]) - (pb[1] - pa[1]) * (pc[0] - pa[0]);
            if area == 0.0 {
                continue;
            }
            let p = [y, z];
            let e0 = edge_sign(pb, pc, p);
            let e1 = edge_sign(pc, pa, p);
            let e2 = edge_sign(pa, pb, p);
            let s = area.signum();
            if e0.0 != s || e1.0 != s || e2.0 != s {
                continue;
            }
            // Barycentric weights from the exact edge values (zero on an
            // edge is fine: the perturbation only decided ownership).
            let (w0, w1, w2) = (e0.1 / area, e1.1 / area, e2.1 / area);
            hits.push(w0 * a[0] + w1 * b[0] + w2 * c[0]);
        }
        hits.sort_by(|a, b| a.total_cmp(b));
        hits
    }
}

/// Sign (±1) and value of the 2-D edge function of p against the directed
/// edge a → b, with ties broken by perturbing p by (δ, δ²). The value is
/// computed on the lexicographically ordered edge so both triangles sharing
/// an edge see exactly opposite numbers.
fn edge_sign(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> (f64, f64) {
    let flip = (a[0], a[1]) > (b[0], b[1]);
    let (a, b) = if flip { (b, a) } else { (a, b) };
    let value = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let sign = if value != 0.0 {
        value.signum()
    } else if b[1] != a[1] {
        // d/dδ of the edge function: −(b_v − a_v).
        -(b[1] - a[1]).signum()
    } else {
        (b[0] - a[0]).signum()
    };
    if flip {
        (-sign, -value)
    } else {
        (sign, value)
    }
}

/// Vertices and faces of an .obj (faces fan-triangulated; texture / normal
/// indices and every other statement ignored).
fn parse_obj(text: &str) -> Result<TriMesh, String> {
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("v") => {
                let mut v = [0.0; 3];
                for value in &mut v {
                    *value = words
                        .next()
                        .and_then(|w| w.parse().ok())
                        .ok_or_else(|| format!("line {}: bad vertex", line_no + 1))?;
                }
                vertices.push(v);
            }
            Some("f") => {
                let mut face = Vec::new();
                for word in words {
                    let index: i64 = word
                        .split('/')
                        .next()
                        .and_then(|w| w.parse().ok())
                        .ok_or_else(|| format!("line {}: bad face index", line_no + 1))?;
                    let resolved = if index < 0 { vertices.len() as i64 + index } else { index - 1 };
                    if resolved < 0 || resolved >= vertices.len() as i64 {
                        return Err(format!("line {}: face index out of range", line_no + 1));
                    }
                    face.push(resolved as u32);
                }
                for i in 1..face.len().saturating_sub(1) {
                    triangles.push([face[0], face[i], face[i + 1]]);
                }
            }
            _ => {}
        }
    }
    if triangles.is_empty() {
        return Err("no faces".into());
    }
    Ok(TriMesh { vertices, triangles })
}
