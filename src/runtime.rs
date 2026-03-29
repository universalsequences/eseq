use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use crate::audio::register_audio_natives;
use crate::buffer::BufferTextStyle;
use crate::host::{BufferId, HostCommand};
use crate::layout::{
    LayoutEngine, LayoutNode, TextMeasurer, reuse_layout_node, same_layout_geometry,
};
use crate::reactive::ReactiveRegistry;
use crate::vm::{EffectTarget, PendingWidgetTree, VM, Value, register_core_natives};
use crate::widgets::register_widget_natives;

pub type RuntimeError = String;
pub type NativeResult = Result<Value, RuntimeError>;

#[derive(Debug, Clone, Default)]
pub struct RuntimeEvalProfile {
    pub clear_layout_effects: Duration,
    pub vm_parse: Duration,
    pub vm_ast: Duration,
    pub vm_compile: Duration,
    pub vm_execute: Duration,
    pub sync_theme: Duration,
    pub invalidate_symbol_cache: Duration,
    pub flush_widget_trees: Duration,
}

fn expand_sdf_expression(
    expr: &crate::parser::Expression,
    macros: &HashMap<String, crate::compiler::MacroDef>,
) -> crate::parser::Expression {
    crate::compiler::Compiler::new_repl(
        vec![],
        vec![],
        vec![],
        std::collections::HashSet::new(),
        HashMap::new(),
        HashMap::new(),
        0,
        macros.clone(),
    )
    .expand_macros(expr, 0)
}

struct SdfCompileResult {
    output: crate::lang::sdf_codegen::SdfShaderOutput,
    expanded_expr: crate::parser::Expression,
    state_symbols: Vec<String>,
}

fn compile_sdf_value(
    value: &Value,
    macros: &HashMap<String, crate::compiler::MacroDef>,
    state_bindings: &std::collections::HashSet<String>,
) -> Result<SdfCompileResult, String> {
    let expr = crate::lang::sdf_codegen::value_to_expression(value).map_err(|e| e.to_string())?;
    let expanded = expand_sdf_expression(&expr, macros);
    let mut state_symbols =
        crate::lang::sdf_codegen::collect_state_symbols(&expanded, state_bindings);
    state_symbols.truncate(crate::widget_render::sdf_widget::MAX_SDF_STATE_UNIFORMS);
    let output =
        crate::lang::sdf_codegen::compile_sdf_to_metal_with_state(&expanded, &state_symbols)
            .map_err(|e| e.to_string())?;
    Ok(SdfCompileResult {
        output,
        expanded_expr: expanded,
        state_symbols,
    })
}

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

thread_local! {
    static MATERIAL_SHADER_CACHE: RefCell<HashMap<u64, String>> = RefCell::new(HashMap::new());
}

/// Extract origin_t from a widget's min/max/origin props.
fn compute_origin_t(map: &HashMap<String, Rc<RefCell<Value>>>) -> f64 {
    let num_prop = |key: &str| -> Option<f64> {
        map.get(key).and_then(|v| match &*v.borrow() {
            Value::Number(n) => Some(*n),
            _ => None,
        })
    };
    let min = num_prop("min").unwrap_or(0.0);
    let max = num_prop("max").unwrap_or(1.0);
    let origin = num_prop("origin").unwrap_or(min);
    let range = max - min;
    if range > 0.0 {
        ((origin - min) / range).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn expr_to_source(expr: &crate::parser::Expression) -> String {
    use crate::parser::Expression;

    match expr {
        Expression::Symbol(s) => s.clone(),
        Expression::Keyword(s) => format!(":{s}"),
        Expression::String(s) => format!("{s:?}"),
        Expression::QuoteSymbol(s) => format!("'{s}"),
        Expression::QuoteList(items) => {
            let inner = items.iter().map(expr_to_source).collect::<Vec<_>>().join(" ");
            format!("'({inner})")
        }
        Expression::Number(n) => {
            if n.fract() == 0.0 {
                format!("{n:.1}")
            } else {
                n.to_string()
            }
        }
        Expression::List(items) => {
            let inner = items.iter().map(expr_to_source).collect::<Vec<_>>().join(" ");
            format!("({inner})")
        }
        Expression::Quasiquote(inner) => format!("`{}", expr_to_source(inner)),
        Expression::Unquote(inner) => format!(",{}", expr_to_source(inner)),
    }
}

fn parse_one_expr(src: &str) -> Result<crate::parser::Expression, String> {
    let tokens = crate::parser::Parser::new(src.to_string())
        .parse()
        .map_err(|e| format!("{e:?}"))?;
    let mut ast = crate::parser::ASTParser::new(tokens);
    ast.parse()
        .map_err(|e| format!("{e:?}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "expected one expression".to_string())
}

/// Build an exact SDF expression for a built-in slider material so `d` and
/// lighting operate on the same geometry as the native widget shader.
fn build_material_shader_expr(
    widget_type: &str,
    material_expr: &crate::parser::Expression,
) -> Result<crate::parser::Expression, String> {
    let material_src = expr_to_source(material_expr);
    let src = match widget_type {
        "slider" | "hslider" => format!(
            r#"
            (sdf/layer
              (if (> value_t 0.005)
                (sdf/fill
                  (let ((__half_w (* 0.5 aspect value_t))
                        (__half_h 0.32)
                        (__radius (min 0.18 (min __half_h (max __half_w 0.001)))))
                    (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 value_t))))
                          (y (* 0.5 y)))
                      (sdf/rounded-rect __half_w __half_h __radius)))
                  {material_src})
                (sdf/paint (sdf/rect 0.0 0.0) (rgba 0.0 0.0 0.0 0.0))))
            "#
        ),
        "vslider" => format!(
            r#"
            (sdf/layer
              (let ((__fill_lo (min value_t origin_t))
                    (__fill_hi (max value_t origin_t))
                    (__fill_span (- __fill_hi __fill_lo)))
                (if (> __fill_span 0.005)
                  (sdf/fill
                    (let ((__half_w (* 0.32 aspect))
                          (__half_h (* 0.5 __fill_span))
                          (__radius (min 0.063 (min __half_w (max (* 0.5 __fill_span) 0.001)))))
                      (let ((x (* 0.5 aspect x))
                            (y (* 0.5 (+ (* aspect y) (- (+ __fill_lo __fill_hi) 1.0)))))
                        (sdf/rounded-rect __half_w __half_h __radius)))
                    {material_src})
                  (sdf/paint (sdf/rect 0.0 0.0) (rgba 0.0 0.0 0.0 0.0)))))
            "#
        ),
        _ => return Err(format!("widget type '{widget_type}' does not support :material")),
    };
    parse_one_expr(&src)
}

