//! `modal-assembly-mesh`: a modal instrument's parts as one mesh, for the
//! shape lab's whole-instrument view. Each solid's surface is turned and
//! moved to where it sits; strings are drawn as rods between their
//! attachments and markers as small diamonds. Every vertex carries a tint
//! (the table's one column) the view's shader colours it by.

use std::path::Path;

use eseqlisp::widget_render::mesh_widget::{
    geometry, geometry_handle, load_geometry_file, replace_geometry, MeshGeometry,
};
use eseqlisp::vm::Value;

use crate::modal_fe::mesh::{cross, sub};

/// `(modal-assembly-mesh parts rods marks)`:
///   parts  (dict :src "x.modes.json" | :mesh-id n | :spec s :dir d
///          :at (x y z) :turn (rx ry rz) :tint t) per solid: its surface (a
///          solve's file, a registered mesh, or a spec meshed here), turned
///          by rx, ry then rz degrees about x, y, z, then moved by :at
///   rods   (dict :from (x y z) :to (x y z) :radius r :tint t) per rod
///   marks  (dict :at (x y z) :size s :tint t) per marker
/// The geometry handle of the whole (the previous assembly's mesh is
/// dropped), or (dict :error …).
pub const ASSEMBLY_NATIVE: &str = "modal-assembly-mesh";

