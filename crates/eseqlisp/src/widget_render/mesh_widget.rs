//! Mesh widgets: a `defwidget` with a `:vertex` body draws a triangle mesh
//! through its own vertex stage (docs/defwidget-vertex-stage.md).
//!
//! The 2-D renderer positions every vertex on the CPU and draws with
//! pass-through vertex shaders (crates/sequencer/docs/gpu-displacement-spec.md),
//! so a mesh does not join it. Instead an instance paints an ordinary image
//! primitive whose `src` names the instance ([`mesh_target_src`]), and leaves
//! a [`MeshDrawJob`] here. The backend renders the job into an offscreen
//! texture with a depth buffer before it composites that image, through the
//! same image pipeline as any `(image :src …)`.
//!
//! Geometry is uploaded once per [`MeshGeometry`] and keyed by its content
//! ([`register_geometry`]); a job carries only the geometry's id, the scalar
//! state uniforms and the array states (per-frame data, e.g. mode amplitudes).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{GpuImagePrimitive, GpuPrimitive, ImageFit, LayoutNode, WidgetViewport};
use crate::lang::sdf_codegen::{MeshArrayState, mesh_array_offsets};
use crate::vm::Value;

/// The `src` prefix of the image a mesh widget instance composites.
pub const MESH_TARGET_SRC_PREFIX: &str = "eseq-mesh-target:";
/// Vertices a mesh may have (positions and the table are per vertex).
pub const MAX_MESH_VERTICES: usize = 1 << 20;
/// Columns of the per-vertex table (modes of a mode-shape table).
pub const MAX_MESH_TABLE_COLS: usize = 256;

/// A triangle mesh with a per-vertex float table: for an "eseq-modes" file,
/// the mode shapes (`table[p * table_cols + k]` is mode k's displacement
/// along the normal at point p).
#[derive(Clone, Debug, PartialEq)]
pub struct MeshGeometry {
    /// Three floats per vertex, in the file's units (metres).
    pub positions: Vec<f32>,
    /// Three floats per vertex, unit length.
    pub normals: Vec<f32>,
    /// Three vertex indices per triangle.
    pub indices: Vec<u32>,
    pub table: Vec<f32>,
    pub table_cols: usize,
    /// One per table column when the source gave them (mode frequencies, Hz).
    pub freqs: Vec<f32>,
    pub mass: Option<f64>,
    /// Centre and radius of the bounding sphere around the bounding box.
    pub center: [f32; 3],
    pub radius: f32,
    /// The largest |table entry|: displacement 1 at the furthest point.
    pub table_peak: f32,
}

impl MeshGeometry {
    /// Check the parts against each other and derive the rest. Without
    /// `normals` each vertex gets the area-weighted normal of its triangles.
    pub fn new(
        positions: Vec<f32>,
        normals: Option<Vec<f32>>,
        indices: Vec<u32>,
        table: Vec<f32>,
        table_cols: usize,
        freqs: Vec<f32>,
        mass: Option<f64>,
    ) -> Result<Self, String> {
        if positions.len() % 3 != 0 || positions.is_empty() {
            return Err(format!("points: {} floats is not a whole number of xyz points", positions.len()));
        }
        let vertices = positions.len() / 3;
        if vertices > MAX_MESH_VERTICES {
            return Err(format!("{vertices} points, over the {MAX_MESH_VERTICES} a mesh may have"));
        }
        if indices.len() % 3 != 0 || indices.is_empty() {
            return Err(format!("triangles: {} indices is not a whole number of triangles", indices.len()));
        }
        if let Some(bad) = indices.iter().find(|&&index| index as usize >= vertices) {
            return Err(format!("triangles: index {bad} past the last of {vertices} points"));
        }
        if table_cols > MAX_MESH_TABLE_COLS {
            return Err(format!("{table_cols} table columns, over the {MAX_MESH_TABLE_COLS} a mesh may have"));
        }
        if table.len() != vertices * table_cols {
            return Err(format!(
                "shapes: {} floats, expected {vertices} points x {table_cols} modes = {}",
                table.len(),
                vertices * table_cols
            ));
        }
        if !freqs.is_empty() && freqs.len() != table_cols {
            return Err(format!("freqs: {} frequencies for {table_cols} modes", freqs.len()));
        }
        let normals = match normals {
            Some(normals) if normals.len() == positions.len() => normals,
            Some(normals) => {
                return Err(format!("normals: {} floats, expected {}", normals.len(), positions.len()));
            }
            None => vertex_normals(&positions, &indices),
        };
        if positions.iter().chain(&normals).chain(&table).any(|value| !value.is_finite()) {
            return Err("points, normals and shapes must be finite".into());
        }
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for point in positions.chunks_exact(3) {
            for axis in 0..3 {
                lo[axis] = lo[axis].min(point[axis]);
                hi[axis] = hi[axis].max(point[axis]);
            }
        }
        let center = [0, 1, 2].map(|axis| (lo[axis] + hi[axis]) * 0.5);
        let radius = positions
            .chunks_exact(3)
            .map(|p| ((p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2) + (p[2] - center[2]).powi(2)).sqrt())
            .fold(0.0_f32, f32::max)
            .max(1e-6);
        let table_peak = table.iter().fold(0.0_f32, |peak, value| peak.max(value.abs()));
        Ok(Self { positions, normals, indices, table, table_cols, freqs, mass, center, radius, table_peak })
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len() / 3
    }

