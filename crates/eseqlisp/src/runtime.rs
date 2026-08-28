use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::audio::register_audio_natives;
use crate::backend::Color;
use crate::buffer::{BufferTextStyle, CommittedBufferUiSnapshot};
use crate::host::{BufferId, HostCommand};
use crate::hot_reload::{ReloadReport, SourceOverlay};
use crate::layout::{
    LayoutEngine, LayoutNode, TextMeasurer, layout_contains_widget_id,
    relayout_subtree_path_result, reuse_layout_failure_reason, reuse_layout_node,
    reuse_layout_node_for_subtree_path_result, same_layout_geometry, subtree_root_paths,
};
use crate::reactive::ReactiveRegistry;
use crate::vm::{
    EffectTarget, PendingUiUpdate, PendingWidgetTree, ReactiveFieldKey, VM, Value, VmStateSnapshot,
    freeze_widget_tree, probed_shallow_clone, register_core_natives,
};
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

#[derive(Debug, Clone, Default)]
pub struct ReactiveFlushStats {
    pub widget_tree_flushes: usize,
    pub pending_widget_tree_count: usize,
    pub affected_buffers: Vec<String>,
    pub active_buffer_targets: usize,
    pub inactive_buffer_targets: usize,
    pub full_buffer_reruns: usize,
    pub subtree_reruns: usize,
    pub reevaluated_subtree_roots: usize,
    pub pending_subtree_patch_count: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClearedEffectSource {
    pub source_buffer_id: Option<BufferId>,
    pub runtime_generation: u64,
}

#[derive(Clone)]
pub(crate) struct PendingInlineWidgets {
    pub source_buffer_id: BufferId,
    pub widgets: Vec<Value>,
}

struct ActiveSubtreeReplacement {
    source_buffer_id: Option<BufferId>,
    source_file: Option<PathBuf>,
    target: EffectTarget,
    subtree_root_id: u64,
    tree: Value,
    reactive_dependencies: Vec<ReactiveFieldKey>,
}

#[derive(Debug, Clone, Default)]
pub struct UiInvalidationTrace {
    pub dirty_fields: Vec<String>,
    pub affected_buffers: Vec<String>,
    pub active_buffer_targets: usize,
    pub inactive_buffer_targets: usize,
    pub widget_tree_flushes: usize,
    pub pending_widget_tree_count: usize,
    pub full_buffer_reruns: usize,
    pub subtree_reruns: usize,
    pub reevaluated_subtree_roots: usize,
    pub pending_subtree_patch_count: usize,
    pub subtree_failure_reason: Option<String>,
    pub reactive_apply_duration: Duration,
    pub reactive_flush_duration: Duration,
    pub reactive_cycle_duration: Duration,
    pub reactive_exec_timings: Vec<(String, Duration)>,
    pub reactive_function_profiles: Vec<crate::vm::ReactiveFunctionProfile>,
    pub relayout_mode: Option<String>,
    pub relayout_duration: Duration,
    pub relayout_failure_reason: Option<String>,
}

/// Monotonic UI work counters suitable for event-boundary snapshots.
///
/// Unlike the one-second `ESEQLISP_PROFILE_UI` aggregate, these counters are
/// always available and never reset, so probes can subtract snapshots around
/// one input event without depending on wall-clock logging windows.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct UiWorkCounters {
    pub full_buffer_reruns: u64,
    pub subtree_reruns: u64,
    pub reevaluated_subtree_roots: u64,
    pub relayout_reused: u64,
    pub relayout_full: u64,
    pub relayout_subtree: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReactiveSetResult {
    pub changed: bool,
    pub effects_dirty: bool,
    pub widgets_dirty: bool,
}

struct RuntimePerfStats {
    enabled: bool,
    window_start: Instant,
    reactive_cycles: u64,
    dirty_updates: u64,
    affected_buffers: u64,
    widget_tree_flushes: u64,
    pending_widget_trees: u64,
    active_buffer_targets: u64,
    inactive_buffer_targets: u64,
    full_buffer_reruns: u64,
    subtree_reruns: u64,
    reevaluated_subtree_roots: u64,
    pending_subtree_patch_count: u64,
    reactive_apply: Duration,
    reactive_flush: Duration,
    reactive_total: Duration,
    relayout_reused: u64,
    relayout_full: u64,
    relayout_subtree: u64,
    relayout_total: Duration,
    relayout_failures: HashMap<String, u64>,
    reactive_exec: HashMap<String, (u64, Duration)>,
}

impl RuntimePerfStats {
    fn new() -> Self {
        Self {
            enabled: std::env::var_os("ESEQLISP_PROFILE_UI").is_some(),
            window_start: Instant::now(),
            reactive_cycles: 0,
            dirty_updates: 0,
            affected_buffers: 0,
            widget_tree_flushes: 0,
            pending_widget_trees: 0,
            active_buffer_targets: 0,
            inactive_buffer_targets: 0,
            full_buffer_reruns: 0,
            subtree_reruns: 0,
            reevaluated_subtree_roots: 0,
            pending_subtree_patch_count: 0,
            reactive_apply: Duration::ZERO,
            reactive_flush: Duration::ZERO,
            reactive_total: Duration::ZERO,
            relayout_reused: 0,
            relayout_full: 0,
            relayout_subtree: 0,
            relayout_total: Duration::ZERO,
            relayout_failures: HashMap::new(),
            reactive_exec: HashMap::new(),
        }
    }

    fn note_reactive_cycle(
        &mut self,
        dirty_updates: usize,
        apply: Duration,
        flush: Duration,
        total: Duration,
        exec_timings: Vec<crate::vm::ReactiveExecTiming>,
        flush_stats: &ReactiveFlushStats,
    ) {
        if !self.enabled {
            return;
        }
        self.reactive_cycles += 1;
        self.dirty_updates += dirty_updates as u64;
        self.affected_buffers += flush_stats.affected_buffers.len() as u64;
        self.widget_tree_flushes += flush_stats.widget_tree_flushes as u64;
        self.pending_widget_trees += flush_stats.pending_widget_tree_count as u64;
        self.active_buffer_targets += flush_stats.active_buffer_targets as u64;
        self.inactive_buffer_targets += flush_stats.inactive_buffer_targets as u64;
        self.full_buffer_reruns += flush_stats.full_buffer_reruns as u64;
        self.subtree_reruns += flush_stats.subtree_reruns as u64;
        self.reevaluated_subtree_roots += flush_stats.reevaluated_subtree_roots as u64;
        self.pending_subtree_patch_count += flush_stats.pending_subtree_patch_count as u64;
        self.reactive_apply += apply;
        self.reactive_flush += flush;
        self.reactive_total += total;
        for timing in exec_timings {
            let key = timing.profile_label();
            let entry = self.reactive_exec.entry(key).or_insert((0, Duration::ZERO));
            entry.0 += 1;
            entry.1 += timing.elapsed;
        }
        self.maybe_emit();
    }

    fn note_relayout(
        &mut self,
        reused: bool,
        subtree_only: bool,
        elapsed: Duration,
        failure_reason: Option<String>,
    ) {
        if !self.enabled {
            return;
        }
        if subtree_only {
            self.relayout_subtree += 1;
        }
        if reused {
            self.relayout_reused += 1;
        } else {
            self.relayout_full += 1;
            if let Some(reason) = failure_reason {
                *self.relayout_failures.entry(reason).or_insert(0) += 1;
            }
        }
        self.relayout_total += elapsed;
        self.maybe_emit();
    }

    fn maybe_emit(&mut self) {
        if !self.enabled || self.window_start.elapsed().as_secs_f64() < 1.0 {
            return;
        }
        let secs = self.window_start.elapsed().as_secs_f64();
        let reactive_avg_ms = if self.reactive_cycles > 0 {
            self.reactive_total.as_secs_f64() * 1000.0 / self.reactive_cycles as f64
        } else {
            0.0
        };
        let apply_avg_ms = if self.reactive_cycles > 0 {
            self.reactive_apply.as_secs_f64() * 1000.0 / self.reactive_cycles as f64
        } else {
            0.0
        };
        let flush_avg_ms = if self.reactive_cycles > 0 {
            self.reactive_flush.as_secs_f64() * 1000.0 / self.reactive_cycles as f64
        } else {
            0.0
        };
        let relayout_calls = self.relayout_reused + self.relayout_full;
        let relayout_avg_ms = if relayout_calls > 0 {
            self.relayout_total.as_secs_f64() * 1000.0 / relayout_calls as f64
        } else {
            0.0
        };
        let affected_buffers_per_cycle = if self.reactive_cycles > 0 {
            self.affected_buffers as f64 / self.reactive_cycles as f64
        } else {
            0.0
        };
        let flushes_per_cycle = if self.reactive_cycles > 0 {
            self.widget_tree_flushes as f64 / self.reactive_cycles as f64
        } else {
            0.0
        };
        eprintln!(
            "[ui-profile][runtime] reactive/s={:.1} dirty/cycle={:.1} buffers/cycle={affected_buffers_per_cycle:.1} flushes/cycle={flushes_per_cycle:.1} apply_avg={apply_avg_ms:.2}ms flush_avg={flush_avg_ms:.2}ms reactive_avg={reactive_avg_ms:.2}ms targets=a{} i{} reruns=full:{} sub:{} roots:{} patches:{} relayout/s={:.1} relayout_avg={relayout_avg_ms:.2}ms reused={} full={} subtree={} fail={} hot={}",
            self.reactive_cycles as f64 / secs,
            if self.reactive_cycles > 0 {
                self.dirty_updates as f64 / self.reactive_cycles as f64
            } else {
                0.0
            },
            self.active_buffer_targets,
            self.inactive_buffer_targets,
            self.full_buffer_reruns,
            self.subtree_reruns,
            self.reevaluated_subtree_roots,
            self.pending_subtree_patch_count,
            relayout_calls as f64 / secs,
            self.relayout_reused,
            self.relayout_full,
            self.relayout_subtree,
            self.top_failure_reason(),
            self.top_reactive_exec(),
        );
        self.window_start = Instant::now();
        self.reactive_cycles = 0;
        self.dirty_updates = 0;
        self.affected_buffers = 0;
        self.widget_tree_flushes = 0;
        self.pending_widget_trees = 0;
        self.active_buffer_targets = 0;
        self.inactive_buffer_targets = 0;
        self.full_buffer_reruns = 0;
        self.subtree_reruns = 0;
        self.reevaluated_subtree_roots = 0;
        self.pending_subtree_patch_count = 0;
        self.reactive_apply = Duration::ZERO;
        self.reactive_flush = Duration::ZERO;
        self.reactive_total = Duration::ZERO;
        self.relayout_reused = 0;
        self.relayout_full = 0;
        self.relayout_subtree = 0;
        self.relayout_total = Duration::ZERO;
        self.relayout_failures.clear();
        self.reactive_exec.clear();
    }

    fn top_failure_reason(&self) -> String {
        self.relayout_failures
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(reason, count)| format!("{reason}({count})"))
            .unwrap_or_else(|| "-".to_string())
    }

    fn top_reactive_exec(&self) -> String {
        let mut entries = self
            .reactive_exec
            .iter()
            .map(|(label, (count, total))| (label, *count, *total))
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| b.2.cmp(&a.2));
        let summary = entries
            .into_iter()
            .take(3)
            .map(|(label, count, total)| {
                format!("{label}:{:.1}ms/{count}", total.as_secs_f64() * 1000.0)
            })
            .collect::<Vec<_>>()
            .join(",");
        if summary.is_empty() {
            "-".to_string()
        } else {
            summary
        }
    }
}

fn summarize_cycle_exec_timings(exec_timings: &[crate::vm::ReactiveExecTiming]) -> String {
    let mut entries = exec_timings
        .iter()
        .map(|timing| (timing.profile_label(), timing.elapsed))
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    let summary = entries
        .into_iter()
        .take(5)
        .map(|(label, elapsed)| format!("{label}:{:.2}ms", elapsed.as_secs_f64() * 1000.0))
        .collect::<Vec<_>>()
        .join(", ");
    if summary.is_empty() {
        "-".to_string()
    } else {
        summary
    }
}

fn format_ui_invalidation_trace(
    trace: &UiInvalidationTrace,
    exec_timings: &[crate::vm::ReactiveExecTiming],
) -> String {
    let dirty_fields = if trace.dirty_fields.is_empty() {
        "-".to_string()
    } else {
        trace.dirty_fields.join(",")
    };
    let affected_buffers = if trace.affected_buffers.is_empty() {
        "-".to_string()
    } else {
        trace.affected_buffers.join(",")
    };
    let relayout_mode = trace.relayout_mode.as_deref().unwrap_or("-");
    let relayout_failure = trace.relayout_failure_reason.as_deref().unwrap_or("-");
    let subtree_failure = trace.subtree_failure_reason.as_deref().unwrap_or("-");
    format!(
        "[ui-trace] dirty=[{dirty_fields}] affected=[{affected_buffers}] targets=a{} i{} flushes={} pending={} reruns=full:{} sub:{} roots:{} patches:{} apply_ms={:.3} flush_ms={:.3} total_ms={:.3} subtree-fail={} relayout={} relayout_ms={:.3} fail={} hot=[{}]",
        trace.active_buffer_targets,
        trace.inactive_buffer_targets,
        trace.widget_tree_flushes,
        trace.pending_widget_tree_count,
        trace.full_buffer_reruns,
        trace.subtree_reruns,
        trace.reevaluated_subtree_roots,
        trace.pending_subtree_patch_count,
        trace.reactive_apply_duration.as_secs_f64() * 1000.0,
        trace.reactive_flush_duration.as_secs_f64() * 1000.0,
        trace.reactive_cycle_duration.as_secs_f64() * 1000.0,
        subtree_failure,
        relayout_mode,
        trace.relayout_duration.as_secs_f64() * 1000.0,
        relayout_failure,
        summarize_cycle_exec_timings(exec_timings),
    )
}

fn trace_ui_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("ESEQLISP_TRACE_UI").is_some())
}

fn trace_ui_field_enabled(namespace: &str, field: &str) -> bool {
    if !trace_ui_enabled() {
        return false;
    }
    let Some(filter) = std::env::var_os("ESEQLISP_TRACE_UI_FILTER") else {
        return true;
    };
    let filter = filter.to_string_lossy();
    let qualified = format!("{namespace}.{field}");
    filter.split(',').map(str::trim).any(|entry| {
        !entry.is_empty() && (entry == field || entry == qualified || entry == namespace)
    })
}

fn summarize_reactive_item(value: &Value) -> String {
    match value {
        Value::List(items) => format!("<list len={}>", items.len()),
        Value::Map(map) => {
            let mut keys = map.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            let rendered = keys.into_iter().take(8).collect::<Vec<_>>().join(",");
            let suffix = if map.len() > 8 { ",..." } else { "" };
            format!("<map keys=[{rendered}{suffix}] len={}>", map.len())
        }
        Value::String(s) if s.len() > 80 => format!("{:?}...", &s[..80]),
        other => crate::vm::format_lisp_value(other),
    }
}

fn summarize_reactive_value(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "<missing>".to_string();
    };
    match value {
        Value::List(items) => {
            let rendered = items
                .iter()
                .take(12)
                .map(|item| summarize_reactive_item(&item.borrow()))
                .collect::<Vec<_>>()
                .join(" ");
            let suffix = if items.len() > 12 { " ..." } else { "" };
            format!("({rendered}{suffix}) len={}", items.len())
        }
        Value::Map(map) => {
            let mut keys = map.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            let rendered = keys.into_iter().take(12).collect::<Vec<_>>().join(",");
            let suffix = if map.len() > 12 { ",..." } else { "" };
            format!("{{keys=[{rendered}{suffix}]}} len={}", map.len())
        }
        other => crate::vm::format_lisp_value(other),
    }
}

fn expand_sdf_expression(
    expr: &crate::parser::Expression,
    vm: &mut VM,
) -> Result<crate::parser::Expression, String> {
    // Module-owned shader macros must be referenced with qualified names.
    vm.expand_macros_expression(expr)
}

struct SdfCompileResult {
    output: crate::lang::sdf_codegen::SdfShaderOutput,
    expanded_expr: crate::parser::Expression,
    state_symbols: Vec<String>,
}

