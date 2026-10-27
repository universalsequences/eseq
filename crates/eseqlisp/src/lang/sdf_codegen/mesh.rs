//! The two stages of a mesh `defwidget` (one with a `:vertex` body) for both
//! shader backends: a vertex function run once per mesh vertex, and a
//! fragment function that reads the varyings it writes. Both are emitted in
//! one module with [`MESH_PREAMBLE_MSL`] / [`MESH_PREAMBLE_WGSL`] ahead of
//! them, so a backend compiles `shader_source` as it is.
//!
//! The ABI both backends bind (docs/defwidget-vertex-stage.md):
//!   vertex   buffer 0 indices (u32, three per triangle; the draw is
//!            non-indexed, so `vertex_id` walks this list), 1 positions and
//!            2 normals (three floats per mesh vertex), 3 the per-vertex
//!            table (`table-cols` floats per vertex), 4 [`MeshUniforms`],
//!            5 the array states, one after another
//!   fragment buffer 0 [`MeshUniforms`], 1 the array states
//!
//! [`MeshUniforms`]: crate::widget_render::mesh_widget::MeshUniforms

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use super::{
    CodegenError, Expression, SdfShaderOptions, ShaderEmitter, ShaderLanguage,
    emit_uniform_declarations_from, metal_safe_symbol, uniform_layout, walk_free_symbols,
};
use crate::theme;

/// At most this many array states per widget (two `float4`s of lengths).
pub const MAX_MESH_ARRAYS: usize = 8;
/// At most this many floats across a widget's array states.
pub const MAX_MESH_ARRAY_FLOATS: usize = 4096;
/// At most this many named varyings from the vertex stage.
pub const MAX_MESH_VARYINGS: usize = 4;

/// One `(name :floats n)` entry of a mesh widget's `:state`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshArrayState {
    pub name: String,
    pub len: usize,
}

/// Where each array state starts in the arrays buffer: one after another,
/// in declaration order.
pub fn mesh_array_offsets(arrays: &[MeshArrayState]) -> Vec<usize> {
    arrays
        .iter()
        .scan(0, |offset, array| {
            let start = *offset;
            *offset += array.len;
            Some(start)
        })
        .collect()
}

/// A value the vertex stage hands the fragment stage, by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshVarying {
    pub name: String,
    /// `float`, `float2`, `float3` or `float4`.
    pub type_name: &'static str,
}

pub struct MeshShaderOutput {
    /// A complete module: preamble, `mesh_vert` and `mesh_frag`.
    pub shader_source: String,
    pub varyings: Vec<MeshVarying>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MeshStage {
    Vertex,
    Fragment,
}

/// The mesh half of a [`ShaderEmitter`]: which stage it is emitting, the
/// array states it may index, and (vertex stage) what `out` wrote.
pub(super) struct MeshEmit {
    stage: MeshStage,
    /// name -> (slot, offset, len).
    arrays: HashMap<String, (usize, usize, usize)>,
    out: Option<MeshOut>,
}

struct MeshOut {
    position: String,
    varyings: Vec<(MeshVarying, String)>,
}

/// Builtins only the vertex stage has: the vertex's attributes.
const VERTEX_ONLY_SYMBOLS: [&str; 3] = ["position", "normal", "vertex-id"];

pub const MESH_PREAMBLE_MSL: &str = r#"#include <metal_stdlib>
using namespace metal;

struct MeshUniforms {
    float4 state[4];
    float4 center_radius;
    float4 view;
    float4 table_info;
    float4 target_size;
    float4 array_len[2];
};

// Turn the mesh about its z axis by yaw, then tilt the camera up by pitch:
// pitch 0 looks along +y at a z-up object, pi/2 looks straight down. The
// result is in view space: x right, y up, z toward the camera.
float3 eseq_orbit_rotate(float3 p, float yaw, float pitch) {
    float cy = cos(yaw);
    float sy = sin(yaw);
    float3 a = float3(cy * p.x - sy * p.y, sy * p.x + cy * p.y, p.z);
    float cp = cos(pitch);
    float sp = sin(pitch);
    return float3(a.x, cp * a.z + sp * a.y, sp * a.z - cp * a.y);
}

// Clip position of mesh point p seen by an orbit camera 3.2 mesh radii from
// the mesh centre, fitted to a target of the given aspect (width / height).
float4 eseq_orbit_project(float3 p, float yaw, float pitch, float zoom, float aspect,
                          float4 center_radius) {
    float3 v = eseq_orbit_rotate((p - center_radius.xyz) / max(center_radius.w, 1e-6), yaw, pitch);
    float depth = 3.2 - v.z;
    float focal = 2.6 * max(zoom, 0.01);
    float2 fit = aspect >= 1.0 ? float2(1.0 / aspect, 1.0) : float2(1.0, aspect);
    float near = 0.05;
    float far = 8.0;
    return float4(v.xy * focal * fit, far * (depth - near) / (far - near), depth);
}
"#;

pub const MESH_PREAMBLE_WGSL: &str = r#"struct MeshUniforms {
    state: array<vec4<f32>, 4>,
    center_radius: vec4<f32>,
    view: vec4<f32>,
    table_info: vec4<f32>,
    target_size: vec4<f32>,
    array_len: array<vec4<f32>, 2>,
};