    /// An "eseq-modes" version 1 file: `{"format":"eseq-modes","version":1,
    /// "freqs":[N],"points":[P*3],"normals":[P*3],"triangles":[T*3],
    /// "shapes":[P*N, point-major],"mass":kg,"spec":{…}}`.
    pub fn from_modes_json(text: &str) -> Result<Self, String> {
        let json: serde_json::Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
        if json.get("format").and_then(|format| format.as_str()) != Some("eseq-modes") {
            return Err("not an eseq-modes file (\"format\" must be \"eseq-modes\")".into());
        }
        match json.get("version").and_then(|version| version.as_u64()) {
            Some(1) => {}
            other => return Err(format!("eseq-modes version {other:?}: only version 1 is read")),
        }
        let floats = |key: &str| -> Result<Vec<f32>, String> {
            let Some(values) = json.get(key).and_then(|values| values.as_array()) else {
                return Err(format!("{key}: missing or not an array"));
            };
            values
                .iter()
                .map(|value| value.as_f64().map(|n| n as f32).ok_or_else(|| format!("{key}: not a number")))
                .collect()
        };
        let freqs = floats("freqs")?;
        let indices = triangle_indices(floats("triangles")?, "triangles")?;
        Self::new(
            floats("points")?,
            Some(floats("normals")?),
            indices,
            floats("shapes")?,
            freqs.len(),
            freqs,
            json.get("mass").and_then(|mass| mass.as_f64()),
        )
    }

    /// Geometry built in Lisp: a dict with the eseq-modes keys `:points`,
    /// `:triangles` and optionally `:normals`, `:shapes`, `:freqs` (or
    /// `:modes`, the table width, when there are no frequencies) and `:mass`.
    /// Each list may be flat or nested (`((x y z) …)`).
    pub fn from_value(value: &Value) -> Result<Self, String> {
        let Value::Map(map) = value else {
            return Err("geometry is a dict with :points and :triangles".into());
        };
        let floats = |key: &str| -> Result<Option<Vec<f32>>, String> {
            let Some(value) = map.get(key) else { return Ok(None) };
            let mut out = Vec::new();
            flatten_numbers(&value.borrow(), &mut out).map_err(|error| format!(":{key}: {error}"))?;
            Ok(Some(out))
        };
        let positions = floats("points")?.ok_or(":points is missing")?;
        let indices = triangle_indices(floats("triangles")?.ok_or(":triangles is missing")?, ":triangles")?;
        let table = floats("shapes")?.unwrap_or_default();
        let freqs = floats("freqs")?.unwrap_or_default();
        let vertices = positions.len() / 3;
        let table_cols = match map.get("modes").map(|value| value.borrow().clone()) {
            Some(Value::Number(n)) if n >= 0.0 => n as usize,
            _ if !freqs.is_empty() => freqs.len(),
            _ if vertices > 0 => table.len() / vertices,
            _ => 0,
        };
        let mass = match map.get("mass").map(|value| value.borrow().clone()) {
            Some(Value::Number(n)) => Some(n),
            _ => None,
        };
        Self::new(positions, floats("normals")?, indices, table, table_cols, freqs, mass)
    }