/// Compile a :material value for a built-in widget, caching the result.
/// Returns the registered SDF widget name if successful.
fn compile_widget_material(
    widget_type: &str,
    material_val: &Value,
    macros: &HashMap<String, crate::compiler::MacroDef>,
    state_binding_keys: &[String],
) -> Result<String, String> {
    let material_expr =
        crate::lang::sdf_codegen::value_to_expression(material_val).map_err(|e| e.to_string())?;

    let shader_expr = build_material_shader_expr(widget_type, &material_expr)?;

    // For vslider, add origin_t so it gets a uniform slot.
    let mut bindings: std::collections::HashSet<String> =
        state_binding_keys.iter().cloned().collect();
    if widget_type == "vslider" {
        bindings.insert("origin_t".to_string());
    }
    let expanded = expand_sdf_expression(&shader_expr, macros);
    let mut hasher = DefaultHasher::new();
    widget_type.hash(&mut hasher);
    expr_to_source(&expanded).hash(&mut hasher);
    let cache_key = hasher.finish();

    if let Some(name) = MATERIAL_SHADER_CACHE.with(|c| c.borrow().get(&cache_key).cloned()) {
        return Ok(name);
    }

    let mut state_symbols = crate::lang::sdf_codegen::collect_state_symbols(&expanded, &bindings);
    state_symbols.truncate(crate::widget_render::sdf_widget::MAX_SDF_STATE_UNIFORMS);
    let output =
        crate::lang::sdf_codegen::compile_sdf_to_metal_with_state(&expanded, &state_symbols)
            .map_err(|e| e.to_string())?;

    let paint_margin = crate::widget_render::sdf_widget::estimate_shadow_paint_margin(
        &expanded, 16.0, 8.0,
    );

    let shader_name = format!("{}__mat_{:x}", widget_type, cache_key);
    crate::widget_render::sdf_widget::register_inline_shader(
        shader_name.clone(),
        output.shader_source,
        state_symbols,
        paint_margin,
    );

    MATERIAL_SHADER_CACHE.with(|c| c.borrow_mut().insert(cache_key, shader_name.clone()));

    Ok(shader_name)
}

#[derive(Debug, Clone)]
pub enum TileOp {
    SplitRight(Option<String>),
    SplitBelow(Option<String>),
    DeleteWindow,
    DeleteOtherWindows,
    OtherWindow,
    SetWindowBuffer(String),
    SetLayout(LayoutSpec),
}