@group(0) @binding(0) var<storage, read> mesh_indices: array<u32>;
@group(0) @binding(1) var<storage, read> mesh_positions: array<f32>;
@group(0) @binding(2) var<storage, read> mesh_normals: array<f32>;
@group(0) @binding(3) var<storage, read> mesh_table: array<f32>;
@group(0) @binding(4) var<uniform> mesh_u: MeshUniforms;
@group(0) @binding(5) var<storage, read> mesh_arrays: array<f32>;

fn eseq_orbit_rotate(p: vec3<f32>, yaw: f32, pitch: f32) -> vec3<f32> {
    let cy = cos(yaw);
    let sy = sin(yaw);
    let a = vec3<f32>(cy * p.x - sy * p.y, sy * p.x + cy * p.y, p.z);
    let cp = cos(pitch);
    let sp = sin(pitch);
    return vec3<f32>(a.x, cp * a.z + sp * a.y, sp * a.z - cp * a.y);
}

fn eseq_orbit_project(p: vec3<f32>, yaw: f32, pitch: f32, zoom: f32, aspect: f32,
                      center_radius: vec4<f32>) -> vec4<f32> {
    let v = eseq_orbit_rotate((p - center_radius.xyz) / max(center_radius.w, 1e-6), yaw, pitch);
    let depth = 3.2 - v.z;
    let focal = 2.6 * max(zoom, 0.01);
    let fit = select(vec2<f32>(1.0, aspect), vec2<f32>(1.0 / aspect, 1.0), aspect >= 1.0);
    let near = 0.05;
    let far = 8.0;
    return vec4<f32>(v.xy * focal * fit, far * (depth - near) / (far - near), depth);
}
"#;

impl ShaderEmitter {
    /// A mesh-only form, or None to emit `head` as any shader would.
    pub(super) fn emit_mesh_form(
        &mut self,
        head: &str,
        args: &[Expression],
    ) -> Option<Result<String, CodegenError>> {
        let mesh = self.mesh.as_ref()?;
        let shadowed = self.scopes.iter().any(|scope| scope.contains_key(head));
        if !shadowed && let Some(&(_, offset, len)) = mesh.arrays.get(head) {
            return Some(self.emit_array_read(head, offset, len, args));
        }
        let vertex = mesh.stage == MeshStage::Vertex;
        let vertex_only = |form: &str| {
            Err(CodegenError::UnsupportedExpression(format!(
                "({form} …) reads the vertex: only in :vertex"
            )))
        };
        Some(match head {
            "out" if vertex => self.emit_mesh_out(args),
            "out" => Err(CodegenError::UnsupportedExpression(
                "(out …) ends the :vertex body, not the :shader".into(),
            )),
            "dot" => match self.mesh_dot_operands(args) {
                Some(_) if !vertex => vertex_only("table-row"),
                Some((array, row)) => self.emit_table_dot(&array, row),
                None => return None,
            },
            "table-row" => Err(CodegenError::UnsupportedExpression(
                "(table-row r) is only an operand of (dot <array> (table-row r))".into(),
            )),
            "table" if vertex => self.emit_table_read(args),
            "table" => vertex_only("table"),
            "size" => self.emit_array_size(args),
            "orbit-project" => self.emit_orbit_project(args),
            "orbit-rotate" => match args {
                [_, _, _] => self.emit_func_call("eseq_orbit_rotate", args),
                _ => Err(CodegenError::UnsupportedExpression(
                    "orbit-rotate takes (v yaw pitch)".into(),
                )),
            },
            _ => return None,
        })
    }