    fn content_id(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for values in [&self.positions, &self.normals, &self.table] {
            values.len().hash(&mut hasher);
            values.iter().for_each(|value| value.to_bits().hash(&mut hasher));
        }
        self.indices.hash(&mut hasher);
        self.table_cols.hash(&mut hasher);
        // The handle reports these, so two meshes differing only here differ.
        self.freqs.iter().for_each(|value| value.to_bits().hash(&mut hasher));
        self.mass.map(f64::to_bits).hash(&mut hasher);
        // Ids travel through Lisp as numbers: keep them exact in an f64.
        hasher.finish() & ((1 << 52) - 1)
    }
}

/// Triangle vertex indices from floats: whole and non-negative.
fn triangle_indices(values: Vec<f32>, key: &str) -> Result<Vec<u32>, String> {
    values
        .into_iter()
        .map(|index| {
            if index >= 0.0 && index.fract() == 0.0 {
                Ok(index as u32)
            } else {
                Err(format!("{key}: index {index}"))
            }
        })
        .collect()
}

fn flatten_numbers(value: &Value, out: &mut Vec<f32>) -> Result<(), String> {
    match value {
        Value::Number(n) => out.push(*n as f32),
        Value::List(items) => {
            for item in items {
                flatten_numbers(&item.borrow(), out)?;
            }
        }
        other => return Err(format!("{other:?} is not a number")),
    }
    Ok(())
}

fn vertex_normals(positions: &[f32], indices: &[u32]) -> Vec<f32> {
    let mut normals = vec![0.0_f32; positions.len()];
    let point = |index: u32| {
        let i = index as usize * 3;
        [positions[i], positions[i + 1], positions[i + 2]]
    };
    for triangle in indices.chunks_exact(3) {
        let [a, b, c] = [point(triangle[0]), point(triangle[1]), point(triangle[2])];
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        for &index in triangle {
            for axis in 0..3 {
                normals[index as usize * 3 + axis] += n[axis];
            }
        }
    }
    for normal in normals.chunks_exact_mut(3) {
        let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        if length > 0.0 {
            normal.iter_mut().for_each(|axis| *axis /= length);
        } else {
            normal.copy_from_slice(&[0.0, 0.0, 1.0]);
        }
    }
    normals
}

/// A mesh widget's compiled stages and its state layout.
#[derive(Clone, Debug)]
pub struct MeshWidgetDef {
    pub name: String,
    /// A complete module with `mesh_vert` and `mesh_frag` for the platform's
    /// backend (MSL on macOS).
    pub shader_source: String,
    /// The macro-expanded bodies, kept to recompile on a theme change.
    pub vertex_expr: crate::parser::Expression,
    pub fragment_expr: crate::parser::Expression,
    /// The scalar uniforms, in slot order (the widget's `state_uniforms`).
    pub state_uniforms: Vec<String>,
    pub arrays: Vec<MeshArrayState>,
    pub animates: bool,
}

/// The uniforms both stages read (`MeshUniforms` in the shader preambles).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeshUniforms {
    /// The scalar state floats, in slot order.
    pub state: [[f32; 4]; 4],
    /// `mesh-center` xyz and `mesh-radius`.
    pub center_radius: [f32; 4],
    /// `aspect`, `itime`, `orbit-yaw`, `orbit-pitch`.
    pub view: [f32; 4],
    /// `table-cols`, `table-peak`, `vertex-count`.
    pub table_info: [f32; 4],
    /// The offscreen target's size in pixels.
    pub target_size: [f32; 4],
    /// How many floats the instance gave each array state.
    pub array_len: [[f32; 4]; 2],
}

/// One instance's offscreen render, left by [`mesh_widget_primitives`] for
/// the backend.
#[derive(Clone, Debug)]
pub struct MeshDrawJob {
    pub widget_type: String,
    pub geometry_id: u64,
    pub width_px: u32,
    pub height_px: u32,
    pub uniforms: MeshUniforms,
    /// The array states, one after another, each padded to its length.
    pub arrays: Vec<f32>,
    pub clear_color: [f32; 4],
    /// Bumped on every new job: the backend re-renders when it changes.
    pub revision: u64,
    /// The widget animates: re-rendered every frame with that frame's itime.
    pub animates: bool,
}