thread_local! {
    static ASSEMBLY_GEOMETRY: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

pub(super) fn assembly_native(args: &[Value]) -> Result<Value, String> {
    let list = |k: usize| match args.get(k) {
        Some(Value::List(items)) => items.iter().map(|item| item.borrow().clone()).collect(),
        _ => Vec::new(),
    };
    let mut mesh = Builder::default();
    let mut errors = Vec::new();
    for part in list(0) {
        if let Err(error) = mesh.add_part(&part) {
            errors.push(error);
        }
    }
    for rod in list(1) {
        let (Some(from), Some(to)) = (xyz(&part_field(&rod, "from")), xyz(&part_field(&rod, "to"))) else {
            continue;
        };
        mesh.add_rod(from, to, number(&rod, "radius").unwrap_or(0.0015), number(&rod, "tint").unwrap_or(0.0));
    }
    for mark in list(2) {
        let Some(at) = xyz(&part_field(&mark, "at")) else { continue };
        mesh.add_diamond(at, number(&mark, "size").unwrap_or(0.006), number(&mark, "tint").unwrap_or(0.0));
    }
    if mesh.indices.is_empty() {
        let error = errors.into_iter().next().unwrap_or_else(|| "nothing to draw".to_string());
        return Ok(error_value(error));
    }
    // too many vertices, or a part placed at a non-finite :at or :turn
    let geometry = match MeshGeometry::new(mesh.positions, Some(mesh.normals), mesh.indices, mesh.tints, 1, Vec::new(), None) {
        Ok(geometry) => geometry,
        Err(error) => return Ok(error_value(error)),
    };
    let id = replace_geometry(ASSEMBLY_GEOMETRY.get(), geometry);
    ASSEMBLY_GEOMETRY.set(Some(id));
    Ok(geometry_handle(id))
}

#[derive(Default)]
struct Builder {
    positions: Vec<f32>,
    normals: Vec<f32>,
    indices: Vec<u32>,
    tints: Vec<f32>,
}

impl Builder {
    fn add_part(&mut self, part: &Value) -> Result<(), String> {
        let at = xyz(&part_field(part, "at")).unwrap_or([0.0; 3]);
        let turn = rotation(xyz(&part_field(part, "turn")).unwrap_or([0.0; 3]));
        let tint = number(part, "tint").unwrap_or(0.0) as f32;
        let mut add = |positions: &[f32], normals: &[f32], indices: &[u32]| {
            let base = self.vertex_count();
            for (p, n) in positions.chunks_exact(3).zip(normals.chunks_exact(3)) {
                let p = apply(&turn, [p[0] as f64, p[1] as f64, p[2] as f64]);
                let n = apply(&turn, [n[0] as f64, n[1] as f64, n[2] as f64]);
                self.vertex([p[0] + at[0], p[1] + at[1], p[2] + at[2]], n, tint);
            }
            self.indices.extend(indices.iter().map(|&i| base + i));
        };
        let id = match (part_field(part, "src"), number(part, "mesh-id"), part_field(part, "spec")) {
            (Value::String(src), _, _) => load_geometry_file(Path::new(&src))?,
            (_, Some(id), _) => id as u64,
            // an unsolved solid: meshed here, not solved
            (_, _, spec @ Value::Map(_)) => {
                let dir = match part_field(part, "dir") {
                    Value::String(dir) => std::path::PathBuf::from(dir),
                    _ => std::path::PathBuf::from("."),
                };
                let (spec, base) = super::modal_patch::solid_spec(&spec, &dir)?;
                let (solid, _) = crate::modal_fe::preview(&spec, &base)?;
                add(&solid.points, &solid.normals, &solid.triangles);
                return Ok(());
            }
            _ => return Err("a part needs :src, :mesh-id or :spec".to_string()),
        };
        let source = geometry(id).ok_or_else(|| format!("no mesh {id}"))?;
        add(&source.positions, &source.normals, &source.indices);
        Ok(())
    }

    /// An eight-sided prism from `from` to `to`, its ends closed.
    fn add_rod(&mut self, from: [f64; 3], to: [f64; 3], radius: f64, tint: f64) {
        let axis = sub(to, from);
        if norm(axis) < 1e-9 {
            return;
        }
        let (u, v) = perpendiculars(axis);
        const SIDES: usize = 8;
        let ring = |k: usize| {
            let angle = std::f64::consts::TAU * k as f64 / SIDES as f64;
            let (s, c) = angle.sin_cos();
            [c * u[0] + s * v[0], c * u[1] + s * v[1], c * u[2] + s * v[2]]
        };
        let base = self.vertex_count();
        for k in 0..SIDES {
            let n = ring(k);
            for end in [from, to] {
                self.vertex([end[0] + radius * n[0], end[1] + radius * n[1], end[2] + radius * n[2]], n, tint as f32);
            }
        }
        for k in 0..SIDES as u32 {
            let next = (k + 1) % SIDES as u32;
            let (a0, a1, b0, b1) = (base + 2 * k, base + 2 * k + 1, base + 2 * next, base + 2 * next + 1);
            self.indices.extend([a0, b0, b1, a0, b1, a1]);
        }
        // the end caps: a fan over each end's ring (offset 0 from, 1 to)
        for offset in [0u32, 1] {
            for k in 1..SIDES as u32 - 1 {
                self.indices.extend([base + offset, base + 2 * k + offset, base + 2 * (k + 1) + offset]);
            }
        }
    }

    /// An octahedron of half-size `size` at `at`, flat-shaded.
    fn add_diamond(&mut self, at: [f64; 3], size: f64, tint: f64) {
        let tips = [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]];
        for sx in [0, 1] {
            for sy in [2, 3] {
                for sz in [4, 5] {
                    let corners = [tips[sx], tips[sy], tips[sz]];
                    let n = [corners[0][0], corners[1][1], corners[2][2]];
                    let n = scale(n, 1.0 / 3f64.sqrt());
                    let base = self.vertex_count();
                    for c in corners {
                        self.vertex([at[0] + size * c[0], at[1] + size * c[1], at[2] + size * c[2]], n, tint as f32);
                    }
                    self.indices.extend([base, base + 1, base + 2]);
                }
            }
        }
    }

    fn vertex(&mut self, p: [f64; 3], n: [f64; 3], tint: f32) {
        self.positions.extend(p.map(|x| x as f32));
        self.normals.extend(n.map(|x| x as f32));
        self.tints.push(tint);
    }

    fn vertex_count(&self) -> u32 {
        (self.positions.len() / 3) as u32
    }
}