/// Compile an SDF widget shader in the language of the platform's render
/// backend: MSL for the macOS Metal backend, WGSL for the wgpu backend
/// everywhere else. The registry stores exactly one source per widget, so the
/// emitter choice must match the backend that will consume it.
fn compile_sdf_for_platform_backend(
    expr: &crate::parser::Expression,
    state_symbols: &[String],
    options: crate::lang::sdf_codegen::SdfShaderOptions,
) -> Result<crate::lang::sdf_codegen::SdfShaderOutput, crate::lang::sdf_codegen::CodegenError> {
    #[cfg(target_os = "macos")]
    {
        crate::lang::sdf_codegen::compile_sdf_to_metal_with_state_and_options(
            expr,
            state_symbols,
            options,
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        crate::lang::sdf_codegen::compile_sdf_to_wgsl_with_state_and_options(
            expr,
            state_symbols,
            options,
        )
    }
}

fn compile_sdf_value(
    value: &Value,
    vm: &mut VM,
    state_bindings: &std::collections::HashSet<String>,
) -> Result<SdfCompileResult, String> {
    let expr = crate::lang::sdf_codegen::value_to_expression(value).map_err(|e| e.to_string())?;
    let expanded = expand_sdf_expression(&expr, vm)?;
    let mut state_symbols =
        crate::lang::sdf_codegen::collect_state_symbols(&expanded, state_bindings);
    state_symbols.truncate(crate::widget_render::sdf_widget::MAX_SDF_STATE_UNIFORMS);
    let options = crate::lang::sdf_codegen::SdfShaderOptions::from_env()?;
    let output = compile_sdf_for_platform_backend(&expanded, &state_symbols, options)
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
    /// Theme generation the SDF widget registry was last recompiled against.
    static SDF_THEME_RECOMPILE_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Iterative walk — SDF expressions can nest deeply enough that recursion is a
/// stack risk (see Expression::clone).
fn expression_contains_keyword(expr: &crate::parser::Expression) -> bool {
    use crate::parser::Expression;
    let mut stack = vec![expr];
    while let Some(e) = stack.pop() {
        match e {
            Expression::Keyword(_) => return true,
            Expression::List(items) | Expression::QuoteList(items) => stack.extend(items.iter()),
            Expression::Quasiquote(inner)
            | Expression::Unquote(inner)
            | Expression::UnquoteSplicing(inner) => stack.push(inner),
            Expression::Symbol(_)
            | Expression::String(_)
            | Expression::QuoteSymbol(_)
            | Expression::Number(_) => {}
        }
    }
    false
}

/// Theme keyword colors are baked into SDF shader source as literals at emit
/// time, so a theme switch has to re-emit those shaders. Re-registering a def
/// under the same name changes its stored source and bumps the registry
/// generation, which makes both render backends rebuild exactly the changed
/// pipelines on the next frame. Returns true if any shader source changed.
pub fn recompile_theme_dependent_sdf_shaders() -> bool {
    let generation = crate::ui::theme::generation();
    if SDF_THEME_RECOMPILE_GENERATION.with(|g| g.get()) == generation {
        return false;
    }
    SDF_THEME_RECOMPILE_GENERATION.with(|g| g.set(generation));
    let options = match crate::lang::sdf_codegen::SdfShaderOptions::from_env() {
        Ok(options) => options,
        Err(_) => return false,
    };
    let mut changed = false;
    for def in crate::widget_render::sdf_widget::sdf_widget_defs() {
        if !expression_contains_keyword(&def.sdf_expr) {
            continue;
        }
        match compile_sdf_for_platform_backend(&def.sdf_expr, &def.state_uniforms, options) {
            Ok(output) => {
                if output.shader_source != def.shader_source {
                    let mut new_def = (*def).clone();
                    new_def.shader_source = output.shader_source;
                    new_def.region_count = output.region_count;
                    crate::widget_render::sdf_widget::register_sdf_widget(new_def);
                    changed = true;
                }
            }
            Err(e) => {
                eprintln!("theme recompile of SDF widget '{}' failed: {}", def.name, e);
            }
        }
    }
    changed
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
            let inner = items
                .iter()
                .map(expr_to_source)
                .collect::<Vec<_>>()
                .join(" ");
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
            let inner = items
                .iter()
                .map(expr_to_source)
                .collect::<Vec<_>>()
                .join(" ");
            format!("({inner})")
        }
        Expression::Quasiquote(inner) => format!("`{}", expr_to_source(inner)),
        Expression::Unquote(inner) => format!(",{}", expr_to_source(inner)),
        Expression::UnquoteSplicing(inner) => format!(",@{}", expr_to_source(inner)),
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
        _ => {
            return Err(format!(
                "widget type '{widget_type}' does not support :material"
            ));
        }
    };
    parse_one_expr(&src)
}

/// Compile a :material value for a built-in widget, caching the result.
/// Returns the registered SDF widget name if successful.
fn compile_widget_material(
    widget_type: &str,
    material_val: &Value,
    vm: &mut VM,
    state_binding_keys: &[String],
    prop_binding_keys: &[String],
) -> Result<String, String> {
    let material_expr =
        crate::lang::sdf_codegen::value_to_expression(material_val).map_err(|e| e.to_string())?;

    let shader_expr = build_material_shader_expr(widget_type, &material_expr)?;

    // For vslider, add origin_t so it gets a uniform slot.
    let mut bindings: std::collections::HashSet<String> =
        state_binding_keys.iter().cloned().collect();
    bindings.extend(prop_binding_keys.iter().cloned());
    if widget_type == "vslider" {
        bindings.insert("origin_t".to_string());
    }
    let expanded = expand_sdf_expression(&shader_expr, vm)?;
    let mut hasher = DefaultHasher::new();
    widget_type.hash(&mut hasher);
    expr_to_source(&expanded).hash(&mut hasher);
    let mut binding_keys = bindings.iter().cloned().collect::<Vec<_>>();
    binding_keys.sort();
    binding_keys.hash(&mut hasher);
    let options = crate::lang::sdf_codegen::SdfShaderOptions::from_env()?;
    options.hash(&mut hasher);
    let cache_key = hasher.finish();

    if let Some(name) = MATERIAL_SHADER_CACHE.with(|c| c.borrow().get(&cache_key).cloned()) {
        return Ok(name);
    }

    let mut state_symbols = crate::lang::sdf_codegen::collect_state_symbols(&expanded, &bindings);
    state_symbols.truncate(crate::widget_render::sdf_widget::MAX_SDF_STATE_UNIFORMS);
    let output = compile_sdf_for_platform_backend(&expanded, &state_symbols, options)
        .map_err(|e| e.to_string())?;

    let paint_margin =
        crate::widget_render::sdf_widget::estimate_shadow_paint_margin(&expanded, 16.0, 8.0);

    let shader_name = format!("{}__mat_{:x}", widget_type, cache_key);
    crate::widget_render::sdf_widget::register_inline_shader(
        shader_name.clone(),
        output.shader_source,
        expanded,
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
    /// Switch the buffer in the tile currently showing `current` to `new_name`.
    SetWindowBufferFor {
        current: String,
        new_name: String,
    },
    SetWindowTabsFor {
        current: String,
        tabs: Vec<LayoutTabSpec>,
    },
    ClearWindowTabsFor {
        current: String,
    },
    SetLayout(LayoutSpec),
}

/// Declarative layout specification for `set-layout`.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutTabSpec {
    pub label: String,
    pub buffer_name: String,
    pub on_close: Option<Value>,
}