    pub(super) fn mesh_expr_type(&self, head: &str) -> Option<&'static str> {
        self.mesh.as_ref()?;
        match head {
            "orbit-project" => Some("float4"),
            "orbit-rotate" => Some("float3"),
            _ => None,
        }
    }

    fn array_name<'a>(&self, expr: &'a Expression) -> Option<&'a str> {
        let Expression::Symbol(name) = expr else {
            return None;
        };
        let mesh = self.mesh.as_ref()?;
        (mesh.arrays.contains_key(name) && !self.scopes.iter().any(|scope| scope.contains_key(name)))
            .then_some(name.as_str())
    }

    /// `(dot amps (table-row r))` (either order): the array and `r`.
    fn mesh_dot_operands<'a>(&self, args: &'a [Expression]) -> Option<(String, &'a Expression)> {
        let [a, b] = args else {
            return None;
        };
        let row_of = |expr: &'a Expression| match expr {
            Expression::List(items) => match items.as_slice() {
                [Expression::Symbol(head), row] if head == "table-row" => Some(row),
                _ => None,
            },
            _ => None,
        };
        if let (Some(array), Some(row)) = (self.array_name(a), row_of(b)) {
            return Some((array.to_string(), row));
        }
        if let (Some(row), Some(array)) = (row_of(a), self.array_name(b)) {
            return Some((array.to_string(), row));
        }
        None
    }

    fn uint_cast(&self) -> &'static str {
        match self.language {
            ShaderLanguage::Metal => "uint",
            ShaderLanguage::Wgsl => "u32",
        }
    }

    /// `type_name name = value;` with `type_name` already in this language
    /// (`uint` / `u32`), which [`ShaderEmitter::declaration`] does not map.
    fn declaration_of(&self, type_name: &str, name: &str, value: &str) -> String {
        match self.language {
            ShaderLanguage::Metal => format!("{type_name} {name} = {value};"),
            ShaderLanguage::Wgsl => format!("let {name}: {type_name} = {value};"),
        }
    }

    /// `(amps i)`: element `i` of an array state, clamped into it.
    fn emit_array_read(
        &mut self,
        name: &str,
        offset: usize,
        len: usize,
        args: &[Expression],
    ) -> Result<String, CodegenError> {
        let [index] = args else {
            return Err(CodegenError::UnsupportedExpression(format!(
                "({name} i): array state {name} takes one index"
            )));
        };
        let index = self.emit_expr(index)?;
        let uint = self.uint_cast();
        Ok(format!(
            "mesh_arrays[{offset}u + min({uint}(max({index}, 0.0)), {}u)]",
            len.saturating_sub(1)
        ))
    }

    /// `(size amps)`: how many floats the instance gave `amps`.
    fn emit_array_size(&mut self, args: &[Expression]) -> Result<String, CodegenError> {
        let [array] = args else {
            return Err(CodegenError::UnsupportedExpression("size takes one array state".into()));
        };
        let Some(name) = self.array_name(array) else {
            return Err(CodegenError::UnsupportedExpression(
                "(size a): a must be an array state".into(),
            ));
        };
        let (slot, _, _) = self.mesh.as_ref().unwrap().arrays[name];
        Ok(format!("mesh_u.array_len[{}].{}", slot / 4, ["x", "y", "z", "w"][slot % 4]))
    }

    /// A mesh row index from `row`, clamped to the mesh's vertices.
    fn mesh_row_index(&mut self, row: &Expression) -> Result<String, CodegenError> {
        let row = self.emit_expr(row)?;
        let uint = self.uint_cast();
        Ok(format!(
            "min({uint}(max({row}, 0.0)), {uint}(max(vertex_count, 1.0)) - 1u)"
        ))
    }

    /// `(dot amps (table-row r))`: the sum over k of amps[k] times row r's
    /// k-th table entry, for k below both the array's length and the table's
    /// width. With a mode-shape table and mode amplitudes this is the modal
    /// displacement of vertex r.
    fn emit_table_dot(&mut self, array: &str, row: &Expression) -> Result<String, CodegenError> {
        let (_, offset, len) = self.mesh.as_ref().unwrap().arrays[array];
        let row = self.mesh_row_index(row)?;
        let [acc, r, cols, terms, k] = [(); 5].map(|_| self.fresh_var());
        self.statements.push(self.mutable_declaration("float", &acc, "0.0"));
        let uint = self.uint_cast();
        let lines = [
            "{".to_string(),
            self.declaration_of(uint, &r, &row),
            self.declaration_of(uint, &cols, &format!("{uint}(table_cols)")),
            self.declaration_of(uint, &terms, &format!("min({cols}, {len}u)")),
            match self.language {
                ShaderLanguage::Metal => format!("for (uint {k} = 0u; {k} < {terms}; {k}++) {{"),
                ShaderLanguage::Wgsl => format!("for (var {k}: u32 = 0u; {k} < {terms}; {k} = {k} + 1u) {{"),
            },
            format!("{acc} = {acc} + mesh_arrays[{offset}u + {k}] * mesh_table[{r} * {cols} + {k}];"),
            "}".to_string(),
            "}".to_string(),
        ];
        self.statements.extend(lines);
        Ok(acc)
    }

    /// `(table r k)`: row r's k-th table entry, 0 for a mesh without a table.
    fn emit_table_read(&mut self, args: &[Expression]) -> Result<String, CodegenError> {
        let [row, column] = args else {
            return Err(CodegenError::UnsupportedExpression("table takes (row column)".into()));
        };
        let row = self.mesh_row_index(row)?;
        let column = self.emit_expr(column)?;
        let value = self.fresh_var();
        self.statements.push(self.mutable_declaration("float", &value, "0.0"));
        let uint = self.uint_cast();
        self.statements.push(format!(
            "if (table_cols > 0.5) {{ {value} = mesh_table[({row}) * {uint}(table_cols) + min({uint}(max({column}, 0.0)), {uint}(table_cols) - 1u)]; }}"
        ));
        Ok(value)
    }

    /// `(orbit-project p yaw pitch [zoom])`: p's clip position.
    fn emit_orbit_project(&mut self, args: &[Expression]) -> Result<String, CodegenError> {
        if !(3..=4).contains(&args.len()) {
            return Err(CodegenError::UnsupportedExpression(
                "orbit-project takes (p yaw pitch [zoom])".into(),
            ));
        }
        let mut parts = args
            .iter()
            .map(|arg| self.emit_expr(arg))
            .collect::<Result<Vec<_>, _>>()?;
        if parts.len() == 3 {
            parts.push("1.0".into());
        }
        Ok(format!(
            "eseq_orbit_project({}, aspect, mesh_u.center_radius)",
            parts.join(", ")
        ))
    }

    /// `(out :position clip :varyings ((name value) …))`: what the vertex
    /// stage writes. Recorded for the wrapper; the form itself has no value.
    fn emit_mesh_out(&mut self, args: &[Expression]) -> Result<String, CodegenError> {
        if self.mesh.as_ref().unwrap().out.is_some() {
            return Err(CodegenError::UnsupportedExpression(
                "the :vertex body writes (out …) once".into(),
            ));
        }
        let mut position = None;
        let mut varyings_expr = None;
        let mut i = 0;
        while i < args.len() {
            match (&args[i], args.get(i + 1)) {
                (Expression::Keyword(key), Some(value)) if key == "position" => position = Some(value),
                (Expression::Keyword(key), Some(value)) if key == "varyings" => {
                    varyings_expr = Some(value)
                }
                (Expression::Keyword(key), _) => {
                    return Err(CodegenError::UnsupportedExpression(format!(
                        "(out …) takes :position and :varyings, not :{key}"
                    )));
                }
                _ => {
                    return Err(CodegenError::UnsupportedExpression(
                        "(out :position clip :varyings ((name value) …))".into(),
                    ));
                }
            }
            i += 2;
        }
        let Some(position) = position else {
            return Err(CodegenError::UnsupportedExpression("(out …) needs :position".into()));
        };
        let mut position_value = self.emit_expr(position)?;
        if self.expr_type(position) == Some("float3") {
            position_value = format!("{}({position_value}, 1.0)", self.constructor("float4"));
        }
        let mut varyings: Vec<(MeshVarying, String)> = Vec::new();
        let pairs: &[Expression] = match varyings_expr {
            None => &[],
            Some(Expression::List(pairs)) => pairs,
            Some(_) => {
                return Err(CodegenError::UnsupportedExpression(
                    ":varyings is a list of (name value)".into(),
                ));
            }
        };
        for pair in pairs {
            let Expression::List(pair) = pair else {
                return Err(CodegenError::UnsupportedExpression(
                    ":varyings is a list of (name value)".into(),
                ));
            };
            let [Expression::Symbol(name), value] = pair.as_slice() else {
                return Err(CodegenError::UnsupportedExpression(
                    "a varying is (name value)".into(),
                ));
            };
            if varyings.iter().any(|(varying, _)| &varying.name == name) {
                return Err(CodegenError::UnsupportedExpression(format!(
                    "varying {name} written twice"
                )));
            }
            if self.expr_is_bool(value) {
                return Err(CodegenError::UnsupportedExpression(format!(
                    "varying {name} is a bool: pass a number"
                )));
            }
            let type_name = self.expr_type(value).unwrap_or("float");
            let value = self.emit_expr(value)?;
            varyings.push((MeshVarying { name: name.clone(), type_name }, value));
        }
        if varyings.len() > MAX_MESH_VARYINGS {
            return Err(CodegenError::UnsupportedExpression(format!(
                "{} varyings, over the {MAX_MESH_VARYINGS} a mesh widget has",
                varyings.len()
            )));
        }
        self.mesh.as_mut().unwrap().out = Some(MeshOut {
            position: position_value,
            varyings,
        });
        Ok("0.0".into())
    }
}

