//! The natives `eseq.modal` (`content/ui/modal.lisp`) calls: modal data
//! files, circular plates, finite-element solids and nearest points. A patch
//! script is generated like any program (see [`super::generator`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use eseqlisp::vm::{Value, VM};

use super::generator::current_script_dir;

/// The native a patch script reads modal data with:
/// `(modal-read-data "bell.modes.json")` or `(modal-read-data "bell.txt")`.
pub const READ_DATA_NATIVE: &str = "modal-read-data";

/// Read a modal data file into the dict `eseq.modal/modal-data` takes. A
/// relative `path` is taken from the folder of the script being generated.
/// - `.json`: an "eseq-modes" v1 file (`eseq modal solve` output): `:freqs`,
///   `:shapes` (one list of N per point), `:points` / `:normals` (one
///   `(x y z)` per point), `:triangles` (flat point indices), `:mass`;
///   optionally `:losses` (1/s per mode) and an identified model's
///   `:measured`, `:strokes`, `:wash` and `:contact-s` (see
///   tools/pm-ride/export_modes.py).
/// - anything else: an IRCAM resonance model (modres text, one mode per
///   line: index, frequency Hz, amplitude dB, bandwidth Hz; `;` comments):
///   `:freqs`, `:amps` (linear), `:bandwidths`.
pub fn read_modal_data_file(path: &str) -> Result<Value, String> {
    let path = match current_script_dir() {
        Some(dir) if Path::new(path).is_relative() => dir.join(path),
        _ => PathBuf::from(path),
    };
    let text = std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let parsed = if path.extension().is_some_and(|ext| ext == "json") {
        modes_json_value(&text).map(|(value, mesh)| if mesh { with_file(value, &path) } else { value })
    } else {
        modres_value(&text)
    };
    parsed.map_err(|error| format!("{}: {error}", path.display()))
}

/// The native eseq.modal's circular plates are solved with:
/// `(modal-circ-plate-modes :free nu count samples)`.
pub const CIRC_PLATE_NATIVE: &str = "modal-circ-plate-modes";