#[derive(Debug, Clone)]
pub enum LayoutSpec {
    Buffer {
        name: String,
        tabs: Vec<LayoutTabSpec>,
        hide_status: bool,
        borderless: bool,
        border_width_px: f32,
        border_radius_px: f32,
        background_color: Option<Color>,
        background_color_name: Option<String>,
        min_width: Option<f32>,
        min_height: Option<f32>,
        max_width: Option<f32>,
        max_height: Option<f32>,
        collapse_threshold: Option<f32>,
        on_collapse: Option<Value>,
    },
    Rows {
        gap: f32,
        remember: Option<String>,
        panes: Vec<(f32, LayoutSpec)>,
    },
    Cols {
        gap: f32,
        remember: Option<String>,
        panes: Vec<(f32, LayoutSpec)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolMetadata {
    pub signature: String,
    pub docs: String,
    /// Accepted keyword argument names, normalized to include the leading `:`.
    /// Explicit metadata avoids confusing keyword-valued examples for names.
    pub keyword_args: Vec<String>,
}

fn normalize_keyword_args<I, S>(keyword_args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen = HashSet::new();
    keyword_args
        .into_iter()
        .map(|keyword| format!(":{}", keyword.as_ref().trim_start_matches(':')))
        .filter(|keyword| keyword.len() > 1 && seen.insert(keyword.clone()))
        .collect()
}

#[derive(Clone)]
pub(crate) struct PendingModeDefinition {
    pub name: String,
    pub read_only: bool,
    pub live_keys: bool,
    pub on_enter: Option<String>,
    pub on_key: Option<String>,
}

/// Buffer metadata mirrored into the runtime for `buffer-info-list`.
#[derive(Clone, Default)]
pub struct SharedBufferInfo {
    pub name: String,
    pub mode: String,
    pub path: Option<String>,
    pub line_count: usize,
    pub dirty: bool,
    pub read_only: bool,
}

#[derive(Clone, Default)]
pub(crate) struct RuntimeBridgeState {
    /// The module of the chunk executing the current native call (None =
    /// implicit eseq.vanilla). Stamped by `register_native_impl` before
    /// each dispatch so registration natives (`bind-key`, `define-mode`,
    /// …) can qualify late-bound handler-name strings (spec §5).
    pub current_native_module: Option<String>,
    pub current_buffer_id: Option<BufferId>,
    pub current_buffer_name: String,
    pub current_buffer_path: Option<PathBuf>,
    pub current_buffer_text: String,
    pub current_sexp: Option<String>,
    pub status_message: Option<String>,
    pub queued_commands: Vec<HostCommand>,
    pub lisp_bindings: HashMap<String, String>,
    pub pending_eval_buffer: Option<BufferId>,
    pub pending_save: bool,
    pub pending_save_as: Option<PathBuf>,
    pub pending_load: bool,
    pub current_buffer_read_only: bool,
    pub pending_set_read_only: Option<bool>,
    pub current_buffer_mode: String,
    pub pending_mode_defs: Vec<PendingModeDefinition>,
    pub pending_mode_bindings: Vec<(String, String, String)>, // (mode, key, handler)
    pub pending_set_mode: Option<String>,
    pub pending_set_mode_for: Vec<(String, String)>, // (buffer_name, mode_name)
    pub pending_open_file: Option<String>,
    pub pending_widget_tree: Option<Value>,
    pub pending_buffer_widget_trees: Vec<PendingUiUpdate>,
    pub pending_inline_widgets: Option<PendingInlineWidgets>,
    pub pending_create_buffer: Option<String>,
    pub pending_cleared_effect_sources: Vec<ClearedEffectSource>,
    pub pending_switch_buffer: Option<String>,
    pub pending_set_text: Option<String>,
    pub pending_set_text_for: Vec<(String, String)>,
    pub pending_append_text_for: Vec<(String, String, String)>,
    pub pending_append_lines_for: Vec<(String, Vec<String>)>,
    pub pending_remove_lines_for: Vec<(String, Vec<String>)>,
    pub pending_set_lines: Option<Vec<String>>,
    pub pending_set_buffer_styles: Option<Vec<BufferTextStyle>>,
    pub pending_goto_line: Option<usize>,
    pub current_line_number: usize,
    pub current_line_text: String,
    pub buffer_names: Vec<String>,
    /// Per-buffer metadata mirrored for the buffer-list UI, same order as
    /// `buffer_names` (most to least recently selected).
    pub buffer_infos: Vec<SharedBufferInfo>,
    /// Names of buffers currently presented in a tile, sorted by name.
    pub visible_buffer_names: Vec<String>,
    pub pending_cycle_view_mode: bool,
    pub pending_set_view_mode: Option<String>,
    pub current_view_mode: String,
    pub pending_set_text_zoom: Option<f64>,
    pub current_text_zoom: f64,
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
    reactive_reads: Vec<ReactiveFieldKey>,
    reactive_invalidations: Vec<(ReactiveFieldKey, Value)>,
}

impl NativeContext {
    pub(crate) fn new(shared: SharedBridgeState) -> Self {
        Self {
            shared,
            reactive_reads: Vec::new(),
            reactive_invalidations: Vec::new(),
        }
    }

    /// Inject a reactive dependency for the currently rendering effect.
    /// Calls made outside reactive rendering are intentionally inert.
    pub fn track_reactive_read(&mut self, namespace: impl Into<String>, field: impl Into<String>) {
        self.reactive_reads
            .push(ReactiveFieldKey::new(namespace, field));
    }

    /// Dirty effects which previously tracked this host-owned reactive source.
    /// `generation` must change for each semantic invalidation.
    pub fn invalidate_reactive_source(
        &mut self,
        namespace: impl Into<String>,
        field: impl Into<String>,
        generation: Value,
    ) {
        self.reactive_invalidations
            .push((ReactiveFieldKey::new(namespace, field), generation));
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

    /// The declared module of the chunk executing this native call, if
    /// any (None inside headerless eseq.vanilla code).
    pub fn current_module(&self) -> Option<String> {
        self.shared.borrow().current_native_module.clone()
    }

    /// Registry auto-qualification for late-bound handler-name strings
    /// (spec §5): a `bind-key` / mode handler registered from a declared
    /// module stores `module/handler` so dispatch resolves against the
    /// registering module first (the editor falls back to the flat base
    /// name when the module has no such global). Vanilla registrations
    /// stay verbatim.
    pub fn qualify_registration_name(&self, name: &str) -> String {
        match self.current_module() {
            Some(module) if !crate::modules::is_qualified(name) => {
                crate::modules::qualify(&module, name)
            }
            _ => name.to_string(),
        }
    }

    pub fn bind_key(&mut self, key: String, handler: String) {
        let handler = self.qualify_registration_name(&handler);
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

    pub fn request_eval_buffer(&mut self) {
        self.shared.borrow_mut().pending_eval_buffer = self.current_buffer_id();
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
        live_keys: bool,
        on_enter: Option<String>,
        on_key: Option<String>,
    ) {
        // Registry auto-qualification (spec §5): a declared module's mode
        // name AND its late-bound handler strings capture the module
        // current at definition time.
        let name = self.qualify_registration_name(&name);
        let on_enter = on_enter.map(|h| self.qualify_registration_name(&h));
        let on_key = on_key.map(|h| self.qualify_registration_name(&h));
        self.shared
            .borrow_mut()
            .pending_mode_defs
            .push(PendingModeDefinition {
                name,
                read_only,
                live_keys,
                on_enter,
                on_key,
            });
    }

    pub fn mode_bind_key(&mut self, mode: String, key: String, handler: String) {
        // Mode references qualify against the current module first; the
        // editor falls back to the flat mode name when the module has no
        // mode of that name (referencing a vanilla mode from a module).
        let mode = self.qualify_registration_name(&mode);
        let handler = self.qualify_registration_name(&handler);
        self.shared
            .borrow_mut()
            .pending_mode_bindings
            .push((mode, key, handler));
    }

    pub fn set_buffer_mode(&mut self, mode: String) {
        let mode = self.qualify_registration_name(&mode);
        self.shared.borrow_mut().pending_set_mode = Some(mode);
    }

    pub fn set_buffer_mode_for(&mut self, buffer_name: String, mode: String) {
        let mode = self.qualify_registration_name(&mode);
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
        freeze_widget_tree(&tree);
        self.shared
            .borrow_mut()
            .pending_widget_tree
            .replace(probed_shallow_clone("w2:render-widget", &tree));
    }

    pub fn render_widget_to_buffer(&mut self, buffer_name: String, tree: Value) {
        freeze_widget_tree(&tree);
        let source_buffer_id = self.shared.borrow().current_buffer_id;
        self.shared
            .borrow_mut()
            .pending_buffer_widget_trees
            .push(PendingUiUpdate::FullTree(PendingWidgetTree {
                source_buffer_id,
                source_file: None,
                target: EffectTarget::BufferName(buffer_name),
                tree: probed_shallow_clone("w2:render-widget-to-buffer", &tree),
                reactive_dependencies: Vec::new(),
            }));
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

    pub fn visible_buffer_names(&self) -> Vec<String> {
        self.shared.borrow().visible_buffer_names.clone()
    }

    pub fn buffer_infos(&self) -> Vec<SharedBufferInfo> {
        self.shared.borrow().buffer_infos.clone()
    }

    pub fn set_buffer_text_for(&mut self, name: String, text: String) {
        self.shared
            .borrow_mut()
            .pending_set_text_for
            .push((name, text));
    }

    pub fn append_buffer_text_for(&mut self, name: String, text: String, separator: String) {
        self.shared
            .borrow_mut()
            .pending_append_text_for
            .push((name, text, separator));
    }

    pub fn append_buffer_lines_for(&mut self, name: String, lines: Vec<String>) {
        self.shared
            .borrow_mut()
            .pending_append_lines_for
            .push((name, lines));
    }

    pub fn remove_buffer_lines_for(&mut self, name: String, lines: Vec<String>) {
        self.shared
            .borrow_mut()
            .pending_remove_lines_for
            .push((name, lines));
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

    pub fn set_window_buffer_for(&mut self, current: String, new_name: String) {
        self.shared
            .borrow_mut()
            .pending_tile_ops
            .push(TileOp::SetWindowBufferFor { current, new_name });
    }

    pub fn set_window_tabs_for(&mut self, current: String, tabs: Vec<LayoutTabSpec>) {
        self.shared
            .borrow_mut()
            .pending_tile_ops
            .push(TileOp::SetWindowTabsFor { current, tabs });
    }

    pub fn clear_window_tabs_for(&mut self, current: String) {
        self.shared
            .borrow_mut()
            .pending_tile_ops
            .push(TileOp::ClearWindowTabsFor { current });
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

    pub fn set_text_zoom(&mut self, zoom: f64) {
        self.shared.borrow_mut().pending_set_text_zoom = Some(zoom);
    }

    pub fn text_zoom(&self) -> f64 {
        let zoom = self.shared.borrow().current_text_zoom;
        if zoom.is_finite() && zoom > 0.0 {
            zoom
        } else {
            1.0
        }
    }

    pub fn apply_theme(&mut self, map: Value) {
        self.shared.borrow_mut().pending_apply_theme = Some(map);
    }
}

pub struct Runtime {
    vm: VM,
    pub(crate) shared: SharedBridgeState,
    sync_theme_to_global: bool,
    symbol_metadata: HashMap<String, SymbolMetadata>,
    symbol_revision: u64,
    cached_completion_symbols: Option<Vec<String>>,
    cached_completion_metadata: Option<HashMap<String, SymbolMetadata>>,
    module_completion_roots: Vec<crate::hot_reload::ModuleLoadRoot>,
    cached_module_completions: Option<Vec<String>>,
    pub reactive_registry: ReactiveRegistry,
    pending_injected_reactive_invalidations: HashMap<ReactiveFieldKey, Value>,
    #[cfg(test)]
    rendered_layouts: Vec<Vec<String>>,
    pub current_layout: Option<Arc<LayoutNode>>,
    layout_revision: u64,
    dirty_widget_ids: Vec<u64>,
    force_layout_revision_bump: bool,
    deferred_layout_invalidated: bool,
    current_widget_tree: Option<Value>,
    current_committed_ui_snapshot: Option<CommittedBufferUiSnapshot>,
    current_committed_ui_snapshot_generation: u64,
    layout_cols: f32,
    layout_rows: f32,
    layout_aspect: f32,
    layout_cell_w: f32,
    layout_cell_h: f32,
    /// Whole-window viewport in the current tile's local cell coordinates
    /// (negative origin for non-top-left tiles). Set by the tiled frame
    /// builder; `None` means frame-anchored widgets fall back to the tile's
    /// own root area.
    layout_frame_viewport: Option<crate::layout::Rect>,
    layout_content_scroll: (f32, f32),
    widget_id_offset: u64,
    text_measurer: Option<Rc<dyn TextMeasurer>>,
    perf_stats: RuntimePerfStats,
    ui_work_counters: UiWorkCounters,
    last_ui_invalidation_trace: Option<UiInvalidationTrace>,
}

struct RuntimeStateSnapshot {
    vm: VmStateSnapshot,
    shared: RuntimeBridgeState,
    symbol_metadata: HashMap<String, SymbolMetadata>,
    symbol_revision: u64,
    cached_completion_symbols: Option<Vec<String>>,
    cached_completion_metadata: Option<HashMap<String, SymbolMetadata>>,
    cached_module_completions: Option<Vec<String>>,
    reactive_registry: ReactiveRegistry,
    pending_injected_reactive_invalidations: HashMap<ReactiveFieldKey, Value>,
    current_layout: Option<Arc<LayoutNode>>,
    layout_revision: u64,
    dirty_widget_ids: Vec<u64>,
    force_layout_revision_bump: bool,
    deferred_layout_invalidated: bool,
    current_widget_tree: Option<Value>,
    current_committed_ui_snapshot: Option<CommittedBufferUiSnapshot>,
    current_committed_ui_snapshot_generation: u64,
    last_ui_invalidation_trace: Option<UiInvalidationTrace>,
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
        let reactive_registry = ReactiveRegistry::with_float_slots(vm.reactive_float_slots.clone());
        let mut runtime = Self {
            vm,
            shared,
            sync_theme_to_global: true,
            symbol_metadata: HashMap::new(),
            symbol_revision: 0,
            cached_completion_symbols: None,
            cached_completion_metadata: None,
            module_completion_roots: Vec::new(),
            cached_module_completions: None,
            reactive_registry,
            pending_injected_reactive_invalidations: HashMap::new(),
            #[cfg(test)]
            rendered_layouts: Vec::new(),
            current_layout: None,
            layout_revision: 0,
            dirty_widget_ids: Vec::new(),
            force_layout_revision_bump: false,
            deferred_layout_invalidated: false,
            current_widget_tree: None,
            current_committed_ui_snapshot: None,
            current_committed_ui_snapshot_generation: 0,
            layout_cols: 80.0,
            layout_rows: 24.0,
            layout_aspect: 1.0,
            layout_cell_w: 1.0,
            layout_cell_h: 1.0,
            layout_frame_viewport: None,
            layout_content_scroll: (0.0, 0.0),
            widget_id_offset: 0,
            text_measurer: None,
            perf_stats: RuntimePerfStats::new(),
            ui_work_counters: UiWorkCounters::default(),
            last_ui_invalidation_trace: None,
        };
        runtime.document_builtin_symbols();
        runtime.register_reactive("THEME", crate::theme::reactive_fields(), true);
        register_audio_natives(&mut runtime);
        runtime
            .vm
            .register_native_with_vm("current-source-path", |_args, vm| {
                vm.source_manager
                    .current_source_file()
                    .map(|path| Value::String(path.display().to_string()))
                    .unwrap_or_else(|| Value::String(String::new()))
            });
        // (load path) — read through the source manager so dirty editor overlays,
        // load-stack-relative paths, and explicit `@/` cwd-relative paths
        // participate in normal evaluation.
        runtime.vm.register_native_with_vm("load", |args, vm| {
            let Some(Value::String(path_str)) = args.first() else {
                return Value::String("load: expects a string path".into());
            };
            let loaded = match vm.source_manager.load_source(path_str) {
                Ok(loaded) => loaded,
                Err(error) => {
                    let message = format!("load: {error}");
                    vm.source_load_errors.push(message.clone());
                    return Value::String(message);
                }
            };
            let loaded_path_display = loaded.path.display().to_string();
            match vm.eval_module_source(loaded.path, &loaded.text, loaded.revision) {
                Ok(v) => v.unwrap_or(Value::Bool(true)),
                Err(e) => {
                    let message = format!("load: {loaded_path_display}: eval error: {e:?}");
                    vm.source_load_errors.push(message.clone());
                    Value::String(message)
                }
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
        let sdf_src = include_str!("../../../content/core/sdf-stdlib.lisp");
        if !sdf_src.trim().is_empty() {
            let _ = runtime.eval_str(sdf_src);
        }
        // Register sdf->metal: takes a quoted SDF expression, returns Metal shader string
        runtime.vm.register_native_with_vm("sdf->metal", move |args, vm| {
            let Some(val) = args.first() else {
                return Value::String("error: sdf->metal requires 1 argument".into());
            };
            match compile_sdf_value(val, vm, &std::collections::HashSet::new()) {
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
                let mut bindable_props: Vec<String> = Vec::new();
                let mut animates = false;

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
                            "animates" => {
                                animates = matches!(args[i + 1], Value::Bool(true));
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
                            "bindable" => {
                                if let Value::List(items) = &args[i + 1] {
                                    for item in items {
                                        match &*item.borrow() {
                                            Value::Symbol(s)
                                            | Value::Keyword(s)
                                            | Value::String(s) => bindable_props.push(s.clone()),
                                            _ => {}
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
                let compiled = match compile_sdf_value(&shader_val, vm, &state_bindings) {
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
                    bindable_props,
                    region_count: compiled.output.region_count,
                    width,
                    height,
                    paint_margin,
                    animates,
                });

                let widget_type = name.clone();
                let state_uniforms = compiled.state_symbols;
                vm.register_native_with_vm(&name, move |args, vm| {
                    let mut widget = crate::widgets::build_widget(&widget_type, args);
                    vm.qualify_widget_stable_key(&mut widget);
                    if let Value::Map(map) = &mut widget {
                        for state_name in &state_uniforms {
                            let explicit_value =
                                map.get(state_name).map(|cell| cell.borrow().clone());
                            if let Some(value) =
                                explicit_value.or_else(|| vm.read_tracked_state_value(state_name))
                            {
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
                    vm.qualify_widget_stable_key(&mut widget);
                    if let Value::Map(map) = &mut widget {
                        if let Some(material_cell) = map.get("material") {
                            let material_val = material_cell.borrow().clone();
                            if !matches!(material_val, Value::Nil) {
                                let keys: Vec<String> =
                                    vm.state_bindings.keys().cloned().collect();
                                let prop_keys = map.keys().cloned().collect::<Vec<_>>();
                                match compile_widget_material(
                                    &wtype,
                                    &material_val,
                                    vm,
                                    &keys,
                                    &prop_keys,
                                ) {
                                    Ok(shader_name) => {
                                        if let Some(def) = crate::widget_render::sdf_widget::sdf_widget_def(&shader_name) {
                                            for state_name in &def.state_uniforms {
                                                let explicit_value = map
                                                    .get(state_name)
                                                    .map(|cell| cell.borrow().clone());
                                                if let Some(value) = explicit_value
                                                    .or_else(|| vm.read_tracked_state_value(state_name))
                                                {
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

    pub fn reactive_binding_store(&self) -> crate::reactive::ReactiveBindingStore {
        self.vm.reactive_float_slots.clone()
    }

    pub fn set_theme_sync_enabled(&mut self, enabled: bool) {
        self.sync_theme_to_global = enabled;
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

    pub fn register_native_with_docs_and_keywords<F, I, S>(
        &mut self,
        name: &str,
        signature: impl Into<String>,
        docs: impl Into<String>,
        keyword_args: I,
        f: F,
    ) where
        F: Fn(Vec<Value>, &mut NativeContext) -> NativeResult + 'static,
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.register_native_impl(name, Some(signature.into()), Some(docs.into()), f);
        if let Some(metadata) = self.symbol_metadata.get_mut(name) {
            metadata.keyword_args = normalize_keyword_args(keyword_args);
        }
        self.invalidate_symbol_cache();
    }

    pub fn register_vm_native_with_docs_and_keywords<F, I, S>(
        &mut self,
        name: &str,
        signature: impl Into<String>,
        docs: impl Into<String>,
        keyword_args: I,
        f: F,
    ) where
        F: Fn(Vec<Value>, &mut crate::vm::VM) -> Value + 'static,
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.vm.register_native_with_vm(name, f);
        self.symbol_metadata.insert(
            name.to_string(),
            SymbolMetadata {
                signature: signature.into(),
                docs: docs.into(),
                keyword_args: normalize_keyword_args(keyword_args),
            },
        );
        self.invalidate_symbol_cache();
    }

    pub fn register_vm_native_with_docs<F>(
        &mut self,
        name: &str,
        signature: impl Into<String>,
        docs: impl Into<String>,
        f: F,
    ) where
        F: Fn(Vec<Value>, &mut crate::vm::VM) -> Value + 'static,
    {
        self.vm.register_native_with_vm(name, f);
        self.symbol_metadata.insert(
            name.to_string(),
            SymbolMetadata {
                signature: signature.into(),
                docs: docs.into(),
                keyword_args: Vec::new(),
            },
        );
        self.invalidate_symbol_cache();
    }

    pub fn add_global_store_hook(&mut self, hook: crate::vm::GlobalStoreHook) {
        self.vm.add_global_store_hook(hook);
    }

    pub fn set_inline_widget_metadata_resolver(
        &mut self,
        resolver: crate::vm::InlineWidgetMetadataResolver,
    ) {
        self.vm.set_inline_widget_metadata_resolver(resolver);
    }

    pub fn document_symbol(
        &mut self,
        name: impl Into<String>,
        signature: impl Into<String>,
        docs: impl Into<String>,
    ) {
        self.symbol_metadata.insert(
            name.into(),
            SymbolMetadata {
                signature: signature.into(),
                docs: docs.into(),
                keyword_args: Vec::new(),
            },
        );
        self.invalidate_symbol_cache();
    }

    pub fn document_symbol_with_keywords<I, S>(
        &mut self,
        name: impl Into<String>,
        signature: impl Into<String>,
        docs: impl Into<String>,
        keyword_args: I,
    ) where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.symbol_metadata.insert(
            name.into(),
            SymbolMetadata {
                signature: signature.into(),
                docs: docs.into(),
                keyword_args: normalize_keyword_args(keyword_args),
            },
        );
        self.invalidate_symbol_cache();
    }

    pub fn document_symbols(
        &mut self,
        metadata: impl IntoIterator<Item = (&'static str, &'static str, &'static str)>,
    ) {
        for (name, signature, docs) in metadata {
            self.symbol_metadata.insert(
                name.to_string(),
                SymbolMetadata {
                    signature: signature.to_string(),
                    docs: docs.to_string(),
                    keyword_args: Vec::new(),
                },
            );
        }
        self.invalidate_symbol_cache();
    }

    fn document_builtin_symbols(&mut self) {
        self.document_symbols([
            ("dict", "(dict :key value ...)", "Return a map from keyword/value pairs."),
            (
                "ui/style",
                "(ui/style :state style-map ...)",
                "Return a widget style map, commonly keyed by :pressed, :hover, or :focused.",
            ),
            (
                "box",
                "(box :background value :background-color color :border-color color :border-width px :corner-radius px :padding cells :width cells :height cells :aspect ratio :align mode :h-align mode :v-align mode :flex weight :selected bool :selected-background-color color :selected-border-color color :muted bool :muted-background-color color :muted-border-color color :drop-hover-background-color color :drop-hover-border-color color :drag-type type :drag-payload value :drag-modifier modifier :capture-pointer bool :focusable bool :key value :on-click callback :on-right-click callback :on-double-click callback :on-drag callback :on-drop callback :on-mouse-down callback :on-mouse-up callback child ...)",
                "Create a layout container with optional background, border, interaction, drag/drop, and selection styling.",
            ),
            (
                "context-menu",
                "(context-menu :is-open bool :anchor-col cells :anchor-row cells :on-close callback child ...)",
                "Pointer-anchored action-menu overlay: opens at the anchor, flips/clamps to stay on screen, closes on Escape, outside click, or item selection.",
            ),
            (
                "menu-item",
                "(menu-item \"label\" :shortcut text :disabled bool :on-select callback)",
                "One row of a context-menu: label, optional right-aligned shortcut hint, optional disabled state.",
            ),
            (
                "menu-separator",
                "(menu-separator)",
                "A horizontal divider row inside a context-menu.",
            ),
            (
                "get",
                "(get collection :key)",
                "Return a value from a map or keyword/value list, or nil when missing.",
            ),
            (
                "reactive-get",
                "(reactive-get namespace field)",
                "Read a reactive namespace field and track it as a dependency.",
            ),
            (
                "reactive-set",
                "(reactive-set namespace field value)",
                "Write a field in a writable reactive namespace and rerun dependent effects.",
            ),
            (
                "subtree-owner",
                "(subtree-owner key callback)",
                "Evaluate callback while assigning stable ownership to the generated widget subtree.",
            ),
            ("merge", "(merge map :key value ...)", "Return a copy of map with keyword/value overrides applied."),
            ("keys", "(keys map)", "Return the map keys as keywords."),
            ("first", "(first list)", "Return the first list item, or nil."),
            ("rest", "(rest list)", "Return a list without its first item."),
            ("cons", "(cons value list)", "Return a new list with value prepended."),
            ("len", "(len list-or-string)", "Return the length of a list or string."),
            ("append", "(append list ...)", "Concatenate lists."),
            ("list", "(list value ...)", "Return a list containing the arguments."),
            ("empty?", "(empty? value)", "Return whether a list, string, map, or nil is empty."),
            ("set-nth", "(set-nth list index value)", "Return a copy of list with the 0-based item replaced."),
            ("each", "(each list owner-path callback)", "Map over a list with item index and optional widget ownership metadata."),
            ("map", "(map callback list)", "Return a list containing callback applied to each item."),
            ("filter", "(filter callback list)", "Return list items for which callback is truthy."),
            ("find-by-key", "(find-by-key list :key value)", "Return the first map in list whose :key field equals value, or nil."),
            ("reduce", "(reduce callback initial list)", "Reduce list by calling callback with accumulator and item."),
            ("for-each", "(for-each callback list)", "Call callback for each item and return nil."),
            ("defhook", "(defhook \"name\")", "Declare an extension hook and define a function of that name that runs its listeners."),
            ("add-hook", "(add-hook \"name\" \"entry-key\" callback)", "Register a hook listener; re-adding the same entry-key replaces it in place."),
            ("remove-hook", "(remove-hook \"name\" \"entry-key\")", "Remove the hook listener registered under entry-key."),
            ("run-hook", "(run-hook \"name\" args...)", "Run every listener registered on a hook, in registration order."),
            ("zip", "(zip list ...)", "Return rows formed from the corresponding items of each list."),
            ("nth", "(nth list index)", "Return the 0-based list item, or nil."),
            ("reverse", "(reverse list)", "Return a reversed copy of list."),
            ("chunks", "(chunks list size)", "Split list into sub-lists of up to size items."),
            ("range", "(range end) or (range start end)", "Return integer numbers in the half-open range."),
            ("rand-int", "(rand-int end) or (rand-int start end)", "Return a pseudo-random integer in the half-open range."),
            ("not", "(not value)", "Return true when value is nil, false, or missing."),
            ("str", "(str value ...)", "Concatenate values as display strings."),
            ("substring", "(substring string start [end])", "Return a character-indexed substring."),
            ("str-contains?", "(str-contains? haystack needle)", "Return whether haystack contains needle, case-insensitively."),
            ("gensym", "(gensym base)", "Return a deterministic symbol unique to the current macro expansion site."),
            ("macroexpand", "(macroexpand 'form)", "Return the fully expanded syntax for a quoted form in the authoring VM."),
            ("source", "(source value ...)", "Concatenate values as Lisp source text."),
            ("fmt", "(fmt template value ...)", "Format values into {} placeholders in template."),
            ("abs", "(abs x)", "Return the absolute value."),
            ("sqrt", "(sqrt x)", "Return the square root."),
            ("sin", "(sin x)", "Return sine in radians."),
            ("cos", "(cos x)", "Return cosine in radians."),
            ("floor", "(floor x)", "Round down to an integer value."),
            ("ceil", "(ceil x)", "Round up to an integer value."),
            ("round", "(round x)", "Round to the nearest integer value."),
            ("fract", "(fract x)", "Return the fractional part."),
            ("pow", "(pow base exponent)", "Return base raised to exponent."),
            ("atan2", "(atan2 y x)", "Return the angle of vector x/y in radians."),
            ("mod", "(mod a b)", "Return the floating-point remainder of a divided by b."),
            ("clamp", "(clamp value low high)", "Clamp value to the inclusive numeric range."),
            ("mix", "(mix a b t)", "Linearly interpolate between a and b by t."),
            ("smoothstep", "(smoothstep edge0 edge1 x)", "Return smooth Hermite interpolation between two edges."),
            ("vec2", "(vec2 x y)", "Return a two-number vector list."),
            ("length", "(length vec2)", "Return the length of a two-number vector."),
            ("dot", "(dot a b)", "Return the dot product of two vec2 lists."),
            (
                "load",
                "(load path)",
                "Read and evaluate a Lisp file. Relative paths follow the loading file; @/ paths follow the process working directory.",
            ),
            ("sdf->metal", "(sdf->metal sdf-expr)", "Compile a quoted SDF expression to Metal shader source."),
            ("defwidget", "(defwidget name :width w :height h :animates bool :shader expr ...)", "Register an SDF-backed widget constructor."),
            ("vec3", "(vec3 x y z)", "Return a tagged SDF vec3 expression."),
            ("vec4", "(vec4 x y z w)", "Return a tagged SDF vec4 expression."),
            ("rgba", "(rgba r g b a)", "Return a tagged SDF color expression."),
            ("material", "(material :key value ...)", "Return a tagged SDF material expression."),
            ("lighting", "(lighting :key value ...)", "Return a tagged SDF lighting expression."),
            ("shadow", "(shadow :key value ...)", "Return a tagged SDF shadow expression."),
        ]);

        for &widget in crate::widgets::BUILTIN_WIDGET_NAMES {
            let definition = crate::widget_render::widget_definition(widget).or_else(|| {
                if widget == "select" {
                    // `select` is the lightweight constructor alias for dropdowns.
                    crate::widget_render::widget_definition("dropdown")
                } else {
                    None
                }
            });
            let metadata = self
                .symbol_metadata
                .entry(widget.to_string())
                .or_insert_with(|| SymbolMetadata {
                    signature: format!("({widget} [children-or-props ...])"),
                    docs: format!("Construct a {widget} UI widget map."),
                    keyword_args: Vec::new(),
                });
            if let Some(definition) = definition {
                let props = definition.completion_props();
                if !props.is_empty() {
                    metadata.keyword_args = normalize_keyword_args(props.iter().copied());
                }
            }
        }
        self.invalidate_symbol_cache();
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
        self.vm.register_native_with_vm(name, move |args, vm| {
            // Stamp the executing chunk's module so registration natives
            // (bind-key, define-mode, …) can qualify late-bound handler
            // names against the module current at their call site (spec §5).
            let module = vm.current_module_name();
            shared.borrow_mut().current_native_module =
                (module != crate::modules::IMPLICIT_MODULE).then(|| module.to_string());
            let mut ctx = NativeContext::new(shared.clone());
            match f(args, &mut ctx) {
                Ok(value) => {
                    for field in ctx.reactive_reads {
                        vm.inject_reactive_read(&field.namespace, &field.field);
                    }
                    for (field, generation) in ctx.reactive_invalidations {
                        vm.invalidate_injected_reactive_source(
                            &field.namespace,
                            &field.field,
                            generation,
                        );
                    }
                    value
                }
                Err(error) => {
                    ctx.set_status(format!("Error: {error}"));
                    Value::Bool(false)
                }
            }
        });
        if let (Some(signature), Some(docs)) = (signature, docs) {
            self.symbol_metadata
                .insert(name.to_string(), SymbolMetadata {
                    signature,
                    docs,
                    keyword_args: Vec::new(),
                });
        }
        self.invalidate_symbol_cache();
    }

    pub fn macros(&self) -> &std::collections::HashMap<String, crate::compiler::MacroDef> {
        &self.vm.macros
    }

    pub(crate) fn expand_macros_expression(
        &mut self,
        expr: &crate::parser::Expression,
    ) -> Result<crate::parser::Expression, String> {
        self.vm.expand_macros_expression(expr)
    }

    /// Modules declared via `(module NAME)` → declaring file, if any
    /// (module-system spec §2/§4).
    pub fn declared_modules(
        &self,
    ) -> &std::collections::HashMap<String, Option<std::path::PathBuf>> {
        &self.vm.declared_modules
    }

    fn snapshot_state(&self) -> RuntimeStateSnapshot {
        RuntimeStateSnapshot {
            vm: self.vm.snapshot_state(),
            shared: self.shared.borrow().clone(),
            symbol_metadata: self.symbol_metadata.clone(),
            symbol_revision: self.symbol_revision,
            cached_completion_symbols: self.cached_completion_symbols.clone(),
            cached_completion_metadata: self.cached_completion_metadata.clone(),
            cached_module_completions: self.cached_module_completions.clone(),
            reactive_registry: self.reactive_registry.clone(),
            pending_injected_reactive_invalidations: self
                .pending_injected_reactive_invalidations
                .clone(),
            current_layout: self.current_layout.clone(),
            layout_revision: self.layout_revision,
            dirty_widget_ids: self.dirty_widget_ids.clone(),
            force_layout_revision_bump: self.force_layout_revision_bump,
            deferred_layout_invalidated: self.deferred_layout_invalidated,
            current_widget_tree: self.current_widget_tree.clone(),
            current_committed_ui_snapshot: self.current_committed_ui_snapshot.clone(),
            current_committed_ui_snapshot_generation: self.current_committed_ui_snapshot_generation,
            last_ui_invalidation_trace: self.last_ui_invalidation_trace.clone(),
        }
    }

    fn restore_state(&mut self, snapshot: RuntimeStateSnapshot) {
        self.vm.restore_state(snapshot.vm);
        *self.shared.borrow_mut() = snapshot.shared;
        self.symbol_metadata = snapshot.symbol_metadata;
        self.symbol_revision = snapshot.symbol_revision;
        self.cached_completion_symbols = snapshot.cached_completion_symbols;
        self.cached_completion_metadata = snapshot.cached_completion_metadata;
        self.cached_module_completions = snapshot.cached_module_completions;
        self.reactive_registry = snapshot.reactive_registry;
        self.pending_injected_reactive_invalidations =
            snapshot.pending_injected_reactive_invalidations;
        self.current_layout = snapshot.current_layout;
        self.layout_revision = snapshot.layout_revision;
        self.dirty_widget_ids = snapshot.dirty_widget_ids;
        self.force_layout_revision_bump = snapshot.force_layout_revision_bump;
        self.deferred_layout_invalidated = snapshot.deferred_layout_invalidated;
        self.current_widget_tree = snapshot.current_widget_tree;
        self.current_committed_ui_snapshot = snapshot.current_committed_ui_snapshot;
        self.current_committed_ui_snapshot_generation =
            snapshot.current_committed_ui_snapshot_generation;
        self.last_ui_invalidation_trace = snapshot.last_ui_invalidation_trace;
    }

    /// Set the root used by `@/` load paths. Embedding applications should
    /// point this at their immutable factory content root.
    pub fn set_load_root(&mut self, root: std::path::PathBuf) {
        self.vm.source_manager.set_cwd(root);
    }

    /// Configure ordered module import roots. This is separate from the raw
    /// `(load …)` root so user modules can shadow package and factory modules
    /// without changing relative loads inside any source file.
    /// Drain module/source load errors accumulated by `(import …)` /
    /// `(load …)` evaluated through `eval_str`, which (unlike the
    /// path-based eval entry points) does not consume them itself. Callers
    /// that programmatically import modules use this to fail loudly and to
    /// keep a stale entry from poisoning a later path-based eval.
    pub fn take_source_load_errors(&mut self) -> Vec<String> {
        self.vm.take_source_load_errors()
    }

    pub fn set_module_load_path(&mut self, roots: Vec<std::path::PathBuf>) {
        self.set_scoped_module_load_path(
            roots
                .into_iter()
                .map(|path| crate::hot_reload::ModuleLoadRoot {
                    path,
                    module_prefix: None,
                })
                .collect(),
        );
    }

    pub fn set_scoped_module_load_path(
        &mut self,
        roots: Vec<crate::hot_reload::ModuleLoadRoot>,
    ) {
        self.vm.source_manager.set_scoped_module_load_roots(roots.clone());
        self.module_completion_roots = roots;
        self.invalidate_symbol_cache();
    }

    pub fn exclude_module_alias_scan_root(&mut self, root: std::path::PathBuf) {
        self.vm.source_manager.exclude_module_alias_scan_root(root);
    }

    pub fn eval_str(&mut self, src: &str) -> Result<Option<Value>, crate::vm::VMError> {
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        self.vm.set_current_effect_context(current_buffer_id);
        self.vm.begin_inline_widget_capture();
        if src.contains("(effect") || src.contains("(effect-buffer") {
            self.clear_layout_effects();
        }

        let result = self.vm.eval_str(src);
        if result.is_ok() {
            self.publish_inline_widget_capture(current_buffer_id, false);
            self.flush_vm_reactive_sets();
            if self.sync_theme_to_global {
                self.sync_theme_from_vm();
            }
            self.invalidate_symbol_cache();
            self.flush_widget_trees();
        }
        result
    }

    pub fn eval_source_at_path(
        &mut self,
        path: PathBuf,
        source: &str,
    ) -> Result<Option<Value>, crate::vm::VMError> {
        let source_buffer_id = self.source_buffer_id_for_path(Some(&path));
        self.vm.set_current_effect_context(source_buffer_id);
        self.vm.begin_inline_widget_capture();
        let path = self.vm.source_manager.canonicalize_path(&path);
        let revision = crate::hot_reload::hash_source(source);
        let result = self.vm.eval_module_source(path, source, revision);
        let load_errors = self.vm.take_source_load_errors();
        if !load_errors.is_empty() {
            for error in load_errors {
                self.vm.source_manager.push_diagnostic(error);
            }
            let _ = self.vm.take_inline_widgets();
            return Err(crate::vm::VMError::CompileError);
        }
        if result.is_ok() {
            self.publish_inline_widget_capture(source_buffer_id, true);
            self.flush_vm_reactive_sets();
            if self.sync_theme_to_global {
                self.sync_theme_from_vm();
            }
            self.invalidate_symbol_cache();
            self.flush_widget_trees();
        }
        result
    }

    pub fn eval_source_transactional(
        &mut self,
        path: Option<PathBuf>,
        source: &str,
        overlays: Vec<SourceOverlay>,
    ) -> ReloadReport {
        let snapshot = self.snapshot_state();
        let source_buffer_id = self.source_buffer_id_for_path(path.as_deref());
        self.vm.set_current_effect_context(source_buffer_id);
        self.vm.source_manager.set_overlays(overlays);
        self.vm.source_manager.begin_transaction();
        self.vm.set_preserve_state_on_redefinition(true);
        self.vm.begin_import_pass();
        self.vm.begin_inline_widget_capture();

        let requested_path = path
            .as_ref()
            .map(|path| self.vm.source_manager.canonicalize_path(path));
        let mut evaluated_path = requested_path.clone();
        let mut eval_source = source.to_string();
        let mut eval_revision = crate::hot_reload::hash_source(source);

        if let Some(requested) = requested_path.as_ref() {
            if let Some(root) = self.vm.source_manager.owner_root_for(requested)
                && root != *requested
            {
                match self.vm.source_manager.source_for_path(&root) {
                    Ok(loaded) => {
                        evaluated_path = Some(loaded.path);
                        eval_source = loaded.text;
                        eval_revision = loaded.revision;
                    }
                    Err(error) => {
                        self.restore_state(snapshot);
                        return ReloadReport {
                            success: false,
                            requested_path,
                            evaluated_path: Some(root),
                            diagnostics: vec![format!("Lisp reload failed: {error}")],
                            ..ReloadReport::default()
                        };
                    }
                }
            }
        }

        let eval_result = if let Some(path) = evaluated_path.clone() {
            if eval_source.contains("(effect") || eval_source.contains("(effect-buffer") {
                self.vm.clear_effects_for_module(&path);
            }
            self.vm
                .eval_module_source(path, &eval_source, eval_revision)
        } else {
            self.vm.eval_str(&eval_source)
        };

        let eval_result = eval_result.and_then(|value| {
            let load_errors = self.vm.take_source_load_errors();
            if !load_errors.is_empty() {
                for error in load_errors {
                    self.vm.source_manager.push_diagnostic(error);
                }
                return Err(crate::vm::VMError::CompileError);
            }
            Ok(value)
        });

        if let Err(error) = eval_result {
            let diagnostics = self.vm.source_manager.diagnostics();
            self.restore_state(snapshot);
            return ReloadReport {
                success: false,
                requested_path,
                evaluated_path,
                diagnostics: if diagnostics.is_empty() {
                    vec![format!("Lisp reload failed: {error:?}")]
                } else {
                    diagnostics
                },
                ..ReloadReport::default()
            };
        }

        let changed_symbols = self.vm.source_manager.changed_symbols();
        let mut rerendered_roots = self.vm.mark_effects_depending_on_symbols(&changed_symbols);
        if let Err(error) = self.vm.rerender_dirty_effects() {
            let diagnostics = vec![format!("Lisp render-root reload failed: {error:?}")];
            self.restore_state(snapshot);
            return ReloadReport {
                success: false,
                requested_path,
                evaluated_path,
                diagnostics,
                ..ReloadReport::default()
            };
        }

        if self.sync_theme_to_global {
            self.sync_theme_from_vm();
        }
        self.invalidate_symbol_cache();
        self.flush_widget_trees();
        self.publish_inline_widget_capture(source_buffer_id, true);
        self.vm.set_preserve_state_on_redefinition(false);

        let mut changed_symbols = changed_symbols.into_iter().collect::<Vec<_>>();
        changed_symbols.sort();
        rerendered_roots.sort();
        rerendered_roots.dedup();
        ReloadReport {
            success: true,
            requested_path,
            evaluated_path,
            changed_symbols,
            rerendered_roots,
            diagnostics: self.vm.source_manager.diagnostics(),
        }
    }

    fn publish_inline_widget_capture(
        &mut self,
        source_buffer_id: Option<BufferId>,
        replace_even_empty: bool,
    ) {
        let widgets = self.vm.take_inline_widgets();
        if !replace_even_empty && widgets.is_empty() {
            return;
        }
        if std::env::var("ESEQ_INLINE_TRACE").is_ok_and(|value| value != "0") {
            eprintln!(
                "[inline-widgets] publish source_buffer_id={source_buffer_id:?} count={} replace_even_empty={replace_even_empty}",
                widgets.len()
            );
        }
        let Some(source_buffer_id) = source_buffer_id else {
            return;
        };
        self.shared.borrow_mut().pending_inline_widgets = Some(PendingInlineWidgets {
            source_buffer_id,
            widgets,
        });
    }

    fn source_buffer_id_for_path(
        &self,
        requested_path: Option<&std::path::Path>,
    ) -> Option<BufferId> {
        let shared = self.shared.borrow();
        let buffer_id = shared.current_buffer_id?;
        let Some(requested_path) = requested_path else {
            return Some(buffer_id);
        };
        let current_buffer_path = shared.current_buffer_path.as_deref()?;
        let current_path = self
            .vm
            .source_manager
            .canonicalize_path(current_buffer_path);
        let requested_path = self.vm.source_manager.canonicalize_path(requested_path);
        (current_path == requested_path).then_some(buffer_id)
    }

    pub fn reload_paths_transactional(
        &mut self,
        paths: Vec<PathBuf>,
        overlays: Vec<SourceOverlay>,
    ) -> ReloadReport {
        let snapshot = self.snapshot_state();
        self.vm.set_current_effect_context(None);
        self.vm.begin_import_pass();
        self.vm.begin_inline_widget_capture();
        self.vm.source_manager.set_overlays(overlays);
        self.vm.source_manager.begin_transaction();
        self.vm.set_preserve_state_on_redefinition(true);

        let mut requested_paths = paths
            .into_iter()
            .map(|path| self.vm.source_manager.canonicalize_path(&path))
            .collect::<Vec<_>>();
        requested_paths.sort();
        requested_paths.dedup();

        let mut eval_targets = Vec::new();
        for requested in &requested_paths {
            let target = self
                .vm
                .source_manager
                .owner_root_for(requested)
                .unwrap_or_else(|| requested.clone());
            if !eval_targets.contains(&target) {
                eval_targets.push(target);
            }
        }

        let mut evaluated_path = None;
        for target in eval_targets {
            let loaded = match self.vm.source_manager.source_for_path(&target) {
                Ok(loaded) => loaded,
                Err(error) => {
                    self.restore_state(snapshot);
                    return ReloadReport {
                        success: false,
                        requested_path: requested_paths.first().cloned(),
                        evaluated_path: Some(target),
                        diagnostics: vec![format!("Lisp reload failed: {error}")],
                        ..ReloadReport::default()
                    };
                }
            };
            evaluated_path = Some(loaded.path.clone());
            if loaded.text.contains("(effect") || loaded.text.contains("(effect-buffer") {
                self.vm.clear_effects_for_module(&loaded.path);
            }
            if let Err(error) =
                self.vm
                    .eval_module_source(loaded.path, &loaded.text, loaded.revision)
            {
                let diagnostics = self.vm.source_manager.diagnostics();
                self.restore_state(snapshot);
                return ReloadReport {
                    success: false,
                    requested_path: requested_paths.first().cloned(),
                    evaluated_path,
                    diagnostics: if diagnostics.is_empty() {
                        vec![format!("Lisp reload failed: {error:?}")]
                    } else {
                        diagnostics
                    },
                    ..ReloadReport::default()
                };
            }
        }

        let load_errors = self.vm.take_source_load_errors();
        if !load_errors.is_empty() {
            for error in load_errors {
                self.vm.source_manager.push_diagnostic(error);
            }
            let diagnostics = self.vm.source_manager.diagnostics();
            self.restore_state(snapshot);
            return ReloadReport {
                success: false,
                requested_path: requested_paths.first().cloned(),
                evaluated_path,
                diagnostics,
                ..ReloadReport::default()
            };
        }

        let changed_symbols = self.vm.source_manager.changed_symbols();
        let mut rerendered_roots = self.vm.mark_effects_depending_on_symbols(&changed_symbols);
        if let Err(error) = self.vm.rerender_dirty_effects() {
            self.restore_state(snapshot);
            return ReloadReport {
                success: false,
                requested_path: requested_paths.first().cloned(),
                evaluated_path,
                diagnostics: vec![format!("Lisp render-root reload failed: {error:?}")],
                ..ReloadReport::default()
            };
        }

        if self.sync_theme_to_global {
            self.sync_theme_from_vm();
        }
        self.invalidate_symbol_cache();
        self.flush_widget_trees();
        let _ = self.vm.take_inline_widgets();
        self.vm.set_preserve_state_on_redefinition(false);

        let mut changed_symbols = changed_symbols.into_iter().collect::<Vec<_>>();
        changed_symbols.sort();
        rerendered_roots.sort();
        rerendered_roots.dedup();
        ReloadReport {
            success: true,
            requested_path: requested_paths.first().cloned(),
            evaluated_path,
            changed_symbols,
            rerendered_roots,
            diagnostics: self.vm.source_manager.diagnostics(),
        }
    }

    pub fn lisp_source_paths(&self) -> Vec<PathBuf> {
        self.vm.source_manager.module_graph().known_paths()
    }

    pub fn lisp_source_revision(&self) -> u64 {
        self.vm.source_manager.module_graph().revision()
    }

    pub fn evaluated_source_text(&self, path: &std::path::Path, revision: u64) -> Option<String> {
        self.vm
            .source_manager
            .evaluated_source(path, revision)
            .map(str::to_string)
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
        if self.sync_theme_to_global {
            self.sync_theme_from_vm();
        }
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
        if self.sync_theme_to_global && name == "THEME" {
            self.sync_theme_from_vm();
        }
        self.invalidate_symbol_cache();
    }

    pub fn global_value(&self, name: &str) -> Option<Value> {
        self.vm.global_value(name)
    }

    pub fn has_global(&self, name: &str) -> bool {
        self.vm.has_global(name)
    }

    /// Resolve a stored handler-name string for dispatch (spec §5): a
    /// handler registered from a declared module was stored qualified
    /// (`module/handler`); if that module never defined it, fall back to
    /// the flat base name (the normal ladder — the handler was a vanilla
    /// global or an editor builtin referenced from the module).
    pub fn resolve_handler_name<'a>(&self, handler: &'a str) -> &'a str {
        if let Some((_, base)) = crate::modules::split_qualified(handler)
            && !self.vm.has_global(handler)
        {
            return base;
        }
        handler
    }

    /// Borrows one field of a reactive namespace without cloning the whole
    /// namespace map. `global_value("SEQ")` clones every key/value pair in the
    /// namespace, so its cost grows with total UI state; prefer this whenever
    /// only a single field is needed.
    pub fn reactive_field_value(&self, namespace: &str, field: &str) -> Option<&Value> {
        self.reactive_registry.field_value(namespace, field)
    }

    pub fn register_reactive(&mut self, name: &str, fields: Vec<(&str, Value)>, writable: bool) {
        let map = self.reactive_registry.register(name, fields, writable);
        // Mark the namespace reactive BEFORE the global write so the
        // by-name ladder pins it to the flat slot; then alias any stale
        // `eseq.vanilla/NAME` slot interned by code compiled earlier.
        self.vm.reactive_namespaces.insert(name.to_string());
        self.vm.set_global_value(name, map);
        self.vm.alias_stale_qualified_slot(name);
        if writable {
            self.vm
                .writable_reactive_namespaces
                .insert(name.to_string());
        } else {
            self.vm.writable_reactive_namespaces.remove(name);
        }
        if self.sync_theme_to_global && name == "THEME" {
            self.sync_theme_from_vm();
        }
        self.invalidate_symbol_cache();
    }

    /// Advance a host-owned reactive source previously injected by a native
    /// and immediately process the targeted dirty effects. Injected sources
    /// live directly in the VM DAG rather than the ordinary reactive registry,
    /// so they do not participate in `run_reactive_cycle` batching.
    pub fn invalidate_reactive_source(
        &mut self,
        namespace: &str,
        field: &str,
        generation: Value,
    ) -> Result<(), crate::vm::VMError> {
        self.vm
            .invalidate_injected_reactive_source(namespace, field, generation);
        self.vm.process_dirty_reactive()?;
        self.flush_vm_reactive_sets();
        if self.sync_theme_to_global {
            self.sync_theme_from_vm();
        }
        self.flush_widget_trees();
        Ok(())
    }

    /// Queue fresh generations for every subscribed host-owned field in a
    /// namespace. The invalidations join the next ordinary reactive cycle, so
    /// effects observe all reactive registry writes from that cycle before
    /// they re-render. Repeated calls before the cycle retain only the newest
    /// generation for each field.
    pub fn queue_reactive_namespace_invalidation(
        &mut self,
        namespace: &str,
        mut generation_for_field: impl FnMut(&str) -> Value,
    ) {
        for field in self.vm.subscribed_injected_reactive_fields(namespace) {
            let generation = generation_for_field(&field);
            self.pending_injected_reactive_invalidations.insert(
                ReactiveFieldKey::new(namespace, field),
                generation,
            );
        }
    }

    /// Update one reactive field.
    ///
    /// `#[track_caller]` makes filtered UI traces identify the host write site,
    /// rather than only the field and value. This is especially useful for
    /// shared status/error fields that have many independent producers.
    #[track_caller]
    pub fn set_reactive(
        &mut self,
        namespace: &str,
        field: &str,
        value: Value,
    ) -> ReactiveSetResult {
        // Fast path: unchanged writes skip the subscriber lookup and clones
        // below. Hot sync paths issue thousands of no-op sets per frame.
        if !trace_ui_enabled()
            && self
                .reactive_registry
                .is_unchanged(namespace, field, &value)
        {
            return ReactiveSetResult {
                changed: false,
                effects_dirty: false,
                widgets_dirty: false,
            };
        }
        let trace = trace_ui_field_enabled(namespace, field);
        let previous = if trace {
            self.vm.global_value(namespace).and_then(|namespace_value| {
                let Value::Map(map) = namespace_value else {
                    return None;
                };
                map.get(field).map(|value| value.borrow().clone())
            })
        } else {
            None
        };
        let next_for_trace = trace.then(|| value.clone());
        let enqueue_effect_dirty = self.vm.has_reactive_subscribers(namespace, field);
        let value_for_vm = value.clone();
        let outcome = self
            .reactive_registry
            .set(namespace, field, value, enqueue_effect_dirty);
        if outcome.changed || !outcome.registered {
            self.vm
                .update_reactive_global(namespace, field, value_for_vm);
        }
        let widget_ids = outcome.widget_ids;
        let widgets_dirty = !widget_ids.is_empty();
        if trace {
            let caller = std::panic::Location::caller();
            let preview_widgets = widget_ids
                .iter()
                .take(12)
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let widget_suffix = if widget_ids.len() > 12 { ",..." } else { "" };
            eprintln!(
                "[ui-trace][set-reactive] {namespace}.{field} caller={}:{}:{} prev={} next={} subscribers={} effect_dirty={} widgets_dirty={} widget_ids=[{}{}] widget_count={}",
                caller.file(),
                caller.line(),
                caller.column(),
                summarize_reactive_value(previous.as_ref()),
                summarize_reactive_value(next_for_trace.as_ref()),
                enqueue_effect_dirty,
                outcome.effect_dirty,
                widgets_dirty,
                preview_widgets,
                widget_suffix,
                widget_ids.len(),
            );
        }
        for widget_id in widget_ids {
            if !self.dirty_widget_ids.contains(&widget_id) {
                self.dirty_widget_ids.push(widget_id);
            }
        }
        // Reactive bindings dirtied specific widgets, but they do not mutate
        // widget-local state such as hover, scroll, focus, or animation state.
        // Keep the global widget primitive cache generation stable so high-rate
        // bindings do not invalidate unrelated static widget primitives.
        if self.sync_theme_to_global && namespace == "THEME" {
            self.sync_theme_from_registry();
        }
        ReactiveSetResult {
            changed: outcome.changed || !outcome.registered,
            effects_dirty: outcome.effect_dirty,
            widgets_dirty,
        }
    }

    /// Value-patch variant of set_reactive (docs/fx-value-delta-spec.md).
    /// When the candidate differs from the stored value only at Number/Bool
    /// leaves inside an identically-shaped container tree, the differing
    /// leaves are written into the stored tree's shared cells and NO
    /// subscribers are dirtied — later Lisp evals read the fresh values, and
    /// live display flows through the per-param field bindings synced
    /// alongside. Any shape/string/variant difference (device added, slot
    /// count, rename, track switch) falls back to the full set_reactive
    /// pipeline, so this is safe wherever set_reactive is.
    #[track_caller]
    pub fn set_reactive_value_patch(
        &mut self,
        namespace: &str,
        field: &str,
        value: Value,
    ) -> ReactiveSetResult {
        match self
            .reactive_registry
            .classify_value_patch(namespace, field, &value)
        {
            Some(crate::reactive::ReactiveValueDelta::Equal) => ReactiveSetResult {
                changed: false,
                effects_dirty: false,
                widgets_dirty: false,
            },
            Some(crate::reactive::ReactiveValueDelta::Patchable) => {
                let patched = self
                    .reactive_registry
                    .apply_value_patch(namespace, field, &value);
                static SCENE_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                if *SCENE_TRACE
                    .get_or_init(|| std::env::var("ESEQ_SCENE_TRACE").is_ok_and(|v| v == "1"))
                    || trace_ui_enabled()
                {
                    eprintln!("[reactive-value-patch] {namespace}.{field} leaves={patched}");
                }
                ReactiveSetResult {
                    changed: false,
                    effects_dirty: false,
                    widgets_dirty: false,
                }
            }
            Some(crate::reactive::ReactiveValueDelta::Structural) | None => {
                self.set_reactive(namespace, field, value)
            }
        }
    }

    pub fn set_reactive_list_index(
        &mut self,
        namespace: &str,
        field: &str,
        index: usize,
        value: Value,
    ) -> ReactiveSetResult {
        let enqueue_effect_dirty = self.vm.has_reactive_subscribers(namespace, field);
        let value_for_vm = value.clone();
        let outcome = self.reactive_registry.set_list_index(
            namespace,
            field,
            index,
            value,
            enqueue_effect_dirty,
        );
        if outcome.changed || !outcome.registered {
            self.vm
                .update_reactive_global_list_index(namespace, field, index, value_for_vm);
        }
        let widget_ids = outcome.widget_ids;
        let widgets_dirty = !widget_ids.is_empty();
        for widget_id in widget_ids {
            if !self.dirty_widget_ids.contains(&widget_id) {
                self.dirty_widget_ids.push(widget_id);
            }
        }
        ReactiveSetResult {
            changed: outcome.changed || !outcome.registered,
            effects_dirty: outcome.effect_dirty,
            widgets_dirty,
        }
    }

    pub fn set_layout_viewport(&mut self, cols: u16, rows: u16) {
        self.set_layout_viewport_exact(cols as f32, rows as f32);
    }

    pub fn set_layout_viewport_exact(&mut self, cols: f32, rows: f32) {
        self.set_layout_viewport_exact_inner(cols, rows, true);
    }

    /// `set_layout_viewport_exact` that leaves a pending deferred layout
    /// invalidation pending instead of settling it.
    ///
    /// Input routing sets the layout viewport to the tile it is about to
    /// dispatch into, and it does that once per *raw* event. Settling there
    /// turns a coalesced scroll burst back into one full relayout per event —
    /// precisely what `invalidate_layout_deferred` exists to avoid — while the
    /// layout it would rebuild is thrown away again by the next event in the
    /// same burst. Render paths still settle; they are the ones that need the
    /// new geometry.
    pub fn set_layout_viewport_exact_deferring(&mut self, cols: f32, rows: f32) {
        self.set_layout_viewport_exact_inner(cols, rows, false);
    }

    fn set_layout_viewport_exact_inner(&mut self, cols: f32, rows: f32, settle_deferred: bool) {
        let cols = cols.max(1.0);
        let rows = rows.max(1.0);
        if self.layout_cols.to_bits() == cols.to_bits()
            && self.layout_rows.to_bits() == rows.to_bits()
        {
            if settle_deferred {
                self.flush_deferred_layout_invalidation();
            }
            if self.current_layout.is_none() && self.current_widget_tree.is_some() {
                self.relayout_current_tree_because("viewport-settle");
            }
            return;
        }
        self.layout_cols = cols;
        self.layout_rows = rows;
        // Viewport changes invalidate layout geometry even if the widget tree is unchanged.
        let previous_layout = self.current_layout.clone();
        self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
        self.current_layout = None;
        self.deferred_layout_invalidated = false;
        self.relayout_current_tree_because("viewport-resize");
    }

    /// Set the whole-window viewport in the current tile's local cell
    /// coordinates (see `LayoutEngine::frame_viewport`). Invalidates the
    /// current layout lazily on change — the next viewport/relayout call
    /// rebuilds it — so callers can set this before `set_layout_viewport_exact`
    /// without triggering a double layout.
    pub fn set_layout_frame_viewport(&mut self, frame_viewport: Option<crate::layout::Rect>) {
        if self.layout_frame_viewport == frame_viewport {
            return;
        }
        self.layout_frame_viewport = frame_viewport;
        let previous_layout = self.current_layout.clone();
        self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
        self.current_layout = None;
        self.force_layout_revision_bump = true;
    }

    pub fn layout_frame_viewport(&self) -> Option<crate::layout::Rect> {
        self.layout_frame_viewport
    }

    /// Set the tile's content-scroll offsets `(cols, rows)` for subsequent
    /// layout passes (see `LayoutEngine::content_scroll`).
    ///
    /// Deliberately does NOT invalidate the layout: panning a tile would then
    /// rebuild the whole widget layout every frame. Only frame-anchored
    /// widgets read it, and scroll is trapped while one of those is open, so
    /// the next relayout picks up the current value.
    pub fn set_layout_content_scroll(&mut self, content_scroll: (f32, f32)) {
        self.layout_content_scroll = content_scroll;
    }

    /// Force a full relayout on the next render pass.
    /// Used when internal widget state (e.g. tree expand/collapse) changes
    /// the widget's size without changing the widget tree data.
    pub fn invalidate_layout(&mut self) {
        let previous_layout = self.current_layout.clone();
        self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
        self.current_layout = None;
        self.force_layout_revision_bump = true;
        self.relayout_current_tree_because("invalidate-layout");
    }

    /// Coalesce state-driven relayout work until the next frame build.
    ///
    /// Scroll gestures can arrive in dense bursts. Virtualized layouts need the
    /// latest scroll state to choose their materialized child window, but doing
    /// a full layout on every raw scroll event would make the burst itself
    /// expensive. This keeps the previous layout around for hit testing while
    /// marking one full relayout for the next render pass.
    pub fn invalidate_layout_deferred(&mut self) {
        let previous_layout = self.current_layout.clone();
        self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
        self.force_layout_revision_bump = true;
        self.deferred_layout_invalidated = true;
    }

    pub fn flush_deferred_layout_invalidation(&mut self) {
        if !self.deferred_layout_invalidated {
            return;
        }
        self.deferred_layout_invalidated = false;
        let previous_layout = self.current_layout.clone();
        self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
        self.current_layout = None;
        self.relayout_current_tree_because("deferred-invalidation");
    }

    pub fn layout_aspect(&self) -> f32 {
        self.layout_aspect
    }

    pub fn layout_cell_dims(&self) -> (f32, f32) {
        (self.layout_cell_w, self.layout_cell_h)
    }

    pub fn set_layout_cell_dimensions(&mut self, cell_w: f32, cell_h: f32) {
        if !cell_w.is_finite() || !cell_h.is_finite() || cell_w <= 0.0 || cell_h <= 0.0 {
            return;
        }
        if (self.layout_cell_w - cell_w).abs() < f32::EPSILON
            && (self.layout_cell_h - cell_h).abs() < f32::EPSILON
        {
            return;
        }
        self.layout_cell_w = cell_w;
        self.layout_cell_h = cell_h;
        let previous_layout = self.current_layout.clone();
        self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
        self.current_layout = None;
        self.relayout_current_tree_because("cell-dimensions");
    }

    /// Set the text measurer for proportional font layout (Metal backend).
    /// Also stores cell dimensions for pixel↔cell conversion.
    pub fn set_text_measurer(&mut self, measurer: Box<dyn TextMeasurer>, cell_w: f32, cell_h: f32) {
        let measurer: Rc<dyn TextMeasurer> = Rc::from(measurer);
        // Render-pass code (e.g. text-input caret metrics) needs a measurer to
        // recover from measure-pass cache misses under subtree layout reuse.
        crate::widget_render::set_render_text_measurer(Rc::clone(&measurer));
        self.text_measurer = Some(measurer);
        self.layout_cell_w = cell_w;
        self.layout_cell_h = cell_h;
        // Force relayout with the new measurer.
        let previous_layout = self.current_layout.clone();
        self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
        self.current_layout = None;
        self.relayout_current_tree_because("text-measurer");
    }

    pub fn set_widget_id_offset(&mut self, offset: u64) {
        if self.widget_id_offset == offset {
            return;
        }
        let previous_layout = self.current_layout.clone();
        self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
        self.widget_id_offset = offset;
        self.current_layout = None;
        self.relayout_current_tree_because("widget-id-offset");
    }

    pub fn set_layout_aspect(&mut self, aspect: f32) {
        if (self.layout_aspect - aspect).abs() < f32::EPSILON {
            return;
        }
        let previous_layout = self.current_layout.clone();
        self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
        self.layout_aspect = aspect;
        self.current_layout = None;
        self.relayout_current_tree_because("layout-aspect");
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
            self.flush_vm_reactive_sets();
            if self.sync_theme_to_global {
                self.sync_theme_from_vm();
            }
            self.flush_widget_trees();
        }
        result
    }

    pub fn invoke_global(
        &mut self,
        name: &str,
        args: Vec<Value>,
    ) -> Result<Option<Value>, crate::vm::VMError> {
        let callable = self
            .vm
            .global_value(name)
            .ok_or_else(|| crate::vm::VMError::UnknownVariable(name.to_string()))?;
        self.invoke(callable, args)
    }

    /// Install the editor's authoritative set of named buffers that hold a
    /// committed widget tree but are not presented by any tile. Effects
    /// targeting them stay dirty in the VM; `run_reactive_cycle` resumes them
    /// as soon as their target leaves this set.
    pub fn set_hidden_effect_buffer_names(&mut self, names: HashSet<String>) {
        self.vm.set_hidden_effect_buffer_names(names);
    }

    /// True when an effect deferred while its target buffer was hidden is now
    /// visible and waiting for a reactive cycle to resume it.
    pub fn has_resumable_hidden_effect_work(&self) -> bool {
        self.vm.has_visible_deferred_effects()
    }

    pub fn run_reactive_cycle(&mut self) {
        let total_started = Instant::now();
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        self.vm.set_current_effect_context(current_buffer_id);
        let dirty = self.reactive_registry.drain_dirty();
        let injected = std::mem::take(&mut self.pending_injected_reactive_invalidations);
        // Deferred effects for hidden buffers stay in the DAG's dirty set
        // indefinitely, so `process_dirty_reactive` would pay a full
        // `topo_sort_dirty` every idle cycle to produce no work. Skip that
        // unless a deferred effect's target has become visible, which is the
        // only other reason an empty-dirty cycle has anything to do.
        if dirty.is_empty() && injected.is_empty() && !self.vm.has_visible_deferred_effects() {
            if trace_ui_enabled() {
                eprintln!("[ui-trace][reactive-cycle] dirty=[] no-op");
            }
            return;
        }

        let dirty_len = dirty.len() + injected.len();
        let mut dirty_fields = dirty
            .iter()
            .map(|(namespace, field, _)| format!("{namespace}.{field}"))
            .collect::<Vec<_>>();
        dirty_fields.extend(
            injected
                .keys()
                .map(|field| format!("{}.{}", field.namespace, field.field)),
        );
        for (field, generation) in injected {
            self.vm.invalidate_injected_reactive_source(
                &field.namespace,
                &field.field,
                generation,
            );
        }
        let apply_started = Instant::now();
        let apply_result = if dirty.is_empty() {
            self.vm.process_visible_dirty_effects()
        } else {
            self.vm.apply_reactive_changes(dirty)
        };
        match apply_result {
            Ok(()) => {
                let apply_elapsed = apply_started.elapsed();
                let exec_timings = self.vm.take_reactive_exec_timings();
                let function_profiles = self.vm.take_reactive_function_profiles();
                if dirty_len == 0 && exec_timings.is_empty() {
                    if trace_ui_enabled() {
                        eprintln!("[ui-trace][reactive-cycle] dirty=[] no-op");
                    }
                    return;
                }
                self.flush_vm_reactive_sets();
                if self.sync_theme_to_global {
                    self.sync_theme_from_vm();
                }
                self.last_ui_invalidation_trace = Some(UiInvalidationTrace {
                    dirty_fields,
                    reactive_apply_duration: apply_elapsed,
                    reactive_exec_timings: exec_timings
                        .iter()
                        .map(|timing| (timing.profile_label(), timing.elapsed))
                        .collect(),
                    reactive_function_profiles: function_profiles,
                    ..UiInvalidationTrace::default()
                });
                let flush_started = Instant::now();
                let flush_stats = self.flush_widget_trees();
                let flush_elapsed = flush_started.elapsed();
                if let Some(trace) = self.last_ui_invalidation_trace.as_mut() {
                    trace.affected_buffers = flush_stats.affected_buffers.clone();
                    trace.active_buffer_targets = flush_stats.active_buffer_targets;
                    trace.inactive_buffer_targets = flush_stats.inactive_buffer_targets;
                    trace.widget_tree_flushes = flush_stats.widget_tree_flushes;
                    trace.pending_widget_tree_count = flush_stats.pending_widget_tree_count;
                    trace.full_buffer_reruns = flush_stats.full_buffer_reruns;
                    trace.subtree_reruns = flush_stats.subtree_reruns;
                    trace.reevaluated_subtree_roots = flush_stats.reevaluated_subtree_roots;
                    trace.pending_subtree_patch_count = flush_stats.pending_subtree_patch_count;
                    trace.reactive_flush_duration = flush_elapsed;
                    trace.reactive_cycle_duration = total_started.elapsed();
                }
                if std::env::var_os("ESEQLISP_TRACE_UI").is_some()
                    && let Some(trace) = self.last_ui_invalidation_trace.as_ref()
                {
                    eprintln!("{}", format_ui_invalidation_trace(trace, &exec_timings));
                }
                self.perf_stats.note_reactive_cycle(
                    dirty_len,
                    apply_elapsed,
                    flush_elapsed,
                    total_started.elapsed(),
                    exec_timings,
                    &flush_stats,
                );
            }
            Err(error) => {
                if std::env::var_os("ESEQLISP_TRACE_UI").is_some() {
                    let context = self
                        .vm
                        .take_last_reactive_error_context()
                        .unwrap_or_else(|| "<unknown>".to_string());
                    let detail = self
                        .vm
                        .take_last_reactive_error_detail()
                        .unwrap_or_else(|| "-".to_string());
                    let dirty_label = dirty_fields.join(",");
                    eprintln!(
                        "[ui-trace] reactive-cycle-error dirty=[{dirty_label}] context={context} error={error:?} detail={detail}"
                    );
                }
            }
        }
    }

    fn flush_vm_reactive_sets(&mut self) {
        for (namespace, field, value) in self.vm.take_pending_reactive_sets() {
            let outcome = self.reactive_registry.set(&namespace, &field, value, false);
            for widget_id in outcome.widget_ids {
                if !self.dirty_widget_ids.contains(&widget_id) {
                    self.dirty_widget_ids.push(widget_id);
                }
            }
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

        // Completions show bare names: `eseq.vanilla/foo` inserts and
        // resolves as `foo` (module-system spec slice 0).
        let mut symbols = self
            .vm
            .global_names()
            .iter()
            .map(|name| crate::modules::strip_implicit(name).to_string())
            .collect::<Vec<_>>();
        symbols.extend(
            self.symbol_metadata
                .keys()
                .map(|name| crate::modules::strip_implicit(name).to_string()),
        );
        for global in self.vm.global_names() {
            if let Some(Value::Map(map)) = self.vm.global_value(global) {
                let display = crate::modules::strip_implicit(global);
                let mut keys = map.keys().cloned().collect::<Vec<_>>();
                keys.sort();
                symbols.extend(keys.into_iter().map(|key| format!("{display}.{key}")));
            }
        }
        symbols.sort();
        symbols.dedup();
        self.cached_completion_symbols = Some(symbols.clone());
        symbols
    }

    pub fn module_completions(&mut self) -> Vec<String> {
        if let Some(modules) = &self.cached_module_completions {
            return modules.clone();
        }

        let modules = discover_module_completions(&self.module_completion_roots);
        self.cached_module_completions = Some(modules.clone());
        modules
    }

    pub fn completion_metadata(&mut self) -> HashMap<String, SymbolMetadata> {
        if let Some(metadata) = &self.cached_completion_metadata {
            return metadata.clone();
        }

        let mut metadata = self.symbol_metadata.clone();
        for global in self.vm.global_names() {
            if let Some(Value::Map(map)) = self.vm.global_value(global) {
                let display = crate::modules::strip_implicit(global);
                let mut keys = map.keys().cloned().collect::<Vec<_>>();
                keys.sort();
                for key in keys {
                    let label = format!("{display}.{key}");
                    metadata.entry(label).or_insert_with(|| SymbolMetadata {
                        signature: format!("{display}.{key}"),
                        docs: format!("Field '{key}' on runtime map '{display}'."),
                        keyword_args: Vec::new(),
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

    pub fn enqueue_host_command(&mut self, command: HostCommand) {
        self.shared.borrow_mut().queued_commands.push(command);
    }

    pub fn layout_revision(&self) -> u64 {
        self.layout_revision
    }

    pub fn take_dirty_widget_ids(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.dirty_widget_ids)
    }

    fn replace_dirty_widget_ids_for_layout(
        &mut self,
        previous_layout: Option<&LayoutNode>,
        replacement: impl IntoIterator<Item = u64>,
    ) {
        if let Some(previous_layout) = previous_layout {
            self.dirty_widget_ids
                .retain(|widget_id| !layout_contains_widget_id(previous_layout, *widget_id));
        }
        for widget_id in replacement {
            if !self.dirty_widget_ids.contains(&widget_id) {
                self.dirty_widget_ids.push(widget_id);
            }
        }
    }

    pub fn has_dirty_widget_ids(&self) -> bool {
        !self.dirty_widget_ids.is_empty()
    }

    pub fn replace_widget_bindings_from_layouts<'a>(
        &mut self,
        layouts: impl IntoIterator<Item = &'a LayoutNode>,
    ) {
        self.reactive_registry
            .replace_widget_bindings_from_layouts(layouts);
    }

    pub fn replace_widget_bindings_from_entry_lists<'a>(
        &mut self,
        entry_lists: impl IntoIterator<Item = &'a [(crate::vm::ReactiveBindingKey, u64)]>,
    ) {
        self.reactive_registry
            .replace_widget_bindings_from_entry_lists(entry_lists);
    }

    pub fn update_widget_bindings_with_tile_delta<'a>(
        &mut self,
        removed: impl IntoIterator<Item = &'a [(crate::vm::ReactiveBindingKey, u64)]>,
        added: impl IntoIterator<Item = &'a [(crate::vm::ReactiveBindingKey, u64)]>,
    ) {
        self.reactive_registry
            .update_widget_bindings_with_tile_delta(removed, added);
    }

    pub fn widget_bindings_revision(&self) -> u64 {
        self.reactive_registry.widget_bindings_revision()
    }

    pub fn last_ui_invalidation_trace(&self) -> Option<UiInvalidationTrace> {
        self.last_ui_invalidation_trace.clone()
    }

    pub fn ui_work_counters(&self) -> UiWorkCounters {
        self.ui_work_counters
    }

    pub(crate) fn drain_host_commands(&mut self) -> Vec<HostCommand> {
        let mut shared = self.shared.borrow_mut();
        std::mem::take(&mut shared.queued_commands)
    }

    pub(crate) fn lisp_bindings(&self) -> HashMap<String, String> {
        self.shared.borrow().lisp_bindings.clone()
    }

    pub(crate) fn take_pending_eval_buffer(&mut self) -> Option<BufferId> {
        let mut shared = self.shared.borrow_mut();
        shared.pending_eval_buffer.take()
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

    pub(crate) fn take_pending_mode_defs(&mut self) -> Vec<PendingModeDefinition> {
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

    pub fn take_pending_buffer_widget_trees(&mut self) -> Vec<PendingUiUpdate> {
        std::mem::take(&mut self.shared.borrow_mut().pending_buffer_widget_trees)
    }

    pub(crate) fn take_pending_inline_widgets(&mut self) -> Option<PendingInlineWidgets> {
        self.shared.borrow_mut().pending_inline_widgets.take()
    }

    pub fn clear_subtree_effects_for_named_target(&mut self, target_name: &str) {
        self.vm.clear_subtree_effects_for_named_target(target_name);
        self.shared
            .borrow_mut()
            .pending_buffer_widget_trees
            .retain(|pending| match pending {
                PendingUiUpdate::ReplaceSubtree { target, .. } => {
                    target != &EffectTarget::BufferName(target_name.to_string())
                }
                PendingUiUpdate::FullTree(_) => true,
            });
    }

    pub(crate) fn take_pending_open_file(&mut self) -> Option<String> {
        self.shared.borrow_mut().pending_open_file.take()
    }

    pub(crate) fn take_pending_create_buffer(&mut self) -> Option<String> {
        self.shared.borrow_mut().pending_create_buffer.take()
    }

    pub(crate) fn take_pending_cleared_effect_sources(&mut self) -> Vec<ClearedEffectSource> {
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

    pub(crate) fn take_pending_set_text_for(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.shared.borrow_mut().pending_set_text_for)
    }

    pub(crate) fn take_pending_append_text_for(&mut self) -> Vec<(String, String, String)> {
        std::mem::take(&mut self.shared.borrow_mut().pending_append_text_for)
    }

    pub(crate) fn take_pending_append_lines_for(&mut self) -> Vec<(String, Vec<String>)> {
        std::mem::take(&mut self.shared.borrow_mut().pending_append_lines_for)
    }

    pub(crate) fn take_pending_remove_lines_for(&mut self) -> Vec<(String, Vec<String>)> {
        std::mem::take(&mut self.shared.borrow_mut().pending_remove_lines_for)
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

    pub(crate) fn take_pending_set_text_zoom(&mut self) -> Option<f64> {
        self.shared.borrow_mut().pending_set_text_zoom.take()
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

    #[cfg(test)]
    pub fn debug_effect_count_for_module(&self, module: &std::path::Path) -> usize {
        self.vm.effect_count_for_module(module)
    }

    pub fn current_widget_tree(&self) -> Option<Value> {
        self.current_widget_tree.clone()
    }

    pub fn current_committed_ui_snapshot(&self) -> Option<CommittedBufferUiSnapshot> {
        self.current_committed_ui_snapshot.clone()
    }

    pub fn current_committed_ui_snapshot_generation(&self) -> Option<u64> {
        self.current_committed_ui_snapshot
            .as_ref()
            .map(|_| self.current_committed_ui_snapshot_generation)
    }

    fn commit_current_ui_snapshot(&mut self, snapshot: Option<CommittedBufferUiSnapshot>) {
        self.current_committed_ui_snapshot = snapshot;
        self.current_committed_ui_snapshot_generation = self
            .current_committed_ui_snapshot_generation
            .wrapping_add(1);
    }

    pub fn current_subtree_roots_for_field(&self, namespace: &str, field: &str) -> Vec<u64> {
        let Some(snapshot) = self.current_committed_ui_snapshot.as_ref() else {
            return Vec::new();
        };
        snapshot.subtree_roots_for_field(&ReactiveFieldKey::new(namespace, field))
    }

    pub fn layout_snapshot_for_tree(&mut self, tree: &Value) -> Option<Arc<LayoutNode>> {
        self.layout_snapshot_for_tree_with_viewport(tree, None)
    }

    pub fn layout_snapshot_for_tree_with_viewport(
        &mut self,
        tree: &Value,
        viewport: Option<(f32, f32)>,
    ) -> Option<Arc<LayoutNode>> {
        self.layout_snapshot_for_tree_with_viewport_and_offset(tree, viewport, 0)
    }

    pub fn layout_snapshot_for_tree_with_viewport_and_offset(
        &mut self,
        tree: &Value,
        viewport: Option<(f32, f32)>,
        widget_id_offset: u64,
    ) -> Option<Arc<LayoutNode>> {
        self.layout_snapshot_for_tree_with_geometry_and_offset(
            tree,
            viewport,
            self.layout_frame_viewport,
            widget_id_offset,
        )
    }

    pub fn layout_snapshot_for_tree_with_geometry_and_offset(
        &mut self,
        tree: &Value,
        viewport: Option<(f32, f32)>,
        frame_viewport: Option<crate::layout::Rect>,
        widget_id_offset: u64,
    ) -> Option<Arc<LayoutNode>> {
        let saved_tree = self.current_widget_tree.clone();
        let saved_committed_snapshot = self.current_committed_ui_snapshot.clone();
        let saved_committed_snapshot_generation = self.current_committed_ui_snapshot_generation;
        let saved_layout = self.current_layout.clone();
        let saved_revision = self.layout_revision;
        let saved_dirty = self.dirty_widget_ids.clone();
        let saved_force_layout_revision_bump = self.force_layout_revision_bump;
        let saved_widget_bindings = self.reactive_registry.widget_bindings_snapshot();
        #[cfg(test)]
        let saved_rendered_layouts = self.rendered_layouts.clone();
        let saved_cols = self.layout_cols;
        let saved_rows = self.layout_rows;
        let saved_frame_viewport = self.layout_frame_viewport;
        let saved_widget_id_offset = self.widget_id_offset;

        if let Some((cols, rows)) = viewport {
            self.layout_cols = cols;
            self.layout_rows = rows;
        }
        self.layout_frame_viewport = frame_viewport;
        self.widget_id_offset = widget_id_offset;

        // Snapshotting an arbitrary buffer/tree should not try to reuse against
        // the currently active layout; that mixes unrelated trees and inflates
        // both relayout work and profiling noise.
        freeze_widget_tree(tree);
        self.current_layout = None;
        self.current_widget_tree = Some(probed_shallow_clone("w2:snapshot-layout-store", &tree));
        self.commit_current_ui_snapshot(Some(CommittedBufferUiSnapshot::from_tree(
            probed_shallow_clone("w2:snapshot-layout-commit", &tree),
            None,
            Vec::new(),
        )));
        self.relayout_current_tree_because("layout-snapshot");
        let snapshot = self.current_layout.clone();

        self.current_widget_tree = saved_tree;
        self.current_committed_ui_snapshot = saved_committed_snapshot;
        self.current_committed_ui_snapshot_generation = saved_committed_snapshot_generation;
        self.current_layout = saved_layout;
        self.layout_revision = saved_revision;
        self.dirty_widget_ids = saved_dirty;
        self.force_layout_revision_bump = saved_force_layout_revision_bump;
        self.reactive_registry
            .restore_widget_bindings(saved_widget_bindings);
        #[cfg(test)]
        {
            self.rendered_layouts = saved_rendered_layouts;
        }
        self.layout_cols = saved_cols;
        self.layout_rows = saved_rows;
        self.layout_frame_viewport = saved_frame_viewport;
        self.widget_id_offset = saved_widget_id_offset;
        snapshot
    }

    pub(crate) fn relayout_subtree_for_tree_with_viewport(
        &self,
        existing: &LayoutNode,
        tree: &Value,
        child_path: &[usize],
        viewport: Option<(f32, f32)>,
        frame_viewport: Option<crate::layout::Rect>,
        dirty_widget_ids: &mut Vec<u64>,
    ) -> Result<LayoutNode, String> {
        let (cols, rows) = viewport.unwrap_or((self.layout_cols, self.layout_rows));
        let mut engine = if let Some(measurer) = self.text_measurer.as_deref() {
            LayoutEngine::with_text_measurer_exact(
                cols,
                rows,
                self.layout_aspect,
                measurer,
                self.layout_cell_w,
                self.layout_cell_h,
            )
        } else {
            LayoutEngine::new_exact(cols, rows, self.layout_aspect)
        };
        engine.frame_viewport = frame_viewport;
        engine.content_scroll = self.layout_content_scroll;
        relayout_subtree_path_result(existing, tree, child_path, dirty_widget_ids, &engine)
    }

    /// Reconcile a cached layout against a changed widget tree, reusing every
    /// unchanged descendant and rebuilding changed subtrees in place when the
    /// change occupies exactly the space its predecessor did (see
    /// `layout::reconcile_layout_node`). Errs when a changed subtree needs
    /// different space, in which case the caller falls back to a full
    /// relayout.
    pub(crate) fn reconcile_layout_for_tree_with_viewport(
        &self,
        existing: &LayoutNode,
        tree: &Value,
        viewport: Option<(f32, f32)>,
        frame_viewport: Option<crate::layout::Rect>,
        dirty_widget_ids: &mut Vec<u64>,
    ) -> Result<(LayoutNode, usize), String> {
        let (cols, rows) = viewport.unwrap_or((self.layout_cols, self.layout_rows));
        let mut engine = if let Some(measurer) = self.text_measurer.as_deref() {
            LayoutEngine::with_text_measurer_exact(
                cols,
                rows,
                self.layout_aspect,
                measurer,
                self.layout_cell_w,
                self.layout_cell_h,
            )
        } else {
            LayoutEngine::new_exact(cols, rows, self.layout_aspect)
        };
        engine.frame_viewport = frame_viewport;
        engine.content_scroll = self.layout_content_scroll;
        crate::layout::reconcile_layout_node(existing, tree, &engine, dirty_widget_ids)
    }

    /// Clear the current widget tree and layout without destroying reactive effects.
    /// Used when switching to a buffer/tile that has no widget tree.
    pub fn clear_current_widget_tree(&mut self) {
        let previous_layout = self.current_layout.clone();
        self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
        self.current_widget_tree = None;
        self.commit_current_ui_snapshot(None);
        self.current_layout = None;
        self.reactive_registry
            .replace_widget_bindings_from_layout(None);
        self.layout_revision = self.layout_revision.wrapping_add(1);
    }

    pub fn layout_rows(&self) -> u16 {
        self.layout_rows.floor().max(1.0) as u16
    }

    pub fn layout_cols(&self) -> u16 {
        self.layout_cols.floor().max(1.0) as u16
    }

    pub fn layout_rows_exact(&self) -> f32 {
        self.layout_rows
    }

    pub fn layout_cols_exact(&self) -> f32 {
        self.layout_cols
    }

    pub fn set_widget_tree(&mut self, tree: Value) {
        // Replace the visual widget tree without destroying reactive effects.
        // Effects from other buffers must survive buffer switches.
        freeze_widget_tree(&tree);
        let previous_layout = self.current_layout.clone();
        self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
        self.current_layout = None;
        self.layout_revision = self.layout_revision.wrapping_add(1);
        self.current_widget_tree = Some(probed_shallow_clone("w2:set-widget-tree-store", &tree));
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        self.commit_current_ui_snapshot(Some(CommittedBufferUiSnapshot::from_tree(
            probed_shallow_clone("w2:set-widget-tree-commit", &tree),
            current_buffer_id,
            Vec::new(),
        )));
        self.relayout_current_tree_because("set-widget-tree");
    }

    pub(crate) fn position_current_layout(
        &mut self,
        positioner: impl FnOnce(&mut LayoutNode) -> bool,
    ) {
        let Some(current) = self.current_layout.as_ref() else {
            return;
        };
        let mut positioned = (**current).clone();
        if positioner(&mut positioned) {
            self.current_layout = Some(Arc::new(positioned));
            self.layout_revision = self.layout_revision.wrapping_add(1);
        }
    }

    /// Restore a previously saved widget tree for display only,
    /// without clearing reactive effects.
    pub fn restore_widget_tree(&mut self, tree: Value) {
        freeze_widget_tree(&tree);
        self.current_widget_tree =
            Some(probed_shallow_clone("w2:restore-widget-tree-store", &tree));
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        self.commit_current_ui_snapshot(Some(CommittedBufferUiSnapshot::from_tree(
            probed_shallow_clone("w2:restore-widget-tree-commit", &tree),
            current_buffer_id,
            Vec::new(),
        )));
        self.relayout_current_tree_because("restore-widget-tree");
        // Force layout revision bump so GPU caches rebuild
        self.layout_revision = self.layout_revision.wrapping_add(1);
    }

    /// Restore a previously saved widget tree and, when available, its cached
    /// layout for the viewport that is about to become active.
    pub fn restore_widget_tree_with_cached_layout(
        &mut self,
        tree: Value,
        snapshot: Option<CommittedBufferUiSnapshot>,
        cached_layout: Option<Arc<LayoutNode>>,
        viewport: Option<(f32, f32)>,
        widget_id_offset: u64,
        layout_revision: u64,
    ) {
        let viewport = viewport.map(|(cols, rows)| (cols.max(1.0), rows.max(1.0)));
        if let Some((cols, rows)) = viewport {
            self.layout_cols = cols;
            self.layout_rows = rows;
        }
        self.widget_id_offset = widget_id_offset;
        freeze_widget_tree(&tree);
        self.current_widget_tree = Some(tree.clone());
        let snapshot = snapshot.or_else(|| {
            Some(CommittedBufferUiSnapshot::from_tree(
                tree,
                self.shared.borrow().current_buffer_id,
                Vec::new(),
            ))
        });
        self.commit_current_ui_snapshot(snapshot);
        let cached_layout = cached_layout.filter(|layout| {
            viewport.is_none_or(|(cols, rows)| {
                crate::layout::layout_root_matches_viewport(layout.as_ref(), cols, rows)
            })
        });
        if let Some(layout) = cached_layout {
            let previous_layout = self.current_layout.clone();
            self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
            self.current_layout = Some(layout);
            self.reactive_registry
                .replace_widget_bindings_from_layout(self.current_layout.as_deref());
            self.layout_revision = layout_revision.wrapping_add(1);
        } else {
            self.current_layout = None;
            self.relayout_current_tree_because("restore-cached-layout-miss");
            self.layout_revision = self.layout_revision.wrapping_add(1);
        }
    }

    pub fn adopt_current_widget_tree_snapshot(
        &mut self,
        tree: Value,
        snapshot: Option<CommittedBufferUiSnapshot>,
        widget_id_offset: u64,
    ) {
        self.widget_id_offset = widget_id_offset;
        freeze_widget_tree(&tree);
        self.current_widget_tree = Some(tree.clone());
        let snapshot = snapshot.or_else(|| {
            Some(CommittedBufferUiSnapshot::from_tree(
                tree,
                self.shared.borrow().current_buffer_id,
                Vec::new(),
            ))
        });
        self.commit_current_ui_snapshot(snapshot);
        self.relayout_current_tree_because("adopt-snapshot");
        self.layout_revision = self.layout_revision.wrapping_add(1);
    }

    fn replace_current_subtree_without_relayout(
        &mut self,
        subtree_root_id: u64,
        tree: Value,
        reactive_dependencies: Vec<ReactiveFieldKey>,
    ) -> bool {
        let Some(snapshot) = self.current_committed_ui_snapshot.take() else {
            if let Some(trace) = self.last_ui_invalidation_trace.as_mut() {
                trace.subtree_failure_reason = Some("missing-snapshot".to_string());
            }
            return false;
        };
        if let Some(reason) = snapshot.subtree_replace_failure_reason(subtree_root_id, &tree) {
            if let Some(trace) = self.last_ui_invalidation_trace.as_mut() {
                trace.subtree_failure_reason = Some(reason.to_string());
            }
            self.current_committed_ui_snapshot = Some(snapshot);
            return false;
        }
        let Some(merged) = snapshot.replacing_subtree(subtree_root_id, tree, reactive_dependencies)
        else {
            if let Some(trace) = self.last_ui_invalidation_trace.as_mut() {
                trace.subtree_failure_reason = Some("replace-missed".to_string());
            }
            return false;
        };
        self.current_widget_tree = Some(merged.tree.clone());
        self.commit_current_ui_snapshot(Some(merged));
        if let Some(trace) = self.last_ui_invalidation_trace.as_mut() {
            trace.subtree_failure_reason = None;
        }
        true
    }

    fn replace_current_subtrees_without_relayout(
        &mut self,
        replacements: &[(u64, Value, Vec<ReactiveFieldKey>)],
    ) -> bool {
        if replacements.is_empty() {
            return false;
        }
        let Some(snapshot) = self.current_committed_ui_snapshot.take() else {
            if let Some(trace) = self.last_ui_invalidation_trace.as_mut() {
                trace.subtree_failure_reason = Some("missing-snapshot".to_string());
            }
            return false;
        };
        let Some(merged) = snapshot.clone().replacing_subtrees(replacements) else {
            if let Some(trace) = self.last_ui_invalidation_trace.as_mut() {
                trace.subtree_failure_reason = Some("replace-batch-missed".to_string());
            }
            self.current_committed_ui_snapshot = Some(snapshot);
            return false;
        };
        self.current_widget_tree = Some(merged.tree.clone());
        self.commit_current_ui_snapshot(Some(merged));
        if let Some(trace) = self.last_ui_invalidation_trace.as_mut() {
            trace.subtree_failure_reason = None;
        }
        true
    }

    fn relayout_current_layout_for_subtrees(
        &mut self,
        subtree_roots: &[u64],
    ) -> Result<(), String> {
        let relayout_started = Instant::now();
        if subtree_roots.is_empty() {
            return Ok(());
        }
        let tree = self
            .current_widget_tree
            .as_ref()
            .ok_or_else(|| "missing-tree".to_string())?;
        let mut layout = self
            .current_layout
            .clone()
            .ok_or_else(|| "missing-layout".to_string())?;
        let mut roots = subtree_roots.to_vec();
        roots.sort_unstable();
        roots.dedup();
        let mut dirty_widget_ids = Vec::new();
        let mut geometry_changed = false;
        let mut used_partial_relayout = false;
        for subtree_root_id in roots {
            let previous_layout = layout.clone();
            let paths = subtree_root_paths(layout.as_ref());
            let Some(child_path) = paths.get(&subtree_root_id) else {
                return Err(format!("missing-subtree-path:{subtree_root_id}"));
            };
            let child_layout = layout_node_at_path(layout.as_ref(), child_path)
                .ok_or_else(|| format!("missing-layout-path:{subtree_root_id}"))?;
            let updated = match reuse_layout_node_for_subtree_path_result(
                layout.as_ref(),
                tree,
                child_path,
                &mut dirty_widget_ids,
            ) {
                Ok(updated) => updated,
                Err(reuse_reason) => {
                    used_partial_relayout = true;
                    self.relayout_subtree_for_tree_with_viewport(
                        layout.as_ref(),
                        tree,
                        child_path,
                        None,
                        self.layout_frame_viewport,
                        &mut dirty_widget_ids,
                    )
                    .map_err(|relayout_reason| {
                        format!(
                            "subtree:{subtree_root_id}:{reuse_reason}; partial-relayout:{relayout_reason}"
                        )
                    })?
                }
            };
            let updated_child = layout_node_at_path(&updated, child_path)
                .ok_or_else(|| format!("missing-updated-layout-path:{subtree_root_id}"))?;
            self.reactive_registry
                .replace_widget_bindings_for_layout_subtree(child_layout, updated_child);
            if !same_layout_geometry(previous_layout.as_ref(), &updated) {
                geometry_changed = true;
            }
            layout = Arc::new(updated);
        }
        let mut combined_dirty_widget_ids = std::mem::take(&mut self.dirty_widget_ids);
        combined_dirty_widget_ids.extend(dirty_widget_ids);
        combined_dirty_widget_ids.sort_unstable();
        combined_dirty_widget_ids.dedup();
        self.current_layout = Some(layout);
        self.dirty_widget_ids = combined_dirty_widget_ids;
        if geometry_changed || self.force_layout_revision_bump {
            self.layout_revision = self.layout_revision.wrapping_add(1);
        }
        self.force_layout_revision_bump = false;
        self.update_last_trace_relayout(
            if used_partial_relayout {
                "subtree-relayout"
            } else {
                "subtree-reuse"
            },
            None,
            relayout_started.elapsed(),
        );
        self.ui_work_counters.relayout_reused += 1;
        self.ui_work_counters.relayout_subtree += 1;
        self.perf_stats
            .note_relayout(true, true, relayout_started.elapsed(), None);
        Ok(())
    }

    pub(crate) fn try_upgrade_full_tree_to_current_subtree(
        &mut self,
        pending: &PendingWidgetTree,
    ) -> bool {
        self.try_upgrade_full_tree_to_current_subtree_without_relayout(pending)
            .inspect(|upgraded| {
                if *upgraded {
                    self.relayout_current_tree_because("subtree-upgrade");
                    self.layout_revision = self.layout_revision.wrapping_add(1);
                }
            })
            .unwrap_or(false)
    }

    fn try_upgrade_full_tree_to_current_subtree_without_relayout(
        &mut self,
        pending: &PendingWidgetTree,
    ) -> Option<bool> {
        let Some(snapshot) = self.current_committed_ui_snapshot.as_ref() else {
            return None;
        };
        let Some(subtree_root_id) =
            snapshot.matching_non_root_subtree_root_id_for_tree(&pending.tree)
        else {
            return Some(false);
        };
        Some(self.replace_current_subtree_without_relayout(
            subtree_root_id,
            probed_shallow_clone("w2:upgrade-full-tree-subtree", &pending.tree),
            pending.reactive_dependencies.clone(),
        ))
    }

    pub fn clear_layout_effects(&mut self) {
        let current_buffer_id = self.shared.borrow().current_buffer_id;
        self.vm.clear_effects_for_owner(current_buffer_id);
        let previous_layout = self.current_layout.clone();
        self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
        self.current_layout = None;
        self.reactive_registry
            .replace_widget_bindings_from_layout(None);
        self.layout_revision = self.layout_revision.wrapping_add(1);
        self.current_widget_tree = None;
        self.commit_current_ui_snapshot(None);
        self.shared
            .borrow_mut()
            .pending_cleared_effect_sources
            .push(ClearedEffectSource {
                source_buffer_id: current_buffer_id,
                runtime_generation: self.current_committed_ui_snapshot_generation,
            });
        #[cfg(test)]
        self.rendered_layouts.clear();
    }

    fn flush_widget_trees(&mut self) -> ReactiveFlushStats {
        let trees = std::mem::take(&mut self.vm.pending_widget_trees);
        let pending_widget_tree_count = trees.len();
        let (current_buffer_id, current_buffer_name) = {
            let shared = self.shared.borrow();
            (shared.current_buffer_id, shared.current_buffer_name.clone())
        };
        let trace = trace_ui_enabled();
        if trace && pending_widget_tree_count > 0 {
            eprintln!(
                "[ui-trace][flush] pending={} active_buffer_id={:?} active_buffer_name={}",
                pending_widget_tree_count, current_buffer_id, current_buffer_name
            );
        }
        let mut affected_buffers = HashSet::new();
        let mut active_buffer_targets = 0usize;
        let mut inactive_buffer_targets = 0usize;
        let mut full_buffer_reruns = 0usize;
        let mut subtree_reruns = 0usize;
        let mut reevaluated_subtree_roots = 0usize;
        let mut pending_subtree_patch_count = 0usize;
        let mut active_tree_requires_full_relayout = false;
        let mut active_changed_subtree_roots = Vec::new();
        let mut active_subtree_replacements: Vec<ActiveSubtreeReplacement> = Vec::new();
        let mut inactive_pending = Vec::new();
        let flush_active_subtree_replacements =
            |runtime: &mut Self,
             replacements: &mut Vec<ActiveSubtreeReplacement>,
             active_changed_subtree_roots: &mut Vec<u64>,
             fallback_pending: &mut Vec<PendingUiUpdate>| {
                if replacements.is_empty() {
                    return;
                }
                let batch = replacements
                    .iter()
                    .map(|replacement| {
                        (
                            replacement.subtree_root_id,
                            probed_shallow_clone("w2:flush-subtree-batch", &replacement.tree),
                            replacement.reactive_dependencies.clone(),
                        )
                    })
                    .collect::<Vec<_>>();
                if runtime.replace_current_subtrees_without_relayout(&batch) {
                    active_changed_subtree_roots.extend(
                        replacements
                            .iter()
                            .map(|replacement| replacement.subtree_root_id),
                    );
                } else {
                    if trace {
                        eprintln!(
                            "[ui-trace][flush] active subtree batch missed; delegating {} patches to editor buffer snapshots",
                            replacements.len()
                        );
                    }
                    fallback_pending.extend(replacements.drain(..).map(|replacement| {
                        PendingUiUpdate::ReplaceSubtree {
                            source_buffer_id: replacement.source_buffer_id,
                            source_file: replacement.source_file,
                            target: replacement.target,
                            subtree_root_id: replacement.subtree_root_id,
                            tree: replacement.tree,
                            reactive_dependencies: replacement.reactive_dependencies,
                        }
                    }));
                    return;
                }
                replacements.clear();
            };
        for pending in trees {
            affected_buffers.insert(effect_target_label(pending.target()));
            let targets_active_buffer = match pending.target() {
                EffectTarget::BufferId(id) => *id == current_buffer_id,
                EffectTarget::BufferName(name) => *name == current_buffer_name,
            };
            if trace {
                let kind = match &pending {
                    PendingUiUpdate::FullTree(_) => "full",
                    PendingUiUpdate::ReplaceSubtree { .. } => "subtree",
                };
                eprintln!(
                    "[ui-trace][flush] item kind={} target={} active={}",
                    kind,
                    effect_target_label(pending.target()),
                    targets_active_buffer
                );
            }
            if targets_active_buffer {
                active_buffer_targets += 1;
                match &pending {
                    PendingUiUpdate::FullTree(pending) => {
                        if let Some(true) =
                            self.current_committed_ui_snapshot
                                .as_ref()
                                .and_then(|snapshot| {
                                    snapshot
                                        .matching_non_root_subtree_root_id_for_tree(&pending.tree)
                                        .map(|subtree_root_id| {
                                            active_subtree_replacements.push(
                                                ActiveSubtreeReplacement {
                                                    source_buffer_id: pending.source_buffer_id,
                                                    source_file: pending.source_file.clone(),
                                                    target: pending.target.clone(),
                                                    subtree_root_id,
                                                    tree: probed_shallow_clone(
                                                        "w2:flush-full-as-subtree",
                                                        &pending.tree,
                                                    ),
                                                    reactive_dependencies: pending
                                                        .reactive_dependencies
                                                        .clone(),
                                                },
                                            );
                                            true
                                        })
                                })
                        {
                            subtree_reruns += 1;
                            reevaluated_subtree_roots += 1;
                            pending_subtree_patch_count += 1;
                        } else {
                            flush_active_subtree_replacements(
                                self,
                                &mut active_subtree_replacements,
                                &mut active_changed_subtree_roots,
                                &mut inactive_pending,
                            );
                            full_buffer_reruns += 1;
                            let unchanged = self
                                .current_widget_tree
                                .as_ref()
                                .is_some_and(|current| *current == pending.tree);
                            if !unchanged {
                                self.current_widget_tree = Some(probed_shallow_clone(
                                    "w2:flush-full-tree-store",
                                    &pending.tree,
                                ));
                                self.commit_current_ui_snapshot(Some(
                                    CommittedBufferUiSnapshot::from_tree(
                                        probed_shallow_clone(
                                            "w2:flush-full-tree-commit",
                                            &pending.tree,
                                        ),
                                        pending.source_buffer_id,
                                        pending.reactive_dependencies.clone(),
                                    ),
                                ));
                                active_tree_requires_full_relayout = true;
                            } else {
                                self.commit_current_ui_snapshot(Some(
                                    CommittedBufferUiSnapshot::from_tree(
                                        probed_shallow_clone(
                                            "w2:flush-unchanged-tree-commit",
                                            &pending.tree,
                                        ),
                                        pending.source_buffer_id,
                                        pending.reactive_dependencies.clone(),
                                    ),
                                ));
                            }
                        }
                    }
                    PendingUiUpdate::ReplaceSubtree {
                        source_buffer_id,
                        subtree_root_id,
                        tree,
                        reactive_dependencies,
                        ..
                    } => {
                        active_subtree_replacements.push(ActiveSubtreeReplacement {
                            source_buffer_id: *source_buffer_id,
                            source_file: pending.source_file().map(PathBuf::from),
                            target: pending.target().clone(),
                            subtree_root_id: *subtree_root_id,
                            tree: probed_shallow_clone("w2:flush-replace-subtree", tree),
                            reactive_dependencies: reactive_dependencies.clone(),
                        });
                        subtree_reruns += 1;
                        reevaluated_subtree_roots += 1;
                        pending_subtree_patch_count += 1;
                    }
                }
            } else {
                inactive_buffer_targets += 1;
                if matches!(&pending, PendingUiUpdate::FullTree(_)) {
                    full_buffer_reruns += 1;
                }
                inactive_pending.push(pending);
            }
        }
        flush_active_subtree_replacements(
            self,
            &mut active_subtree_replacements,
            &mut active_changed_subtree_roots,
            &mut inactive_pending,
        );
        if active_tree_requires_full_relayout {
            self.relayout_current_tree_because("reactive-full-tree");
            self.layout_revision = self.layout_revision.wrapping_add(1);
        } else if !active_changed_subtree_roots.is_empty()
            && let Err(reason) =
                self.relayout_current_layout_for_subtrees(&active_changed_subtree_roots)
        {
            if let Some(trace) = self.last_ui_invalidation_trace.as_mut() {
                trace.subtree_failure_reason = Some(reason);
            }
            self.relayout_current_tree_because("reactive-subtree-fallback");
            self.layout_revision = self.layout_revision.wrapping_add(1);
        }
        if trace && pending_widget_tree_count > 0 {
            let active_tree_changed =
                active_tree_requires_full_relayout || !active_changed_subtree_roots.is_empty();
            eprintln!(
                "[ui-trace][flush] complete active_changed={} active_targets={} inactive_targets={} full={} subtree={} patches={}",
                active_tree_changed,
                active_buffer_targets,
                inactive_buffer_targets,
                full_buffer_reruns,
                subtree_reruns,
                pending_subtree_patch_count
            );
        }
        self.shared
            .borrow_mut()
            .pending_buffer_widget_trees
            .extend(inactive_pending);
        let mut affected_buffers = affected_buffers.into_iter().collect::<Vec<_>>();
        affected_buffers.sort();
        self.ui_work_counters.full_buffer_reruns += full_buffer_reruns as u64;
        self.ui_work_counters.subtree_reruns += subtree_reruns as u64;
        self.ui_work_counters.reevaluated_subtree_roots +=
            reevaluated_subtree_roots as u64;
        ReactiveFlushStats {
            widget_tree_flushes: pending_widget_tree_count,
            pending_widget_tree_count,
            affected_buffers,
            active_buffer_targets,
            inactive_buffer_targets,
            full_buffer_reruns,
            subtree_reruns,
            reevaluated_subtree_roots,
            pending_subtree_patch_count,
        }
    }

    /// Relayout the current tree, recording `cause` as the call site that asked
    /// for it.
    ///
    /// Most callers null `current_layout` before calling in, which destroys the
    /// `previous_layout` the reuse path needs *and* the input
    /// `reuse_layout_failure_reason` is derived from — so the profiler used to
    /// report those relayouts as `fail=-`, with no way to tell which setter
    /// fired. `cause` is the fallback reason so `[ui-profile][runtime] fail=`
    /// names the caller instead.
    fn relayout_current_tree_because(&mut self, cause: &'static str) {
        let relayout_started = Instant::now();
        let previous_layout = self.current_layout.clone();
        let Some(tree) = self.current_widget_tree.as_ref() else {
            let had_layout = previous_layout.is_some();
            self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
            self.current_layout = None;
            self.reactive_registry
                .replace_widget_bindings_from_layout(None);
            if had_layout {
                self.layout_revision = self.layout_revision.wrapping_add(1);
            }
            self.update_last_trace_relayout("clear", None, relayout_started.elapsed());
            self.ui_work_counters.relayout_reused += 1;
            self.perf_stats
                .note_relayout(true, false, relayout_started.elapsed(), None);
            return;
        };
        let mut dirty_widget_ids = Vec::new();
        if let Some(existing) = previous_layout.as_ref()
            && let Some(updated) = reuse_layout_node(existing.as_ref(), tree, &mut dirty_widget_ids)
        {
            let geometry_changed = !same_layout_geometry(existing.as_ref(), &updated);
            #[cfg(test)]
            self.rendered_layouts
                .push(crate::layout::format_layout_tree_lines(&updated, 0));
            self.current_layout = Some(Arc::new(updated));
            self.reactive_registry
                .replace_widget_bindings_from_layout(self.current_layout.as_deref());
            self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), dirty_widget_ids);
            if geometry_changed || self.force_layout_revision_bump {
                self.layout_revision = self.layout_revision.wrapping_add(1);
            }
            self.force_layout_revision_bump = false;
            self.update_last_trace_relayout("reuse", None, relayout_started.elapsed());
            self.ui_work_counters.relayout_reused += 1;
            self.perf_stats
                .note_relayout(true, false, relayout_started.elapsed(), None);
            return;
        }
        let failure_reason = previous_layout
            .as_ref()
            .and_then(|existing| reuse_layout_failure_reason(existing.as_ref(), tree))
            .or_else(|| Some(format!("cleared:{cause}")));
        let mut engine = if let Some(measurer) = self.text_measurer.as_deref() {
            LayoutEngine::with_text_measurer_exact(
                self.layout_cols,
                self.layout_rows,
                self.layout_aspect,
                measurer,
                self.layout_cell_w,
                self.layout_cell_h,
            )
        } else {
            LayoutEngine::new_exact(self.layout_cols, self.layout_rows, self.layout_aspect)
        };
        engine.frame_viewport = self.layout_frame_viewport;
        engine.content_scroll = self.layout_content_scroll;
        if let Some(layout) = engine.layout_with_id_offset(tree, self.widget_id_offset) {
            let geometry_changed = previous_layout
                .as_ref()
                .is_none_or(|existing| !same_layout_geometry(existing.as_ref(), &layout));
            #[cfg(test)]
            self.rendered_layouts
                .push(crate::layout::format_layout_tree_lines(&layout, 0));
            self.current_layout = Some(Arc::new(layout));
            self.reactive_registry
                .replace_widget_bindings_from_layout(self.current_layout.as_deref());
            if geometry_changed {
                self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
                self.layout_revision = self.layout_revision.wrapping_add(1);
            } else if self.force_layout_revision_bump {
                self.layout_revision = self.layout_revision.wrapping_add(1);
                self.replace_dirty_widget_ids_for_layout(previous_layout.as_deref(), []);
            } else if let Some(layout) = self.current_layout.as_ref() {
                let shader_widget_ids = collect_shader_widget_ids(layout);
                self.replace_dirty_widget_ids_for_layout(
                    previous_layout.as_deref(),
                    shader_widget_ids,
                );
            }
            self.force_layout_revision_bump = false;
            self.update_last_trace_relayout(
                "full",
                failure_reason.clone(),
                relayout_started.elapsed(),
            );
            self.ui_work_counters.relayout_full += 1;
            self.perf_stats
                .note_relayout(false, false, relayout_started.elapsed(), failure_reason);
        }
    }

    fn update_last_trace_relayout(
        &mut self,
        mode: &str,
        failure_reason: Option<String>,
        elapsed: Duration,
    ) {
        if let Some(trace) = self.last_ui_invalidation_trace.as_mut() {
            trace.relayout_mode = Some(mode.to_string());
            trace.relayout_duration = elapsed;
            trace.relayout_failure_reason = failure_reason;
        }
    }

    fn invalidate_symbol_cache(&mut self) {
        self.symbol_revision = self.symbol_revision.wrapping_add(1);
        self.cached_completion_symbols = None;
        self.cached_completion_metadata = None;
        self.cached_module_completions = None;
    }
}

fn discover_module_completions(roots: &[crate::hot_reload::ModuleLoadRoot]) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    for root in roots {
        if let Some(prefix) = &root.module_prefix {
            names.insert(prefix.clone());
        }

        let mut files = Vec::new();
        collect_module_source_files(&root.path, &mut HashSet::new(), &mut files);
        for path in files {
            let Ok(relative) = path.strip_prefix(&root.path) else {
                continue;
            };
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok((Some(module), _)) = crate::modules::inspect_exports(&source) else {
                continue;
            };
            let relative_module = match &root.module_prefix {
                Some(prefix) => {
                    let Some(suffix) = module
                        .strip_prefix(prefix)
                        .and_then(|suffix| suffix.strip_prefix('.'))
                    else {
                        continue;
                    };
                    suffix
                }
                None => module.as_str(),
            };
            if crate::modules::module_relative_file_candidates(relative_module)
                .iter()
                .any(|candidate| candidate == relative)
            {
                names.insert(module);
            }
        }
    }
    names.into_iter().collect()
}

fn collect_module_source_files(
    root: &std::path::Path,
    visited_directories: &mut HashSet<PathBuf>,
    files: &mut Vec<PathBuf>,
) {
    let Ok(canonical_root) = root.canonicalize() else {
        return;
    };
    if !visited_directories.insert(canonical_root) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_module_source_files(&path, visited_directories, files);
        } else if path.is_file() && path.extension().is_some_and(|ext| ext == "lisp") {
            files.push(path);
        }
    }
}

fn collect_shader_widget_ids(node: &LayoutNode) -> Vec<u64> {
    let mut ids = Vec::new();
    collect_shader_widget_ids_recursive(node, &mut ids);
    ids
}

fn effect_target_label(target: &EffectTarget) -> String {
    match target {
        EffectTarget::BufferId(Some(id)) => format!("buf#{id}"),
        EffectTarget::BufferId(None) => "active-buffer".to_string(),
        EffectTarget::BufferName(name) => name.clone(),
    }
}

fn layout_node_at_path<'a>(node: &'a LayoutNode, path: &[usize]) -> Option<&'a LayoutNode> {
    let mut current = node;
    for index in path {
        current = current.children.get(*index)?;
    }
    Some(current)
}

fn collect_shader_widget_ids_recursive(node: &LayoutNode, ids: &mut Vec<u64>) {
    if matches!(
        node.widget_type.as_str(),
        "slider" | "hslider" | "vslider" | "toggle" | "matrix" | "knob"
    ) {
        ids.push(node.widget_id);
    }
    for child in &node.children {
        collect_shader_widget_ids_recursive(child, ids);
    }
}

#[cfg(test)]
mod theme_shader_recompile_tests {
    use crate::backend::Color;
    use crate::parser::Expression;
    use crate::ui::theme;
    use crate::widget_render::sdf_widget::{SdfWidgetDef, register_sdf_widget, sdf_widget_def};

    fn parse_one(src: &str) -> Expression {
        let tokens = crate::parser::Parser::new(src.to_string()).parse().unwrap();
        let mut ast = crate::parser::ASTParser::new(tokens);
        ast.parse().unwrap().into_iter().next().unwrap()
    }

    #[test]
    fn theme_change_reemits_keyword_baked_shaders() {
        let expr = parse_one("(sdf/layer (sdf/fill (- x 0.5) :accent))");
        let options = crate::lang::sdf_codegen::SdfShaderOptions::from_env().unwrap();
        let baked = super::compile_sdf_for_platform_backend(&expr, &[], options).unwrap();
        register_sdf_widget(SdfWidgetDef {
            name: "theme-recompile-probe".into(),
            shader_source: baked.shader_source.clone(),
            sdf_expr: expr,
            state_uniforms: Vec::new(),
            bindable_props: Vec::new(),
            region_count: baked.region_count,
            width: 1.0,
            height: 1.0,
            paint_margin: 0.0,
            animates: false,
        });

        // Same theme the shader was baked against: nothing to re-emit.
        assert!(!super::recompile_theme_dependent_sdf_shaders());

        let mut theme = theme::current();
        theme.accent = Color::from_hex(0x01, 0x02, 0x03);
        theme::set_current(theme);

        assert!(super::recompile_theme_dependent_sdf_shaders());
        let def = sdf_widget_def("theme-recompile-probe").unwrap();
        assert_ne!(def.shader_source, baked.shader_source);

        // Guarded by theme generation: a second pass is a no-op.
        assert!(!super::recompile_theme_dependent_sdf_shaders());
    }
}