fn varying_field(name: &str) -> String {
    format!("vary_{}", metal_safe_symbol(name))
}

/// The uniforms both stages read, as locals named like their Lisp symbols
/// (`orbit-yaw` reads `orbit_yaw`).
fn emit_common_declarations(shader: &mut String, emitter: &ShaderEmitter) {
    let lines: [(&'static str, &str, &str); 9] = [
        ("float", "aspect", "mesh_u.view.x"),
        ("float", "itime", "mesh_u.view.y"),
        ("float", "orbit_yaw", "mesh_u.view.z"),
        ("float", "orbit_pitch", "mesh_u.view.w"),
        ("float3", "mesh_center", "mesh_u.center_radius.xyz"),
        ("float", "mesh_radius", "mesh_u.center_radius.w"),
        ("float", "table_cols", "mesh_u.table_info.x"),
        ("float", "table_peak", "mesh_u.table_info.y"),
        ("float", "vertex_count", "mesh_u.table_info.z"),
    ];
    for (type_name, name, value) in lines {
        writeln!(shader, "    {}", emitter.declaration(type_name, name, value)).unwrap();
    }
}

fn emit_mesh_uniform_declarations(
    shader: &mut String,
    state_symbols: &[String],
    language: ShaderLanguage,
) {
    emit_uniform_declarations_from(shader, state_symbols, language, |idx| {
        format!("mesh_u.state[{}].{}", idx / 4, ["x", "y", "z", "w"][idx % 4])
    });
}


fn check_arrays(arrays: &[MeshArrayState], state_symbols: &[String]) -> Result<(), CodegenError> {
    let error = |message: String| Err(CodegenError::UnsupportedExpression(message));
    if arrays.len() > MAX_MESH_ARRAYS {
        return error(format!("{} array states, over the {MAX_MESH_ARRAYS} a widget has", arrays.len()));
    }
    let total: usize = arrays.iter().map(|array| array.len).sum();
    if total > MAX_MESH_ARRAY_FLOATS {
        return error(format!("array states hold {total} floats, over the {MAX_MESH_ARRAY_FLOATS} budget"));
    }
    let mut seen = HashSet::new();
    for array in arrays {
        if array.len == 0 {
            return error(format!("array state {} has no floats", array.name));
        }
        if !seen.insert(&array.name) || state_symbols.contains(&array.name) {
            return error(format!("state {} declared twice", array.name));
        }
    }
    Ok(())
}

/// Reading a vertex attribute in the fragment stage is a codegen error, not
/// an undeclared identifier in the backend's compiler.
fn check_fragment_symbols(fragment: &Expression) -> Result<(), CodegenError> {
    let mut misuse = None;
    walk_free_symbols(fragment, &mut Vec::new(), &mut |name, scopes| {
        if misuse.is_none()
            && VERTEX_ONLY_SYMBOLS.contains(&name)
            && !scopes.iter().any(|scope| scope.contains(name))
        {
            misuse = Some(name.to_string());
        }
    });
    match misuse {
        Some(name) => Err(CodegenError::UnsupportedExpression(format!(
            "{name} is a vertex attribute: read it in :vertex and pass it as a varying"
        ))),
        None => Ok(()),
    }
}

fn mesh_emitter(
    language: ShaderLanguage,
    stage: MeshStage,
    state_symbols: &[String],
    arrays: &[MeshArrayState],
    theme: theme::Theme,
) -> ShaderEmitter {
    let mut emitter =
        ShaderEmitter::new(language, uniform_layout(state_symbols), theme, SdfShaderOptions::default());
    emitter.mesh = Some(MeshEmit {
        stage,
        arrays: arrays
            .iter()
            .zip(mesh_array_offsets(arrays))
            .enumerate()
            .map(|(slot, (array, offset))| (array.name.clone(), (slot, offset, array.len)))
            .collect(),
        out: None,
    });
    let mut types = HashMap::from([("mesh-center".to_string(), "float3")]);
    if stage == MeshStage::Vertex {
        types.insert("position".into(), "float3");
        types.insert("normal".into(), "float3");
    }
    emitter.type_scopes.push(types);
    emitter
}

pub(super) fn compile_mesh_shader(
    language: ShaderLanguage,
    vertex: &Expression,
    fragment: &Expression,
    state_symbols: &[String],
    arrays: &[MeshArrayState],
    theme: theme::Theme,
) -> Result<MeshShaderOutput, CodegenError> {
    check_arrays(arrays, state_symbols)?;
    check_fragment_symbols(fragment)?;

    let mut vert = mesh_emitter(language, MeshStage::Vertex, state_symbols, arrays, theme.clone());
    vert.emit_expr(vertex)?;
    let Some(out) = vert.mesh.as_mut().unwrap().out.take() else {
        return Err(CodegenError::UnsupportedExpression(
            "the :vertex body must end in (out :position clip …)".into(),
        ));
    };
    let varyings: Vec<MeshVarying> = out.varyings.iter().map(|(varying, _)| varying.clone()).collect();

    let mut frag = mesh_emitter(language, MeshStage::Fragment, state_symbols, arrays, theme);
    let input = frag.input_name();
    frag.scopes.push(
        varyings
            .iter()
            .map(|varying| (varying.name.clone(), format!("{input}.{}", varying_field(&varying.name))))
            .collect(),
    );
    frag.type_scopes.push(
        varyings
            .iter()
            .filter(|varying| varying.type_name != "float")
            .map(|varying| (varying.name.clone(), varying.type_name))
            .collect(),
    );
    let is_color = super::expr_returns_float4(fragment)
        || matches!(frag.expr_type(fragment), Some("float3" | "float4"));
    if !is_color {
        return Err(CodegenError::UnsupportedExpression(
            "a mesh widget's :shader returns a colour".into(),
        ));
    }
    let color = frag.emit_color_expr(fragment)?;

    let mut shader = String::with_capacity(4096);
    match language {
        ShaderLanguage::Metal => {
            shader.push_str(MESH_PREAMBLE_MSL);
            shader.push_str("\nstruct MeshVaryings {\n    float4 position [[position]];\n");
            for varying in &varyings {
                writeln!(shader, "    {} {};", varying.type_name, varying_field(&varying.name)).unwrap();
            }
            shader.push_str("};\n\n");
            shader.push_str(
                "vertex MeshVaryings mesh_vert(\n    uint vid [[vertex_id]],\n    \
                 device const uint* mesh_indices [[buffer(0)]],\n    \
                 device const float* mesh_positions [[buffer(1)]],\n    \
                 device const float* mesh_normals [[buffer(2)]],\n    \
                 device const float* mesh_table [[buffer(3)]],\n    \
                 constant MeshUniforms& mesh_u [[buffer(4)]],\n    \
                 device const float* mesh_arrays [[buffer(5)]])\n{\n",
            );
            shader.push_str("    uint mesh_vertex = mesh_indices[vid];\n");
            for (name, buffer) in [("position", "mesh_positions"), ("normal", "mesh_normals")] {
                writeln!(
                    shader,
                    "    float3 {name} = float3({buffer}[mesh_vertex * 3u], {buffer}[mesh_vertex * 3u + 1u], {buffer}[mesh_vertex * 3u + 2u]);"
                )
                .unwrap();
            }
            shader.push_str("    float vertex_id = float(mesh_vertex);\n");
        }
        ShaderLanguage::Wgsl => {
            shader.push_str(MESH_PREAMBLE_WGSL);
            shader.push_str("\nstruct MeshVaryings {\n    @builtin(position) position: vec4<f32>,\n");
            for (location, varying) in varyings.iter().enumerate() {
                writeln!(
                    shader,
                    "    @location({location}) {}: {},",
                    varying_field(&varying.name),
                    vert.type_name(varying.type_name)
                )
                .unwrap();
            }
            shader.push_str("};\n\n");
            shader.push_str("@vertex\nfn mesh_vert(@builtin(vertex_index) vid: u32) -> MeshVaryings {\n");
            shader.push_str("    let mesh_vertex: u32 = mesh_indices[vid];\n");
            for (name, buffer) in [("position", "mesh_positions"), ("normal", "mesh_normals")] {
                writeln!(
                    shader,
                    "    let {name}: vec3<f32> = vec3<f32>({buffer}[mesh_vertex * 3u], {buffer}[mesh_vertex * 3u + 1u], {buffer}[mesh_vertex * 3u + 2u]);"
                )
                .unwrap();
            }
            shader.push_str("    let vertex_id: f32 = f32(mesh_vertex);\n");
        }
    }
    emit_common_declarations(&mut shader, &vert);
    emit_mesh_uniform_declarations(&mut shader, state_symbols, language);
    for statement in &vert.statements {
        writeln!(shader, "    {statement}").unwrap();
    }
    match language {
        ShaderLanguage::Metal => shader.push_str("    MeshVaryings vout;\n"),
        ShaderLanguage::Wgsl => shader.push_str("    var vout: MeshVaryings;\n"),
    }
    writeln!(shader, "    vout.position = {};", out.position).unwrap();
    for (varying, value) in &out.varyings {
        writeln!(shader, "    vout.{} = {value};", varying_field(&varying.name)).unwrap();
    }
    shader.push_str("    return vout;\n}\n\n");

    match language {
        ShaderLanguage::Metal => shader.push_str(
            "fragment float4 mesh_frag(\n    MeshVaryings in [[stage_in]],\n    \
             constant MeshUniforms& mesh_u [[buffer(0)]],\n    \
             device const float* mesh_arrays [[buffer(1)]])\n{\n",
        ),
        ShaderLanguage::Wgsl => shader.push_str(
            "@fragment\nfn mesh_frag(input: MeshVaryings) -> @location(0) vec4<f32> {\n",
        ),
    }
    emit_common_declarations(&mut shader, &frag);
    // x and y as a 2-D widget has them: -1..1 across the target, y down.
    let target = "mesh_u.target_size";
    for (name, axis) in [("x", "x"), ("y", "y")] {
        let value = format!("{input}.position.{axis} / max({target}.{axis}, 1.0) * 2.0 - 1.0");
        writeln!(shader, "    {}", frag.declaration("float", name, &value)).unwrap();
    }
    emit_mesh_uniform_declarations(&mut shader, state_symbols, language);
    for statement in &frag.statements {
        writeln!(shader, "    {statement}").unwrap();
    }
    match language {
        ShaderLanguage::Metal => {
            writeln!(shader, "    float4 result = {color};").unwrap();
            shader.push_str("    if (result.a < 0.001) discard_fragment();\n    return result;\n}\n");
        }
        ShaderLanguage::Wgsl => {
            writeln!(shader, "    let result: vec4<f32> = {color};").unwrap();
            shader.push_str("    if (result.a < 0.001) { discard; }\n    return result;\n}\n");
        }
    }
    Ok(MeshShaderOutput { shader_source: shader, varyings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Expression {
        let tokens = crate::parser::Parser::new(src.to_string()).parse().unwrap();
        crate::parser::ASTParser::new(tokens).parse().unwrap().into_iter().next().unwrap()
    }

    const VERTEX: &str = "(let ((u (* scale (dot amps (table-row vertex-id))))
                                (p (+ position (* u normal)))
                                (yaw2 (+ yaw orbit-yaw)))
                            (out :position (orbit-project p yaw2 pitch zoom)
                                 :varyings ((disp (/ u (max table-peak 0.0001)))
                                            (view (orbit-rotate p yaw2 pitch))
                                            (shade (dot normal (vec3 0.0 0.0 1.0))))))";
    const FRAGMENT: &str = "(let ((n (normalize (cross (dfdx view) (dfdy view))))
                                  (lit (+ 0.3 (* 0.7 (abs (dot n (vec3 0.3 0.5 0.8)))))))
                              (rgba (* lit (+ 0.5 disp)) (* lit (amps 1)) (* lit shade) 1.0))";

    fn arrays() -> Vec<MeshArrayState> {
        vec![
            MeshArrayState { name: "gains".into(), len: 4 },
            MeshArrayState { name: "amps".into(), len: 64 },
        ]
    }

    fn state() -> Vec<String> {
        ["scale", "yaw", "pitch", "zoom"].map(String::from).to_vec()
    }

    fn compile(
        language: ShaderLanguage,
        vertex: &str,
        fragment: &str,
    ) -> Result<MeshShaderOutput, CodegenError> {
        compile_mesh_shader(
            language, &parse(vertex), &parse(fragment), &state(), &arrays(), theme::default_theme(),
        )
    }

    #[test]
    fn vertex_stage_reads_attributes_and_writes_clip_position_and_varyings() {
        let output = compile(ShaderLanguage::Metal, VERTEX, FRAGMENT).unwrap();
        let source = &output.shader_source;
        assert!(source.contains("vertex MeshVaryings mesh_vert("), "{source}");
        assert!(source.contains("uint mesh_vertex = mesh_indices[vid];"), "{source}");
        assert!(source.contains("float3 position = float3(mesh_positions[mesh_vertex * 3u]"), "{source}");
        // The table dot loops over the array (offset 4: after `gains`) and
        // stops at the shorter of the array and the table row.
        assert!(source.contains(", 64u);"), "{source}");
        assert!(source.contains("+ mesh_arrays[4u + _v"), "{source}");
        assert!(source.contains("vout.position = eseq_orbit_project("), "{source}");
        assert!(source.contains("float sdf_state_scale = mesh_u.state[0].x;"), "{source}");
        assert!(source.contains("float sdf_state_zoom = mesh_u.state[0].w;"), "{source}");
        assert_eq!(
            output.varyings,
            [("disp", "float"), ("view", "float3"), ("shade", "float")]
                .map(|(name, type_name)| MeshVarying { name: name.into(), type_name })
                .to_vec()
        );
        assert!(source.contains("    float3 vary_view;"), "{source}");
        // The fragment stage reads the varyings by name, and an array read
        // is clamped into the array.
        assert!(source.contains("fragment float4 mesh_frag("), "{source}");
        assert!(source.contains("in.vary_view"), "{source}");
        assert!(source.contains("mesh_arrays[4u + min(uint(max(1.0, 0.0)), 63u)]"), "{source}");
        assert!(source.contains("dfdx(in.vary_view)"), "{source}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mesh_module_compiles_with_metal() {
        use objc2_foundation::NSString;
        use objc2_metal::{MTLCreateSystemDefaultDevice, MTLDevice, MTLLibrary};
        let Some(device) = MTLCreateSystemDefaultDevice() else {
            return; // no Metal device
        };
        let output = compile(ShaderLanguage::Metal, VERTEX, FRAGMENT).unwrap();
        let library = device
            .newLibraryWithSource_options_error(&NSString::from_str(&output.shader_source), None)
            .unwrap_or_else(|error| panic!("{error:?}\n\n{}", output.shader_source));
        for name in ["mesh_vert", "mesh_frag"] {
            assert!(library.newFunctionWithName(&NSString::from_str(name)).is_some(), "{name}");
        }
    }

    #[test]
    fn mesh_module_is_valid_wgsl() {
        let output = compile(ShaderLanguage::Wgsl, VERTEX, FRAGMENT).unwrap();
        let source = output.shader_source.as_str();
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{}\n\n{source}", error.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{error:#?}\n\n{source}"));
        assert!(source.contains("@location(1) vary_view: vec3<f32>,"), "{source}");
        assert!(source.contains("dpdx(input.vary_view)"), "{source}");
    }

    #[test]
    fn table_reads_and_array_sizes_lower_in_the_vertex_stage() {
        let vertex = "(out :position (vec4 position 1.0)
                          :varyings ((a (table vertex-id 2)) (n (size amps))))";
        let output = compile(ShaderLanguage::Metal, vertex, "(rgba a n 0.0 1.0)").unwrap();
        let source = &output.shader_source;
        assert!(source.contains("if (table_cols > 0.5) {"), "{source}");
        assert!(source.contains("mesh_u.array_len[0].y"), "{source}");
        // A float3 clip position is widened to a point.
        let output =
            compile(ShaderLanguage::Metal, "(out :position position)", "(rgba 1 1 1 1)").unwrap();
        assert!(output.shader_source.contains("vout.position = float4(position, 1.0);"));
    }

    #[test]
    fn mesh_stage_misuse_is_a_codegen_error() {
        let error = |vertex: &str, fragment: &str| match compile(ShaderLanguage::Metal, vertex, fragment) {
            Ok(_) => panic!("{vertex} / {fragment} compiled"),
            Err(error) => error.to_string(),
        };
        let out = "(out :position (vec4 position 1.0))";
        assert!(error(out, "(rgba normal 1.0)").contains("normal is a vertex attribute"));
        assert!(error("(vec4 position 1.0)", "(rgba 1 1 1 1)").contains("must end in (out"));
        assert!(error(out, "(rgba (table 0 0) 0 0 1)").contains("only in :vertex"));
        assert!(error(out, "(do (out :position 0) (rgba 1 1 1 1))").contains("ends the :vertex body"));
        assert!(error("(out :position (vec4 (table-row 0) 1.0))", "(rgba 1 1 1 1)")
            .contains("table-row"));
        assert!(error(out, "0.5").contains("returns a colour"));
        assert!(error(
            "(out :position (vec4 position 1.0) :varyings ((a 1) (b 2) (c 3) (d 4) (e 5)))",
            "(rgba 1 1 1 1)"
        )
        .contains("over the 4"));
        let too_big = [MeshArrayState { name: "big".into(), len: MAX_MESH_ARRAY_FLOATS + 1 }];
        let fragment = parse("(rgba 1 1 1 1)");
        let result = compile_mesh_shader(
            ShaderLanguage::Metal, &parse(out), &fragment, &[], &too_big, theme::default_theme(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn array_offsets_follow_declaration_order() {
        assert_eq!(mesh_array_offsets(&arrays()), vec![0, 4]);
    }
}