/// The rotation by rx, ry then rz degrees about the fixed x, y and z axes:
/// R = Rz · Ry · Rx.
fn rotation(degrees: [f64; 3]) -> [[f64; 3]; 3] {
    let [x, y, z] = degrees.map(f64::to_radians);
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ]
}

fn apply(r: &[[f64; 3]; 3], p: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| r[i][0] * p[0] + r[i][1] * p[1] + r[i][2] * p[2])
}

fn perpendiculars(axis: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let a = scale(axis, 1.0 / norm(axis));
    let helper = if a[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
    let u = cross(a, helper);
    let u = scale(u, 1.0 / norm(u));
    (u, cross(a, u))
}

fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    a.map(|x| x * k)
}

fn norm(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn part_field(value: &Value, key: &str) -> Value {
    match value {
        Value::Map(map) => map.get(key).map(|item| item.borrow().clone()).unwrap_or(Value::Nil),
        _ => Value::Nil,
    }
}

fn number(value: &Value, key: &str) -> Option<f64> {
    match part_field(value, key) {
        Value::Number(x) => Some(x),
        _ => None,
    }
}

fn xyz(value: &Value) -> Option<[f64; 3]> {
    let Value::List(items) = value else { return None };
    let n = |k: usize| match items.get(k).map(|item| item.borrow().clone()) {
        Some(Value::Number(x)) => Some(x),
        _ => None,
    };
    Some([n(0)?, n(1)?, n(2)?])
}

fn error_value(error: String) -> Value {
    super::modal_patch::data_dict(vec![("error", Value::String(error))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_are_x_then_y_then_z() {
        let close = |a: [f64; 3], b: [f64; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-12);
        // 90 about z takes x to y
        assert!(close(apply(&rotation([0.0, 0.0, 90.0]), [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
        // 90 about x (first) takes y to z, then 90 about z leaves z
        assert!(close(apply(&rotation([90.0, 0.0, 90.0]), [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]));
        // x first: z goes to -y, then z-turn takes -y to x
        assert!(close(apply(&rotation([90.0, 0.0, 90.0]), [0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]));
    }

    #[test]
    fn rods_and_marks_make_a_mesh_with_their_tints() {
        let mut mesh = Builder::default();
        mesh.add_rod([0.0; 3], [1.0, 0.0, 0.0], 0.01, 8.0);
        mesh.add_diamond([0.0, 0.0, 0.5], 0.02, 6.0);
        assert_eq!(mesh.vertex_count(), 16 + 24);
        assert_eq!(mesh.indices.len(), 3 * (16 + 2 * 6 + 8));
        assert!(mesh.tints[..16].iter().all(|&t| t == 8.0));
        assert!(mesh.tints[16..].iter().all(|&t| t == 6.0));
        let geometry =
            MeshGeometry::new(mesh.positions, Some(mesh.normals), mesh.indices, mesh.tints, 1, Vec::new(), None);
        assert!(geometry.is_ok());
    }

    /// A mesh the geometry rejects (here a marker at a NaN) is an error
    /// dict, as the doc says, not a raised error.
    #[test]
    fn a_rejected_mesh_is_an_error_dict() {
        let at = Value::List([f64::NAN, 0.0, 0.0].map(|x| std::rc::Rc::new(std::cell::RefCell::new(Value::Number(x)))).into());
        let mark = super::super::modal_patch::data_dict(vec![("at", at)]);
        let marks = Value::List(vec![std::rc::Rc::new(std::cell::RefCell::new(mark))]);
        let value = assembly_native(&[Value::List(Vec::new()), Value::List(Vec::new()), marks]).unwrap();
        assert!(matches!(part_field(&value, "error"), Value::String(_)), "{value:?}");
    }
}