impl MeshDrawJob {
    fn same_inputs(&self, other: &Self) -> bool {
        self.widget_type == other.widget_type
            && self.geometry_id == other.geometry_id
            && (self.width_px, self.height_px) == (other.width_px, other.height_px)
            && self.uniforms == other.uniforms
            && self.arrays == other.arrays
            && self.clear_color == other.clear_color
            && self.animates == other.animates
    }
}

// The uniform blocks are shaped by these limits.
const _: () = assert!(super::sdf_widget::MAX_SDF_STATE_UNIFORMS == 16);
const _: () = assert!(crate::lang::sdf_codegen::MAX_MESH_ARRAYS == 8);

#[derive(Clone, Copy, Default)]
struct Orbit {
    yaw: f32,
    pitch: f32,
    last: Option<(f32, f32)>,
}

static MESH_WIDGET_GENERATION: AtomicU64 = AtomicU64::new(1);
static MESH_JOB_REVISION: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static MESH_WIDGETS: RefCell<HashMap<String, Rc<MeshWidgetDef>>> = RefCell::new(HashMap::new());
    static GEOMETRIES: RefCell<HashMap<u64, Rc<MeshGeometry>>> = RefCell::new(HashMap::new());
    /// A file's path and modification time -> its geometry id.
    static GEOMETRY_FILES: RefCell<HashMap<PathBuf, (Option<std::time::SystemTime>, u64)>> =
        RefCell::new(HashMap::new());
    static MESH_JOBS: RefCell<HashMap<u64, Rc<MeshDrawJob>>> = RefCell::new(HashMap::new());
    static ORBITS: RefCell<HashMap<u64, Orbit>> = RefCell::new(HashMap::new());
    /// Geometry errors already reported, so a broken prop logs once.
    static REPORTED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