/// The natives eseq.modal calls, by name, as (args) -> value or a
/// user-facing error. The CLI and tests register them on a bare VM
/// ([`register_modal_natives`]); the app on its runtime.
pub fn modal_natives() -> [(&'static str, fn(&[Value]) -> Result<Value, String>); 8] {
    [
        (NET_NATIVE, net_native),
        (READ_DATA_NATIVE, read_data_native),
        (CIRC_PLATE_NATIVE, circ_plate_native),
        (SOLVE_NATIVE, solve_native),
        (MESH_NATIVE, mesh_native),
        (NEAREST_NATIVE, nearest_native),
        (PICK_NATIVE, pick_native),
        (super::modal_assembly::ASSEMBLY_NATIVE, super::modal_assembly::assembly_native),
    ]
}

/// `(modal-mesh-pick mesh-id a b n u v)`: the surface point of a registered
/// mesh (a `modal-mesh` preview) under (u v) in the view plane whose across
/// and up axes are a and b (0 x, 1 y, 2 z), seen from +n: of the vertices
/// nearest the line through (u v) along n, the one furthest toward +n, as
/// (x y z); nil for an unknown mesh. What a click on the shape lab's canvas
/// places a named point on.
pub const PICK_NATIVE: &str = "modal-mesh-pick";

fn pick_native(args: &[Value]) -> Result<Value, String> {
    let number = |k: usize| match args.get(k) {
        Some(Value::Number(x)) => Some(*x),
        _ => None,
    };
    let (Some(id), Some(a), Some(b), Some(n), Some(u), Some(v)) =
        (number(0), number(1), number(2), number(3), number(4), number(5))
    else {
        return Err(format!("{PICK_NATIVE} takes a mesh id, axes a b n and a point u v"));
    };
    let (a, b, n) = (a as usize % 3, b as usize % 3, n as usize % 3);
    let Some(geometry) = eseqlisp::widget_render::mesh_widget::geometry(id as u64) else {
        return Ok(Value::Nil);
    };
    let vertices: Vec<[f64; 3]> =
        geometry.positions.chunks_exact(3).map(|p| [p[0] as f64, p[1] as f64, p[2] as f64]).collect();
    let off_line = |p: &[f64; 3]| ((p[a] - u).powi(2) + (p[b] - v).powi(2)).sqrt();
    let Some(nearest) = vertices.iter().map(off_line).min_by(f64::total_cmp) else {
        return Ok(Value::Nil);
    };
    // the vertices about as near the line as the nearest (within 2% of the
    // mesh), so the front face wins over the back one
    let reach = nearest + 0.02 * geometry.radius as f64;
    // none is within reach only when (u v) is not finite
    let Some(front) = vertices.iter().filter(|p| off_line(p) <= reach).max_by(|p, q| p[n].total_cmp(&q[n])) else {
        return Ok(Value::Nil);
    };
    Ok(Value::List(front.iter().map(|&x| cell(Value::Number(x))).collect()))
}

/// The native a tensioned network (eseq.modal's net and web) is solved
/// with: `(modal-net-solve (dict :points ((x y z) ...) :masses (kg ...)
/// :edges ((i j stiffness) ...) :fixed (i ...) :modes n))`, into the dict
/// `modal-read-data` returns (freqs, shapes as a row per node, points, mass
/// 1), each shape at eseq.modal's unit modal mass. See
/// [`crate::modal_fe::net`].
pub const NET_NATIVE: &str = "modal-net-solve";

fn net_native(args: &[Value]) -> Result<Value, String> {
    let usage = || format!("{NET_NATIVE} takes (dict :points :masses :edges :fixed :modes)");
    let Some(Value::Map(spec)) = args.first() else { return Err(usage()) };
    let field = |key: &str| spec.get(key).map(|v| v.borrow().clone());
    let list = |value: Option<Value>| -> Vec<Value> {
        match value {
            Some(Value::List(items)) => items.iter().map(|item| item.borrow().clone()).collect(),
            _ => Vec::new(),
        }
    };
    let number = |value: &Value| match value {
        Value::Number(x) => Some(*x),
        _ => None,
    };
    let numbers_of = |value: &Value| -> Option<Vec<f64>> {
        match value {
            Value::List(items) => items.iter().map(|item| number(&item.borrow())).collect(),
            _ => None,
        }
    };
    let points: Vec<Vec<f64>> = list(field("points"))
        .iter()
        .map(|p| numbers_of(p).filter(|xyz| xyz.len() == 3).ok_or_else(|| "a net's points are (x y z) lists".to_string()))
        .collect::<Result<_, _>>()?;
    let n = points.len();
    let masses: Vec<f64> = list(field("masses")).iter().map(|m| number(m).ok_or_else(usage)).collect::<Result<_, _>>()?;
    if masses.len() != n {
        return Err(format!("a net needs one mass per point ({n} points, {} masses)", masses.len()));
    }
    let edges = list(field("edges"))
        .iter()
        .map(|e| match numbers_of(e).as_deref() {
            Some([i, j, s]) if *i >= 0.0 && *j >= 0.0 => Ok((*i as usize, *j as usize, *s)),
            _ => Err("a net's edges are (i j stiffness) lists".to_string()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut fixed = vec![false; n];
    for anchor in list(field("fixed")) {
        let i = number(&anchor).ok_or_else(usage)? as usize;
        *fixed.get_mut(i).ok_or_else(|| format!("a net's anchor {i} names a node it does not have ({n} nodes)"))? = true;
    }
    let count = field("modes").as_ref().and_then(number).unwrap_or(40.0).max(1.0) as usize;
    let net = crate::modal_fe::net::Net { masses, edges, fixed };
    let modes = crate::modal_fe::net::net_modes(&net, count)?;
    Ok(data_dict(vec![
        ("freqs", numbers(modes.freqs.iter().copied())),
        ("shapes", rows(&modes.shapes, modes.freqs.len())),
        ("points", Value::List(points.into_iter().map(|p| cell(numbers(p))).collect())),
        ("mass", Value::Number(1.0)),
    ]))
}

/// `(modal-nearest-point points (x y z))`: the index of the nearest of a
/// list of (x y z) points (a data object's access by position).
pub const NEAREST_NATIVE: &str = "modal-nearest-point";

fn nearest_native(args: &[Value]) -> Result<Value, String> {
    let xyz = |value: &Value| -> Option<[f64; 3]> {
        let Value::List(items) = value else { return None };
        let n = |k: usize| match items.get(k).map(|v| v.borrow().clone()) {
            Some(Value::Number(x)) => Some(x),
            _ => None,
        };
        Some([n(0)?, n(1)?, n(2)?])
    };
    let (Some(Value::List(points)), Some(at)) = (args.first(), args.get(1).and_then(xyz)) else {
        return Err(format!("{NEAREST_NATIVE} takes a list of (x y z) points and an (x y z)"));
    };
    let distance = |p: [f64; 3]| (0..3).map(|i| (p[i] - at[i]).powi(2)).sum::<f64>();
    let best = points
        .iter()
        .enumerate()
        .filter_map(|(k, point)| xyz(&point.borrow()).map(|p| (k, distance(p))))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(0, |(k, _)| k);
    Ok(Value::Number(best as f64))
}

/// The native a finite-element solid is solved with:
/// `(modal-solve "bell.json")` or `(modal-solve (dict :solid ... :material ...))`.
pub const SOLVE_NATIVE: &str = "modal-solve";

/// Solve a finite-element solid spec (see `crate::modal_fe::spec`) given as
/// a JSON file path (relative to the script) or a Lisp dict (keys as in the
/// JSON, `-` read as `_`; the material as `:made-of`, any eseq.modal
/// material, since a call quotes whatever follows `:material`), into the
/// dict `modal-read-data` returns for an
/// eseq-modes file, with each mode's shape scaled to eseq.modal's unit modal
/// mass (mean square 1/2 over the surface points; the solver's are
/// mass-normalised in kg). Solutions are cached ([`solve_solid_cached`]), so only
/// the first evaluation of a solid pays for the solve; the app never pays it
/// on its event loop ([`deferring_solves`]).
fn solve_native(args: &[Value]) -> Result<Value, String> {
    let script_dir = current_script_dir().unwrap_or_else(|| PathBuf::from("."));
    let (spec, base) = solid_spec(args.first().unwrap_or(&Value::Nil), &script_dir)?;
    // the cache check's parse is the value: a hit parses the file once
    let (cache, (value, _)) =
        solve_solid_cached_as(&spec, &base, &crate::modal_fe::SolveProgress::default(), modes_json_value)?;
    Ok(with_file(value, &cache))
}

/// A solid spec given to [`SOLVE_NATIVE`] (a JSON file path, relative to
/// `script_dir`, or a Lisp dict), and the folder its relative .obj paths
/// resolve against.
pub fn solid_spec(value: &Value, script_dir: &Path) -> Result<(crate::modal_fe::SolidSpec, PathBuf), String> {
    use crate::modal_fe::SolidSpec;
    match value {
        Value::String(path) => {
            let path = if Path::new(path).is_relative() { script_dir.join(path) } else { PathBuf::from(path) };
            let base = path.parent().map(Path::to_path_buf).unwrap_or_else(|| script_dir.to_path_buf());
            Ok((SolidSpec::load(&path)?, base))
        }
        Value::Map(_) => {
            let mut spec = lisp_to_json(value);
            // a solid's :grain (along, across) with its material's :wood
            // constants make the spec's orthotropic grain
            let grain = spec.as_object_mut().and_then(|spec| spec.remove("grain"));
            if let Some(material) = spec.as_object_mut().and_then(|spec| spec.remove("made_of")) {
                let pick = |key: &str| material.get(key).cloned().unwrap_or(serde_json::Value::Null);
                spec["material"] =
                    serde_json::json!({"density": pick("density"), "young": pick("young"), "poisson": pick("poisson")});
                if let Some(grain) = grain.filter(|grain| !grain.is_null()) {
                    let axes = match &grain {
                        serde_json::Value::Array(axes) => axes.clone(),
                        axis => vec![axis.clone()],
                    };
                    let along = axes.first().and_then(|a| a.as_str()).unwrap_or("x").trim_start_matches(':').to_string();
                    // across defaults to the next axis round (x -> y -> z -> x)
                    let next = match along.as_str() { "x" => "y", "y" => "z", _ => "x" };
                    let across = axes.get(1).and_then(|a| a.as_str()).map_or(next.to_string(), |a| a.trim_start_matches(':').to_string());
                    let wood = pick("wood");
                    if !wood.is_object() {
                        return Err(format!("a solid with :grain must be made of a wood (a material with :wood), got {material}"));
                    }
                    let mut grain = wood;
                    grain["along"] = serde_json::Value::String(along);
                    grain["across"] = serde_json::Value::String(across);
                    spec["material"]["grain"] = grain;
                }
            }
            let json = spec.to_string();
            Ok((SolidSpec::from_json(&json).map_err(|error| format!("{error} in {json}"))?, script_dir.to_path_buf()))
        }
        _ => Err(format!("{SOLVE_NATIVE} takes a spec file path or a spec dict")),
    }
}

/// Solve `spec` at eseq.modal's unit modal mass, or read it from the cache
/// [`SOLVE_NATIVE`] keeps: the cached eseq-modes file and its text. Safe on
/// any thread, so an editor can solve in the background into the same cache
/// a script's evaluation then reads, watching (or cancelling) it through
/// `progress`.
pub fn solve_solid_cached(
    spec: &crate::modal_fe::SolidSpec,
    base: &Path,
    progress: &crate::modal_fe::SolveProgress,
) -> Result<(PathBuf, String), String> {
    solve_solid_cached_as(spec, base, progress, |text| modes_json_value(text).map(|_| text.to_string()))
}

/// [`solve_solid_cached`], the file's text read by `parse`, which also
/// checks a cached file (one it rejects is solved again).
fn solve_solid_cached_as<T>(
    spec: &crate::modal_fe::SolidSpec,
    base: &Path,
    progress: &crate::modal_fe::SolveProgress,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<(PathBuf, T), String> {
    use crate::modal_fe::{solve_watched, spec_hash};
    // The key: the spec, the solver's version (a fix re-solves) and, for a
    // spec with .obj files, their contents and the folder their relative
    // paths resolve from (canonical). A spec without them is the same solid
    // wherever its script is (a relative or absolute path, a draft's copy).
    let spec_json = serde_json::to_value(spec).map_err(|error| error.to_string())?;
    let objs = obj_paths(&spec_json);
    let obj_hashes: String = objs
        .iter()
        .map(|path| crate::sample_manifest::sha256_hex(&std::fs::read(base.join(path)).unwrap_or_default()))
        .collect();
    let keyed = |dir: Option<String>| {
        let mut key = format!("{SOLVER_VERSION}\n{}\n", spec_hash(spec));
        if let Some(dir) = dir {
            key.push_str(&format!("{dir}\n"));
        }
        key.push_str(&obj_hashes);
        crate::app_paths::app_paths()
            .modal_solid_cache_dir()
            .join(format!("{}.unit.modes.json", crate::sample_manifest::sha256_hex(key.as_bytes())))
    };
    let canonical = cache_dir_key(base).display().to_string();
    let cache = keyed(if objs.is_empty() { None } else { Some(canonical.clone()) });
    // A cached file is already at unit modal mass; one that does not parse
    // is solved again.
    let read = |path: &Path| std::fs::read_to_string(path).ok().and_then(|text| parse(&text).ok());
    if let Some(parsed) = read(&cache) {
        return Ok((cache, parsed));
    }
    // a solve cached under an earlier key (keyed by the folder as given, or
    // canonical) is moved to this one, rather than solved again
    let mut legacy = vec![keyed(Some(base.display().to_string()))];
    if objs.is_empty() {
        legacy.push(keyed(Some(canonical)));
    }
    for old in legacy {
        if let Some(parsed) = read(&old) {
            if std::fs::rename(&old, &cache).is_ok() {
                return Ok((cache, parsed));
            }
            return Ok((old, parsed));
        }
    }
    if let Some(error) = defer_solve(spec, base, &cache) {
        return Err(error);
    }
    let solved = solve_watched(spec, base, progress);
    let mut failures = SOLVE_FAILURES.lock().unwrap_or_else(|e| e.into_inner());
    match &solved {
        Err(error) if !progress.is_cancelled() => {
            failures.insert(cache.clone(), error.clone());
        }
        _ => {
            failures.remove(&cache);
        }
    }
    drop(failures);
    let (mut solid, _) = solved?;
    unit_modal_mass(&mut solid);
    let text = solid.to_json(spec);
    if let Some(dir) = cache.parent() {
        let _ = std::fs::create_dir_all(dir);
        // write beside, then rename: a reader never sees half a file (the
        // thread id keeps two background solves apart)
        let staging = dir.join(format!(".{}-{:?}.tmp", std::process::id(), std::thread::current().id()));
        if std::fs::write(&staging, &text).is_ok() {
            let _ = std::fs::rename(&staging, &cache);
        }
    }
    Ok((cache, parse(&text)?))
}

thread_local! {
    /// The solves an evaluation under [`deferring_solves`] found uncached.
    static DEFERRED_SOLVES: RefCell<Option<Vec<DeferredSolve>>> = const { RefCell::new(None) };
}

/// Solves that failed (by cache file): a deferring evaluation answers with
/// the failure, as a solve on the spot would have, rather than solving
/// again.
static SOLVE_FAILURES: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, String>>> =
    std::sync::LazyLock::new(Default::default);

/// A solid a script asked [`SOLVE_NATIVE`] for that was not cached yet.
pub struct DeferredSolve {
    pub spec: crate::modal_fe::SolidSpec,
    pub base: PathBuf,
    /// The cache file it solves into: the solid's identity.
    pub cache: PathBuf,
}

/// Run `f` (a script's evaluation) with [`SOLVE_NATIVE`] refusing to solve:
/// a solid it finds uncached is recorded and fails the call, so the app's
/// event loop never runs a solve of minutes. The caller solves the recorded
/// solids in the background and evaluates again, now from the cache.
pub fn deferring_solves<T>(f: impl FnOnce() -> T) -> (T, Vec<DeferredSolve>) {
    let outer = DEFERRED_SOLVES.with(|deferred| deferred.borrow_mut().replace(Vec::new()));
    let value = f();
    let deferred = DEFERRED_SOLVES.with(|deferred| std::mem::replace(&mut *deferred.borrow_mut(), outer));
    (value, deferred.unwrap_or_default())
}

/// Under [`deferring_solves`], record an uncached solve and the error that
/// fails the call (or the error it failed with before); otherwise None
/// (solve now).
fn defer_solve(spec: &crate::modal_fe::SolidSpec, base: &Path, cache: &Path) -> Option<String> {
    DEFERRED_SOLVES.with(|deferred| {
        let mut deferred = deferred.borrow_mut();
        let deferred = deferred.as_mut()?;
        if let Some(error) = SOLVE_FAILURES.lock().unwrap_or_else(|e| e.into_inner()).get(cache) {
            return Some(error.clone());
        }
        if !deferred.iter().any(|other| other.cache == cache) {
            deferred.push(DeferredSolve { spec: spec.clone(), base: base.to_path_buf(), cache: cache.to_path_buf() });
        }
        Some(format!("{SOLVE_NATIVE}: solving this solid in the background"))
    })
}

/// A script folder as a solve's cache key names it: absolute and without
/// symlinks, else as given (a folder that does not exist).
fn cache_dir_key(base: &Path) -> PathBuf {
    base.canonicalize().unwrap_or_else(|_| base.to_path_buf())
}

/// The native a shape editor previews a solid with:
/// `(modal-mesh spec [dir])`, spec as for [`SOLVE_NATIVE`] (relative .obj
/// paths from `dir`), meshed but not solved. A mesh handle for
/// `mode-mesh-view :geometry` with the mesh's `:nodes`, `:elements` and
/// `:mass` (kg), and with a `:block`, the nodes it clamps (`:blocked`) and
/// the node nearest its centre (`:block-nearest`, (x y z)); or
/// `(dict :error message)`, as a shape being edited is often briefly
/// invalid. Each preview replaces the last one's geometry.
pub const MESH_NATIVE: &str = "modal-mesh";

thread_local! {
    static PREVIEW_GEOMETRY: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

fn mesh_native(args: &[Value]) -> Result<Value, String> {
    use eseqlisp::widget_render::mesh_widget::{geometry_handle, replace_geometry, MeshGeometry};
    let dir = match args.get(1) {
        Some(Value::String(dir)) => PathBuf::from(dir),
        _ => current_script_dir().unwrap_or_else(|| PathBuf::from(".")),
    };
    let preview = solid_spec(args.first().unwrap_or(&Value::Nil), &dir).and_then(|(spec, base)| {
        let (solid, stats) = crate::modal_fe::preview(&spec, &base)?;
        let points = solid.point_count();
        let geometry = MeshGeometry::new(
            solid.points,
            Some(solid.normals),
            solid.triangles,
            vec![0.0; points],
            1,
            Vec::new(),
            Some(solid.mass),
        )?;
        Ok((geometry, stats))
    });
    let (geometry, stats) = match preview {
        Ok(preview) => preview,
        Err(error) => return Ok(data_dict(vec![("error", Value::String(error))])),
    };
    let id = replace_geometry(PREVIEW_GEOMETRY.get(), geometry);
    PREVIEW_GEOMETRY.set(Some(id));
    let mut handle = geometry_handle(id);
    if let Value::Map(map) = &mut handle {
        map.insert("nodes".to_string(), cell(Value::Number(stats.nodes as f64)));
        map.insert("elements".to_string(), cell(Value::Number(stats.elements as f64)));
        if let Some(nearest) = stats.block_nearest {
            map.insert("blocked".to_string(), cell(Value::Number(stats.blocked_nodes as f64)));
            let xyz = nearest.iter().map(|&x| cell(Value::Number(x))).collect();
            map.insert("block-nearest".to_string(), cell(Value::List(xyz)));
        }
    }
    Ok(handle)
}

/// Bumped when the solver's output changes, so cached solves are redone.
const SOLVER_VERSION: &str = "modal-fe 1";

/// Every .obj path a spec names (in "obj" solids, at any depth).
fn obj_paths(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Object(map) => map
            .iter()
            .flat_map(|(key, item)| match (key.as_str(), item.get("path").and_then(|p| p.as_str())) {
                ("obj", Some(path)) => vec![path.to_string()],
                _ => obj_paths(item),
            })
            .collect(),
        serde_json::Value::Array(items) => items.iter().flat_map(obj_paths).collect(),
        _ => Vec::new(),
    }
}

/// Scale every mode's shape column to mean square 1/2 over the points
/// (eseq.modal's unit modal mass; the solver's are mass-normalised in kg).
fn unit_modal_mass(solid: &mut crate::modal_fe::ModalSolid) {
    let modes = solid.freqs.len();
    if modes == 0 {
        return;
    }
    let points = solid.shapes.len() / modes;
    for k in 0..modes {
        let mean_square = (0..points).map(|p| (solid.shapes[p * modes + k] as f64).powi(2)).sum::<f64>()
            / points.max(1) as f64;
        if mean_square > 0.0 {
            let scale = (0.5 / mean_square).sqrt() as f32;
            for p in 0..points {
                solid.shapes[p * modes + k] *= scale;
            }
        }
    }
}

/// value (a data dict) with :file, the eseq-modes file it came from, for a
/// mesh view of the object.
fn with_file(value: Value, path: &Path) -> Value {
    match value {
        Value::Map(mut map) => {
            map.insert("file".to_string(), cell(Value::String(path.display().to_string())));
            Value::Map(map)
        }
        other => other,
    }
}

/// A Lisp value as JSON: dicts become objects (keyword-style keys with `-`
/// read as `_`), lists arrays, keywords strings.
fn lisp_to_json(value: &Value) -> serde_json::Value {
    match value {
        // whole numbers as integers, for counts (modes, segments, cells)
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 9e15 => serde_json::json!(*n as i64),
        Value::Number(n) => serde_json::json!(n),
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::String(text) => serde_json::Value::String(text.clone()),
        Value::Keyword(k) | Value::Symbol(k) => serde_json::Value::String(k.trim_start_matches(':').replace('-', "_")),
        Value::List(items) => serde_json::Value::Array(items.iter().map(|item| lisp_to_json(&item.borrow())).collect()),
        Value::Map(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, item)| (key.trim_start_matches(':').replace('-', "_"), lisp_to_json(&item.borrow())))
                .collect(),
        ),
        _ => serde_json::Value::Null,
    }
}

fn read_data_native(args: &[Value]) -> Result<Value, String> {
    match args.first() {
        Some(Value::String(path)) => read_modal_data_file(path),
        _ => Err(format!("{READ_DATA_NATIVE} takes a file path")),
    }
}

/// (edge nu count samples) -> one (dict :lambda :m :radial) per mode.
fn circ_plate_native(args: &[Value]) -> Result<Value, String> {
    use super::modal_plates::{circ_plate_modes, PlateEdge};
    let number = |k: usize| match args.get(k) {
        Some(Value::Number(n)) => Ok(*n),
        _ => Err(format!("{CIRC_PLATE_NATIVE} takes (edge nu count samples)")),
    };
    let edge = match args.first() {
        Some(Value::Keyword(k) | Value::String(k)) if k.trim_start_matches(':') == "clamped" => PlateEdge::Clamped,
        Some(Value::Keyword(k) | Value::String(k)) if k.trim_start_matches(':') == "free" => PlateEdge::Free,
        _ => return Err(format!("{CIRC_PLATE_NATIVE}: edge is :free or :clamped")),
    };
    let modes = circ_plate_modes(edge, number(1)?, number(2)?.max(1.0) as usize, number(3)?.max(2.0) as usize);
    Ok(Value::List(
        modes
            .into_iter()
            .map(|mode| {
                cell(data_dict(vec![
                    ("lambda", Value::Number(mode.lambda)),
                    ("m", Value::Number(mode.m as f64)),
                    ("radial", numbers(mode.radial)),
                ]))
            })
            .collect(),
    ))
}

/// Register [`modal_natives`] on a bare VM (the CLI and tests). A failing
/// call returns its message as a string; eseq.modal's check reports it.
pub fn register_modal_natives(vm: &mut VM) {
    for (name, native) in modal_natives() {
        vm.register_native(name, move |args| native(&args).unwrap_or_else(Value::String));
    }
}

fn cell(value: Value) -> Rc<RefCell<Value>> {
    Rc::new(RefCell::new(value))
}

fn numbers(values: impl IntoIterator<Item = f64>) -> Value {
    Value::List(values.into_iter().map(|v| cell(Value::Number(v))).collect())
}

fn rows(values: &[f64], width: usize) -> Value {
    Value::List(values.chunks(width.max(1)).map(|row| cell(numbers(row.iter().copied()))).collect())
}

pub(super) fn data_dict(fields: Vec<(&str, Value)>) -> Value {
    Value::Map(fields.into_iter().map(|(key, value)| (key.to_string(), cell(value))).collect::<HashMap<_, _>>())
}

/// The data dict of an eseq-modes file, and whether it has a mesh to view
/// (triangles).
fn modes_json_value(text: &str) -> Result<(Value, bool), String> {
    let json: serde_json::Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    if json["format"] != "eseq-modes" || json["version"] != 1 {
        return Err("not an eseq-modes v1 file".to_string());
    }
    let floats = |key: &str| -> Result<Vec<f64>, String> {
        json[key]
            .as_array()
            .ok_or_else(|| format!("missing \"{key}\""))?
            .iter()
            .map(|v| v.as_f64().ok_or_else(|| format!("\"{key}\" holds a non-number")))
            .collect()
    };
    let freqs = floats("freqs")?;
    let shapes = floats("shapes")?;
    let points = floats("points")?;
    if points.len() % 3 != 0 {
        return Err("\"points\" must hold x y z per point".to_string());
    }
    if freqs.is_empty() || shapes.len() != freqs.len() * (points.len() / 3) {
        return Err("\"shapes\" must hold one value per point per mode".to_string());
    }
    let mut fields = vec![
        ("freqs", numbers(freqs.iter().copied())),
        ("shapes", rows(&shapes, freqs.len())),
        ("points", rows(&points, 3)),
        ("mass", Value::Number(json["mass"].as_f64().unwrap_or(1.0))),
    ];
    if json.get("losses").is_some() {
        let losses = floats("losses")?;
        if losses.len() != freqs.len() {
            return Err("\"losses\" must hold one value per mode".to_string());
        }
        fields.push(("losses", numbers(losses)));
    }
    // an identified model's measured data, passed through for eseq.modal:
    // its strokes, wash and the stick contact they were measured with
    let mut value = data_dict(fields);
    if let Value::Map(map) = &mut value {
        for key in ["measured", "strokes", "wash", "contact_s"] {
            if let Some(item) = json.get(key) {
                map.insert(key.replace('_', "-"), cell(json_to_lisp(item)));
            }
        }
    }
    // normals and triangles are for the mesh view, which reads :file itself
    let mesh = json["triangles"].as_array().is_some_and(|triangles| !triangles.is_empty());
    Ok((value, mesh))
}

/// JSON as a Lisp value, the reverse of [`lisp_to_json`]: objects become
/// dicts (keys with `_` read as `-`), arrays lists.
fn json_to_lisp(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(text) => Value::String(text.clone()),
        serde_json::Value::Array(items) => Value::List(items.iter().map(|item| cell(json_to_lisp(item))).collect()),
        serde_json::Value::Object(map) => Value::Map(
            map.iter().map(|(key, item)| (key.replace('_', "-"), cell(json_to_lisp(item)))).collect(),
        ),
    }
}

fn modres_value(text: &str) -> Result<Value, String> {
    let (mut freqs, mut amps, mut bandwidths) = (Vec::new(), Vec::new(), Vec::new());
    for (number, line) in text.lines().enumerate() {
        let line = line.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<f64> = line
            .split_whitespace()
            .map(|field| field.parse::<f64>().map_err(|_| format!("line {}: {field:?} is not a number", number + 1)))
            .collect::<Result<_, _>>()?;
        let [_, freq, amp_db, bandwidth] = fields[..] else {
            return Err(format!("line {}: expected index, frequency, amplitude (dB), bandwidth", number + 1));
        };
        freqs.push(freq);
        amps.push(10f64.powf(amp_db / 20.0));
        bandwidths.push(bandwidth);
    }
    if freqs.is_empty() {
        return Err("no modes".to_string());
    }
    Ok(data_dict(vec![("freqs", numbers(freqs)), ("amps", numbers(amps)), ("bandwidths", numbers(bandwidths))]))
}

#[cfg(test)]
mod tests {
    /// A dsp source with its whitespace collapsed, for matching forms that
    /// the pretty-printer may wrap.
    fn squash(dsp: &str) -> String {
        dsp.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    use super::*;
    use crate::lisp_host::eseq::generator::generate_instrument;

    /// A generated instrument's panel source.
    fn ui(instrument: &crate::lisp_host::eseq::generator::GeneratedInstrument) -> &str {
        instrument.ui.as_deref().expect("a modal patch has a panel")
    }

    fn modal_vm() -> VM {
        let mut vm = crate::lisp_host::eseq::generator::tests::gen_vm();
        register_modal_natives(&mut vm);
        vm
    }

    const CELLO: &str = "(import eseq.modal :as m)
(import eseq.modal.materials :as mat)
(m/patch :name \"probe-cello\"
  :objects (list (m/string 'str :made-of mat/nylon) (m/plate 'body))
  :connections (list (m/bow 'bow 'str 0.12)
                     (m/spring 'bridge 'str 0.99 'body (list 0.55 0.4)))
  :listens (list (m/listen 'str 0.85)))";

    #[test]
    fn generates_grouped_dsp_from_a_patch_script() {
        let mut vm = modal_vm();
        let instrument =
            generate_instrument(&mut vm, PathBuf::from("cello.lisp"), CELLO).unwrap();
        assert_eq!(instrument.name, "probe-cello");
        assert!(instrument.dsp.contains("(param str_length @group str"), "{}", instrument.dsp);
        assert!(instrument.dsp.contains("(param bow_pressure @group bow"));
        assert!(instrument.dsp.contains("bow-contact"));
        assert!(ui(&instrument).contains("(defsynth-ui"), "{}", ui(&instrument));
        assert!(ui(&instrument).contains("\"bow.bow_pressure\""), "{}", ui(&instrument));
    }

    #[test]
    fn generates_in_the_app_runtime_after_user_definitions() {
        // The app evaluates scripts in its UI runtime, where a script's own
        // defs land beside everything else; a script may compute its patch.
        let mut runtime = eseqlisp::Runtime::new();
        let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content");
        runtime.set_module_load_path(vec![content]);
        let script = "(import eseq.modal :as m)
(def ratios (list 1.5 2))
(def drone (k) (m/string (symbol \"sym\" k) :tune (nth ratios k) :mute 0))
(m/patch :name \"drones\"
  :objects (append (list (m/plate 'body)) (map drone (range (len ratios))))
  :connections (map (lambda (k) (m/spring (symbol \"b\" k) (symbol \"sym\" k) 0.96 'body (list 0.5 0.5)))
                    (range (len ratios)))
  :listens (list (m/listen 'body (list 0.3 0.6))))";
        let instrument =
            generate_instrument(&mut runtime, PathBuf::from("drones.lisp"), script).unwrap();
        assert_eq!(instrument.name, "drones");
        assert!(instrument.dsp.contains("(param sym1_tune @group sym1 @default 2"), "{}", instrument.dsp);
        assert!(instrument.dsp.contains("(param b0_stiff @group b0"));
    }

    #[test]
    fn reports_patch_problems_by_name() {
        let mut vm = modal_vm();
        let script = CELLO.replace("'bridge 'str 0.99 'body", "'bridge 'str 0.99 'bdy");
        let errors =
            generate_instrument(&mut vm, PathBuf::from("cello.lisp"), &script).unwrap_err();
        assert_eq!(errors, vec!["spring bridge: no object bdy".to_string()]);
    }

    #[test]
    fn a_list_of_materials_becomes_a_menu_and_one_becomes_numbers() {
        let mut vm = modal_vm();
        let script = CELLO.replace(
            "(m/plate 'body)",
            "(m/plate 'body :made-of (list mat/spruce mat/maple (mat/material :name \"cedar\" :density 380 :young 9e9)))",
        );
        let instrument =
            generate_instrument(&mut vm, PathBuf::from("cello.lisp"), &script).unwrap();
        let dsp = &instrument.dsp;
        // One material: its numbers, and no menu.
        assert!(dsp.contains("(def str_density 1140)"), "{dsp}");
        assert!(dsp.contains("(def str_young 3000000000)"), "{dsp}");
        assert!(!dsp.contains("str_material"), "{dsp}");
        // Several: a menu param named after them, picked by its index.
        assert!(dsp.contains("(param body_material @group body @options [\"spruce\" \"maple\" \"cedar\"]"), "{dsp}");
        assert!(dsp.contains("(selector (+ 1 (clip (round body_material) 0 2)) 450 650 380)"), "{dsp}");
        assert!(ui(&instrument).contains("\"body.body_material\""), "{}", ui(&instrument));
    }

    #[test]
    fn rejects_a_keyword_material() {
        let mut vm = modal_vm();
        let script = CELLO.replace("mat/nylon", ":nylon");
        let errors =
            generate_instrument(&mut vm, PathBuf::from("cello.lisp"), &script).unwrap_err();
        assert_eq!(
            errors,
            vec!["string str: :made-of takes a material such as mat/steel, or a list of them; got :nylon".to_string()]
        );
    }

    #[test]
    fn checks_every_material_field() {
        let mut vm = modal_vm();
        for (made_of, expected) in [
            ("nil", "string str: :made-of takes a material such as mat/steel, or a list of them; got nil"),
            ("(mat/material :name \"gut\" :density nil)", "string str: material gut needs a number :density"),
            ("(list mat/nylon (merge mat/steel :name \"nylon\"))", "string str: two materials are called nylon"),
        ] {
            let script = CELLO.replace("mat/nylon)", &format!("{made_of})"));
            let errors = generate_instrument(&mut vm, PathBuf::from("cello.lisp"), &script).unwrap_err();
            assert_eq!(errors, vec![expected.to_string()], "{made_of}");
        }
    }

    /// A data object whose patch is in `dir`, struck by a mallet at `at`
    /// and heard at `listen`.
    fn data_patch(data: &str, at: &str, listen: &str) -> String {
        format!(
            "(import eseq.modal :as m)
(m/patch :name \"probe-data\"
  :objects (list (m/mallet 'mal) (m/modal-data 'obj :data {data}))
  :connections (list (m/felt 'hit 'mal 'obj {at}))
  :listens (list (m/listen 'obj {listen})))"
        )
    }

    #[test]
    fn reads_a_resonance_model_relative_to_the_script() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bell.txt"), "; a bell\n1 110 -6 0.5\n2 220 0 1.0\n").unwrap();
        let script = data_patch("(modal-read-data \"bell.txt\")", "0", "0");
        let instrument =
            generate_instrument(&mut modal_vm(), dir.path().join("bell.lisp"), &script).unwrap();
        let dsp = &instrument.dsp;
        assert!(dsp.contains("(def obj_hz (tensor @shape [2] @data [110 220]))"), "{dsp}");
        // Bandwidths become decay rates pi bw; one point, so no point param.
        assert!(dsp.contains("1.570796325 3.14159265"), "{dsp}");
        assert!(!dsp.contains("obj_pt"), "{dsp}");
    }

    #[test]
    fn reads_eseq_modes_and_finds_points_by_position() {
        let dir = tempfile::tempdir().unwrap();
        // 3 points on a line, 2 modes: point 2 is at x = 1.
        let modes = serde_json::json!({
            "format": "eseq-modes", "version": 1, "freqs": [100.0, 250.0],
            "points": [0, 0, 0, 0.5, 0, 0, 1, 0, 0], "normals": [0, 0, 1, 0, 0, 1, 0, 0, 1],
            "triangles": [0, 1, 2], "shapes": [0.1, 0.2, 0.3, 0.4, 0.5, 0.6], "mass": 0.2
        });
        std::fs::write(dir.path().join("bar.modes.json"), modes.to_string()).unwrap();
        let script = data_patch("(modal-read-data \"bar.modes.json\")", "(list 0.9 0 0)", "1");
        let instrument =
            generate_instrument(&mut modal_vm(), dir.path().join("bar.lisp"), &script).unwrap();
        let dsp = &instrument.dsp;
        assert!(dsp.contains("(def obj_shapes (tensor @shape [3 2] @data [0.1 0.2 0.3 0.4 0.5 0.6]))"), "{dsp}");
        // (0.9 0 0) is nearest point 2; the mic sits on point 1.
        assert!(dsp.contains("(param hit_obj_pt @group hit @default 2 @min 0 @max 2"), "{dsp}");
        assert!(dsp.contains("(param mic1_obj_pt @group mic1 @default 1 @min 0 @max 2"), "{dsp}");
        // Its mesh view reads the file, an asset the instrument's folder
        // gets a copy of.
        let file = dir.path().join("bar.modes.json");
        assert_eq!(instrument.assets, vec![file.clone()]);
        assert!(ui(&instrument).contains(&format!(":src \"{}\"", file.display())), "{}", ui(&instrument));
        let out = dir.path().join("out");
        instrument.write_to(&out).unwrap();
        let saved = std::fs::read_to_string(out.join("ui.lisp")).unwrap();
        assert!(saved.contains(&format!(":src \"{}\"", out.join("bar.modes.json").display())), "{saved}");
        assert!(out.join("bar.modes.json").exists());
    }

    #[test]
    fn a_large_data_object_compiles_only_the_points_in_use() {
        let shapes: Vec<String> = (0..100).map(|k| format!("(list {k} 1)")).collect();
        let data = format!("(dict :freqs (list 100 200) :shapes (list {}))", shapes.join(" "));
        let instrument =
            generate_instrument(&mut modal_vm(), PathBuf::from("big.lisp"), &data_patch(&data, "70", "30"))
                .unwrap();
        let dsp = &instrument.dsp;
        assert!(dsp.contains("(def obj_shapes (tensor @shape [2 2] @data [70 1 30 1]))"), "{dsp}");
        assert!(dsp.contains("(param hit_obj_pt @group hit @default 0 @min 0 @max 1"), "{dsp}");
        assert!(dsp.contains("(param mic1_obj_pt @group mic1 @default 1 @min 0 @max 1"), "{dsp}");
    }

    /// Named points (a solid's :points): accesses name them, and every one is
    /// compiled, so the point knobs step through all of them in name order.
    #[test]
    fn named_points_are_referenced_by_name_and_all_compiled() {
        let shapes: Vec<String> = (0..100).map(|k| format!("(list {k} 1)")).collect();
        let points: Vec<String> = (0..100).map(|k| format!("(list {k} 0 0)")).collect();
        let named = |names: &str| {
            format!(
                "(merge (m/modal-data 'obj :data (dict :freqs (list 100 200) :shapes (list {}) :points (list {})))
                        :named-points (dict {names}))",
                shapes.join(" "),
                points.join(" ")
            )
        };
        let script = |names: &str, at: &str, listen: &str| {
            format!(
                "(import eseq.modal :as m)
(m/patch :name \"probe-named\"
  :objects (list (m/mallet 'mal) {})
  :connections (list (m/felt 'hit 'mal 'obj {at}))
  :listens (list (m/listen 'obj {listen})))",
                named(names)
            )
        };
        let names = ":tip (list 70.2 0 0) :end (list 29.8 0 0) :mid (list 50 0 0)";
        let dsp = generate_instrument(&mut modal_vm(), PathBuf::from("named.lisp"), &script(names, ":tip", ":end"))
            .unwrap()
            .dsp;
        // the two in use first, then the unused :mid
        assert!(dsp.contains("(def obj_shapes (tensor @shape [3 2] @data [70 1 30 1 50 1]))"), "{dsp}");
        assert!(dsp.contains("(param hit_obj_pt @group hit @default 0 @min 0 @max 2"), "{dsp}");
        assert!(dsp.contains("(param mic1_obj_pt @group mic1 @default 1 @min 0 @max 2"), "{dsp}");

        let mut vm = modal_vm();
        for (names, at, expected) in [
            (":tip (list 70 0 0)", ":tap", "felt hit b: modal-data obj has no point named :tap (it has :tip)"),
            (":tip (list 70 0)", ":tip", "modal-data obj: point :tip is (list x y z), got (70 0)"),
        ] {
            let errors =
                generate_instrument(&mut vm, PathBuf::from("x.lisp"), &script(names, at, "0")).unwrap_err();
            assert!(errors.contains(&expected.to_string()), "{errors:?}");
        }
    }

    #[test]
    fn checks_data_objects() {
        let mut vm = modal_vm();
        for (data, at, expected) in [
            ("(dict :freqs (list 100 200) :losses (list 1))", "0", "modal-data obj: 1 losses for 2 modes"),
            ("(dict :freqs (list 100) :shapes (list (list 1) (list 1)))", "5", "felt hit b: modal-data obj has points 0..1, got 5"),
            ("(dict :freqs (list 100))", "(list 1 2 3)", "felt hit b: modal-data obj has no :points to find (1 2 3) among"),
        ] {
            let errors = generate_instrument(&mut vm, PathBuf::from("x.lisp"), &data_patch(data, at, "0"))
                .unwrap_err();
            assert_eq!(errors, vec![expected.to_string()], "{data}");
        }
    }

    /// The dsp of a patch striking `object` (id 'o) at `at` and hearing it at
    /// `listen`.
    fn struck_dsp(object: &str, at: &str, listen: &str) -> String {
        let script = format!(
            "(import eseq.modal :as m)
(import eseq.modal.materials :as mat)
(m/patch :name \"probe\"
  :objects (list (m/mallet 'mal) {object})
  :connections (list (m/felt 'hit 'mal 'o {at}))
  :listens (list (m/listen 'o {listen})))"
        );
        generate_instrument(&mut modal_vm(), PathBuf::from("probe.lisp"), &script).unwrap().dsp
    }

    #[test]
    fn generates_the_analytic_objects() {
        let bar = struck_dsp("(m/bar 'o :modes 4)", "0.4", "0.1");
        assert!(bar.contains("(def o_beta (tensor @shape [4] @data [4.73004074 7.85320462 10.9956078 14.1371655]))"), "{bar}");
        // natural ratios 1 : 2.7565 : 5.4039 : 8.933, undercut targets 1 : 4 : 10
        assert!(bar.contains("@data [1 2.7565"), "{bar}");
        assert!(bar.contains("@data [1 4 10 16.5305"), "{bar}");
        assert!(bar.contains("(param hit_o_at @group hit @default 0.4 @min 0 @max 1"), "{bar}");

        let skin = struck_dsp("(m/rect-membrane 'o :grid 3)", "(list 0.3 0.4)", "(list 0.5 0.5)");
        assert!(skin.contains("(def o_grid (iota 9))"), "{skin}");
        assert!(skin.contains("(make-history o_s1 @shape [9])"), "{skin}");

        let plate = struck_dsp("(m/plate 'o :grid 5)", "(list 0.3 0.4)", "(list 0.5 0.5)");
        assert!(plate.contains("(def o_grid (iota 25))"), "{plate}");

        // A clamped disc's first mode: lambda^2 = 10.2158 (Leissa).
        let disc = struck_dsp("(m/circ-plate 'o :edge :clamped :modes 6)", "(list 0.3 0)", "(list 0.5 1)");
        assert!(disc.contains("@data [10.2158"), "{disc}");
        assert!(disc.contains("(def o_order (tensor @shape [6] @data [0 1 2 0 3 1]))"), "{disc}");

        let osc = struck_dsp("(m/oscillator 'o :tune 0.5)", "0", "0");
        assert!(osc.contains("(def o_hz (tensor @shape [2] @data [440 100000]))"), "{osc}");
        // following the note, its coefficients are held on the coefficient tick
        assert!(osc.contains("(def o_ratio (event-hold (* (mod o_tune) (/ f0 440)) coef_tick))"), "{osc}");
    }

    #[test]
    fn drivers_and_weights() {
        let script = "(import eseq.modal :as m)
(m/patch :name \"probe-drivers\"
  :objects (list (m/string 'str))
  :connections (list (m/force 'ebow 'str 0.3 :signal (list (m/envelope :sustain 1) (m/sine :ratio 2)))
                     (m/position 'clamp 'str 0.5)
                     (m/speed 'push 'str 0.7 :signal (m/constant 0.5))
                     (m/bow 'bow 'str 0.12 :weight 0.5))
  :listens (list (m/listen 'str 0.2)))";
        let instrument =
            generate_instrument(&mut modal_vm(), PathBuf::from("drivers.lisp"), script).unwrap();
        let dsp = &squash(&instrument.dsp);
        // The force is gain x envelope x sine, solved once like an exciter.
        assert!(dsp.contains("(adsr gate hit (mod ebow_env_attack) (mod ebow_env_decay) (mod ebow_env_sustain) (mod ebow_env_release))"), "{dsp}");
        assert!(dsp.contains("(sin (* twopi (phasor (* f0 (mod ebow_sine1_ratio)))))"), "{dsp}");
        assert!(dsp.contains("(def ebow_F (* (mod ebow_weight) (mod ebow_gain) ebow_sig))"), "{dsp}");
        assert!(dsp.contains("(param ebow_env_sustain @group ebow @default 1"), "{dsp}");
        // Position pins to its signal; speed integrates it with a leak.
        assert!(dsp.contains("(def clamp_anchor clamp_sig)"), "{dsp}");
        assert!(dsp.contains("(def push_sig push_const_value)"), "{dsp}");
        assert!(dsp.contains("(* (exp (/ -0.5 samplerate)) (read-history push_anchor_h))"), "{dsp}");
        assert!(dsp.contains("(spring-connection (- clamp_anchor clamp_y_b0) clamp_C_b0 (mod clamp_stiff))"), "{dsp}");
        // Every connection has a weight; the bow's scales its pressure.
        assert!(dsp.contains("(param bow_weight @group bow @default 0.5 @min 0 @max 1"), "{dsp}");
        assert!(dsp.contains("(* (mod bow_pressure) bow_env (mod bow_weight))"), "{dsp}");
    }

    #[test]
    fn spectral_controls_and_mode_selection() {
        let script = "(import eseq.modal :as m)
(m/patch :name \"probe-spectral\"
  :objects (list (m/finger 'fin)
                 (m/spectral (m/plate 'pl :grid 2) :follow-note true :loss-scales (list 2) :gains (list 1 0.5)))
  :connections (list (m/pluck 'pick 'fin 'pl (list 0.3 0.4)))
  :listens (list (m/listen 'pl (list 0.2 0.7) :modes (list 0 3))))";
        let instrument =
            generate_instrument(&mut modal_vm(), PathBuf::from("spectral.lisp"), script).unwrap();
        let dsp = &instrument.dsp;
        assert!(dsp.contains("(param pl_bend @group pl @default 0 @min -2400 @max 2400 @unit ct)"), "{dsp}");
        assert!(dsp.contains("(param pl_diffraction @group pl @default 1"), "{dsp}");
        // The raw spectrum is kept; the warp follows the note from mode 0.
        assert!(dsp.contains("(def pl_f1_0 (sum (* pl_freqs0 (tensor @shape [4] @data [1 0 0 0]))))"), "{dsp}");
        assert!(dsp.contains("(/ f0 pl_f1_0)"), "{dsp}");
        assert!(dsp.contains("(log (+ (* pl_diffraction 1.718281828) 1))"), "{dsp}");
        assert!(dsp.contains("(tensor @shape [4] @data [2 1 1 1])"), "{dsp}");
        // The mic hears modes 0 and 3, through the object's gains.
        assert!(dsp.contains("(tensor @shape [4] @data [1 0.5 1 1])"), "{dsp}");
        assert!(dsp.contains("(tensor @shape [4] @data [1 0 0 1])"), "{dsp}");
    }

    #[test]
    fn checks_spectral_listens_and_data_files() {
        let mut vm = modal_vm();
        let patch = |objects: &str, listen: &str| {
            format!(
                "(import eseq.modal :as m)
(m/patch :name \"probe\" :objects (list (m/mallet 'mal) {objects})
  :connections (list (m/felt 'hit 'mal 'o 0)) :listens (list {listen}))"
            )
        };
        for (objects, listen, expected) in [
            ("(m/spectral (m/mallet 'x)) (m/bar 'o)", "(m/listen 'o 0.1)", "mallet x: spectral needs a modal object"),
            ("(m/bar 'o :modes 4)", "(m/listen 'o 0.1 :modes (list 4))", "listen o: no mode 4 (it has 4, from 0)"),
            ("(m/modal-data 'o :data (modal-read-data \"missing.txt\"))", "(m/listen 'o 0)", "missing.txt"),
            ("(m/modal-data 'o :freqs (list 100) :shapes (list (list 1) (list 1)) :points (list (list 0 0 0)))",
             "(m/listen 'o 0)", "modal-data o: 1 points for 2 shape rows"),
        ] {
            let errors = generate_instrument(&mut vm, PathBuf::from("x.lisp"), &patch(objects, listen)).unwrap_err();
            assert!(errors.iter().any(|error| error.contains(expected)), "{objects}: {errors:?}");
        }
        // Data modes are sorted by frequency, losses and shape columns with them.
        let dsp = generate_instrument(
            &mut vm,
            PathBuf::from("x.lisp"),
            &patch("(m/modal-data 'o :freqs (list 300 100 200) :losses (list 3 1 2) :amps (list 0.3 0.1 0.2))", "(m/listen 'o 0)"),
        )
        .unwrap()
        .dsp;
        assert!(dsp.contains("(def o_hz (tensor @shape [3] @data [100 200 300]))"), "{dsp}");
        assert!(dsp.contains("(def o_loss0 (tensor @shape [3] @data [1 2 3]))"), "{dsp}");
        assert!(dsp.contains("@data [0.1 0.2 0.3]"), "{dsp}");
    }

    #[test]
    fn solves_a_solid_from_a_lisp_spec() {
        // A coarse free steel bar, solved (or read from the cache) on generation.
        let script = "(import eseq.modal :as m)
(import eseq.modal.materials :as mat)
(m/patch :name \"probe-solid\"
  :objects (list (m/mallet 'mal)
                 (m/solid 'bar :made-of mat/steel :modes 4 :resolution (dict :cells 12)
                          :shape (dict :box (dict :size (list 0.3 0.03 0.01)))))
  :connections (list (m/felt 'hit 'mal 'bar (list 0.15 0 0.005)))
  :listens (list (m/listen 'bar (list -0.14 0 0.005))))";
        let dsp = generate_instrument(&mut modal_vm(), PathBuf::from("solid.lisp"), script).unwrap().dsp;
        let squashed = squash(&dsp);
        assert!(squashed.contains("(def bar_hz (tensor @shape [4] @data ["), "{dsp}");
        // its surface points, addressed by (x y z): the nearest point param
        assert!(squashed.contains("(def bar_shapes (tensor @shape ["), "{dsp}");
        assert!(squashed.contains("(param hit_bar_pt @group hit"), "{dsp}");
    }

    /// A bar no earlier run has cached (its length carries the clock), as a
    /// `modal-solve` call.
    fn uncached_solve_source() -> String {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
        let length = 0.3 + f64::from(nanos % 1_000_000) * 1e-10;
        format!(
            "(import eseq.modal.materials :as mat)
             (modal-solve (dict :solid (dict :box (dict :size (list {length} 0.03 0.01))) :modes 4
                                :made-of mat/steel :resolution (dict :cells 8)))"
        )
    }

    /// Under deferring_solves an uncached solid is recorded, not solved;
    /// once solved, the same call reads it from the cache and defers nothing.
    #[test]
    fn a_deferring_evaluation_records_uncached_solids_instead_of_solving() {
        let mut vm = modal_vm();
        let source = uncached_solve_source();
        let (_, deferred) = deferring_solves(|| vm.eval_str(&source));
        assert_eq!(deferred.len(), 1);
        let solve = &deferred[0];
        solve_solid_cached(&solve.spec, &solve.base, &crate::modal_fe::SolveProgress::default()).unwrap();
        let (value, deferred) = deferring_solves(|| vm.eval_str(&source));
        assert!(deferred.is_empty());
        assert!(matches!(value, Ok(Some(Value::Map(_)))), "{value:?}");
    }

    /// The cache folder key is canonical: a relative and an absolute path
    /// to one folder are the same key.
    #[test]
    fn a_relative_and_an_absolute_script_folder_share_a_solve() {
        let here = std::env::current_dir().unwrap();
        assert_eq!(cache_dir_key(Path::new(".")), here.canonicalize().unwrap());
        assert_eq!(cache_dir_key(Path::new("src")), cache_dir_key(&here.join("src")));
        assert_eq!(cache_dir_key(Path::new("/no/such/dir")), PathBuf::from("/no/such/dir"));
    }

    /// modal-mesh-pick finds the surface under a point of a view plane, on
    /// the side the view looks from.
    #[test]
    fn picks_the_front_surface_of_a_preview() {
        let mut vm = modal_vm();
        let mut eval = |source: &str| vm.eval_str(source).unwrap().unwrap_or(Value::Nil);
        let preview = eval(
            "(import eseq.modal.materials :as mat)
             (def bar (modal-mesh (dict :solid (dict :box (dict :size (list 0.3 0.04 0.01))) :modes 4
                                        :made-of mat/steel :resolution (dict :cells 30))))
             (get bar :mesh-id)",
        );
        assert!(matches!(preview, Value::Number(_)), "{preview:?}");
        let xyz = |value: Value| match value {
            Value::List(items) => items
                .iter()
                .map(|item| match &*item.borrow() {
                    Value::Number(x) => *x,
                    other => panic!("{other:?}"),
                })
                .collect::<Vec<f64>>(),
            other => panic!("{other:?}"),
        };
        // the front view (x across, z up, along y): the +y face
        let front = xyz(eval("(modal-mesh-pick (get bar :mesh-id) 0 2 1 0.1 0)"));
        assert!((front[1] - 0.02).abs() < 1e-6 && (front[0] - 0.1).abs() < 0.011, "{front:?}");
        // the top view (x across, y up, along z): the +z face
        let top = xyz(eval("(modal-mesh-pick (get bar :mesh-id) 0 1 2 -0.05 0.01)"));
        assert!((top[2] - 0.005).abs() < 1e-6, "{top:?}");
        assert!(matches!(eval("(modal-mesh-pick 999999 0 1 2 0 0)"), Value::Nil));
        // a point that is not finite picks nothing (it once panicked)
        let id = eval("(get bar :mesh-id)");
        let at = |u: f64| [id.clone(), Value::Number(0.0), Value::Number(1.0), Value::Number(2.0), Value::Number(u), Value::Number(0.0)];
        assert!(matches!(pick_native(&at(f64::NAN)), Ok(Value::Nil)));
    }

    /// modal-mesh previews a spec without solving it: a handle to its surface
    /// with the mesh's counts and its mass, and an error as data for a shape
    /// that does not mesh. A new preview replaces the last one's geometry.
    #[test]
    fn previews_a_solid_without_solving_it() {
        use eseqlisp::widget_render::mesh_widget::geometry;
        let mut vm = modal_vm();
        let mut preview = |shape: &str| {
            vm.eval_str(&format!(
                "(import eseq.modal.materials :as mat)
                 (modal-mesh (dict :solid {shape} :modes 4 :made-of mat/steel :resolution (dict :cells 10)))"
            ))
            .unwrap()
            .unwrap_or(Value::Nil)
        };
        let field = |value: &Value, key: &str| match value {
            Value::Map(map) => map.get(key).map(|item| item.borrow().clone()),
            _ => None,
        };
        let number = |value: &Value, key: &str| match field(value, key) {
            Some(Value::Number(n)) => n,
            other => panic!("{key}: {other:?} in {value:?}"),
        };
        let bar = preview("(dict :box (dict :size (list 0.3 0.03 0.01)))");
        let first_id = number(&bar, "mesh-id") as u64;
        assert!(geometry(first_id).is_some());
        assert!(number(&bar, "elements") >= 10.0 && number(&bar, "points") > 10.0, "{bar:?}");
        // 0.3 x 0.03 x 0.01 m of steel is 0.71 kg
        let mass = number(&bar, "mass");
        assert!((mass - 0.7065).abs() < 0.01, "{mass}");
        let longer = preview("(dict :box (dict :size (list 0.4 0.03 0.01)))");
        assert!(geometry(number(&longer, "mesh-id") as u64).is_some());
        assert!(geometry(first_id).is_none(), "the previous preview's geometry stays registered");
        let broken = preview("(dict :box (dict :size (list 0.4 0.03)))");
        assert!(matches!(field(&broken, "error"), Some(Value::String(_))), "{broken:?}");

        // a block reports what it holds and the node nearest its centre
        let mut clamped = |at: &str| {
            vm.eval_str(&format!(
                "(modal-mesh (dict :solid (dict :box (dict :size (list 0.3 0.03 0.01))) :modes 4 :made-of eseq.modal.materials/steel
                                   :resolution (dict :cells 10) :block (dict :sphere (dict :center (list {at}) :radius 0.02))))"
            ))
            .unwrap()
            .unwrap_or(Value::Nil)
        };
        let held = clamped("0 0 0");
        assert!(number(&held, "blocked") >= 1.0, "{held:?}");
        let missed = clamped("0 0.1 0");  // 85 mm off the bar's side
        assert_eq!(number(&missed, "blocked"), 0.0);
        let Some(Value::List(nearest)) = field(&missed, "block-nearest") else { panic!("{missed:?}") };
        let y = match &*nearest[1].borrow() {
            Value::Number(y) => *y,
            other => panic!("{other:?}"),
        };
        assert!((y - 0.015).abs() < 1e-9, "the nearest node is on the bar's edge: {y}");
        assert!(field(&bar, "blocked").is_none(), "no block, no count");
    }

    #[test]
    fn strike_stop_and_stulov_felt() {
        let script = "(import eseq.modal :as m)
(m/patch :name \"probe-contacts\"
  :objects (list (m/mallet 'mal) (m/string 'str) (m/plate 'pl))
  :connections (list (m/felt 'hit 'mal 'str 0.1 :exponent 2.5)
                     (m/stop 'jaw 'str 0.03 :height -0.001)
                     (m/strike 'rattle 'str 0.6 'pl (list 0.4 0.4) :offset 0.002))
  :listens (list (m/listen 'str 0.2)))";
        let dsp = generate_instrument(&mut modal_vm(), PathBuf::from("contacts.lisp"), script).unwrap().dsp;
        let squashed = squash(&dsp);
        // The stop pushes up from below; the strike pushes the points apart.
        assert!(squashed.contains("(* -1 (mod jaw_weight) (felt-contact (- jaw_y_b0 (mod jaw_height)) jaw_C_b0 (mod jaw_stiff)))"), "{dsp}");
        assert!(squashed.contains("(felt-contact (+ (- rattle_y_a0 rattle_y_b0) (mod rattle_offset)) (+ rattle_C_a0 rattle_C_b0) (mod rattle_stiff))"), "{dsp}");
        // Stulov: 5 Newton steps clamped to the bracket, then the remembered compression.
        assert!(squashed.contains("(def hit_f5 (clip "), "{dsp}");
        assert!(squashed.contains("(def hit_F (* -1 (mod hit_weight) hit_f))"), "{dsp}");
        assert!(squashed.contains("(param hit_exponent @group hit @default 2.5"), "{dsp}");
    }

    #[test]
    fn two_directions_hair_and_fingerboard() {
        let script = "(import eseq.modal :as m)
(m/patch :name \"probe-planes\"
  :objects (list (m/string 'str :planes 2))
  :connections (list (m/bow 'bow 'str 0.12 :hair 0.002) (m/fingerboard 'fret 'str 0.3))
  :listens (list (m/listen (m/plane 'str 1) 0.8)))";
        let instrument = generate_instrument(&mut modal_vm(), PathBuf::from("planes.lisp"), script).unwrap();
        let squashed = squash(&instrument.dsp);
        // The second direction reads the first's params, detuned by split.
        assert!(squashed.contains("(def str_v_f0 (* f0 str_tune str_split))"), "{squashed}");
        assert!(squashed.contains("(* str_v_radius") == false, "{squashed}");
        assert!(squashed.contains("(* str_radius 0.001) str_length)"), "{squashed}");
        // The hair presses str_v down at the bow's point; the bow's pressure follows it.
        assert!(squashed.contains("(felt-contact (- bow_hair_height bow_hair_y_b0) bow_hair_C_b0 bow_hair_stiff)"), "{squashed}");
        assert!(squashed.contains("(def bow_hair_phi_b (* (sin (* str_v_n 3.14159265 bow_str_at)) str_v_mask))"), "{squashed}");
        assert!(squashed.contains("(read-history bow_press_h)"), "{squashed}");
        // The fingerboard clamps by its press signal, its board at the same point.
        assert!(squashed.contains("(* (mod fret_weight) (max 0 fret_sig) (spring-connection (- 0 fret_y_b0) fret_C_b0 (mod fret_stiff)))"), "{squashed}");
        assert!(squashed.contains("(sin (* str_n 3.14159265 fret_str_at))"), "{squashed}");
        // No panel section for the second direction or the hidden hair.
        assert!(!ui(&instrument).contains("STRING v"), "{}", ui(&instrument));
        assert!(!ui(&instrument).contains("HAIR"), "{}", ui(&instrument));
    }

    #[test]
    fn feedback_taps() {
        let script = "(import eseq.modal :as m)
(m/patch :name \"probe-taps\"
  :objects (list (m/mallet 'mal) (m/string 'a) (m/string 'b))
  :connections (list (m/felt 'hit 'mal 'a 0.1)
                     (m/force 'sym 'b 0.3 :saturate true :signal (list (m/tap-speed 'a 0.9) (m/tap-contact 'hit))))
  :listens (list (m/listen 'b 0.2)))";
        let instrument = generate_instrument(&mut modal_vm(), PathBuf::from("taps.lisp"), script).unwrap();
        let squashed = squash(&instrument.dsp);
        // The tap's point is its own param, its shape read from a's last state.
        assert!(squashed.contains("(param sym_tap_a_at @group sym_tap @default 0.9"), "{squashed}");
        assert!(squashed.contains("(- (read-history a_s1) (read-history a_s2))"), "{squashed}");
        // The felt's force is kept a sample for the contact tap.
        assert!(squashed.contains("(make-history hit_F_h)"), "{squashed}");
        assert!(squashed.contains("(write-history hit_F_h hit_F)"), "{squashed}");
        assert!(squashed.contains("(gt (abs (read-history hit_F_h)) 0.0000001)"), "{squashed}");
        assert!(squashed.contains("(tanh sym_sig)"), "{squashed}");
        let bad = script.replace("(m/tap-contact 'hit)", "(m/tap-contact 'nope)");
        let errors = generate_instrument(&mut modal_vm(), PathBuf::from("taps.lisp"), &bad).unwrap_err();
        assert_eq!(errors, vec!["force sym: tap on no connection nope".to_string()]);
    }

    #[test]
    fn melt_and_mix_hybrids() {
        let melt = "(import eseq.modal :as m)
(m/patch :name \"probe-melt\"
  :objects (list (m/mallet 'mal) (m/melt 'h (m/string 'str :modes 16) (m/rect-membrane 'skin :grid 4)))
  :connections (list (m/felt 'hit 'mal 'h (list 0.1 (list 0.3 0.4))))
  :listens (list (m/listen 'h (list 0.7 (list 0.6 0.6)))))";
        let squashed = generate_instrument(&mut modal_vm(), PathBuf::from("melt.lisp"), melt).unwrap().dsp
            .split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(squashed.contains("(def h_freqs (exp (+ (* (- 1 h_morph) (log str_freqs)) (* h_morph (log skin_freqs)))))"), "{squashed}");
        assert!(squashed.contains("(def hit_phi_b (+ (* (- 1 h_morph) hit_phi_b_a) (* h_morph hit_phi_b_b)))"), "{squashed}");
        let unequal = melt.replace(":modes 16", ":modes 12");
        let errors = generate_instrument(&mut modal_vm(), PathBuf::from("melt.lisp"), &unequal).unwrap_err();
        assert_eq!(errors, vec!["melt h: its parts have 12 and 16 modes; give them the same :modes".to_string()]);

        let mix = "(import eseq.modal :as m)
(m/patch :name \"probe-mix\"
  :objects (list (m/finger 'fin) (m/string 'lo) (m/string 'hi :tune 1.5) (m/mix-hybrid 'pair 'lo 'hi))
  :connections (list (m/pluck 'pick 'fin 'pair (list 0.2 0.2)))
  :listens (list (m/listen 'pair (list 0.8 0.8))))";
        let squashed = generate_instrument(&mut modal_vm(), PathBuf::from("mix.lisp"), mix).unwrap().dsp
            .split_whitespace().collect::<Vec<_>>().join(" ");
        // The pluck sees the lever of both strings and pushes each by its share.
        assert!(squashed.contains("(def pick_y_b (+ (* (- 1 (mod pair_morph)) pick_y_b_0) (* (mod pair_morph) pick_y_b_1)))"), "{squashed}");
        assert!(squashed.contains("(* 1 (- 1 (mod pair_morph)) pick_F pick_phi_b_0)"), "{squashed}");
        assert!(squashed.contains("(* 1 (mod pair_morph) pick_F pick_phi_b_1)"), "{squashed}");
        assert!(squashed.contains("(def mic1_v (+ (* (- 1 (mod pair_morph)) mic1_0_v) (* (mod pair_morph) mic1_1_v)))"), "{squashed}");
    }

    #[test]
    fn air_family() {
        let script = "(import eseq.modal :as m)
(m/patch :name \"probe-air\"
  :objects (list (m/tube 'bore :ends :closed-open :modes 4) (m/tube 'pipe :ends :open-open :modes 3)
                 (m/tube 'horn :ends :cone :modes 3) (m/oscillator 'lip))
  :connections (list (m/reed 'reed 'bore 0) (m/tone-hole 'hole 'bore 0.66)
                     (m/lips 'lips 'lip 'horn 0) (m/jet 'jet 'pipe 0.05))
  :listens (list (m/listen 'bore 0.5)))";
        let instrument = generate_instrument(&mut modal_vm(), PathBuf::from("air.lisp"), script).unwrap();
        let squashed = squash(&instrument.dsp);
        // Closed-open: odd harmonics, cos shapes; open-open: every harmonic, sin.
        assert!(squashed.contains("(* (tensor @shape [4] @data [1 3 5 7]) bore_f0)"), "{squashed}");
        assert!(squashed.contains("(def bore_wave (tensor @shape [4] @data [0.5 1.5 2.5 3.5]))"), "{squashed}");
        assert!(squashed.contains("(sin (* pipe_wave 3.14159265"), "{squashed}");
        // The reed: an implicit pressure solve, the tube driven by the flow's change.
        assert!(squashed.contains("(def reed_F3 (* (mod reed_weight) (mod reed_coupling) (- reed_u_reed_F3 (read-history reed_u_h))))"), "{squashed}");
        assert!(squashed.contains("(write-history reed_u_h reed_u_reed_F3)"), "{squashed}");
        // A tone hole is a weighted pressure release.
        assert!(squashed.contains("(spring-connection (- hole_anchor hole_y_b0) hole_C_b0 (mod hole_stiff))"), "{squashed}");
        // The lips push the lip by their own force, the tube by the flow's.
        assert!(squashed.contains("(* -1 lips_F_a lips_phi_a)"), "{squashed}");
        assert!(squashed.contains("(def lips_open0 (max 0 (+ 1 (/ lips_y_a0 (mod lips_rest)))))"), "{squashed}");
        // The jet: delayed by ratio periods, steered by the air's velocity at the mouth.
        assert!(squashed.contains("(delay jet_in (/ (* (mod jet_ratio) samplerate) pipe_f0) @max-delay 9600)"), "{squashed}");
        assert!(squashed.contains("(def jet_mouth (sum (* (read-history pipe_s1) jet_grad)))"), "{squashed}");
        // Blowing into a non-tube, or at an open end, is a check error.
        for (bad, expected) in [
            ("(m/reed 'reed 'pipe 0)", "reed reed: pipe is open there (no pressure); use a closed end"),
            ("(m/jet 'jet 'lip 0)", "jet jet: blows into a tube (m/tube), not lip"),
        ] {
            let script = script.replace("(m/reed 'reed 'bore 0)", bad);
            let errors = generate_instrument(&mut modal_vm(), PathBuf::from("air.lisp"), &script).unwrap_err();
            assert!(errors.contains(&expected.to_string()), "{errors:?}");
        }
        // A one-mode oscillator carries a silent second mode (eseq-k0m2.32).
        assert!(squashed.contains("(def lip_hz (tensor @shape [2] @data [440 100000]))"), "{squashed}");
    }

    #[test]
    fn measured_strokes_on_one_voice() {
        let dir = tempfile::tempdir().unwrap();
        // 3 measured modes (the last out of order), two strokes, two wash bands.
        let modes = serde_json::json!({
            "format": "eseq-modes", "version": 1, "measured": true, "contact_s": 0.0003,
            "freqs": [300.0, 700.0, 500.0], "losses": [3.0, 7.0, 5.0],
            "points": [0, 0, 0], "shapes": [1, 1, 1], "mass": 1,
            "strokes": [
                {"name": "a", "re": [0.1, 0.2, 0.3], "im": [0.4, 0.5, 0.6], "click": {"rms": 0.01, "decay_s": 0.001, "hz": 6000}},
                {"name": "b", "re": [0.7, 0.8, 0.9], "im": [1.0, 1.1, 1.2], "click": {"rms": 0.02, "decay_s": 0.002, "hz": 7000}}
            ],
            "wash": {"hz": [1000, 2000], "buildup": [100, 200], "fast_rate": [30, 40], "slow_rate": [3, 4],
                     "fast_power": [[1, 2], [3, 4]], "slow_power": [[5, 6], [7, 8]]}
        });
        std::fs::write(dir.path().join("ride.modes.json"), modes.to_string()).unwrap();
        let script = "(import eseq.modal :as m)
(m/patch :name \"probe-ride\" :voicing :one :tone 18000
  :objects (list (m/modal-data 'ride :data (modal-read-data \"ride.modes.json\") :follow :octave))
  :connections (list (m/strokes 'stick 'ride))
  :listens (list (m/listen 'ride 0 :hear :position)))";
        let instrument = generate_instrument(&mut modal_vm(), dir.path().join("ride.lisp"), script).unwrap();
        let squashed = squash(&instrument.dsp);
        // Modes and residues sorted together; measured losses get no air loss.
        assert!(squashed.contains("(def ride_hz (tensor @shape [3] @data [300 500 700]))"), "{squashed}");
        assert!(squashed.contains("(def stick_re_t (tensor @shape [2 3] @data [0.1 0.3 0.2 0.7 0.9 0.8]))"), "{squashed}");
        assert!(!squashed.contains("(modal-loss ride_freqs"), "{squashed}");
        // The octave transposes, held on the coefficient tick and latched back.
        assert!(squashed.contains("(def ride_ratio (event-hold (* (mod ride_tune) key_transpose) coef_tick))"), "{squashed}");
        assert!(squashed.contains("(def ride_a (latch ride_a_e coef_tick))"), "{squashed}");
        assert!(squashed.contains("(def stick_ref_pole (exp (/ -1 (* 0.0003 samplerate))))"), "{squashed}");
        // A hit pushes its residue straight into the modes, in two samples.
        assert!(squashed.contains("(modal-advance ride_free (+ (* ride_G 0) stick_push))"), "{squashed}");
        assert!(squashed.contains("(def stick_push (+ (* hit stick_di) (read-history stick_pend)))"), "{squashed}");
        // Heard as position; the click and wash join the mix; the tone filter.
        assert!(squashed.contains("(def mic1_v mic1_y)"), "{squashed}");
        assert!(squashed.contains("(def mix (+ (* mic1_v (mod mic1_gain)) stick_out))"), "{squashed}");
        assert!(squashed.contains("(def stick_wash_fast_t (tensor @shape [2 2] @data [1 2 3 4]))"), "{squashed}");
        assert!(squashed.contains("(min (mod out_tone) (* samplerate 0.43)) 0.707 0)"), "{squashed}");
        // One voice: the host's voice mode param, bound in instrument.json.
        assert!(squashed.contains("(param out_voice_mode @group out @default 1 @min 0 @max 3)"), "{squashed}");
        let out = dir.path().join("out");
        instrument.write_to(&out).unwrap();
        let metadata: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out.join("instrument.json")).unwrap()).unwrap();
        assert_eq!(metadata["voice_controls"]["mode"], "out_voice_mode");
        // No mesh to view in a one-point identified model.
        assert!(!ui(&instrument).contains("mode-mesh-cycle"), "{}", ui(&instrument));
        // Strokes need a data object with strokes.
        let bad = script.replace("(m/strokes 'stick 'ride)", "(m/strokes 'stick 'nope)");
        let errors = generate_instrument(&mut modal_vm(), dir.path().join("ride.lisp"), &bad).unwrap_err();
        assert!(errors.contains(&"strokes stick: no object nope".to_string()), "{errors:?}");
    }

    #[test]
    fn electric_guitar_feeds_its_amp_back() {
        let dir = tempfile::tempdir().unwrap();
        let script = "(import eseq.modal :as m)
(m/patch :name \"probe-lead\" :voicing :one
  :objects (list (m/finger 'pick) (m/string 'str :modes 8) (m/plate 'body :grid 2))
  :connections (list (m/pluck 'plk 'pick 'str 0.8)
                     (m/force 'air 'body (list 0.4 0.6) :gain 0.01 :signal (m/speaker :distance 2 :angle 60))
                     (m/mount 'bridge 'body (list 0.5 0.45) 'str 1))
  :listens (list (m/pickup 'str 0.92 :height 3))
  :amp (m/amp :drive 30))";
        let instrument = generate_instrument(&mut modal_vm(), dir.path().join("lead.lisp"), script).unwrap();
        let squashed = squash(&instrument.dsp);
        // The pickup: speed times the flux gradient at the string's distance, through the coil.
        assert!(squashed.contains("(def mic1_near (/ 1 (- 1 (clip (* (/ (* str_mass (/ 0.5 plk_thresh)) (mod mic1_height)) mic1_y) -0.9 0.6))))"), "{squashed}");
        assert!(squashed.contains("mic1_near mic1_near mic1_near) (min (mod mic1_coil_hz) (* 0.45 samplerate)) (mod mic1_coil_q) 0)"), "{squashed}");
        // The amp plays the mix and is what the output hears.
        assert!(squashed.contains("(def amp_x (* (svf heard 80 0.5 2) (mod amp_drive)))"), "{squashed}");
        assert!(squashed.contains("(def toned_out (* amp_speaker (mod out_gain)))"), "{squashed}");
        assert!(squashed.contains("(param amp_drive @group amp @default 30 @min 1 @max 500 @mod true @mod-mode additive)"), "{squashed}");
        // The speaker's sound reaches the body a sample (at least) later, off axis.
        assert!(squashed.contains("(make-history amp_speaker_h)"), "{squashed}");
        assert!(squashed.contains("(write-history amp_speaker_h amp_speaker)"), "{squashed}");
        assert!(squashed.contains("(def air_speaker_direct (delay (read-history amp_speaker_h) (max 0 (- (/ (* (mod air_speaker_distance) samplerate) 343) 1)) @max-delay 8192))"), "{squashed}");
        assert!(squashed.contains("(sin (* (mod air_speaker_angle) 0.017453292519943295))"), "{squashed}");
        // The room: the floor's path among the reflections.
        assert!(squashed.contains("(delay air_speaker_direct (/ (* (- (sqrt (+ (* (mod air_speaker_distance) (mod air_speaker_distance)) 2.56)) (mod air_speaker_distance)) samplerate) 343) @max-delay 1024)"), "{squashed}");
        // The mount: the body point's acceleration over a sample squared,
        // shared among the string's modes (alternating for the end at 1).
        assert!(squashed.contains("(def bridge_u1 (sum (* (read-history body_s2) bridge_phi_a)))"), "{squashed}");
        assert!(squashed.contains("(def bridge_d2 (+ (- bridge_u (* 2 bridge_u1)) (read-history bridge_u2)))"), "{squashed}");
        assert!(squashed.contains("(def bridge_share (tensor @shape [8] @data [0.6366"), "{squashed}");
        assert!(squashed.contains("(def bridge_push (* bridge_d2 (* -1 (mod bridge_weight) (mod bridge_gain) bridge_share str_mask)))"), "{squashed}");
        assert!(squashed.contains("(modal-advance str_free (+ (* str_G (* plk_F plk_phi_b)) bridge_push))"), "{squashed}");
        assert!(!squashed.contains("param bridge_str_"), "{squashed}");
        // One way by default; with :back the string's end force (last
        // sample but one, where d2 is centred) pushes the body.
        assert!(!squashed.contains("bridge_R"), "{squashed}");
        let two_way = script.replace("'str 1))", "'str 1 :back 1))");
        let back = squash(&generate_instrument(&mut modal_vm(), dir.path().join("lead.lisp"), &two_way).unwrap().dsp);
        assert!(back.contains("(param bridge_back @group bridge @default 1 @min 0 @max 4"), "{back}");
        assert!(back.contains("(def bridge_R (* (mod bridge_weight) (mod bridge_gain) (mod bridge_back) str_mass (sum (* bridge_share str_mask (read-history str_s2) (pow (/ (* twopi str_freqs) (* samplerate body_wref)) 2)))))"), "{back}");
        assert!(back.contains("(* bridge_R bridge_phi_a)"), "{back}");
        assert!(ui(&instrument).contains("\"AMP\""), "{}", ui(&instrument));
        // m/speaker needs an amp; :amp must be one.
        let no_amp = script.replace(":amp (m/amp :drive 30)", "");
        let errors = generate_instrument(&mut modal_vm(), dir.path().join("lead.lisp"), &no_amp).unwrap_err();
        assert!(errors.contains(&"force air: m/speaker needs the patch's :amp".to_string()), "{errors:?}");
        let bad_mount = script.replace("(m/mount 'bridge 'body (list 0.5 0.45) 'str 1)", "(m/mount 'bridge 'str 0.5 'body 2)");
        let errors = generate_instrument(&mut modal_vm(), dir.path().join("lead.lisp"), &bad_mount).unwrap_err();
        assert!(errors.contains(&"mount bridge: body is not a string".to_string()), "{errors:?}");
        assert!(errors.contains(&"mount bridge: end is 0 or 1, got 2".to_string()), "{errors:?}");
        // A mount reads its object's displacement: not a lumped finger's.
        let lumped = script.replace("(m/mount 'bridge 'body (list 0.5 0.45) 'str 1)", "(m/mount 'bridge 'pick 0 'str 1)");
        let errors = generate_instrument(&mut modal_vm(), dir.path().join("lead.lisp"), &lumped).unwrap_err();
        assert!(errors.contains(&"mount bridge: the mount needs a modal object, not pick".to_string()), "{errors:?}");
        let bad_amp = script.replace(":amp (m/amp :drive 30)", ":amp 3");
        let errors = generate_instrument(&mut modal_vm(), dir.path().join("lead.lisp"), &bad_amp).unwrap_err();
        assert!(errors.contains(&"patch: :amp is an m/amp, got 3".to_string()), "{errors:?}");
    }

    #[test]
    fn a_unison_string_is_struck_and_bridged_as_one() {
        let script = "(import eseq.modal :as m)
(m/patch :name \"probe-unison\"
  :objects (list (m/mallet 'ham) (m/string 'str :modes 8 :unison 3 :detune 1.5) (m/plate 'board :grid 2))
  :connections (list (m/felt 'hit 'ham 'str 0.12 :exponent 3)
                     (m/spring 'bridge 'str 0.99 'board (list 0.4 0.5)))
  :listens (list (m/listen 'board (list 0.6 0.3))))";
        let instrument = generate_instrument(&mut modal_vm(), PathBuf::from("unison.lisp"), script).unwrap();
        let squashed = squash(&instrument.dsp);
        // One set of knobs: the copies read the first string's and connection's params.
        assert!(squashed.contains("(param str_detune @group str @default 1.5"), "{squashed}");
        assert!(!squashed.contains("(param str_u1_"), "{squashed}");
        assert!(squashed.contains("(def hit_u2_felt hit_felt)"), "{squashed}");
        assert!(squashed.contains("(def bridge_u1_stiff bridge_stiff)"), "{squashed}");
        // The copies sit +detune and -detune cents away.
        assert!(squashed.contains("(def str_u1_ratio (pow 2 (/ (* 1 str_detune) 1200)))"), "{squashed}");
        assert!(squashed.contains("(def str_u2_ratio (pow 2 (/ (* -1 str_detune) 1200)))"), "{squashed}");
        // Each copy has its own contact state and is bridged, centred in time.
        assert!(squashed.contains("(make-history hit_u2_h)"), "{squashed}");
        assert!(squashed.contains("(spring-centred (- bridge_u2_y_a0 bridge_u2_y_b0) bridge_u2_gap_old"), "{squashed}");
        assert!(!ui(&instrument).contains("str_u1"), "{}", ui(&instrument));
        // The copies reuse the first string's modes, mask and losses, and
        // their connections its points' shapes.
        assert!(squashed.contains("(def str_u1_freqs (* str_freqs str_u1_ratio))"), "{squashed}");
        assert!(squashed.contains("(def str_u2_sigma str_sigma)"), "{squashed}");
        assert!(squashed.contains("(def bridge_u1_phi_b bridge_phi_b)"), "{squashed}");
        assert!(squashed.contains("(def bridge_u1_old_b bridge_old_b)"), "{squashed}");
        // A listen that radiates: a high-pass the mix hears.
        let radiating = script.replace("(m/listen 'board (list 0.6 0.3))", "(m/listen 'board (list 0.6 0.3) :radiate 90)");
        let radiated = generate_instrument(&mut modal_vm(), PathBuf::from("unison.lisp"), &radiating).unwrap();
        let radiated = squash(&radiated.dsp);
        assert!(radiated.contains("(param mic1_radiate @group mic1 @default 90 @min 20 @max 2000 @unit Hz @mod true @mod-mode additive)"), "{radiated}");
        assert!(radiated.contains("(def mic1_out (svf mic1_v (min (mod mic1_radiate) (* 0.45 samplerate)) 0.707 2))"), "{radiated}");
        assert!(radiated.contains("(def mix (* mic1_out (mod mic1_gain)))"), "{radiated}");
        for (args, error) in [
            (":unison 2.5", "string str: :unison is a whole number of strings, got 2.5"),
            (":unison 2 :planes 2", "string str: :unison and :planes 2 do not combine"),
        ] {
            let bad = script.replace(":unison 3 :detune 1.5", args);
            let errors = generate_instrument(&mut modal_vm(), PathBuf::from("unison.lisp"), &bad).unwrap_err();
            assert!(errors.contains(&error.to_string()), "{errors:?}");
        }
    }

    #[test]
    fn knobs_are_modulatable_where_it_costs_nothing() {
        let script = "(import eseq.modal :as m)
(import eseq.modal.materials :as mat)
(m/patch :name \"probe-mod\"
  :objects (list (m/string 'str :made-of mat/nylon :radius-mm (m/by-note 65 0.15 523 1.2))
                 (m/plate 'body :made-of mat/spruce :thick-mm 5 :width 0.5 :height 0.35))
  :connections (list (m/bow 'bow 'str 0.12) (m/spring 'bridge 'str 0.999 'body (list 0.55 0.4)))
  :listens (list (m/listen 'body (list 0.3 0.65))))";
        let param = |dsp: &str, name: &str| {
            let at = dsp.find(&format!("(param {name} ")).unwrap_or_else(|| panic!("no param {name}: {dsp}"));
            dsp[at..at + dsp[at..].find(')').unwrap() + 1].to_string()
        };
        let cheap = squash(&generate_instrument(&mut modal_vm(), PathBuf::from("mod.lisp"), script).unwrap().dsp);
        assert!(cheap.contains("(def mod1 (in 6 @name mod1 @modulator 1))"), "{cheap}");
        // Per-sample work reads the modulated value ...
        assert!(param(&cheap, "bow_pressure").ends_with("@mod true @mod-mode additive)"), "{cheap}");
        assert!(cheap.contains("(* (mod bow_pressure) bow_env (mod bow_weight))"), "{cheap}");
        // ... work on a change tick holds it on the tick, whose change
        // detection reads the knob and watches the sources itself ...
        assert!(cheap.contains("(event-hold (mod str_radius_c4) bynote_tick)"), "{cheap}");
        assert!(cheap.contains("(- str_radius_c4 (read-history bynote_in"), "{cheap}");
        assert!(!cheap.contains("(- (mod str_radius_c4)"), "{cheap}");
        assert!(cheap.contains("(gt (abs (- mod1 (read-history bynote_src0))) 0)"), "{cheap}");
        // ... and a knob only setup reads (the plate's size, a point on it)
        // is left alone, with no mod_tick.
        assert!(!param(&cheap, "body_thick").contains("@mod"), "{cheap}");
        assert!(!param(&cheap, "mic1_body_x").contains("@mod"), "{cheap}");
        assert!(!cheap.contains("mod_tick"), "{cheap}");
        // :modulate :all holds those on mod_tick: setup reads the held value,
        // per-sample code its latched results.
        let all = squash(&generate_instrument(&mut modal_vm(), PathBuf::from("mod.lisp"),
                                                    &script.replace("\"probe-mod\"", "\"probe-mod\" :modulate :all")).unwrap().dsp);
        assert!(param(&all, "body_thick").ends_with("@mod true @mod-mode additive)"), "{all}");
        assert!(all.contains("(def body_thick_modh (event-hold (mod body_thick) mod_tick))"), "{all}");
        assert!(all.contains("(* body_thick_modh 0.001)"), "{all}");
        assert!(all.contains("(def body_a_mod (latch body_a mod_tick))"), "{all}");
        assert!(all.contains("(def mic1_phi_mod (latch mic1_phi mod_tick))"), "{all}");
    }

    #[test]
    fn by_note_values_follow_the_played_note() {
        let script = "(import eseq.modal :as m)
(m/patch :name \"probe-scale\"
  :objects (list (m/mallet 'ham) (m/string 'str :length (m/by-note 27.5 2.0 262 0.66 4186 0.05)
                                               :const-loss (m/by-note 27.5 0 4186 1)))
  :connections (list (m/felt 'hit 'ham 'str (m/by-note 27.5 0.12 4186 0.06)))
  :listens (list (m/listen 'str 0.3)))";
        let instrument = generate_instrument(&mut modal_vm(), PathBuf::from("scale.lisp"), script).unwrap();
        let squashed = squash(&instrument.dsp);
        // The knob is the value at C4 (log-interpolated between 27.5 and 262 Hz) ...
        assert!(squashed.contains("(param str_length_c4 @group str @default 0.6605 @min 0.2 @max 20 @unit m @mod true @mod-mode additive)"), "{squashed}");
        assert!(ui(&instrument).contains("\"str.str_length_c4\""), "{}", ui(&instrument));
        // ... and the curve scales it at the note; a curve through 0 shifts it.
        // Worked out when the pitch or a knob changes, latched back to audio rate.
        assert!(squashed.contains("(def note_ln (log (event-hold f0 bynote_tick)))"), "{squashed}");
        assert!(squashed.contains("(def str_length_e (* (event-hold (mod str_length_c4) bynote_tick) (exp (+"), "{squashed}");
        assert!(squashed.contains("(def str_length (latch str_length_e bynote_tick))"), "{squashed}");
        assert!(squashed.contains("(def str_loss_e (+ (event-hold (mod str_loss_c4) bynote_tick) (+ -0.448"), "{squashed}");
        // Access points follow too; the code reads the note's value.
        assert!(squashed.contains("(def hit_str_at (latch hit_str_at_e bynote_tick))"), "{squashed}");
        assert!(squashed.contains("(sin (* str_n 3.14159265 hit_str_at))"), "{squashed}");
        for (curve, error) in [
            ("(m/by-note 27.5 2.0)", "str_length: by-note takes pitch value pairs, at least two, got (27.5 2)"),
            ("(m/by-note 262 0.66 27.5 2.0)", "str_length: by-note's pitches must be positive and rising, got (262 27.5)"),
            ("(m/by-note 27.5 40 4186 30)", "str_length: m/by-note is 35.16 at C4, outside its knob's 0.2..20"),
        ] {
            let bad = script.replace("(m/by-note 27.5 2.0 262 0.66 4186 0.05)", curve);
            let errors = generate_instrument(&mut modal_vm(), PathBuf::from("scale.lisp"), &bad).unwrap_err();
            assert!(errors.contains(&error.to_string()), "{errors:?}");
        }
        // A curve given as a point is checked too.
        let bad_at = script.replace("(m/by-note 27.5 0.12 4186 0.06)", "(m/by-note 27.5 0.12)");
        let errors = generate_instrument(&mut modal_vm(), PathBuf::from("scale.lisp"), &bad_at).unwrap_err();
        assert!(errors.contains(&"felt hit b: by-note takes pitch value pairs, at least two, got (27.5 0.12)".to_string()), "{errors:?}");
    }

    /// A drum: two membranes coupled by m/cavity's air, snare wires laid
    /// under the bottom one by m/snare-bed. Both are solved as exciters in
    /// their own small spaces (air modes, touches) and push the objects
    /// through ordinary force terms.
    #[test]
    fn a_cavity_couples_two_heads_and_a_snare_bed_rattles_one() {
        use crate::agent::verify::{verify_sources, ArtifactKind, VerifyMode};
        let script = "(import eseq.modal :as m)
(m/patch :name \"probe-drum\"
  :objects (list (m/mallet 'stick) (m/membrane 'top :modes 12 :bend 1) (m/membrane 'bot :tune 1.2 :modes 10)
                 (m/cavity 'air 'top 'bot :modes 3) (m/snare-wires 'wires :count 2 :modes 2))
  :connections (list (m/felt 'hit 'stick 'top (list 0.3 0)) (m/snare-bed 'snares 'bot 'wires :points 2 :iterations 2))
  :listens (list (m/listen 'top (list 0.5 0.5))))";
        let instrument = generate_instrument(&mut modal_vm(), PathBuf::from("drum.lisp"), script).unwrap();
        let squashed = squash(&instrument.dsp);
        // The lowest air modes of the shell: the uniform air spring, then the
        // (1,0) and (2,0) radial families (zeros of J_m').
        assert!(squashed.contains("(tensor @shape [3] @data [0 1.841184 3.054237])"), "{squashed}");
        // A head has the modes it asks for, its coefficients held (worked out
        // when an input changes), only Bend's stretch moving them per sample;
        // its points are x/y across its face, from the (r theta) written.
        assert!(squashed.contains("(make-history top_s1 @shape [12])"), "{squashed}");
        assert!(squashed.contains("(make-history bot_s1 @shape [10])"), "{squashed}");
        assert!(squashed.contains("(def top_a_c (latch top_a_c_e coef_tick))"), "{squashed}");
        assert!(squashed.contains("(def top_a (max (min (- top_a_c (* (read-history top_tens) top_a1)) top_amax) (* -1 top_amax)))"), "{squashed}");
        assert!(squashed.contains("(param hit_top_x @group hit @default 0.3"), "{squashed}");
        assert!(squashed.contains("(param hit_top_y @group hit @default 0"), "{squashed}");
        // The air pushes both heads and its own masses as force terms.
        assert!(squashed.contains("(def top_next (modal-advance top_free (* top_G (+ (* hit_F hit_phi_b) air_solve_push_t))))"), "{squashed}");
        assert!(squashed.contains("(def air_next (modal-advance air_free (* air_G air_solve_push_air)))"), "{squashed}");
        // The wires push the bottom head up and themselves down, after the
        // projected Jacobi passes over the touches (forces never negative).
        assert!(squashed.contains("(def bot_next (modal-advance bot_free (* bot_G (+ snares_push_h air_solve_push_b))))"), "{squashed}");
        assert!(squashed.contains("(def wires_next (modal-advance wires_free (* wires_G (* -1 snares_push_w))))"), "{squashed}");
        assert!(squashed.contains("(def snares_P0 (max 0 (* -1 snares_kD snares_d0)))"), "{squashed}");
        assert!(squashed.contains("(def snares_P1 (max 0 (- snares_P0 (* snares_kD snares_d1))))"), "{squashed}");
        assert!(squashed.contains("(param snares_drag @group snares"), "{squashed}");
        // The gap knob reads in the mics' units.
        assert!(squashed.contains("(def snares_gapd (/ snares_gap (* (/ samplerate (* stick_speed 10))"), "{squashed}");
        assert!(squashed.contains("(param snares_snap @group snares"), "{squashed}");
        let report = verify_sources(ArtifactKind::Instrument, VerifyMode::Library, &instrument.dsp, Some(ui(&instrument)), 44_100, None)
            .unwrap_or_else(|failure| panic!("{failure}\n{}", instrument.dsp));
        assert!(!report.audition.silent);
        // A cavity needs two membranes; a bed a membrane and snare wires.
        let bad = script.replace("(m/cavity 'air 'top 'bot", "(m/cavity 'air 'top 'stick").replace("(m/snare-bed 'snares 'bot 'wires", "(m/snare-bed 'snares 'wires 'bot");
        let errors = generate_instrument(&mut modal_vm(), PathBuf::from("drum.lisp"), &bad).unwrap_err();
        assert!(errors.contains(&"cavity air: stick is not a membrane (m/membrane)".to_string()), "{errors:?}");
        assert!(errors.contains(&"bed snares: wires is not a membrane (m/membrane)".to_string()), "{errors:?}");
        assert!(errors.contains(&"bed snares: bot is not snare wires (m/snare-wires)".to_string()), "{errors:?}");
    }

    /// The Autechre-style pieces: Modalys pitch controls (Note in / Shift /
    /// Note) and a material skin on a head, a heavy snare string in contact
    /// at a gap read in the mics' units, a spider web solved offline into
    /// modes, per-listen left/right levels making the output stereo, the
    /// output filter, and the drum pad placing every point on the head.
    #[test]
    fn a_stereo_snare_with_string_snares_a_web_and_a_drum_pad() {
        use crate::agent::verify::{verify_sources, ArtifactKind, VerifyMode};
        let script = "(import eseq.modal :as m)
(import eseq.modal.materials :as mat)
(m/patch :name \"probe-ae\"
  :objects (list (m/mallet 'stick) (m/membrane 'head :modes 12 :note 44 :shift 12 :made-of (list mat/water mat/steel) :thick-mm 3)
                 (m/string 'sn :note 25 :modes 8 :mass 100 :made-of mat/spider-silk)
                 (m/web 'web :spokes 5 :rings 3 :hub 0.05 :modes 6))
  :connections (list (m/felt 'hit 'stick 'head (list 0.3 0)) (m/strike 'touch 'head (list 0.4 2) 'sn 0.4 :mic-gap true)
                     (m/spring 'tie 'web (list 0 0 0) 'head (list 0.9 1)))
  :listens (list (m/listen 'head (list 0.5 1) :left 1 :right 0) (m/listen 'sn 0.2 :left 0 :right 1))
  :out-filter (dict :type :highpass :cutoff 110)
  :pad 'head)";
        let instrument = generate_instrument(&mut modal_vm(), PathBuf::from("ae.lisp"), script).unwrap();
        let squashed = squash(&instrument.dsp);
        // Note in blends following the note (shifted) with a fixed Note.
        assert!(squashed.contains("(param head_note_in @group head"), "{squashed}");
        assert!(squashed.contains("(def head_f0 (* (+ (* (event-hold (mod head_note_in) coef_tick) (event-hold f0 coef_tick) (pow 2 (/ (event-hold (mod head_shift) coef_tick) 12)))"), "{squashed}");
        assert!(squashed.contains("(def sn_mass sn_mass_k)"), "{squashed}");
        assert!(squashed.contains("(def touch_offd (/ touch_offset (* (/ samplerate (* stick_speed 10))"), "{squashed}");
        // The web's modes are numbers by now, played as a data object.
        assert!(squashed.contains("(def web_freqs (* web_hz web_ratio))"), "{squashed}");
        // Stereo: each listen at its own left and right level.
        assert!(squashed.contains("(def mix_l (+ (* mic1_v (mod mic1_gain) (mod mic1_left))"), "{squashed}");
        assert!(squashed.contains("(out voice_l 1 @name left)"), "{squashed}");
        assert!(squashed.contains("(out voice_r 2 @name right)"), "{squashed}");
        assert!(squashed.contains("(param out_filter @group out @options [\"lowpass\" \"bandpass\" \"highpass\"] @default 2)"), "{squashed}");
        // The pad: the stick (blue), the snare's and the web's contacts
        // (grey), the mic (green), each dragging its point's x and y.
        let ui_text = squash(&ui(&instrument));
        assert!(ui_text.contains("(drum-pad :debug-name \"modal-drum-pad\" :count 4"), "{ui_text}");
        assert!(ui_text.contains("'(\"#2f7fe0\" \"#8a8a8a\" \"#8a8a8a\" \"#36b04f\")"), "{ui_text}");
        assert!(ui_text.contains(":x1 (mx-bind \"hit.hit_head_x\") :y1 (mx-bind \"hit.hit_head_y\")"), "{ui_text}");
        let report = verify_sources(ArtifactKind::Instrument, VerifyMode::Library, &instrument.dsp, Some(ui(&instrument)), 44_100, None)
            .unwrap_or_else(|failure| panic!("{failure}\n{}", instrument.dsp));
        assert!(!report.audition.silent);
    }

    #[test]
    fn rejects_a_script_that_does_not_end_in_a_program() {
        let mut vm = modal_vm();
        let errors = generate_instrument(
            &mut vm,
            PathBuf::from("x.lisp"),
            "(import eseq.modal :as m)\n42",
        )
        .unwrap_err();
        assert!(errors[0].contains("must be a program"), "{errors:?}");
    }
}