/// Declarative layout specification for `set-layout`.
#[derive(Debug, Clone)]
pub enum LayoutSpec {
    Buffer {
        name: String,
        hide_status: bool,
        borderless: bool,
        min_width: Option<f32>,
        min_height: Option<f32>,
        max_width: Option<f32>,
        max_height: Option<f32>,
    },
    Rows(Vec<(f32, LayoutSpec)>),
    Cols(Vec<(f32, LayoutSpec)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolMetadata {
    pub signature: String,
    pub docs: String,
}

#[derive(Default)]
pub(crate) struct RuntimeBridgeState {
    pub current_buffer_id: Option<BufferId>,
    pub current_buffer_name: String,
    pub current_buffer_path: Option<PathBuf>,
    pub current_buffer_text: String,
    pub current_sexp: Option<String>,
    pub status_message: Option<String>,
    pub queued_commands: Vec<HostCommand>,
    pub lisp_bindings: HashMap<String, String>,
    pub pending_save: bool,
    pub pending_save_as: Option<PathBuf>,
    pub pending_load: bool,
    pub current_buffer_read_only: bool,
    pub pending_set_read_only: Option<bool>,
    pub current_buffer_mode: String,
    pub pending_mode_defs: Vec<(String, bool, Option<String>, Option<String>)>, // (name, read_only, on_enter, on_key)
    pub pending_mode_bindings: Vec<(String, String, String)>, // (mode, key, handler)
    pub pending_set_mode: Option<String>,
    pub pending_set_mode_for: Vec<(String, String)>, // (buffer_name, mode_name)
    pub pending_open_file: Option<String>,
    pub pending_widget_tree: Option<Value>,
    pub pending_buffer_widget_trees: Vec<PendingWidgetTree>,
    pub pending_create_buffer: Option<String>,
    pub pending_cleared_effect_sources: Vec<Option<BufferId>>,
    pub pending_switch_buffer: Option<String>,
    pub pending_set_text: Option<String>,
    pub pending_set_lines: Option<Vec<String>>,
    pub pending_set_buffer_styles: Option<Vec<BufferTextStyle>>,
    pub pending_goto_line: Option<usize>,
    pub current_line_number: usize,
    pub current_line_text: String,
    pub buffer_names: Vec<String>,
    pub pending_cycle_view_mode: bool,
    pub pending_set_view_mode: Option<String>,
    pub current_view_mode: String,
    // Tiling operations — processed in order enqueued
    pub pending_tile_ops: Vec<TileOp>,
    pub pending_window_hide_status: bool,
    pub pending_resize_window: Option<f64>,
    /// Theme map to apply (from `apply-theme` native).
    pub pending_apply_theme: Option<Value>,
    /// Scratch buffers to create with initial text (name, text).
    pub pending_scratch_buffers: Vec<(String, String)>,
}

pub(crate) type SharedBridgeState = Rc<RefCell<RuntimeBridgeState>>;

pub struct NativeContext {
    shared: SharedBridgeState,
}

impl NativeContext {
    pub(crate) fn new(shared: SharedBridgeState) -> Self {
        Self { shared }
    }

    pub fn current_buffer_id(&self) -> Option<BufferId> {
        self.shared.borrow().current_buffer_id
    }

    pub fn current_buffer_name(&self) -> String {
        self.shared.borrow().current_buffer_name.clone()
    }

    pub fn current_buffer_text(&self) -> String {
        self.shared.borrow().current_buffer_text.clone()
    }

    pub fn current_buffer_path(&self) -> Option<PathBuf> {
        self.shared.borrow().current_buffer_path.clone()
    }

    pub fn current_sexp(&self) -> Option<String> {
        self.shared.borrow().current_sexp.clone()
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.shared.borrow_mut().status_message = Some(status.into());
    }

    pub fn enqueue_command(&mut self, command: HostCommand) {
        self.shared.borrow_mut().queued_commands.push(command);
    }

    pub fn bind_key(&mut self, key: String, handler: String) {
        self.shared.borrow_mut().lisp_bindings.insert(key, handler);
    }

    pub fn request_save(&mut self) {
        self.shared.borrow_mut().pending_save = true;
    }

    pub fn request_save_as(&mut self, path: impl Into<PathBuf>) {
        self.shared.borrow_mut().pending_save_as = Some(path.into());
    }

    pub fn request_load(&mut self) {
        self.shared.borrow_mut().pending_load = true;
    }

    pub fn current_buffer_read_only(&self) -> bool {
        self.shared.borrow().current_buffer_read_only
    }

    pub fn set_read_only(&mut self, read_only: bool) {
        self.shared.borrow_mut().pending_set_read_only = Some(read_only);
    }

    pub fn current_buffer_mode(&self) -> String {
        self.shared.borrow().current_buffer_mode.clone()
    }

    pub fn define_mode(
        &mut self,
        name: String,
        read_only: bool,
        on_enter: Option<String>,
        on_key: Option<String>,
    ) {
        self.shared
            .borrow_mut()
            .pending_mode_defs
            .push((name, read_only, on_enter, on_key));
    }

    pub fn mode_bind_key(&mut self, mode: String, key: String, handler: String) {
        self.shared
            .borrow_mut()
            .pending_mode_bindings
            .push((mode, key, handler));
    }

    pub fn set_buffer_mode(&mut self, mode: String) {
        self.shared.borrow_mut().pending_set_mode = Some(mode);
    }

    pub fn set_buffer_mode_for(&mut self, buffer_name: String, mode: String) {
        self.shared
            .borrow_mut()
            .pending_set_mode_for
            .push((buffer_name, mode));
    }

    pub fn open_file(&mut self, path: String) {
        self.shared.borrow_mut().pending_open_file = Some(path);
    }

    pub fn create_buffer(&mut self, name: String) {
        self.shared.borrow_mut().pending_create_buffer = Some(name);
    }

    pub fn create_scratch(&mut self, name: String, text: String) {
        self.shared
            .borrow_mut()
            .pending_scratch_buffers
            .push((name, text));
    }

    pub fn switch_to_buffer(&mut self, name: String) {
        self.shared.borrow_mut().pending_switch_buffer = Some(name);
    }

    pub fn set_buffer_text(&mut self, text: String) {
        self.shared.borrow_mut().pending_set_text = Some(text);
    }

    pub fn set_buffer_lines(&mut self, lines: Vec<String>) {
        self.shared.borrow_mut().pending_set_lines = Some(lines);
    }

    pub fn set_buffer_styles(&mut self, styles: Vec<BufferTextStyle>) {
        self.shared.borrow_mut().pending_set_buffer_styles = Some(styles);
    }

    pub fn render_widget(&mut self, tree: Value) {
        self.shared.borrow_mut().pending_widget_tree.replace(tree);
    }

    pub fn render_widget_to_buffer(&mut self, buffer_name: String, tree: Value) {
        let source_buffer_id = self.shared.borrow().current_buffer_id;
        self.shared
            .borrow_mut()
            .pending_buffer_widget_trees
            .push(PendingWidgetTree {
                source_buffer_id,
                target: EffectTarget::BufferName(buffer_name),
                tree,
            });
    }

    pub fn goto_line(&mut self, line: usize) {
        self.shared.borrow_mut().pending_goto_line = Some(line);
    }

    pub fn current_line_number(&self) -> usize {
        self.shared.borrow().current_line_number
    }

    pub fn current_line_text(&self) -> String {
        self.shared.borrow().current_line_text.clone()
    }

    pub fn buffer_names(&self) -> Vec<String> {
        self.shared.borrow().buffer_names.clone()
    }

    // ── Tiling operations ─────────────────────────────────────────────────

    pub fn split_window_right(&mut self, buffer_name: Option<String>) {
        self.shared
            .borrow_mut()
            .pending_tile_ops
            .push(TileOp::SplitRight(buffer_name));
    }

    pub fn split_window_below(&mut self, buffer_name: Option<String>) {
        self.shared
            .borrow_mut()
            .pending_tile_ops
            .push(TileOp::SplitBelow(buffer_name));
    }

    pub fn delete_window(&mut self) {
        self.shared
            .borrow_mut()
            .pending_tile_ops
            .push(TileOp::DeleteWindow);
    }

    pub fn delete_other_windows(&mut self) {
        self.shared
            .borrow_mut()
            .pending_tile_ops
            .push(TileOp::DeleteOtherWindows);
    }

    pub fn other_window(&mut self) {
        self.shared
            .borrow_mut()
            .pending_tile_ops
            .push(TileOp::OtherWindow);
    }

    pub fn set_layout(&mut self, spec: LayoutSpec) {
        self.shared
            .borrow_mut()
            .pending_tile_ops
            .push(TileOp::SetLayout(spec));
    }

    pub fn set_window_buffer(&mut self, name: String) {
        self.shared
            .borrow_mut()
            .pending_tile_ops
            .push(TileOp::SetWindowBuffer(name));
    }

    pub fn window_hide_status(&mut self) {
        self.shared.borrow_mut().pending_window_hide_status = true;
    }

    pub fn resize_window(&mut self, delta: f64) {
        self.shared.borrow_mut().pending_resize_window = Some(delta);
    }

    pub fn cycle_view_mode(&mut self) {
        self.shared.borrow_mut().pending_cycle_view_mode = true;
    }

    pub fn set_view_mode(&mut self, mode: String) {
        self.shared.borrow_mut().pending_set_view_mode = Some(mode);
    }

    pub fn current_view_mode(&self) -> String {
        self.shared.borrow().current_view_mode.clone()
    }

    pub fn apply_theme(&mut self, map: Value) {
        self.shared.borrow_mut().pending_apply_theme = Some(map);
    }
}

pub struct Runtime {
    vm: VM,
    pub(crate) shared: SharedBridgeState,
    symbol_metadata: HashMap<String, SymbolMetadata>,
    symbol_revision: u64,
    cached_completion_symbols: Option<Vec<String>>,
    cached_completion_metadata: Option<HashMap<String, SymbolMetadata>>,
    pub reactive_registry: ReactiveRegistry,
    #[cfg(test)]
    rendered_layouts: Vec<Vec<String>>,
    pub current_layout: Option<Arc<LayoutNode>>,
    layout_revision: u64,
    dirty_widget_ids: Vec<u64>,
    current_widget_tree: Option<Value>,
    layout_cols: u16,
    layout_rows: u16,
    layout_aspect: f32,
    layout_cell_w: f32,
    layout_cell_h: f32,
    text_measurer: Option<Box<dyn TextMeasurer>>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    pub fn new() -> Self {
        let shared = Rc::new(RefCell::new(RuntimeBridgeState::default()));
        let mut vm = VM::new(vec![]);
        register_core_natives(&mut vm);
        crate::vm::register_math_natives(&mut vm);
        register_widget_natives(&mut vm);
        let mut runtime = Self {
            vm,
            shared,
            symbol_metadata: HashMap::new(),
            symbol_revision: 0,
            cached_completion_symbols: None,
            cached_completion_metadata: None,
            reactive_registry: ReactiveRegistry::new(),
            #[cfg(test)]
            rendered_layouts: Vec::new(),
            current_layout: None,
            layout_revision: 0,
            dirty_widget_ids: Vec::new(),
            current_widget_tree: None,
            layout_cols: 80,
            layout_rows: 24,
            layout_aspect: 1.0,
            layout_cell_w: 1.0,
            layout_cell_h: 1.0,
            text_measurer: None,
        };
        runtime.register_reactive("THEME", crate::theme::reactive_fields(), true);
        crate::theme::set_current(crate::theme::default_theme());
        register_audio_natives(&mut runtime);
        // (load path) — read and evaluate a Lisp file; relative paths resolve from CWD.
        runtime
            .vm
            .register_native_with_vm("load", |args, vm| {
                let Some(Value::String(path_str)) = args.first() else {
                    return Value::String("load: expects a string path".into());
                };
                let path = std::path::Path::new(path_str.as_str());
                let source = match std::fs::read_to_string(path) {
                    Ok(s) => s,
                    Err(e) => return Value::String(format!("load: {e}")),
                };
                match vm.eval_str(&source) {
                    Ok(v) => v.unwrap_or(Value::Bool(true)),
                    Err(e) => Value::String(format!("load: eval error: {e:?}")),
                }
            });
        // Register SDF constructor functions that return self-quoting tagged lists.
        // These are compiled to Metal by the SDF codegen; at the Lisp level they
        // preserve their structure for later compilation.
        // Note: vec2 is already registered in vm.rs with numeric semantics — don't override it.
        for name in &["vec3", "vec4", "rgba", "material", "lighting", "shadow"] {
            let tag = name.to_string();
            runtime.vm.register_native(name, move |args| {
                let mut items = vec![Rc::new(RefCell::new(Value::Symbol(tag.clone())))];
                for a in args {
                    items.push(Rc::new(RefCell::new(a)));
                }
                Value::List(items)
            });
        }
        // Load SDF standard library (macros for SDF primitives)
        let sdf_src = include_str!("../sdf-stdlib.lisp");
        if !sdf_src.trim().is_empty() {
            let _ = runtime.eval_str(sdf_src);
        }
        // Register sdf->metal: takes a quoted SDF expression, returns Metal shader string
        let sdf_macros = runtime.vm.macros.clone();
        runtime.vm.register_native("sdf->metal", move |args| {
            let Some(val) = args.first() else {
                return Value::String("error: sdf->metal requires 1 argument".into());
            };
            match compile_sdf_value(val, &sdf_macros, &std::collections::HashSet::new()) {
                Ok(result) => Value::String(result.output.shader_source),
                Err(e) => Value::String(format!("error: {}", e)),
            }
        });
        // Register defwidget: defines a new SDF widget type
        runtime
            .vm
            .register_native_with_vm("defwidget", move |args, vm| {
                use crate::widget_render::sdf_widget::{
                    SdfWidgetDef, estimate_shadow_paint_margin, register_sdf_widget,
                };

                // Parse: (defwidget name :width W :height H :shader '(sdf/layer ...))
                let name = match args.first() {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Symbol(s)) => s.clone(),
                    _ => return Value::String("defwidget: first arg must be widget name".into()),
                };

                let mut width: f32 = 10.0;
                let mut height: f32 = 5.0;
                let mut paint_margin: f32 = 0.0;
                let mut shader_val = None;
                let mut widget_state_names: Vec<String> = Vec::new();

                let mut i = 1;
                while i + 1 < args.len() {
                    if let Value::Keyword(key) = &args[i] {
                        match key.as_str() {
                            "width" => {
                                if let Value::Number(n) = &args[i + 1] {
                                    width = *n as f32;
                                }
                            }
                            "height" => {
                                if let Value::Number(n) = &args[i + 1] {
                                    height = *n as f32;
                                }
                            }
                            "paint-margin" => {
                                if let Value::Number(n) = &args[i + 1] {
                                    paint_margin = (*n as f32).max(0.0);
                                }
                            }
                            "shader" => {
                                shader_val = Some(args[i + 1].clone());
                            }
                            "state" => {
                                // Extract state symbol names from quoted list
                                if let Value::List(items) = &args[i + 1] {
                                    for item in items {
                                        if let Value::Symbol(s) = &*item.borrow() {
                                            widget_state_names.push(s.clone());
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                        i += 2;
                    } else {
                        i += 1;
                    }
                }

                let Some(shader_val) = shader_val else {
                    return Value::String("defwidget: :shader is required".into());
                };

                // Combine VM state bindings with widget-declared state names
                let mut state_bindings: std::collections::HashSet<String> =
                    vm.state_bindings.keys().cloned().collect();
                for name in &widget_state_names {
                    state_bindings.insert(name.clone());
                }
                let compiled = match compile_sdf_value(&shader_val, &vm.macros, &state_bindings) {
                    Ok(o) => o,
                    Err(e) => return Value::String(format!("defwidget shader error: {}", e)),
                };
                let paint_margin = paint_margin.max(estimate_shadow_paint_margin(
                    &compiled.expanded_expr,
                    width,
                    height,
                ));

                register_sdf_widget(SdfWidgetDef {
                    name: name.clone(),
                    shader_source: compiled.output.shader_source,
                    sdf_expr: compiled.expanded_expr,
                    state_uniforms: compiled.state_symbols.clone(),
                    region_count: compiled.output.region_count,
                    width,
                    height,
                    paint_margin,
                });

                let widget_type = name.clone();
                let state_uniforms = compiled.state_symbols;
                vm.register_native_with_vm(&name, move |args, vm| {
                    let mut widget = crate::widgets::build_widget(&widget_type, args);
                    if let Value::Map(map) = &mut widget {
                        for state_name in &state_uniforms {
                            if let Some(value) = vm.read_tracked_state_value(state_name) {
                                map.insert(
                                    crate::widget_render::sdf_widget::shader_state_prop_name(
                                        state_name,
                                    ),
                                    Rc::new(RefCell::new(value)),
                                );
                            }
                        }
                    }
                    widget
                });
                Value::Keyword(name.clone())
            });

        // Override slider natives with material-aware versions.
        // These intercept :material, compile it into an SDF shader, and set
        // __shader_type so the renderer uses the custom pipeline.
        for widget_name in &["slider", "hslider", "vslider"] {
            let wtype = widget_name.to_string();
            runtime
                .vm
                .register_native_with_vm(widget_name, move |args, vm| {
                    let mut widget = crate::widgets::build_widget(&wtype, args);
                    if let Value::Map(map) = &mut widget {
                        if let Some(material_cell) = map.get("material") {
                            let material_val = material_cell.borrow().clone();
                            if !matches!(material_val, Value::Nil) {
                                let keys: Vec<String> =
                                    vm.state_bindings.keys().cloned().collect();
                                match compile_widget_material(
                                    &wtype,
                                    &material_val,
                                    &vm.macros,
                                    &keys,
                                ) {
                                    Ok(shader_name) => {
                                        if let Some(def) = crate::widget_render::sdf_widget::sdf_widget_def(&shader_name) {
                                            for state_name in &def.state_uniforms {
                                                if let Some(value) = vm.read_tracked_state_value(state_name) {
                                                    map.insert(
                                                        crate::widget_render::sdf_widget::shader_state_prop_name(state_name),
                                                        Rc::new(RefCell::new(value)),
                                                    );
                                                }
                                            }
                                        }
                                        if wtype == "vslider" {
                                            let origin_t = compute_origin_t(map);
                                            map.insert(
                                                crate::widget_render::sdf_widget::shader_state_prop_name("origin_t"),
                                                Rc::new(RefCell::new(Value::Number(origin_t))),
                                            );
                                        }
                                        map.insert(
                                            crate::widget_render::sdf_widget::SHADER_TYPE_PROP.to_string(),
                                            Rc::new(RefCell::new(Value::String(shader_name))),
                                        );
                                    }
                                    Err(e) => {
                                        eprintln!("{} :material compile error: {}", wtype, e);
                                    }
                                }
                            }
                        }
                    }
                    widget
                });
        }

        runtime
    }

    pub fn with_init_source(init: impl AsRef<str>) -> Self {
        let mut runtime = Self::new();
        let src = init.as_ref();
        if !src.trim().is_empty() {
            let _ = runtime.eval_str(src);
        }
        runtime
    }

    pub fn register_native<F>(&mut self, name: &str, f: F)
    where
        F: Fn(Vec<Value>, &mut NativeContext) -> NativeResult + 'static,
    {
        self.register_native_impl(name, None, None, f);
    }

    pub fn register_native_with_docs<F>(
        &mut self,
        name: &str,
        signature: impl Into<String>,
        docs: impl Into<String>,
        f: F,
    ) where
        F: Fn(Vec<Value>, &mut NativeContext) -> NativeResult + 'static,
    {
        self.register_native_impl(name, Some(signature.into()), Some(docs.into()), f);
    }

    fn register_native_impl<F>(
        &mut self,
        name: &str,
        signature: Option<String>,
        docs: Option<String>,
        f: F,
    ) where
        F: Fn(Vec<Value>, &mut NativeContext) -> NativeResult + 'static,
    {
        let shared = self.shared.clone();
        self.vm.register_native(name, move |args| {
            let mut ctx = NativeContext::new(shared.clone());
            match f(args, &mut ctx) {
                Ok(value) => value,
                Err(error) => {
                    ctx.set_status(format!("Error: {error}"));
                    Value::Bool(false)
                }
            }
        });
        if let (Some(signature), Some(docs)) = (signature, docs) {
            self.symbol_metadata
                .insert(name.to_string(), SymbolMetadata { signature, docs });
        }
        self.invalidate_symbol_cache();
    }

    pub fn macros(&self) -> &std::collections::HashMap<String, crate::compiler::MacroDef> {
        &self.vm.macros
    }

    pub fn eval_str(&mut self, src: &str) -> Result<Option<Value>, crate::vm::VMError> {
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        self.vm.set_current_effect_context(current_buffer_id);
        if src.contains("(effect") || src.contains("(effect-buffer") {
            self.clear_layout_effects();
        }

        let result = self.vm.eval_str(src);
        if result.is_ok() {
            self.sync_theme_from_vm();
            self.invalidate_symbol_cache();
            self.flush_widget_trees();
        }
        result
    }

    #[cfg(test)]
    pub(crate) fn profile_eval_str(
        &mut self,
        src: &str,
    ) -> Result<(Option<Value>, RuntimeEvalProfile), crate::vm::VMError> {
        let mut profile = RuntimeEvalProfile::default();
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        self.vm.set_current_effect_context(current_buffer_id);
        if src.contains("(effect") || src.contains("(effect-buffer") {
            let clear_started = std::time::Instant::now();
            self.clear_layout_effects();
            profile.clear_layout_effects = clear_started.elapsed();
        }

        let (result, vm_profile) = self.vm.profile_eval_str(src)?;
        profile.vm_parse = vm_profile.parse;
        profile.vm_ast = vm_profile.ast;
        profile.vm_compile = vm_profile.compile;
        profile.vm_execute = vm_profile.execute;

        let sync_started = std::time::Instant::now();
        self.sync_theme_from_vm();
        profile.sync_theme = sync_started.elapsed();

        let cache_started = std::time::Instant::now();
        self.invalidate_symbol_cache();
        profile.invalidate_symbol_cache = cache_started.elapsed();

        let flush_started = std::time::Instant::now();
        self.flush_widget_trees();
        profile.flush_widget_trees = flush_started.elapsed();

        Ok((result, profile))
    }

    pub fn set_global_value(&mut self, name: &str, value: Value) {
        self.vm.set_global_value(name, value);
        if name == "THEME" {
            self.sync_theme_from_vm();
        }
        self.invalidate_symbol_cache();
    }

    pub fn register_reactive(&mut self, name: &str, fields: Vec<(&str, Value)>, writable: bool) {
        let map = self.reactive_registry.register(name, fields, writable);
        self.vm.set_global_value(name, map);
        self.vm.reactive_namespaces.insert(name.to_string());
        if writable {
            self.vm
                .writable_reactive_namespaces
                .insert(name.to_string());
        } else {
            self.vm.writable_reactive_namespaces.remove(name);
        }
        if name == "THEME" {
            self.sync_theme_from_vm();
        }
        self.invalidate_symbol_cache();
    }

    pub fn set_reactive(&mut self, namespace: &str, field: &str, value: Value) {
        self.reactive_registry.set(namespace, field, value);
        if namespace == "THEME" {
            self.sync_theme_from_registry();
        }
    }

    pub fn set_layout_viewport(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if self.layout_cols == cols && self.layout_rows == rows {
            return;
        }
        self.layout_cols = cols;
        self.layout_rows = rows;
        // Viewport changes invalidate layout geometry even if the widget tree is unchanged.
        self.current_layout = None;
        self.dirty_widget_ids.clear();
        self.relayout_current_tree();
    }

    /// Force a full relayout on the next render pass.
    /// Used when internal widget state (e.g. tree expand/collapse) changes
    /// the widget's size without changing the widget tree data.
    pub fn invalidate_layout(&mut self) {
        self.current_layout = None;
        self.dirty_widget_ids.clear();
        self.relayout_current_tree();
    }

    pub fn layout_aspect(&self) -> f32 {
        self.layout_aspect
    }

    pub fn layout_cell_dims(&self) -> (f32, f32) {
        (self.layout_cell_w, self.layout_cell_h)
    }

    /// Set the text measurer for proportional font layout (Metal backend).
    /// Also stores cell dimensions for pixel↔cell conversion.
    pub fn set_text_measurer(&mut self, measurer: Box<dyn TextMeasurer>, cell_w: f32, cell_h: f32) {
        self.text_measurer = Some(measurer);
        self.layout_cell_w = cell_w;
        self.layout_cell_h = cell_h;
        // Force relayout with the new measurer.
        self.current_layout = None;
        self.dirty_widget_ids.clear();
        self.relayout_current_tree();
    }

    pub fn set_layout_aspect(&mut self, aspect: f32) {
        if (self.layout_aspect - aspect).abs() < f32::EPSILON {
            return;
        }
        self.layout_aspect = aspect;
        self.current_layout = None;
        self.dirty_widget_ids.clear();
        self.relayout_current_tree();
    }

    pub fn invoke(
        &mut self,
        callable: Value,
        args: Vec<Value>,
    ) -> Result<Option<Value>, crate::vm::VMError> {
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        self.vm.set_current_effect_context(current_buffer_id);
        let result = self.vm.invoke(callable, args);
        if result.is_ok() {
            self.sync_theme_from_vm();
            self.flush_widget_trees();
        }
        result
    }

    pub fn run_reactive_cycle(&mut self) {
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        self.vm.set_current_effect_context(current_buffer_id);
        let dirty = self.reactive_registry.drain_dirty();
        if dirty.is_empty() {
            return;
        }

        if self.vm.apply_reactive_changes(dirty).is_ok() {
            self.sync_theme_from_vm();
            self.flush_widget_trees();
        }
    }

    fn sync_theme(&mut self, value: Option<Value>) {
        if let Some(v) = value {
            crate::theme::sync_from_value(&v);
        }
    }

    fn sync_theme_from_vm(&mut self) {
        let value = self.vm.global_value("THEME");
        self.sync_theme(value);
    }

    fn sync_theme_from_registry(&mut self) {
        let value = self.reactive_registry.namespace_value("THEME");
        self.sync_theme(value);
    }

    pub fn global_names(&self) -> &[String] {
        self.vm.global_names()
    }

    pub fn symbol_metadata(&self) -> &HashMap<String, SymbolMetadata> {
        &self.symbol_metadata
    }

    pub fn completion_symbols(&mut self) -> Vec<String> {
        if let Some(symbols) = &self.cached_completion_symbols {
            return symbols.clone();
        }

        let mut symbols = self.vm.global_names().to_vec();
        for global in self.vm.global_names() {
            if let Some(Value::Map(map)) = self.vm.global_value(global) {
                let mut keys = map.keys().cloned().collect::<Vec<_>>();
                keys.sort();
                symbols.extend(keys.into_iter().map(|key| format!("{global}.{key}")));
            }
        }
        symbols.sort();
        symbols.dedup();
        self.cached_completion_symbols = Some(symbols.clone());
        symbols
    }

    pub fn completion_metadata(&mut self) -> HashMap<String, SymbolMetadata> {
        if let Some(metadata) = &self.cached_completion_metadata {
            return metadata.clone();
        }

        let mut metadata = self.symbol_metadata.clone();
        for global in self.vm.global_names() {
            if let Some(Value::Map(map)) = self.vm.global_value(global) {
                let mut keys = map.keys().cloned().collect::<Vec<_>>();
                keys.sort();
                for key in keys {
                    let label = format!("{global}.{key}");
                    metadata.entry(label).or_insert_with(|| SymbolMetadata {
                        signature: format!("{global}.{key}"),
                        docs: format!("Field '{key}' on runtime map '{global}'."),
                    });
                }
            }
        }
        self.cached_completion_metadata = Some(metadata.clone());
        metadata
    }

    pub fn symbol_revision(&self) -> u64 {
        self.symbol_revision
    }

    pub fn take_status_message(&mut self) -> Option<String> {
        self.shared.borrow_mut().status_message.take()
    }

    pub fn layout_revision(&self) -> u64 {
        self.layout_revision
    }

    pub fn take_dirty_widget_ids(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.dirty_widget_ids)
    }

    pub(crate) fn drain_host_commands(&mut self) -> Vec<HostCommand> {
        let mut shared = self.shared.borrow_mut();
        std::mem::take(&mut shared.queued_commands)
    }

    pub(crate) fn lisp_bindings(&self) -> HashMap<String, String> {
        self.shared.borrow().lisp_bindings.clone()
    }

    pub(crate) fn take_pending_save(&mut self) -> bool {
        let mut shared = self.shared.borrow_mut();
        let pending = shared.pending_save;
        shared.pending_save = false;
        pending
    }

    pub(crate) fn take_pending_save_as(&mut self) -> Option<PathBuf> {
        self.shared.borrow_mut().pending_save_as.take()
    }

    pub(crate) fn take_pending_load(&mut self) -> bool {
        let mut shared = self.shared.borrow_mut();
        let pending = shared.pending_load;
        shared.pending_load = false;
        pending
    }

    pub(crate) fn take_pending_set_read_only(&mut self) -> Option<bool> {
        self.shared.borrow_mut().pending_set_read_only.take()
    }

    pub(crate) fn take_pending_set_buffer_styles(&mut self) -> Option<Vec<BufferTextStyle>> {
        self.shared.borrow_mut().pending_set_buffer_styles.take()
    }

    pub(crate) fn take_pending_mode_defs(
        &mut self,
    ) -> Vec<(String, bool, Option<String>, Option<String>)> {
        std::mem::take(&mut self.shared.borrow_mut().pending_mode_defs)
    }

    pub(crate) fn take_pending_mode_bindings(&mut self) -> Vec<(String, String, String)> {
        std::mem::take(&mut self.shared.borrow_mut().pending_mode_bindings)
    }

    pub(crate) fn take_pending_set_mode(&mut self) -> Option<String> {
        self.shared.borrow_mut().pending_set_mode.take()
    }

    pub(crate) fn take_pending_set_mode_for(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.shared.borrow_mut().pending_set_mode_for)
    }

    pub(crate) fn take_pending_widget_tree(&mut self) -> Option<Value> {
        self.shared.borrow_mut().pending_widget_tree.take()
    }

    pub(crate) fn take_pending_buffer_widget_trees(&mut self) -> Vec<PendingWidgetTree> {
        std::mem::take(&mut self.shared.borrow_mut().pending_buffer_widget_trees)
    }

    pub(crate) fn take_pending_open_file(&mut self) -> Option<String> {
        self.shared.borrow_mut().pending_open_file.take()
    }

    pub(crate) fn take_pending_create_buffer(&mut self) -> Option<String> {
        self.shared.borrow_mut().pending_create_buffer.take()
    }

    pub(crate) fn take_pending_cleared_effect_sources(&mut self) -> Vec<Option<BufferId>> {
        std::mem::take(&mut self.shared.borrow_mut().pending_cleared_effect_sources)
    }

    pub(crate) fn take_pending_scratch_buffers(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.shared.borrow_mut().pending_scratch_buffers)
    }

    pub(crate) fn take_pending_switch_buffer(&mut self) -> Option<String> {
        self.shared.borrow_mut().pending_switch_buffer.take()
    }

    pub(crate) fn take_pending_set_text(&mut self) -> Option<String> {
        self.shared.borrow_mut().pending_set_text.take()
    }

    pub(crate) fn take_pending_set_lines(&mut self) -> Option<Vec<String>> {
        self.shared.borrow_mut().pending_set_lines.take()
    }

    pub(crate) fn take_pending_goto_line(&mut self) -> Option<usize> {
        self.shared.borrow_mut().pending_goto_line.take()
    }

    // ── Tiling pending operations ──────────────────────────────────────────

    pub(crate) fn take_pending_tile_ops(&mut self) -> Vec<TileOp> {
        std::mem::take(&mut self.shared.borrow_mut().pending_tile_ops)
    }

    pub(crate) fn take_pending_window_hide_status(&mut self) -> bool {
        let mut shared = self.shared.borrow_mut();
        let v = shared.pending_window_hide_status;
        shared.pending_window_hide_status = false;
        v
    }

    pub(crate) fn take_pending_resize_window(&mut self) -> Option<f64> {
        self.shared.borrow_mut().pending_resize_window.take()
    }

    pub(crate) fn take_pending_cycle_view_mode(&mut self) -> bool {
        let mut shared = self.shared.borrow_mut();
        let pending = shared.pending_cycle_view_mode;
        shared.pending_cycle_view_mode = false;
        pending
    }

    pub(crate) fn take_pending_set_view_mode(&mut self) -> Option<String> {
        self.shared.borrow_mut().pending_set_view_mode.take()
    }

    pub(crate) fn take_pending_apply_theme(&mut self) -> Option<Value> {
        self.shared.borrow_mut().pending_apply_theme.take()
    }

    /// Apply a theme map by updating each reactive THEME field and syncing the
    /// global theme. This ensures the reactive namespace stays in sync.
    pub(crate) fn apply_theme_map(&mut self, map: Value) {
        if let Value::Map(ref entries) = map {
            for (field, value) in entries {
                // Theme struct fields use underscores (status_ui_bg) but Lisp
                // keywords use hyphens (status-ui-bg). Convert to match.
                let field_name = field.replace('-', "_");
                self.set_reactive("THEME", &field_name, value.borrow().clone());
            }
        }
    }

    #[cfg(test)]
    pub fn drain_rendered_layouts(&mut self) -> Vec<Vec<String>> {
        std::mem::take(&mut self.rendered_layouts)
    }

    pub fn current_widget_tree(&self) -> Option<Value> {
        self.current_widget_tree.clone()
    }

    pub fn layout_snapshot_for_tree(&mut self, tree: &Value) -> Option<Arc<LayoutNode>> {
        self.layout_snapshot_for_tree_with_viewport(tree, None)
    }

    pub fn layout_snapshot_for_tree_with_viewport(
        &mut self,
        tree: &Value,
        viewport: Option<(u16, u16)>,
    ) -> Option<Arc<LayoutNode>> {
        let saved_tree = self.current_widget_tree.clone();
        let saved_layout = self.current_layout.clone();
        let saved_revision = self.layout_revision;
        let saved_dirty = self.dirty_widget_ids.clone();
        #[cfg(test)]
        let saved_rendered_layouts = self.rendered_layouts.clone();
        let saved_cols = self.layout_cols;
        let saved_rows = self.layout_rows;

        if let Some((cols, rows)) = viewport {
            self.layout_cols = cols;
            self.layout_rows = rows;
        }

        self.current_widget_tree = Some(tree.clone());
        self.relayout_current_tree();
        let snapshot = self.current_layout.clone();

        self.current_widget_tree = saved_tree;
        self.current_layout = saved_layout;
        self.layout_revision = saved_revision;
        self.dirty_widget_ids = saved_dirty;
        #[cfg(test)]
        {
            self.rendered_layouts = saved_rendered_layouts;
        }
        self.layout_cols = saved_cols;
        self.layout_rows = saved_rows;
        snapshot
    }

    /// Clear the current widget tree and layout without destroying reactive effects.
    /// Used when switching to a buffer/tile that has no widget tree.
    pub fn clear_current_widget_tree(&mut self) {
        self.current_widget_tree = None;
        self.current_layout = None;
        self.layout_revision = self.layout_revision.wrapping_add(1);
        self.dirty_widget_ids.clear();
    }

    pub fn layout_rows(&self) -> u16 {
        self.layout_rows
    }

    pub fn layout_cols(&self) -> u16 {
        self.layout_cols
    }

    pub fn set_widget_tree(&mut self, tree: Value) {
        // Replace the visual widget tree without destroying reactive effects.
        // Effects from other buffers must survive buffer switches.
        self.current_layout = None;
        self.layout_revision = self.layout_revision.wrapping_add(1);
        self.dirty_widget_ids.clear();
        self.current_widget_tree = Some(tree);
        self.relayout_current_tree();
    }

    /// Restore a previously saved widget tree for display only,
    /// without clearing reactive effects.
    pub fn restore_widget_tree(&mut self, tree: Value) {
        self.current_widget_tree = Some(tree);
        self.relayout_current_tree();
        // Force layout revision bump so GPU caches rebuild
        self.layout_revision = self.layout_revision.wrapping_add(1);
    }

    pub fn clear_layout_effects(&mut self) {
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        self.vm.clear_effects_for_owner(current_buffer_id);
        self.shared
            .borrow_mut()
            .pending_cleared_effect_sources
            .push(current_buffer_id);
        self.current_layout = None;
        self.layout_revision = self.layout_revision.wrapping_add(1);
        self.dirty_widget_ids.clear();
        self.current_widget_tree = None;
        #[cfg(test)]
        self.rendered_layouts.clear();
    }

    fn flush_widget_trees(&mut self) {
        let trees = std::mem::take(&mut self.vm.pending_widget_trees);
        let (current_buffer_id, current_buffer_name) = {
            let shared = self.shared.borrow();
            (shared.current_buffer_id, shared.current_buffer_name.clone())
        };
        for pending in &trees {
            let targets_active_buffer = match &pending.target {
                EffectTarget::BufferId(id) => *id == current_buffer_id,
                EffectTarget::BufferName(name) => *name == current_buffer_name,
            };
            if targets_active_buffer {
                self.current_widget_tree = Some(pending.tree.clone());
                self.relayout_current_tree();
            }
        }
        self.shared
            .borrow_mut()
            .pending_buffer_widget_trees
            .extend(trees);
    }

    fn relayout_current_tree(&mut self) {
        let Some(tree) = self.current_widget_tree.as_ref() else {
            let had_layout = self.current_layout.is_some();
            self.current_layout = None;
            self.dirty_widget_ids.clear();
            if had_layout {
                self.layout_revision = self.layout_revision.wrapping_add(1);
            }
            return;
        };
        let mut dirty_widget_ids = Vec::new();
        if let Some(existing) = self.current_layout.as_ref()
            && let Some(updated) = reuse_layout_node(existing.as_ref(), tree, &mut dirty_widget_ids)
        {
            #[cfg(test)]
            self.rendered_layouts
                .push(crate::layout::format_layout_tree_lines(&updated, 0));
            self.current_layout = Some(Arc::new(updated));
            self.dirty_widget_ids = dirty_widget_ids;
            if !self.dirty_widget_ids.is_empty() {
                self.layout_revision = self.layout_revision.wrapping_add(1);
            }
            return;
        }
        let engine = if let Some(measurer) = self.text_measurer.as_deref() {
            LayoutEngine::with_text_measurer(
                self.layout_cols,
                self.layout_rows,
                self.layout_aspect,
                measurer,
                self.layout_cell_w,
                self.layout_cell_h,
            )
        } else {
            LayoutEngine::new(self.layout_cols, self.layout_rows, self.layout_aspect)
        };
        if let Some(layout) = engine.layout(tree) {
            let geometry_changed = self
                .current_layout
                .as_ref()
                .is_none_or(|existing| !same_layout_geometry(existing.as_ref(), &layout));
            #[cfg(test)]
            self.rendered_layouts
                .push(crate::layout::format_layout_tree_lines(&layout, 0));
            self.current_layout = Some(Arc::new(layout));
            if geometry_changed {
                self.dirty_widget_ids.clear();
                self.layout_revision = self.layout_revision.wrapping_add(1);
            } else if let Some(layout) = self.current_layout.as_ref() {
                self.dirty_widget_ids = collect_shader_widget_ids(layout);
            }
        }
    }

    fn invalidate_symbol_cache(&mut self) {
        self.symbol_revision = self.symbol_revision.wrapping_add(1);
        self.cached_completion_symbols = None;
        self.cached_completion_metadata = None;
    }
}

fn collect_shader_widget_ids(node: &LayoutNode) -> Vec<u64> {
    let mut ids = Vec::new();
    collect_shader_widget_ids_recursive(node, &mut ids);
    ids
}

fn collect_shader_widget_ids_recursive(node: &LayoutNode, ids: &mut Vec<u64>) {
    if matches!(
        node.widget_type.as_str(),
        "slider" | "hslider" | "vslider" | "toggle" | "knob"
    ) {
        ids.push(node.widget_id);
    }
    for child in &node.children {
        collect_shader_widget_ids_recursive(child, ids);
    }
}