pub fn register_mesh_widget(def: MeshWidgetDef) {
    let name = def.name.clone();
    MESH_WIDGETS.with(|defs| defs.borrow_mut().insert(name, Rc::new(def)));
    MESH_WIDGET_GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// A `defwidget` redefined without `:vertex` is an ordinary widget again.
pub fn unregister_mesh_widget(name: &str) {
    if MESH_WIDGETS.with(|defs| defs.borrow_mut().remove(name)).is_some() {
        MESH_WIDGET_GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn mesh_widget_def(name: &str) -> Option<Rc<MeshWidgetDef>> {
    MESH_WIDGETS.with(|defs| defs.borrow().get(name).cloned())
}

pub fn mesh_widget_defs() -> Vec<Rc<MeshWidgetDef>> {
    MESH_WIDGETS.with(|defs| defs.borrow().values().cloned().collect())
}

pub fn mesh_widget_registry_generation() -> u64 {
    MESH_WIDGET_GENERATION.load(Ordering::Relaxed)
}

/// Keep `geometry` for the backend; the id is its content's, so the same
/// mesh registered twice (a reload, a re-run view) uploads once.
pub fn register_geometry(geometry: MeshGeometry) -> u64 {
    let id = geometry.content_id();
    GEOMETRIES.with(|geometries| {
        geometries.borrow_mut().entry(id).or_insert_with(|| Rc::new(geometry));
    });
    id
}

pub fn geometry(id: u64) -> Option<Rc<MeshGeometry>> {
    GEOMETRIES.with(|geometries| geometries.borrow().get(&id).cloned())
}

/// Load an eseq-modes file (once per modification), registering its geometry.
pub fn load_geometry_file(path: &Path) -> Result<u64, String> {
    let modified = std::fs::metadata(path)
        .map_err(|error| format!("{}: {error}", path.display()))?
        .modified()
        .ok();
    if let Some(id) = GEOMETRY_FILES.with(|files| {
        files.borrow().get(path).filter(|(when, _)| *when == modified).map(|(_, id)| *id)
    }) {
        return Ok(id);
    }
    let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let geometry = MeshGeometry::from_modes_json(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let id = register_geometry(geometry);
    let replaced = GEOMETRY_FILES.with(|files| files.borrow_mut().insert(path.to_path_buf(), (modified, id)));
    // An edited file's previous mesh goes: nothing else names its id.
    if let Some((_, old)) = replaced.filter(|(_, old)| *old != id) {
        GEOMETRIES.with(|geometries| geometries.borrow_mut().remove(&old));
    }
    Ok(id)
}

/// What Lisp holds for registered geometry: `(dict :mesh-id id :points P
/// :modes N :freqs (…) :mass kg :radius r)`.
pub fn geometry_handle(id: u64) -> Value {
    let Some(geometry) = geometry(id) else { return Value::Nil };
    let cell = |value: Value| Rc::new(RefCell::new(value));
    let mut map = HashMap::from([
        ("mesh-id".to_string(), cell(Value::Number(id as f64))),
        ("points".to_string(), cell(Value::Number(geometry.vertex_count() as f64))),
        ("modes".to_string(), cell(Value::Number(geometry.table_cols as f64))),
        ("radius".to_string(), cell(Value::Number(geometry.radius as f64))),
        (
            "freqs".to_string(),
            cell(Value::List(geometry.freqs.iter().map(|f| cell(Value::Number(*f as f64))).collect())),
        ),
    ]);
    if let Some(mass) = geometry.mass {
        map.insert("mass".to_string(), cell(Value::Number(mass)));
    }
    Value::Map(map)
}

/// The geometry an instance draws: its `:geometry` (a handle or a geometry
/// dict) or else its `:src` (an eseq-modes file).
fn instance_geometry(props: &HashMap<String, Value>) -> Result<Option<u64>, String> {
    if let Some(value) = props.get("geometry").filter(|value| !matches!(value, Value::Nil)) {
        if let Value::Map(map) = value
            && let Some(id) = map.get("mesh-id")
        {
            return match &*id.borrow() {
                Value::Number(id) if geometry(*id as u64).is_some() => Ok(Some(*id as u64)),
                _ => Err(":geometry names a mesh that is not loaded".into()),
            };
        }
        return MeshGeometry::from_value(value).map(|geometry| Some(register_geometry(geometry)));
    }
    match props.get("src") {
        Some(Value::String(path)) if !path.is_empty() => load_geometry_file(Path::new(path)).map(Some),
        _ => Ok(None),
    }
}

fn report_once(widget: &str, message: String) {
    let message = format!("{widget}: {message}");
    if REPORTED.with(|reported| reported.borrow_mut().insert(message.clone())) {
        eprintln!("{message}");
    }
}

pub fn mesh_target_src(widget_id: u64) -> String {
    format!("{MESH_TARGET_SRC_PREFIX}{widget_id}")
}

/// The instance whose offscreen target image `src` names, if it names one.
pub fn mesh_target_widget_id(src: &str) -> Option<u64> {
    src.strip_prefix(MESH_TARGET_SRC_PREFIX)?.parse().ok()
}

pub fn mesh_draw_job(widget_id: u64) -> Option<Rc<MeshDrawJob>> {
    MESH_JOBS.with(|jobs| jobs.borrow().get(&widget_id).cloned())
}

/// Drop the jobs of instances no longer drawn.
pub fn retain_mesh_draw_jobs(mut keep: impl FnMut(u64) -> bool) {
    MESH_JOBS.with(|jobs| jobs.borrow_mut().retain(|id, _| keep(*id)));
    ORBITS.with(|orbits| orbits.borrow_mut().retain(|id, _| keep(*id)));
}

/// Built-in drag orbit: a drag across the widget's width turns the mesh
/// half a turn (`orbit-yaw`), one down its height tilts it a quarter turn
/// (`orbit-pitch`). `sx`, `sy` are -1..1 across the widget. True when the
/// orbit moved.
pub fn mesh_orbit_pointer(widget_id: u64, phase: &str, sx: f32, sy: f32) -> bool {
    ORBITS.with(|orbits| {
        let mut orbits = orbits.borrow_mut();
        let orbit = orbits.entry(widget_id).or_default();
        match phase {
            "down" => orbit.last = Some((sx, sy)),
            "drag" => {
                let Some((x0, y0)) = orbit.last.replace((sx, sy)) else { return false };
                let (dx, dy) = (sx - x0, sy - y0);
                if dx == 0.0 && dy == 0.0 {
                    return false;
                }
                orbit.yaw += dx * std::f32::consts::FRAC_PI_2;
                let quarter = std::f32::consts::FRAC_PI_2;
                orbit.pitch = (orbit.pitch + dy * quarter * 0.5).clamp(-quarter * 2.0, quarter * 2.0);
                return true;
            }
            _ => orbit.last = None,
        }
        false
    })
}

fn orbit_of(widget_id: u64) -> (f32, f32) {
    ORBITS.with(|orbits| orbits.borrow().get(&widget_id).map_or((0.0, 0.0), |orbit| (orbit.yaw, orbit.pitch)))
}

/// An instance of mesh widget `def` (registered as the SDF widget `sdf`):
/// leave its draw job and paint the image that composites its target.
pub fn mesh_widget_primitives(
    def: &MeshWidgetDef,
    node: &LayoutNode,
    viewport: WidgetViewport,
    state: [[f32; 4]; 4],
) -> Vec<GpuPrimitive> {
    let px_w = (node.rect.width * viewport.cell_w).round();
    let px_h = (node.rect.height * viewport.cell_h).round();
    if !(px_w >= 1.0 && px_h >= 1.0) {
        return Vec::new();
    }
    let geometry_id = match instance_geometry(&node.props) {
        Ok(Some(id)) => id,
        Ok(None) => return Vec::new(),
        Err(error) => {
            report_once(&def.name, error);
            return Vec::new();
        }
    };
    let Some(geometry) = geometry(geometry_id) else { return Vec::new() };

    let mut uniforms = MeshUniforms { state, ..MeshUniforms::default() };
    let [cx, cy, cz] = geometry.center;
    uniforms.center_radius = [cx, cy, cz, geometry.radius];
    let (yaw, pitch) = orbit_of(node.widget_id);
    uniforms.view = [px_w / px_h, viewport.time_seconds, yaw, pitch];
    uniforms.table_info = [geometry.table_cols as f32, geometry.table_peak, geometry.vertex_count() as f32, 0.0];
    uniforms.target_size = [px_w, px_h, 0.0, 0.0];

    let offsets = mesh_array_offsets(&def.arrays);
    let mut arrays = vec![0.0_f32; def.arrays.iter().map(|array| array.len).sum()];
    let mut values = Vec::new();
    for (slot, (array, offset)) in def.arrays.iter().zip(offsets).enumerate() {
        values.clear();
        if let Some(value) = node.props.get(&array.name) {
            super::matrix::flatten_numbers(value, &mut values);
        }
        let given = values.len().min(array.len);
        arrays[offset..offset + given].copy_from_slice(&values[..given]);
        uniforms.array_len[slot / 4][slot % 4] = given as f32;
    }

    // Without a :background the target is clear, in the theme background's
    // colour: multisampled edges then blend toward what is usually behind.
    let clear_color = node
        .props
        .get("background")
        .and_then(crate::theme::parse_color_value)
        .map_or_else(|| {
            let bg = crate::theme::BG();
            [bg.r, bg.g, bg.b, 0.0]
        }, |color| color.to_rgba());
    let job = MeshDrawJob {
        widget_type: def.name.clone(),
        geometry_id,
        width_px: px_w as u32,
        height_px: px_h as u32,
        uniforms,
        arrays,
        clear_color,
        revision: 0,
        animates: def.animates,
    };
    // A rebuild with the same inputs keeps the revision: no re-render.
    MESH_JOBS.with(|jobs| {
        let mut jobs = jobs.borrow_mut();
        let mut job = job;
        match jobs.get(&node.widget_id) {
            Some(previous) if previous.same_inputs(&job) => {}
            _ => {
                job.revision = MESH_JOB_REVISION.fetch_add(1, Ordering::Relaxed);
                jobs.insert(node.widget_id, Rc::new(job));
            }
        }
    });

    vec![GpuPrimitive::Image(GpuImagePrimitive {
        widget_id: node.widget_id,
        rect: node.rect,
        src: mesh_target_src(node.widget_id),
        fit: ImageFit::Stretch,
        radius_px: super::ui_design_px(super::get_f32_prop(&node.props, "corner-radius", 0.0)),
        opacity: 1.0,
        rotation: 0.0,
        rotation_speed: 0.0,
        clip_circle: false,
    })]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit plate grid with simply supported sin·sin mode shapes, as the
    /// eseq-modes JSON a finite-element solver would write.
    pub(crate) fn plate_modes_json(nx: usize, ny: usize, modes: &[(usize, usize)]) -> String {
        let mut points = Vec::new();
        let mut shapes = Vec::new();
        for j in 0..=ny {
            for i in 0..=nx {
                let (u, v) = (i as f64 / nx as f64, j as f64 / ny as f64);
                points.extend([u - 0.5, v - 0.5, 0.0]);
                for &(m, n) in modes {
                    let pi = std::f64::consts::PI;
                    shapes.push((m as f64 * pi * u).sin() * (n as f64 * pi * v).sin());
                }
            }
        }
        let mut triangles = Vec::new();
        for j in 0..ny {
            for i in 0..nx {
                let a = j * (nx + 1) + i;
                let (b, c, d) = (a + 1, a + nx + 1, a + nx + 2);
                triangles.extend([a, b, d, a, d, c]);
            }
        }
        let normals: Vec<f64> = (0..points.len() / 3).flat_map(|_| [0.0, 0.0, 1.0]).collect();
        let freqs: Vec<f64> = modes.iter().map(|&(m, n)| 100.0 * (m * m + n * n) as f64).collect();
        serde_json::json!({
            "format": "eseq-modes", "version": 1, "freqs": freqs, "points": points,
            "normals": normals, "triangles": triangles, "shapes": shapes, "mass": 0.5,
            "spec": {"kind": "test-plate"},
        })
        .to_string()
    }

    #[test]
    fn eseq_modes_file_parses_into_geometry_with_a_mode_table() {
        let geometry = MeshGeometry::from_modes_json(&plate_modes_json(4, 3, &[(1, 1), (2, 1)])).unwrap();
        assert_eq!(geometry.vertex_count(), 20);
        assert_eq!(geometry.indices.len(), 4 * 3 * 6);
        assert_eq!(geometry.table_cols, 2);
        assert_eq!(geometry.freqs, vec![200.0, 500.0]);
        assert_eq!(geometry.mass, Some(0.5));
        assert!((geometry.radius - (0.5f32 * 0.5 + 0.5 * 0.5).sqrt()).abs() < 1e-5);
        // The grid samples the (1,1) peak at v = 1/3: sin(pi/2) sin(pi/3).
        assert!((geometry.table_peak - (std::f32::consts::PI / 3.0).sin()).abs() < 1e-5);
        // Point-major: point 6 is (i=1, j=1), mode 0 is sin(pi/4) sin(pi/3).
        let expected = (std::f32::consts::PI / 4.0).sin() * (std::f32::consts::PI / 3.0).sin();
        assert!((geometry.table[6 * 2] - expected).abs() < 1e-5);
    }

    #[test]
    fn malformed_modes_files_are_errors_naming_the_part() {
        let error = |text: &str| MeshGeometry::from_modes_json(text).unwrap_err();
        assert!(error(r#"{"format":"obj","version":1}"#).contains("not an eseq-modes file"));
        assert!(error(r#"{"format":"eseq-modes","version":2}"#).contains("version"));
        let mut json: serde_json::Value = serde_json::from_str(&plate_modes_json(2, 2, &[(1, 1)])).unwrap();
        json["shapes"].as_array_mut().unwrap().pop();
        assert!(error(&json.to_string()).contains("shapes"));
        json = serde_json::from_str(&plate_modes_json(2, 2, &[(1, 1)])).unwrap();
        json["triangles"][0] = serde_json::json!(99);
        assert!(error(&json.to_string()).contains("index 99"));
    }

    #[test]
    fn lisp_geometry_accepts_nested_lists_and_derives_normals() {
        let cell = |value: Value| Rc::new(RefCell::new(value));
        let list = |values: Vec<Value>| Value::List(values.into_iter().map(cell).collect());
        let point = |x: f64, y: f64| list(vec![Value::Number(x), Value::Number(y), Value::Number(0.0)]);
        let map = HashMap::from([
            ("points".to_string(), cell(list(vec![point(0.0, 0.0), point(1.0, 0.0), point(0.0, 1.0)]))),
            ("triangles".to_string(), cell(list([0.0, 1.0, 2.0].map(Value::Number).to_vec()))),
            ("shapes".to_string(), cell(list([0.0, 0.5, 1.0].map(Value::Number).to_vec()))),
        ]);
        let geometry = MeshGeometry::from_value(&Value::Map(map)).unwrap();
        assert_eq!(geometry.table_cols, 1);
        assert_eq!(&geometry.normals[..3], &[0.0, 0.0, 1.0]);
        // Same content, same id: a re-run view uploads nothing new.
        let id = register_geometry(geometry.clone());
        assert_eq!(register_geometry(geometry), id);
        assert!(matches!(geometry_handle(id), Value::Map(map) if map.contains_key("mesh-id")));
    }

    #[test]
    fn a_vertex_defwidget_instance_leaves_a_draw_job_and_paints_its_target() {
        let mut runtime = crate::runtime::Runtime::new();
        runtime.set_layout_viewport(40, 10);
        runtime
            .eval_str(
                "(defwidget test-mesh-face :width 10 :height 5 :state (scale (amps :floats 8))
                   :vertex (out :position (vec4 (+ position (* normal scale (dot amps (table-row vertex-id)))) 1.0)
                                :varyings ((d (amps 0))))
                   :shader (rgba d 0.5 0.5 1.0))",
            )
            .expect("defwidget");
        let def = mesh_widget_def("test-mesh-face").expect("a mesh widget");
        assert_eq!(def.state_uniforms, ["scale"]);
        assert_eq!(def.arrays, [MeshArrayState { name: "amps".into(), len: 8 }]);
        assert!(def.shader_source.contains("mesh_vert"));
        runtime
            .eval_str(
                "(effect (test-mesh-face :scale 0.5 :amps (list 1 2 3)
                   :geometry (dict :points (list 0 0 0 1 0 0 0 1 0) :triangles (list 0 1 2)
                                   :shapes (list 1 0 0))))",
            )
            .expect("render");
        let node = (**runtime.current_layout.as_ref().expect("layout")).clone();
        let viewport = WidgetViewport {
            vp_w: 400.0, vp_h: 200.0, cell_w: 10.0, cell_h: 20.0, scroll_top: 0.0,
            focused_widget_id: None, focused_branch: false, overlay_viewport_bottom: 10.0,
            inherited_hover: false, time_seconds: 0.0, scroll_left: 0.0,
        };
        let primitives = super::super::sdf_widget::sdf_widget_primitives(&node.widget_type, &node, viewport);
        let [GpuPrimitive::Image(image)] = primitives.as_slice() else {
            panic!("expected the target image");
        };
        assert_eq!(mesh_target_widget_id(&image.src), Some(node.widget_id));
        let job = mesh_draw_job(node.widget_id).expect("a draw job");
        assert_eq!((job.width_px, job.height_px), (100, 100));
        assert_eq!(job.uniforms.state[0][0], 0.5);
        assert_eq!(job.arrays, [1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(job.uniforms.array_len[0][0], 3.0);
        assert_eq!(job.uniforms.table_info[..3], [1.0, 1.0, 3.0]);

        // Array states need the vertex stage; a malformed entry is an error.
        assert!(runtime.eval_str("(defwidget flat-arrays :state ((a :floats 4)) :shader (rgba 1 1 1 1))").is_err());
        assert!(runtime.eval_str("(defwidget bad-arrays :state ((a :ints 4)) :vertex (out :position (vec4 position 1.0)) :shader (rgba 1 1 1 1))").is_err());
        // Redefined without :vertex it is an ordinary widget again.
        runtime.eval_str("(defwidget test-mesh-face :shader (rgba 1 1 1 1))").expect("redefine");
        assert!(mesh_widget_def("test-mesh-face").is_none());
    }

    #[test]
    fn drag_orbit_accumulates_between_down_and_up() {
        assert!(!mesh_orbit_pointer(7, "down", 0.0, 0.0));
        assert!(mesh_orbit_pointer(7, "drag", 1.0, 0.0));
        let (yaw, pitch) = orbit_of(7);
        assert!((yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6 && pitch == 0.0);
        assert!(!mesh_orbit_pointer(7, "up", 1.0, 0.0));
        assert!(!mesh_orbit_pointer(7, "drag", 0.0, 0.0), "no drag without a press");
        assert_eq!(mesh_target_widget_id(&mesh_target_src(7)), Some(7));
        assert_eq!(mesh_target_widget_id("/tmp/a.png"), None);
    }
}
