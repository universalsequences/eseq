use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::OnceLock;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::{CellBuffer, MouseEventOutcome, WidgetDefinition, WidgetEvent, styled_cell};
#[cfg(target_os = "macos")]
use super::{
    MetalCirclePrimitive, MetalCircleVisibleHalf, MetalPatchCablePrimitive, MetalPrimitive,
    MetalProportionalTextPrimitive, MetalQuadPrimitive, MetalRectPrimitive, WidgetInstance,
    WidgetViewport, ndc_bounds,
};
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::parser::{ASTParser, Expression, Parser, format_expression};
use crate::theme;
use crate::vm::Value;

pub struct PatcherWidget;

pub static PATCHER_WIDGET: PatcherWidget = PatcherWidget;

const DEFAULT_WIDTH: f32 = 96.0;
const DEFAULT_HEIGHT: f32 = 38.0;
const NODE_MIN_WIDTH: f32 = 5.8;
const NODE_HEIGHT: f32 = 1.68;
const CODE_NODE_MIN_WIDTH: f32 = 14.0;
const CODE_NODE_HEIGHT: f32 = 3.7;
const LAYER_SPACING: f32 = 4.95;
const NODE_COLUMN_GAP: f32 = 3.0;
const VIEW_PADDING_X: f32 = 4.0;
const VIEW_PADDING_Y: f32 = 4.0;
const NODE_BORDER_INSET: f32 = 0.14;
const NODE_CORNER_RADIUS_PX: f32 = 18.0;
const NODE_FONT_SIZE: f32 = 16.0;
const CODE_NODE_FONT_SIZE: f32 = 11.0;
const PORT_OUTER_DIAMETER_PX: f32 = 27.0;
const PORT_INNER_DIAMETER_PX: f32 = 18.75;
const PORT_EDGE_PADDING_CELLS: f32 = 1.65;
const TOUCHPAD_PAN_SPEED_CELLS_PER_PIXEL: f32 = 0.05;
const WHEEL_PAN_STEP_CELLS: f32 = 3.0;
const PAN_OVERSCROLL_VIEWPORT_FACTOR: f32 = 1.0;
const PAN_OVERSCROLL_MIN_CELLS: f32 = 48.0;

#[derive(Clone, Debug, Default, PartialEq)]
struct PatcherPanState {
    offset_x: f32,
    offset_y: f32,
    content_width: f32,
    content_height: f32,
    viewport_width: f32,
    viewport_height: f32,
}

thread_local! {
    static PATCHER_PAN_STATES: RefCell<HashMap<u64, PatcherPanState>> =
        RefCell::new(HashMap::new());
    static PATCHER_INTERACTION_STATES: RefCell<HashMap<u64, PatcherInteractionState>> =
        RefCell::new(HashMap::new());
}

fn patcher_state_key(node: &LayoutNode) -> u64 {
    node.stable_widget_id.unwrap_or_else(|| {
        let mut hasher = DefaultHasher::new();
        "patcher".hash(&mut hasher);
        prop_str(&node.props, "path").hash(&mut hasher);
        prop_str(&node.props, "file").hash(&mut hasher);
        prop_str(&node.props, "intent").hash(&mut hasher);
        hasher.finish()
    })
}

fn get_patcher_pan_state(key: u64) -> PatcherPanState {
    PATCHER_PAN_STATES.with(|states| states.borrow().get(&key).cloned().unwrap_or_default())
}

fn set_patcher_pan_state(key: u64, mut state: PatcherPanState) {
    clamp_patcher_pan_state(&mut state);
    let changed = PATCHER_PAN_STATES.with(|states| {
        let mut states = states.borrow_mut();
        let old = states.insert(key, state.clone());
        old.as_ref() != Some(&state)
    });
    if changed {
        super::bump_widget_state_generation();
    }
}

fn clamp_patcher_pan_state(state: &mut PatcherPanState) {
    let max_x = (state.content_width - state.viewport_width).max(0.0);
    let max_y = (state.content_height - state.viewport_height).max(0.0);
    let overscroll_x =
        (state.viewport_width * PAN_OVERSCROLL_VIEWPORT_FACTOR).max(PAN_OVERSCROLL_MIN_CELLS);
    let overscroll_y =
        (state.viewport_height * PAN_OVERSCROLL_VIEWPORT_FACTOR).max(PAN_OVERSCROLL_MIN_CELLS);
    state.offset_x = state.offset_x.clamp(-overscroll_x, max_x + overscroll_x);
    state.offset_y = state.offset_y.clamp(-overscroll_y, max_y + overscroll_y);
}

#[derive(Clone, Debug, Default, PartialEq)]
struct PatcherInteractionState {
    selected_nodes: HashSet<String>,
    hovered_node: Option<String>,
    hover_back_button: bool,
    node_positions: HashMap<String, (f32, f32)>,
    active_macro: Option<String>,
    drag: Option<PatcherDragState>,
}

#[derive(Clone, Debug, PartialEq)]
enum PatcherDragState {
    Nodes {
        start_col: f32,
        start_row: f32,
        start_positions: HashMap<String, (f32, f32)>,
    },
    Marquee {
        start_col: f32,
        start_row: f32,
        current_col: f32,
        current_row: f32,
        base_selection: HashSet<String>,
    },
}

fn get_patcher_interaction_state(key: u64) -> PatcherInteractionState {
    PATCHER_INTERACTION_STATES.with(|states| states.borrow().get(&key).cloned().unwrap_or_default())
}

fn set_patcher_interaction_state(key: u64, state: PatcherInteractionState) {
    let changed = PATCHER_INTERACTION_STATES.with(|states| {
        let mut states = states.borrow_mut();
        let old = states.insert(key, state.clone());
        old.as_ref() != Some(&state)
    });
    if changed {
        super::bump_widget_state_generation();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatcherIntent {
    Instrument,
    Effect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Builtin,
    Param,
    In,
    Out,
    History,
    MacroDefinition,
    MacroInstance,
    CodeIsland,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArgValue {
    Literal(String),
    SymbolRef(String),
}

#[derive(Debug, Clone)]
pub struct PatchNode {
    pub id: String,
    pub op: String,
    pub kind: NodeKind,
    pub label: String,
    pub args: Vec<ArgValue>,
    pub outputs: Vec<String>,
    pub position: (f32, f32),
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionKind {
    Forward,
    Feedback,
}

#[derive(Debug, Clone)]
pub struct PatchConnection {
    pub from_node: String,
    pub from_output: usize,
    pub to_node: String,
    pub to_input: usize,
    pub kind: ConnectionKind,
}

#[derive(Debug, Clone)]
pub struct MacroPatch {
    pub name: String,
    pub params: Vec<String>,
    pub patch: Patch,
}

#[derive(Debug, Clone, Default)]
pub struct Patch {
    pub nodes: Vec<PatchNode>,
    pub connections: Vec<PatchConnection>,
    pub macros: Vec<MacroPatch>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone)]
struct Projector {
    patch: Patch,
    symbol_sources: HashMap<String, (String, usize)>,
    history_nodes: HashMap<String, String>,
    op_occurrences: HashMap<String, usize>,
    used_ids: HashSet<String>,
    known_ops: &'static HashSet<String>,
    known_macros: HashSet<String>,
}

impl Projector {
    fn new(known_macros: HashSet<String>) -> Self {
        Self {
            patch: Patch::default(),
            symbol_sources: HashMap::new(),
            history_nodes: HashMap::new(),
            op_occurrences: HashMap::new(),
            used_ids: HashSet::new(),
            known_ops: dgenlisp_operator_names(),
            known_macros,
        }
    }

    fn project(mut self, exprs: &[Expression]) -> Patch {
        for expr in exprs {
            self.project_top_level(expr);
        }
        assign_layout(&mut self.patch);
        self.patch
    }

    fn project_top_level(&mut self, expr: &Expression) {
        let Expression::List(items) = expr else {
            self.add_code_island(expr, "top-level atom is not visual patch syntax");
            return;
        };
        let Some(head) = symbol_at(items, 0) else {
            self.add_code_island(expr, "top-level form has no symbolic operator");
            return;
        };
        match head {
            "def" => self.project_def(items, expr),
            "defmacro" => self.project_defmacro(items, expr),
            "param" => self.project_param(items, expr),
            "make-history" => self.project_make_history(items, expr),
            "write-history" => {
                let _ = self.project_write_history(items, expr);
            }
            "out" => {
                let id = self.stable_id_for_call(items, None);
                let _ = self.project_call(items, Some(id), None);
            }
            _ => self.add_code_island(expr, "top-level form is not a supported patch form"),
        }
    }

    fn project_param(&mut self, items: &[Expression], original: &Expression) {
        let Some(name) = symbol_at(items, 1) else {
            self.add_code_island(original, "`param` forms must provide a symbolic name");
            return;
        };
        let id = self.stable_id_for_call(items, None);
        let Some(node_id) = self.project_call(items, Some(id), None) else {
            self.add_code_island(original, "`param` value could not be projected");
            return;
        };
        self.symbol_sources.insert(name.to_string(), (node_id, 0));
    }

    fn project_make_history(&mut self, items: &[Expression], original: &Expression) {
        let Some(name) = symbol_at(items, 1) else {
            self.add_code_island(
                original,
                "`make-history` forms must provide a symbolic name",
            );
            return;
        };
        if self.history_nodes.contains_key(name) {
            self.add_code_island(original, "`make-history` declared the same history twice");
            return;
        }
        let id = self.unique_id(name);
        self.history_nodes.insert(name.to_string(), id.clone());
        self.patch.nodes.push(PatchNode {
            id,
            op: "make-history".to_string(),
            kind: NodeKind::History,
            label: "history".to_string(),
            args: Vec::new(),
            outputs: vec!["out".to_string()],
            position: (0.0, 0.0),
            diagnostic: None,
        });
    }

    fn project_def(&mut self, items: &[Expression], original: &Expression) {
        if items.len() != 3 {
            self.add_code_island(original, "`def` forms must have exactly a target and value");
            return;
        }

        match &items[1] {
            Expression::Symbol(name) => {
                let Some((node_id, output_idx)) =
                    self.project_value(&items[2], Some(name.clone()), Some(name.clone()))
                else {
                    self.add_code_island(original, "`def` value could not be projected");
                    return;
                };
                self.symbol_sources
                    .insert(name.clone(), (node_id, output_idx));
            }
            Expression::List(outputs) => {
                let names = outputs
                    .iter()
                    .map(|expr| match expr {
                        Expression::Symbol(name) => Some(name.clone()),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>();
                let Some(names) = names else {
                    self.add_code_island(
                        original,
                        "destructuring `def` contains non-symbol outputs",
                    );
                    return;
                };
                let stable = names.join("_");
                let Some((node_id, _)) = self.project_value(&items[2], Some(stable), None) else {
                    self.add_code_island(
                        original,
                        "destructuring `def` value could not be projected",
                    );
                    return;
                };
                if let Some(node) = self.patch.nodes.iter_mut().find(|node| node.id == node_id) {
                    node.outputs = names.clone();
                    node.label = format!("{} -> {}", node.op, names.join(" "));
                }
                for (idx, name) in names.into_iter().enumerate() {
                    self.symbol_sources.insert(name, (node_id.clone(), idx));
                }
            }
            _ => self.add_code_island(original, "`def` target is not a symbol or symbol tuple"),
        }
    }

    fn project_defmacro(&mut self, items: &[Expression], original: &Expression) {
        let (Some(name), Some(Expression::List(params))) = (symbol_at(items, 1), items.get(2))
        else {
            self.add_code_island(
                original,
                "`defmacro` must provide a name and parameter list",
            );
            return;
        };
        let Some(param_names) = params
            .iter()
            .map(|expr| match expr {
                Expression::Symbol(name) => Some(name.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
        else {
            self.add_code_island(original, "`defmacro` parameters must be symbols");
            return;
        };

        let body = if items.len() > 3 { &items[3..] } else { &[] };
        let mut projector = Projector::new(HashSet::new());
        for (idx, param) in param_names.iter().enumerate() {
            let id = projector.unique_id(param);
            projector
                .symbol_sources
                .insert(param.clone(), (id.clone(), 0));
            projector.patch.nodes.push(PatchNode {
                id,
                op: "in".to_string(),
                kind: NodeKind::In,
                label: format!("in {}", idx + 1),
                args: vec![ArgValue::Literal((idx + 1).to_string())],
                outputs: vec![param.clone()],
                position: (0.0, 0.0),
                diagnostic: None,
            });
        }
        for (idx, expr) in body.iter().enumerate() {
            if idx + 1 == body.len() {
                projector.project_macro_return(expr);
            } else {
                projector.project_top_level(expr);
            }
        }
        let mut patch = projector.patch;
        assign_layout(&mut patch);
        self.patch.macros.push(MacroPatch {
            name: name.to_string(),
            params: param_names,
            patch,
        });
    }

    fn project_macro_return(&mut self, expr: &Expression) {
        if let Expression::List(items) = expr
            && symbol_at(items, 0).is_some_and(|head| {
                matches!(
                    head,
                    "def" | "defmacro" | "param" | "out" | "make-history" | "write-history"
                )
            })
        {
            self.project_top_level(expr);
            return;
        }

        let Some((from_node, from_output)) =
            self.project_value(expr, Some("return".to_string()), None)
        else {
            self.add_code_island(expr, "macro return value is not visual patch syntax");
            return;
        };
        let id = self.unique_id("out");
        self.patch.nodes.push(PatchNode {
            id: id.clone(),
            op: "out".to_string(),
            kind: NodeKind::Out,
            label: "out 1".to_string(),
            args: vec![ArgValue::Literal("1".to_string())],
            outputs: Vec::new(),
            position: (0.0, 0.0),
            diagnostic: None,
        });
        self.patch.connections.push(PatchConnection {
            from_node,
            from_output,
            to_node: id,
            to_input: 0,
            kind: ConnectionKind::Forward,
        });
    }

    fn project_value(
        &mut self,
        expr: &Expression,
        stable_id: Option<String>,
        def_name: Option<String>,
    ) -> Option<(String, usize)> {
        match expr {
            Expression::Symbol(name) => self.symbol_sources.get(name).cloned(),
            Expression::List(items) => self
                .project_call(items, stable_id, def_name)
                .map(|id| (id, 0)),
            _ => None,
        }
    }

    fn project_read_history(
        &mut self,
        items: &[Expression],
        original: &Expression,
    ) -> Option<String> {
        let Some(name) = symbol_at(items, 1) else {
            self.add_code_island(
                original,
                "`read-history` forms must provide a symbolic history name",
            );
            return None;
        };
        if let Some(node_id) = self.history_nodes.get(name) {
            Some(node_id.clone())
        } else {
            self.add_code_island(
                original,
                "`read-history` references a history with no `make-history`",
            );
            None
        }
    }

    fn project_write_history(
        &mut self,
        items: &[Expression],
        original: &Expression,
    ) -> Option<(String, usize)> {
        let Some(name) = symbol_at(items, 1) else {
            self.add_code_island(
                original,
                "`write-history` forms must provide a symbolic history name",
            );
            return None;
        };
        let Some(history_node) = self.history_nodes.get(name).cloned() else {
            self.add_code_island(
                original,
                "`write-history` references a history with no `make-history`",
            );
            return None;
        };
        let Some(value) = positional_args(items, 1).into_iter().nth(1) else {
            self.add_code_island(
                original,
                "`write-history` forms must provide a value to store",
            );
            return None;
        };
        let Some((from_node, from_output)) = self.project_value(value, None, None) else {
            self.add_code_island(original, "`write-history` value could not be projected");
            return None;
        };
        self.patch.connections.push(PatchConnection {
            from_node: from_node.clone(),
            from_output,
            to_node: history_node,
            to_input: 0,
            kind: ConnectionKind::Feedback,
        });
        Some((from_node, from_output))
    }

    fn project_call(
        &mut self,
        items: &[Expression],
        stable_id: Option<String>,
        def_name: Option<String>,
    ) -> Option<String> {
        let op = symbol_at(items, 0)?.to_string();
        if is_unsupported_call_head(&op) {
            let expr = Expression::List(items.to_vec());
            self.add_code_island(
                &expr,
                "control-flow and binding forms are not visualized in V1",
            );
            return None;
        }
        let original = Expression::List(items.to_vec());
        if op == "read-history" {
            return self.project_read_history(items, &original);
        }
        if op == "write-history" {
            return self
                .project_write_history(items, &original)
                .map(|(node_id, _)| node_id);
        }

        let kind = node_kind_for_op(&op, &self.known_macros);
        let id = stable_id.unwrap_or_else(|| self.stable_id_for_call(items, None));
        let mut node = PatchNode {
            id: self.unique_id(&id),
            op: op.clone(),
            kind,
            label: node_label(&op, items, def_name.as_deref()),
            args: Vec::new(),
            outputs: default_outputs(&op),
            position: (0.0, 0.0),
            diagnostic: self.operator_diagnostic(&op, kind),
        };

        for (idx, arg) in call_input_args(&op, items).into_iter().enumerate() {
            match arg {
                Expression::Symbol(name) => {
                    if let Some((from_node, from_output)) = self.symbol_sources.get(name).cloned() {
                        self.patch.connections.push(PatchConnection {
                            from_node,
                            from_output,
                            to_node: node.id.clone(),
                            to_input: idx,
                            kind: connection_kind_for_op(&op),
                        });
                        node.args.push(ArgValue::SymbolRef(name.clone()));
                    } else {
                        node.args.push(ArgValue::Literal(name.clone()));
                    }
                }
                Expression::List(nested) => {
                    if let Some((from_node, from_output)) =
                        self.project_value(&Expression::List(nested.clone()), None, None)
                    {
                        self.patch.connections.push(PatchConnection {
                            from_node,
                            from_output,
                            to_node: node.id.clone(),
                            to_input: idx,
                            kind: connection_kind_for_op(&op),
                        });
                    }
                    node.args.push(ArgValue::Literal("<expr>".to_string()));
                }
                other => node
                    .args
                    .push(ArgValue::Literal(format_patch_literal(other))),
            }
        }
        let id = node.id.clone();
        self.patch.nodes.push(node);
        Some(id)
    }

    fn stable_id_for_call(&mut self, items: &[Expression], fallback: Option<&str>) -> String {
        let op = symbol_at(items, 0).unwrap_or(fallback.unwrap_or("node"));
        if matches!(op, "in" | "out" | "param") {
            if let Some(name) = attribute_value(items, "@name").or_else(|| {
                (op == "param")
                    .then(|| symbol_at(items, 1).map(str::to_string))
                    .flatten()
            }) {
                return name;
            }
        }
        if op == "make-history" || op == "read-history" || op == "write-history" {
            if let Some(name) = symbol_at(items, 1) {
                return name.to_string();
            }
        }
        let next = self.op_occurrences.entry(op.to_string()).or_insert(0);
        let id = format!("{op}#{}", *next);
        *next += 1;
        id
    }

    fn unique_id(&mut self, base: &str) -> String {
        if self.used_ids.insert(base.to_string()) {
            return base.to_string();
        }
        let mut idx = 1usize;
        loop {
            let candidate = format!("{base}~{idx}");
            if self.used_ids.insert(candidate.clone()) {
                return candidate;
            }
            idx += 1;
        }
    }

    fn add_code_island(&mut self, expr: &Expression, reason: &str) {
        let id = self.unique_id("code");
        self.patch.nodes.push(PatchNode {
            id,
            op: "code".to_string(),
            kind: NodeKind::CodeIsland,
            label: preview(&format_expression(expr), 48),
            args: Vec::new(),
            outputs: Vec::new(),
            position: (0.0, 0.0),
            diagnostic: Some(reason.to_string()),
        });
        self.patch.diagnostics.push(reason.to_string());
    }

    fn operator_diagnostic(&self, op: &str, kind: NodeKind) -> Option<String> {
        if matches!(
            kind,
            NodeKind::CodeIsland
                | NodeKind::MacroDefinition
                | NodeKind::MacroInstance
                | NodeKind::In
                | NodeKind::Out
                | NodeKind::Param
                | NodeKind::History
        ) || self.known_ops.contains(op)
        {
            None
        } else {
            Some(format!("unknown DGenLisp operator `{op}`"))
        }
    }
}

pub fn parse_patch_source(source: &str, _intent: PatcherIntent) -> Result<Patch, String> {
    let tokens = Parser::new(source.to_string())
        .parse()
        .map_err(|error| format!("failed to tokenize dsp.lisp: {error:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("failed to parse dsp.lisp: {error:?}"))?;
    let macros = exprs
        .iter()
        .filter_map(|expr| {
            let Expression::List(items) = expr else {
                return None;
            };
            (symbol_at(items, 0) == Some("defmacro"))
                .then(|| symbol_at(items, 1).map(str::to_string))
                .flatten()
        })
        .collect();
    Ok(Projector::new(macros).project(&exprs))
}

fn symbol_at(items: &[Expression], idx: usize) -> Option<&str> {
    match items.get(idx) {
        Some(Expression::Symbol(symbol)) => Some(symbol),
        _ => None,
    }
}

fn attribute_value(items: &[Expression], attr: &str) -> Option<String> {
    items
        .windows(2)
        .find_map(|pair| match (&pair[0], &pair[1]) {
            (Expression::Symbol(key), value) if key == attr => Some(format_expression(value)),
            _ => None,
        })
}

fn node_kind_for_op(op: &str, macros: &HashSet<String>) -> NodeKind {
    match op {
        "in" => NodeKind::In,
        "out" => NodeKind::Out,
        "param" => NodeKind::Param,
        "make-history" | "read-history" | "write-history" => NodeKind::History,
        _ if macros.contains(op) => NodeKind::MacroInstance,
        _ => NodeKind::Builtin,
    }
}

fn is_unsupported_call_head(op: &str) -> bool {
    matches!(
        op,
        "let" | "if" | "do" | "lambda" | "match" | "each" | "quote" | "quasiquote"
    )
}

fn call_input_args<'a>(op: &str, items: &'a [Expression]) -> Vec<&'a Expression> {
    match op {
        "in" => positional_args(items, 1).into_iter().take(1).collect(),
        "param" | "make-history" => Vec::new(),
        "out" => positional_args(items, 1).into_iter().take(2).collect(),
        "write-history" => positional_args(items, 1)
            .into_iter()
            .nth(1)
            .into_iter()
            .collect(),
        _ => positional_args(items, 1),
    }
}

fn positional_args(items: &[Expression], start: usize) -> Vec<&Expression> {
    let mut args = Vec::new();
    let mut idx = start;
    while idx < items.len() {
        if matches!(&items[idx], Expression::Symbol(symbol) if symbol.starts_with('@')) {
            idx += 2;
            continue;
        }
        args.push(&items[idx]);
        idx += 1;
    }
    args
}

fn connection_kind_for_op(op: &str) -> ConnectionKind {
    if op == "write-history" {
        ConnectionKind::Feedback
    } else {
        ConnectionKind::Forward
    }
}

fn node_label(op: &str, items: &[Expression], def_name: Option<&str>) -> String {
    match op {
        "in" => attribute_value(items, "@name").unwrap_or_else(|| "in".to_string()),
        "out" => attribute_value(items, "@name").unwrap_or_else(|| "out".to_string()),
        "param" => param_label(items),
        "make-history" => "history".to_string(),
        _ => def_name
            .map(|name| format!("{op} {name}"))
            .unwrap_or_else(|| op.to_string()),
    }
}

fn param_label(items: &[Expression]) -> String {
    let mut label = String::from("param");
    for item in items.iter().skip(1) {
        label.push(' ');
        label.push_str(&format_patch_literal(item));
    }
    label
}

fn format_patch_literal(expr: &Expression) -> String {
    match expr {
        Expression::Number(n) if *n == n.trunc() && n.abs() < 1e15 => {
            format!("{:.0}", n)
        }
        _ => format_expression(expr),
    }
}

fn default_outputs(op: &str) -> Vec<String> {
    match op {
        "out" | "write-history" | "make-history" => Vec::new(),
        _ => vec!["out".to_string()],
    }
}

fn preview(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...");
            break;
        }
        out.push(ch);
    }
    out
}

fn assign_layout(patch: &mut Patch) {
    let mut id_to_idx = HashMap::new();
    for (idx, node) in patch.nodes.iter().enumerate() {
        id_to_idx.insert(node.id.clone(), idx);
    }

    let mut layers = vec![0usize; patch.nodes.len()];
    for (idx, node) in patch.nodes.iter().enumerate() {
        layers[idx] = match node.kind {
            NodeKind::In | NodeKind::Param => 0,
            NodeKind::CodeIsland | NodeKind::MacroDefinition => 1,
            _ => 1,
        };
    }

    for _ in 0..patch.nodes.len() {
        let mut changed = false;
        for connection in &patch.connections {
            let (Some(&from), Some(&to)) = (
                id_to_idx.get(&connection.from_node),
                id_to_idx.get(&connection.to_node),
            ) else {
                continue;
            };
            if connection.kind == ConnectionKind::Feedback {
                continue;
            }
            let next = layers[from].saturating_add(1);
            if layers[to] < next {
                layers[to] = next;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let out_layer = layers.iter().copied().max().unwrap_or(1).saturating_add(1);
    for (idx, node) in patch.nodes.iter().enumerate() {
        if node.kind == NodeKind::Out {
            layers[idx] = out_layer;
        }
    }

    let mut by_layer: HashMap<usize, Vec<usize>> = HashMap::new();
    for (idx, layer) in layers.iter().copied().enumerate() {
        by_layer.entry(layer).or_default().push(idx);
    }

    let mut layer_keys = by_layer.keys().copied().collect::<Vec<_>>();
    layer_keys.sort_unstable();
    for layer in layer_keys {
        let nodes = by_layer.get_mut(&layer).unwrap();
        nodes.sort_by(|a, b| patch.nodes[*a].id.cmp(&patch.nodes[*b].id));
        let mut col = VIEW_PADDING_X;
        for idx in nodes.iter().copied() {
            let (width, _) = node_size(&patch.nodes[idx]);
            patch.nodes[idx].position = (col, VIEW_PADDING_Y + layer as f32 * LAYER_SPACING);
            col += width + NODE_COLUMN_GAP;
        }
    }
}

fn dgenlisp_operator_names() -> &'static HashSet<String> {
    static OPERATOR_NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    OPERATOR_NAMES.get_or_init(|| {
        let metadata: serde_json::Value = serde_json::from_str(include_str!(
            "../../../sequencer/tools/dgenlisp-operators.json"
        ))
        .expect("bundled dgenlisp-operators.json must be valid JSON");
        let operators = metadata
            .get("operators")
            .and_then(serde_json::Value::as_array)
            .expect("bundled dgenlisp-operators.json must contain an operators array");

        let mut names = HashSet::new();
        for operator in operators {
            let Some(name) = operator.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            names.insert(name.to_string());
            if let Some(aliases) = operator
                .get("aliases")
                .and_then(serde_json::Value::as_array)
            {
                for alias in aliases {
                    if let Some(alias) = alias.as_str() {
                        names.insert(alias.to_string());
                    }
                }
            }
        }
        names
    })
}

fn pan_patcher_by_wheel(node: &LayoutNode, mouse_kind: MouseEventKind) {
    let (delta_x, delta_y) = match mouse_kind {
        MouseEventKind::ScrollUp => (0.0, -WHEEL_PAN_STEP_CELLS),
        MouseEventKind::ScrollDown => (0.0, WHEEL_PAN_STEP_CELLS),
        MouseEventKind::ScrollLeft => (-WHEEL_PAN_STEP_CELLS, 0.0),
        MouseEventKind::ScrollRight => (WHEEL_PAN_STEP_CELLS, 0.0),
        _ => (0.0, 0.0),
    };
    pan_patcher_by_delta(node, delta_x, delta_y);
}

fn pan_patcher_by_delta(node: &LayoutNode, delta_x: f32, delta_y: f32) {
    if delta_x == 0.0 && delta_y == 0.0 {
        return;
    }
    let key = patcher_state_key(node);
    let mut state = get_patcher_pan_state(key);
    sync_patcher_pan_bounds(node, &mut state);
    state.offset_x += delta_x;
    state.offset_y += delta_y;
    set_patcher_pan_state(key, state);
}

fn sync_patcher_pan_bounds(node: &LayoutNode, state: &mut PatcherPanState) {
    state.viewport_width = node.rect.width;
    state.viewport_height = node.rect.height;
    if let Ok((_, patch)) = load_patch_from_props(&node.props) {
        let interaction_state = get_patcher_interaction_state(patcher_state_key(node));
        let view_key = active_patcher_view_key(&interaction_state);
        let patch = active_patcher_patch(&patch, &interaction_state);
        let patch = patch_with_interaction_positions(patch, &interaction_state, &view_key);
        let content_size = patch_content_size(&patch);
        state.content_width = content_size.0.max(node.rect.width);
        state.content_height = content_size.1.max(node.rect.height);
    } else {
        state.content_width = node.rect.width;
        state.content_height = node.rect.height;
    }
}

fn patch_content_size(patch: &Patch) -> (f32, f32) {
    let mut max_col: f32 = VIEW_PADDING_X * 2.0;
    let mut max_row: f32 = VIEW_PADDING_Y * 2.0;
    for node in &patch.nodes {
        let (width, height) = node_size(node);
        max_col = max_col.max(2.0 + node.position.0 + width + VIEW_PADDING_X);
        max_row = max_row.max(2.4 + node.position.1 + height + VIEW_PADDING_Y);
    }
    (max_col, max_row)
}

fn active_patcher_view_key(interaction_state: &PatcherInteractionState) -> String {
    interaction_state
        .active_macro
        .as_deref()
        .map(|name| format!("macro:{name}"))
        .unwrap_or_else(|| "root".to_string())
}

fn scoped_node_position_key(view_key: &str, node_id: &str) -> String {
    format!("{view_key}::{node_id}")
}

fn active_patcher_patch(root_patch: &Patch, interaction_state: &PatcherInteractionState) -> Patch {
    interaction_state
        .active_macro
        .as_deref()
        .and_then(|name| {
            root_patch
                .macros
                .iter()
                .find(|macro_patch| macro_patch.name == name)
                .map(|macro_patch| macro_patch.patch.clone())
        })
        .unwrap_or_else(|| root_patch.clone())
}

fn patcher_breadcrumb(
    path: &std::path::Path,
    interaction_state: &PatcherInteractionState,
) -> String {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("dsp.lisp");
    match interaction_state.active_macro.as_deref() {
        Some(name) => format!("root / {file_name} / {name}"),
        None => format!("root / {file_name}"),
    }
}

fn patcher_back_label(interaction_state: &PatcherInteractionState) -> Option<&'static str> {
    interaction_state.active_macro.as_ref().map(|_| "<")
}

fn patch_with_interaction_positions(
    mut patch: Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
) -> Patch {
    for node in &mut patch.nodes {
        let position_key = scoped_node_position_key(view_key, &node.id);
        if let Some(position) = interaction_state.node_positions.get(&position_key) {
            node.position = *position;
        }
    }
    patch
}

fn patcher_origin(rect: Rect, pan_state: &PatcherPanState) -> (f32, f32) {
    (
        rect.col + 2.0 - pan_state.offset_x,
        rect.row + 2.4 - pan_state.offset_y,
    )
}

fn patch_node_rects(
    patch: &Patch,
    rect: Rect,
    pan_state: &PatcherPanState,
) -> HashMap<String, Rect> {
    let origin = patcher_origin(rect, pan_state);
    patch
        .nodes
        .iter()
        .map(|node| {
            let size = node_size(node);
            (
                node.id.clone(),
                Rect {
                    col: origin.0 + node.position.0,
                    row: origin.1 + node.position.1,
                    width: size.0,
                    height: size.1,
                },
            )
        })
        .collect()
}

fn hit_patcher_node(
    patch: &Patch,
    rect: Rect,
    pan_state: &PatcherPanState,
    local_col: f32,
    local_row: f32,
) -> Option<String> {
    let node_rects = patch_node_rects(patch, rect, pan_state);
    patch.nodes.iter().rev().find_map(|node| {
        let node_rect = node_rects.get(&node.id)?;
        rect_contains(*node_rect, local_col, local_row).then(|| node.id.clone())
    })
}

fn rect_contains(rect: Rect, col: f32, row: f32) -> bool {
    col >= rect.col
        && col <= rect.col + rect.width
        && row >= rect.row
        && row <= rect.row + rect.height
}

fn rects_intersect(a: Rect, b: Rect) -> bool {
    a.col <= b.col + b.width
        && a.col + a.width >= b.col
        && a.row <= b.row + b.height
        && a.row + a.height >= b.row
}

fn rect_from_points(start_col: f32, start_row: f32, current_col: f32, current_row: f32) -> Rect {
    let col = start_col.min(current_col);
    let row = start_row.min(current_row);
    Rect {
        col,
        row,
        width: (start_col - current_col).abs(),
        height: (start_row - current_row).abs(),
    }
}

fn load_interactive_patch_for_node(node: &LayoutNode) -> Option<(Patch, PatcherPanState, String)> {
    let key = patcher_state_key(node);
    let interaction_state = get_patcher_interaction_state(key);
    let (_, root_patch) = load_patch_from_props(&node.props).ok()?;
    let view_key = active_patcher_view_key(&interaction_state);
    let patch = active_patcher_patch(&root_patch, &interaction_state);
    let patch = patch_with_interaction_positions(patch, &interaction_state, &view_key);
    let mut pan_state = get_patcher_pan_state(key);
    sync_patcher_pan_bounds(node, &mut pan_state);
    Some((patch, pan_state, view_key))
}

fn handle_patcher_pointer_down(
    node: &LayoutNode,
    local_col: f32,
    local_row: f32,
    modifiers: KeyModifiers,
) {
    let key = patcher_state_key(node);
    let mut state = get_patcher_interaction_state(key);
    if state.active_macro.is_some()
        && rect_contains(patcher_back_button_rect(node.rect), local_col, local_row)
    {
        navigate_patcher_to_root(key, &mut state);
        return;
    }
    let Some((patch, pan_state, _view_key)) = load_interactive_patch_for_node(node) else {
        return;
    };
    let hit = hit_patcher_node(&patch, node.rect, &pan_state, local_col, local_row);
    state.hovered_node = hit.clone();
    let shift = modifiers.contains(KeyModifiers::SHIFT);
    match hit {
        Some(node_id) => {
            if shift {
                if !state.selected_nodes.insert(node_id.clone()) {
                    state.selected_nodes.remove(&node_id);
                }
            } else if !state.selected_nodes.contains(&node_id) {
                state.selected_nodes.clear();
                state.selected_nodes.insert(node_id);
            }
            let start_positions = patch
                .nodes
                .iter()
                .filter(|node| state.selected_nodes.contains(&node.id))
                .map(|node| (node.id.clone(), node.position))
                .collect();
            state.drag = Some(PatcherDragState::Nodes {
                start_col: local_col,
                start_row: local_row,
                start_positions,
            });
        }
        None => {
            let base_selection = if shift {
                state.selected_nodes.clone()
            } else {
                state.selected_nodes.clear();
                HashSet::new()
            };
            state.drag = Some(PatcherDragState::Marquee {
                start_col: local_col,
                start_row: local_row,
                current_col: local_col,
                current_row: local_row,
                base_selection,
            });
        }
    }
    set_patcher_interaction_state(key, state);
}

fn handle_patcher_pointer_drag(node: &LayoutNode, local_col: f32, local_row: f32) {
    let key = patcher_state_key(node);
    let Some((patch, pan_state, view_key)) = load_interactive_patch_for_node(node) else {
        return;
    };
    let mut state = get_patcher_interaction_state(key);
    match state.drag.clone() {
        Some(PatcherDragState::Nodes {
            start_col,
            start_row,
            start_positions,
        }) => {
            let delta = (local_col - start_col, local_row - start_row);
            for (node_id, start_position) in start_positions {
                let position_key = scoped_node_position_key(&view_key, &node_id);
                state.node_positions.insert(
                    position_key,
                    (start_position.0 + delta.0, start_position.1 + delta.1),
                );
            }
        }
        Some(PatcherDragState::Marquee {
            start_col,
            start_row,
            base_selection,
            ..
        }) => {
            let marquee = rect_from_points(start_col, start_row, local_col, local_row);
            let node_rects = patch_node_rects(&patch, node.rect, &pan_state);
            state.selected_nodes = base_selection.clone();
            for patch_node in &patch.nodes {
                if let Some(node_rect) = node_rects.get(&patch_node.id)
                    && rects_intersect(marquee, *node_rect)
                {
                    state.selected_nodes.insert(patch_node.id.clone());
                }
            }
            state.drag = Some(PatcherDragState::Marquee {
                start_col,
                start_row,
                current_col: local_col,
                current_row: local_row,
                base_selection,
            });
        }
        None => {}
    }
    state.hovered_node = hit_patcher_node(&patch, node.rect, &pan_state, local_col, local_row);
    set_patcher_interaction_state(key, state);
}

fn handle_patcher_pointer_up(node: &LayoutNode, local_col: f32, local_row: f32) {
    let key = patcher_state_key(node);
    let mut state = get_patcher_interaction_state(key);
    if let Some((patch, pan_state, _view_key)) = load_interactive_patch_for_node(node) {
        state.hovered_node = hit_patcher_node(&patch, node.rect, &pan_state, local_col, local_row);
    }
    state.drag = None;
    set_patcher_interaction_state(key, state);
}

fn handle_patcher_pointer_moved(node: &LayoutNode, local_col: f32, local_row: f32) {
    let key = patcher_state_key(node);
    let Some((patch, pan_state, _view_key)) = load_interactive_patch_for_node(node) else {
        return;
    };
    let mut state = get_patcher_interaction_state(key);
    state.hover_back_button = state.active_macro.is_some()
        && rect_contains(patcher_back_button_rect(node.rect), local_col, local_row);
    state.hovered_node = hit_patcher_node(&patch, node.rect, &pan_state, local_col, local_row);
    set_patcher_interaction_state(key, state);
}

fn handle_patcher_double_click(node: &LayoutNode, local_col: f32, local_row: f32) -> bool {
    let key = patcher_state_key(node);
    let Ok((_, root_patch)) = load_patch_from_props(&node.props) else {
        return false;
    };
    let mut state = get_patcher_interaction_state(key);
    if state.active_macro.is_some()
        && rect_contains(patcher_breadcrumb_rect(node.rect), local_col, local_row)
    {
        navigate_patcher_to_root(key, &mut state);
        return true;
    }

    let view_key = active_patcher_view_key(&state);
    let patch = active_patcher_patch(&root_patch, &state);
    let patch = patch_with_interaction_positions(patch, &state, &view_key);
    let mut pan_state = get_patcher_pan_state(key);
    sync_patcher_pan_bounds(node, &mut pan_state);
    let Some(node_id) = hit_patcher_node(&patch, node.rect, &pan_state, local_col, local_row)
    else {
        return false;
    };
    let Some(macro_name) = patch
        .nodes
        .iter()
        .find(|patch_node| patch_node.id == node_id && patch_node.kind == NodeKind::MacroInstance)
        .map(|patch_node| patch_node.op.clone())
    else {
        return false;
    };
    if !root_patch
        .macros
        .iter()
        .any(|macro_patch| macro_patch.name == macro_name)
    {
        return false;
    }
    state.active_macro = Some(macro_name);
    state.selected_nodes.clear();
    state.hovered_node = None;
    state.drag = None;
    set_patcher_interaction_state(key, state);
    reset_patcher_pan(key);
    true
}

fn navigate_patcher_to_root(key: u64, state: &mut PatcherInteractionState) {
    state.active_macro = None;
    state.selected_nodes.clear();
    state.hovered_node = None;
    state.hover_back_button = false;
    state.drag = None;
    set_patcher_interaction_state(key, state.clone());
    reset_patcher_pan(key);
}

fn patcher_breadcrumb_rect(rect: Rect) -> Rect {
    Rect {
        row: rect.row,
        col: rect.col,
        width: rect.width,
        height: 2.0,
    }
}

fn patcher_back_button_rect(rect: Rect) -> Rect {
    Rect {
        row: rect.row + 0.45,
        col: rect.col + 0.9,
        width: 2.0,
        height: 1.4,
    }
}

fn reset_patcher_pan(key: u64) {
    let mut pan = get_patcher_pan_state(key);
    pan.offset_x = 0.0;
    pan.offset_y = 0.0;
    set_patcher_pan_state(key, pan);
}

impl WidgetDefinition for PatcherWidget {
    fn names(&self) -> &'static [&'static str] {
        &["patcher"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let width = if matches!(prop_keyword(node, "width").as_deref(), Some("fill")) {
            constraints.max_width
        } else {
            get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or(DEFAULT_WIDTH)
                .min(constraints.max_width.max(1.0))
        };
        let height = if matches!(prop_keyword(node, "height").as_deref(), Some("fill")) {
            constraints.max_height
        } else {
            get_prop_num(node, "height")
                .map(f64_to_f32)
                .unwrap_or(DEFAULT_HEIGHT)
        };
        Some(Size {
            width: width.max(1.0),
            height: height.max(1.0),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        for row_offset in 0..rect.height.round() as u16 {
            for col_offset in 0..rect.width.round() as u16 {
                let row = rect.row.round() as u16 + row_offset;
                let col = rect.col.round() as u16 + col_offset;
                buf.set(
                    row,
                    col,
                    styled_cell(' ', theme::FG(), Some(theme::BLACK())),
                );
            }
        }
        let title = match load_patch_from_props(props) {
            Ok((path, patch)) => format!(
                "patcher {}  nodes:{}  cables:{}",
                path.display(),
                patch.nodes.len(),
                patch.connections.len()
            ),
            Err(error) => format!("patcher error: {error}"),
        };
        for (idx, ch) in title.chars().take(rect.width as usize).enumerate() {
            buf.set(
                rect.row.round() as u16,
                rect.col.round() as u16 + idx as u16,
                styled_cell(ch, theme::GREEN(), Some(theme::BLACK())),
            );
        }
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        modifiers: KeyModifiers,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                handle_patcher_pointer_down(node, local_col, local_row, modifiers);
                MouseEventOutcome::Consume
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                handle_patcher_pointer_drag(node, local_col, local_row);
                MouseEventOutcome::Consume
            }
            MouseEventKind::Up(MouseButton::Left) => {
                handle_patcher_pointer_up(node, local_col, local_row);
                MouseEventOutcome::Consume
            }
            MouseEventKind::Moved => {
                handle_patcher_pointer_moved(node, local_col, local_row);
                MouseEventOutcome::Consume
            }
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {
                pan_patcher_by_wheel(node, mouse_kind);
                MouseEventOutcome::Consume
            }
            _ => MouseEventOutcome::Consume,
        }
    }

    fn scroll_gesture_event(
        &self,
        node: &LayoutNode,
        _local_col: f32,
        _local_row: f32,
        delta_x: f32,
        delta_y: f32,
    ) -> Option<WidgetEvent> {
        pan_patcher_by_delta(
            node,
            -delta_x * TOUCHPAD_PAN_SPEED_CELLS_PER_PIXEL,
            -delta_y * TOUCHPAD_PAN_SPEED_CELLS_PER_PIXEL,
        );
        Some(WidgetEvent::Custom(Value::Nil))
    }

    fn captures_scroll_gesture(&self) -> bool {
        true
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn unclamped_drag(&self) -> bool {
        true
    }

    fn double_click_event(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
    ) -> Option<WidgetEvent> {
        if handle_patcher_double_click(node, local_col, local_row) {
            Some(WidgetEvent::Custom(Value::Nil))
        } else {
            None
        }
    }

    fn handle_event(&self, _node: &LayoutNode, event: WidgetEvent) -> Option<super::EventOutput> {
        match event {
            WidgetEvent::Custom(Value::Nil) => None,
            _ => None,
        }
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let mut prims = Vec::new();
        prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
            rect: node.rect,
            color: crate::backend::Color::from_hex(0x02, 0x02, 0x03),
        }));
        let key = patcher_state_key(node);
        let mut pan_state = get_patcher_pan_state(key);
        pan_state.viewport_width = node.rect.width;
        pan_state.viewport_height = node.rect.height;

        let loaded = load_patch_from_props(&node.props);
        match loaded {
            Ok((path, root_patch)) => {
                let interaction_state = get_patcher_interaction_state(key);
                let view_key = active_patcher_view_key(&interaction_state);
                let patch = active_patcher_patch(&root_patch, &interaction_state);
                let patch = patch_with_interaction_positions(patch, &interaction_state, &view_key);
                let content_size = patch_content_size(&patch);
                pan_state.content_width = content_size.0.max(node.rect.width);
                pan_state.content_height = content_size.1.max(node.rect.height);
                set_patcher_pan_state(key, pan_state.clone());
                pan_state = get_patcher_pan_state(key);
                push_grid(
                    &mut prims,
                    node.rect,
                    pan_state.offset_x,
                    pan_state.offset_y,
                );
                draw_patch(
                    &mut prims,
                    &patch,
                    node.rect,
                    viewport,
                    &pan_state,
                    &interaction_state,
                );
                push_marquee(&mut prims, node.rect, viewport, &interaction_state);
                push_back_button(&mut prims, node.rect, viewport, &interaction_state);
                prims.push(MetalPrimitive::ProportionalText(
                    MetalProportionalTextPrimitive {
                        row: node.rect.row + 0.7,
                        col: if interaction_state.active_macro.is_some() {
                            node.rect.col + 3.2
                        } else {
                            node.rect.col + 1.0
                        },
                        align_width: if interaction_state.active_macro.is_some() {
                            node.rect.width - 4.2
                        } else {
                            node.rect.width - 2.0
                        },
                        h_align: 0.0,
                        text: format!("{}", patcher_breadcrumb(&path, &interaction_state)),
                        font_size: 12.0,
                        fg: crate::backend::Color::from_hex(0xa8, 0xac, 0xb8),
                        bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
                    },
                ));
                if !patch.diagnostics.is_empty() {
                    prims.push(MetalPrimitive::ProportionalText(
                        MetalProportionalTextPrimitive {
                            row: node.rect.row + node.rect.height - 1.7,
                            col: node.rect.col + 1.0,
                            align_width: node.rect.width - 2.0,
                            h_align: 0.0,
                            text: format!(
                                "{} unsupported form(s) rendered as code islands",
                                patch.diagnostics.len()
                            ),
                            font_size: 11.0,
                            fg: crate::backend::Color::from_hex(0xff, 0x6b, 0x73),
                            bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
                        },
                    ));
                }
            }
            Err(error) => {
                prims.push(MetalPrimitive::ProportionalText(
                    MetalProportionalTextPrimitive {
                        row: node.rect.row + 2.0,
                        col: node.rect.col + 2.0,
                        align_width: node.rect.width - 4.0,
                        h_align: 0.0,
                        text: error,
                        font_size: 13.0,
                        fg: crate::backend::Color::from_hex(0xff, 0x6b, 0x73),
                        bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
                    },
                ));
            }
        }
        prims
    }
}

fn prop_keyword(node: &Value, key: &str) -> Option<String> {
    let Value::Map(map) = node else {
        return None;
    };
    map.get(key).and_then(|value| match &*value.borrow() {
        Value::Keyword(value) | Value::String(value) => Some(value.clone()),
        _ => None,
    })
}

fn load_patch_from_props(props: &HashMap<String, Value>) -> Result<(PathBuf, Patch), String> {
    let path = prop_str(props, "path")
        .or_else(|| prop_str(props, "file"))
        .ok_or_else(|| "patcher requires :path".to_string())?;
    let path = PathBuf::from(path);
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    let intent = match props.get("intent") {
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "effect" => {
            PatcherIntent::Effect
        }
        _ => PatcherIntent::Instrument,
    };
    let patch = parse_patch_source(&source, intent)?;
    Ok((path, patch))
}

fn prop_str(props: &HashMap<String, Value>, key: &str) -> Option<String> {
    props.get(key).and_then(|value| match value {
        Value::String(value) | Value::Keyword(value) | Value::Symbol(value) => Some(value.clone()),
        _ => None,
    })
}

#[cfg(target_os = "macos")]
fn push_grid(prims: &mut Vec<MetalPrimitive>, rect: Rect, offset_x: f32, offset_y: f32) {
    let minor = crate::backend::Color::rgba(0.13, 0.14, 0.16, 0.34);
    let major = crate::backend::Color::rgba(0.22, 0.23, 0.27, 0.46);
    let col_spacing = 4.0;
    let row_spacing = 2.5;
    let col_phase = offset_x.rem_euclid(col_spacing);
    let row_phase = offset_y.rem_euclid(row_spacing);
    let mut col = rect.col - col_phase;
    let mut idx = (offset_x / col_spacing).floor().max(0.0) as usize;
    while col < rect.col + rect.width {
        prims.push(MetalPrimitive::Quad(MetalQuadPrimitive {
            x: col,
            y: rect.row,
            width: 0.035,
            height: rect.height,
            color: if idx % 5 == 0 { major } else { minor },
        }));
        col += col_spacing;
        idx += 1;
    }
    let mut row = rect.row - row_phase;
    idx = (offset_y / row_spacing).floor().max(0.0) as usize;
    while row < rect.row + rect.height {
        prims.push(MetalPrimitive::Quad(MetalQuadPrimitive {
            x: rect.col,
            y: row,
            width: rect.width,
            height: 0.035,
            color: if idx % 5 == 0 { major } else { minor },
        }));
        row += row_spacing;
        idx += 1;
    }
}

#[cfg(target_os = "macos")]
fn push_marquee(
    prims: &mut Vec<MetalPrimitive>,
    _rect: Rect,
    _viewport: WidgetViewport,
    interaction_state: &PatcherInteractionState,
) {
    let Some(PatcherDragState::Marquee {
        start_col,
        start_row,
        current_col,
        current_row,
        ..
    }) = &interaction_state.drag
    else {
        return;
    };
    let marquee = rect_from_points(*start_col, *start_row, *current_col, *current_row);
    if marquee.width < 0.05 || marquee.height < 0.05 {
        return;
    }
    let fill = crate::backend::Color::rgba(0.22, 0.48, 1.0, 0.12);
    let border = crate::backend::Color::rgba(0.38, 0.62, 1.0, 0.72);
    prims.push(MetalPrimitive::Quad(MetalQuadPrimitive {
        x: marquee.col,
        y: marquee.row,
        width: marquee.width,
        height: marquee.height,
        color: fill,
    }));
    let thickness = 0.08;
    prims.push(MetalPrimitive::Quad(MetalQuadPrimitive {
        x: marquee.col,
        y: marquee.row,
        width: marquee.width,
        height: thickness,
        color: border,
    }));
    prims.push(MetalPrimitive::Quad(MetalQuadPrimitive {
        x: marquee.col,
        y: marquee.row + marquee.height - thickness,
        width: marquee.width,
        height: thickness,
        color: border,
    }));
    prims.push(MetalPrimitive::Quad(MetalQuadPrimitive {
        x: marquee.col,
        y: marquee.row,
        width: thickness,
        height: marquee.height,
        color: border,
    }));
    prims.push(MetalPrimitive::Quad(MetalQuadPrimitive {
        x: marquee.col + marquee.width - thickness,
        y: marquee.row,
        width: thickness,
        height: marquee.height,
        color: border,
    }));
}

#[cfg(target_os = "macos")]
fn push_back_button(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    viewport: WidgetViewport,
    interaction_state: &PatcherInteractionState,
) {
    let Some(label) = patcher_back_label(interaction_state) else {
        return;
    };
    let button_rect = patcher_back_button_rect(rect);
    let border = if interaction_state.hover_back_button {
        crate::backend::Color::from_hex(0x6d, 0xae, 0xff)
    } else {
        crate::backend::Color::from_hex(0x44, 0x45, 0x50)
    };
    let bg = if interaction_state.hover_back_button {
        crate::backend::Color::from_hex(0x1e, 0x25, 0x36)
    } else {
        crate::backend::Color::from_hex(0x14, 0x15, 0x1a)
    };
    push_rounded_rect(prims, button_rect, border, viewport, 9.0, false);
    push_rounded_rect(
        prims,
        Rect {
            row: button_rect.row + 0.08,
            col: button_rect.col + 0.08,
            width: (button_rect.width - 0.16).max(0.0),
            height: (button_rect.height - 0.16).max(0.0),
        },
        bg,
        viewport,
        7.0,
        false,
    );
    prims.push(MetalPrimitive::ProportionalText(
        MetalProportionalTextPrimitive {
            row: button_rect.row + 0.24,
            col: button_rect.col + 0.7,
            align_width: button_rect.width - 1.1,
            h_align: 0.0,
            text: label.to_string(),
            font_size: 11.0,
            fg: if interaction_state.hover_back_button {
                crate::backend::Color::from_hex(0xd7, 0xe6, 0xff)
            } else {
                crate::backend::Color::from_hex(0xa8, 0xac, 0xb8)
            },
            bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
        },
    ));
}

#[cfg(target_os = "macos")]
fn draw_patch(
    prims: &mut Vec<MetalPrimitive>,
    patch: &Patch,
    rect: Rect,
    viewport: WidgetViewport,
    pan_state: &PatcherPanState,
    interaction_state: &PatcherInteractionState,
) {
    let node_rects = patch_node_rects(patch, rect, pan_state);
    let input_indices = patch_input_indices(patch);
    let output_counts = patch_output_counts(patch);

    for connection in &patch.connections {
        let (Some(from), Some(to)) = (
            node_rects.get(&connection.from_node),
            node_rects.get(&connection.to_node),
        ) else {
            continue;
        };
        let start = port_center(
            *from,
            connection.from_output,
            output_counts
                .get(&connection.from_node)
                .copied()
                .unwrap_or(1),
            false,
        );
        let to_indices = input_indices.get(&connection.to_node);
        let visible_input = to_indices
            .and_then(|indices| {
                indices
                    .iter()
                    .position(|input| *input == connection.to_input)
            })
            .unwrap_or(0);
        let end = port_center(
            *to,
            visible_input,
            to_indices.map(|indices| indices.len()).unwrap_or(1),
            true,
        );
        push_cable(prims, start, end, connection.kind);
    }

    for node in &patch.nodes {
        let Some(rect) = node_rects.get(&node.id).copied() else {
            continue;
        };
        push_node(
            prims,
            node,
            rect,
            input_indices
                .get(&node.id)
                .map(|indices| indices.len())
                .unwrap_or(0),
            output_counts.get(&node.id).copied().unwrap_or(0),
            viewport,
            interaction_state.selected_nodes.contains(&node.id),
            interaction_state.hovered_node.as_deref() == Some(node.id.as_str()),
        );
    }
}

fn patch_input_indices(patch: &Patch) -> HashMap<String, Vec<usize>> {
    let mut indices: HashMap<String, Vec<usize>> = HashMap::new();
    for connection in &patch.connections {
        let node_indices = indices.entry(connection.to_node.clone()).or_default();
        if !node_indices.contains(&connection.to_input) {
            node_indices.push(connection.to_input);
        }
    }
    for node_indices in indices.values_mut() {
        node_indices.sort_unstable();
    }
    indices
}

fn node_display_label(node: &PatchNode) -> String {
    let base = match node.kind {
        NodeKind::Builtin | NodeKind::MacroInstance | NodeKind::In | NodeKind::Out => {
            node.op.as_str()
        }
        _ => node.label.as_str(),
    };
    let mut label = base.to_string();
    for arg in &node.args {
        if let ArgValue::Literal(value) = arg {
            if value == "<expr>" {
                continue;
            }
            label.push(' ');
            label.push_str(value);
        }
    }
    label
}

#[cfg(target_os = "macos")]
fn patch_output_counts(patch: &Patch) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for node in &patch.nodes {
        if !node.outputs.is_empty() {
            counts.insert(node.id.clone(), node.outputs.len());
        }
    }
    for connection in &patch.connections {
        let needed = connection.from_output + 1;
        counts
            .entry(connection.from_node.clone())
            .and_modify(|count| *count = (*count).max(needed))
            .or_insert(needed);
    }
    counts
}

#[cfg(target_os = "macos")]
fn port_center(rect: Rect, index: usize, count: usize, top: bool) -> (f32, f32) {
    let count = count.max(1);
    let usable = (rect.width - PORT_EDGE_PADDING_CELLS * 2.0).max(0.0);
    let x = if count == 1 {
        rect.col + PORT_EDGE_PADDING_CELLS.min(rect.width * 0.5)
    } else {
        rect.col
            + PORT_EDGE_PADDING_CELLS
            + usable * (index.min(count - 1) as f32) / ((count - 1) as f32)
    };
    let y = if top {
        rect.row
    } else {
        rect.row + rect.height
    };
    (x, y)
}

fn node_size(node: &PatchNode) -> (f32, f32) {
    let char_width = if node.kind == NodeKind::CodeIsland {
        0.52
    } else {
        1.16
    };
    let horizontal_padding = if node.kind == NodeKind::CodeIsland {
        2.0
    } else {
        2.65
    };
    let label = node_display_label(node);
    let text_width = label.chars().count() as f32 * char_width + horizontal_padding;
    if node.kind == NodeKind::CodeIsland {
        (
            text_width.max(CODE_NODE_MIN_WIDTH).min(34.0),
            CODE_NODE_HEIGHT,
        )
    } else {
        (text_width.max(NODE_MIN_WIDTH).min(96.0), NODE_HEIGHT)
    }
}

#[cfg(target_os = "macos")]
fn push_cable(
    prims: &mut Vec<MetalPrimitive>,
    start: (f32, f32),
    end: (f32, f32),
    kind: ConnectionKind,
) {
    let color = match kind {
        ConnectionKind::Forward => crate::backend::Color::rgba(0.74, 0.75, 0.84, 0.92),
        ConnectionKind::Feedback => crate::backend::Color::rgba(1.0, 0.59, 0.04, 0.88),
    };
    let curve = super::cable::cable_curve(start, end);
    prims.push(MetalPrimitive::PatchCable(MetalPatchCablePrimitive {
        start: [curve.p0.0, curve.p0.1],
        control1: [curve.p1.0, curve.p1.1],
        control2: [curve.p2.0, curve.p2.1],
        end: [curve.p3.0, curve.p3.1],
        radius_px: if kind == ConnectionKind::Feedback {
            2.25
        } else {
            2.65
        },
        color,
    }));
}

#[cfg(target_os = "macos")]
fn push_node(
    prims: &mut Vec<MetalPrimitive>,
    node: &PatchNode,
    rect: Rect,
    input_count: usize,
    output_count: usize,
    viewport: WidgetViewport,
    selected: bool,
    hovered: bool,
) {
    let (bg, mut border, text) = match node.kind {
        NodeKind::In | NodeKind::Out => (
            crate::backend::Color::from_hex(0x18, 0x19, 0x1e),
            crate::backend::Color::from_hex(0x44, 0x45, 0x50),
            crate::backend::Color::from_hex(0x4c, 0xe0, 0x72),
        ),
        NodeKind::Param => (
            crate::backend::Color::from_hex(0x19, 0x19, 0x22),
            crate::backend::Color::from_hex(0x3b, 0x69, 0xb1),
            crate::backend::Color::from_hex(0x6d, 0xae, 0xff),
        ),
        NodeKind::CodeIsland => (
            crate::backend::Color::from_hex(0x24, 0x16, 0x18),
            crate::backend::Color::from_hex(0xff, 0x5a, 0x65),
            crate::backend::Color::from_hex(0xff, 0x8a, 0x92),
        ),
        _ => (
            crate::backend::Color::from_hex(0x16, 0x16, 0x1a),
            if node.diagnostic.is_some() {
                crate::backend::Color::from_hex(0xff, 0x5a, 0x65)
            } else {
                crate::backend::Color::from_hex(0x40, 0x40, 0x4a)
            },
            crate::backend::Color::from_hex(0x4c, 0xe0, 0x72),
        ),
    };
    if hovered {
        border = crate::backend::Color::from_hex(0x78, 0x7c, 0x8e);
    }
    if selected {
        border = crate::backend::Color::from_hex(0x4a, 0x8d, 0xff);
    }
    push_rounded_rect(prims, rect, border, viewport, NODE_CORNER_RADIUS_PX, false);
    push_rounded_rect(
        prims,
        Rect {
            row: rect.row + NODE_BORDER_INSET,
            col: rect.col + NODE_BORDER_INSET,
            width: (rect.width - NODE_BORDER_INSET * 2.0).max(0.0),
            height: (rect.height - NODE_BORDER_INSET * 2.0).max(0.0),
        },
        bg,
        viewport,
        (NODE_CORNER_RADIUS_PX - 3.0).max(0.0),
        false,
    );
    for index in 0..input_count {
        push_port(prims, port_center(rect, index, input_count, true), true, bg);
    }
    for index in 0..output_count {
        push_port(
            prims,
            port_center(rect, index, output_count, false),
            false,
            bg,
        );
    }
    push_node_label(prims, node, rect, text);
    if let Some(diagnostic) = &node.diagnostic {
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: rect.row + 2.65,
                col: rect.col + 1.0,
                align_width: rect.width - 2.0,
                h_align: 0.0,
                text: preview(diagnostic, 32),
                font_size: 9.5,
                fg: crate::backend::Color::from_hex(0xff, 0xa3, 0xa8),
                bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
            },
        ));
    }
}

#[cfg(target_os = "macos")]
fn push_node_label(
    prims: &mut Vec<MetalPrimitive>,
    node: &PatchNode,
    rect: Rect,
    head_color: crate::backend::Color,
) {
    let font_size = if node.kind == NodeKind::CodeIsland {
        CODE_NODE_FONT_SIZE
    } else {
        NODE_FONT_SIZE
    };
    let baseline_row = if node.kind == NodeKind::CodeIsland {
        rect.row + 0.55
    } else {
        rect.row + 0.36
    };
    let text_col = rect.col + 0.92;
    let label = node_display_label(node);
    let (head, tail) = split_label_head_tail(&label);
    let bg = crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0);
    prims.push(MetalPrimitive::ProportionalText(
        MetalProportionalTextPrimitive {
            row: baseline_row,
            col: text_col,
            align_width: rect.width - 1.84,
            h_align: 0.0,
            text: head.to_string(),
            font_size,
            fg: head_color,
            bg,
        },
    ));
    if !tail.is_empty() {
        let tail_col = text_col + estimated_text_cells(head, font_size) + 0.72;
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: baseline_row,
                col: tail_col,
                align_width: (rect.col + rect.width - tail_col - 0.92).max(0.0),
                h_align: 0.0,
                text: tail.to_string(),
                font_size,
                fg: crate::backend::Color::from_hex(0xf2, 0xf2, 0xf4),
                bg,
            },
        ));
    }
}

#[cfg(target_os = "macos")]
fn split_label_head_tail(label: &str) -> (&str, &str) {
    let trimmed = label.trim();
    match trimmed.find(char::is_whitespace) {
        Some(idx) => {
            let (head, tail) = trimmed.split_at(idx);
            (head, tail.trim_start())
        }
        None => (trimmed, ""),
    }
}

#[cfg(target_os = "macos")]
fn estimated_text_cells(text: &str, font_size: f32) -> f32 {
    let width_per_char = (font_size / NODE_FONT_SIZE) * 1.03;
    text.chars().count() as f32 * width_per_char
}

#[cfg(target_os = "macos")]
fn push_port(
    prims: &mut Vec<MetalPrimitive>,
    center: (f32, f32),
    input: bool,
    node_bg: crate::backend::Color,
) {
    let color = if input {
        crate::backend::Color::from_hex(0xff, 0xee, 0x00)
    } else {
        crate::backend::Color::from_hex(0xff, 0x9f, 0x0a)
    };
    let visible_half = if input {
        MetalCircleVisibleHalf::Bottom
    } else {
        MetalCircleVisibleHalf::Top
    };
    prims.push(MetalPrimitive::Circle(MetalCirclePrimitive {
        center: [center.0, center.1],
        radius_px: PORT_OUTER_DIAMETER_PX * 0.5,
        color,
        visible_half,
    }));
    prims.push(MetalPrimitive::Circle(MetalCirclePrimitive {
        center: [center.0, center.1],
        radius_px: PORT_INNER_DIAMETER_PX * 0.5,
        color: node_bg,
        visible_half,
    }));
}

#[cfg(target_os = "macos")]
fn normalized_corner_radius(rect: Rect, viewport: WidgetViewport, radius_px: f32) -> f32 {
    if radius_px <= 0.0 {
        return 0.001;
    }
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((radius_px * 2.0) / px_h).clamp(0.001, 0.5)
}

#[cfg(target_os = "macos")]
fn push_rounded_rect(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    color: crate::backend::Color,
    viewport: WidgetViewport,
    radius_px: f32,
    is_background: bool,
) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    prims.push(MetalPrimitive::WidgetInstance {
        widget_type: "box".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [0.0; 4],
            uniform_b: [0.0; 4],
            color_a: color.to_rgba(),
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: normalized_corner_radius(rect, viewport, radius_px),
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
        is_background,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Patch {
        parse_patch_source(source, PatcherIntent::Instrument).unwrap()
    }

    #[test]
    fn projects_instrument_plumbing_and_nested_calls() {
        let patch = parse(
            r#"
            (def pitch (in 1 @name pitch))
            (def sig (triangle (phasor pitch)))
            (out sig 1 @name audio)
            "#,
        );
        assert!(
            patch
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::In && node.id == "pitch")
        );
        assert!(patch.nodes.iter().any(|node| node.op == "phasor"));
        assert!(patch.nodes.iter().any(|node| node.op == "triangle"));
        assert!(
            patch
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::Out && node.id == "audio")
        );
        assert!(patch.connections.len() >= 3, "{:#?}", patch.connections);
    }

    #[test]
    fn operator_metadata_comes_from_generated_dgenlisp_json() {
        let names = dgenlisp_operator_names();
        assert!(names.len() >= 100, "expected generated operator metadata");
        assert!(names.contains("phasor"));
        assert!(names.contains("spectrum-delay"));
        assert!(names.contains("tosignal"));
    }

    #[test]
    fn projects_params_and_attributes_as_param_node() {
        let patch = parse("(param cutoff @default 800 @min 20 @max 12000)");
        let node = patch.nodes.iter().find(|node| node.id == "cutoff").unwrap();
        assert_eq!(node.kind, NodeKind::Param);
        assert_eq!(
            node_display_label(node),
            "param cutoff @default 800 @min 20 @max 12000"
        );
    }

    #[test]
    fn param_references_project_as_connections_not_literal_args() {
        let patch = parse(
            r#"
            (param size @min 0 @max 3000 @default 300)
            (def input (in 1))
            (def delayed (delay input size))
            "#,
        );
        let param = patch
            .nodes
            .iter()
            .find(|node| node.kind == NodeKind::Param)
            .unwrap();
        let delay = patch.nodes.iter().find(|node| node.op == "delay").unwrap();

        assert_eq!(
            node_display_label(param),
            "param size @min 0 @max 3000 @default 300"
        );
        assert_eq!(node_display_label(delay), "delay");
        assert!(
            patch.connections.iter().any(|connection| {
                connection.from_node == param.id
                    && connection.to_node == delay.id
                    && connection.to_input == 1
            }),
            "{:#?}",
            patch.connections
        );
    }

    #[test]
    fn projects_destructuring_def_outputs() {
        let patch = parse("(def (re im) (fft input))");
        let node = patch.nodes.iter().find(|node| node.op == "fft").unwrap();
        assert_eq!(node.outputs, vec!["re".to_string(), "im".to_string()]);
    }

    #[test]
    fn collapses_history_read_and_write_into_make_history_node() {
        let patch = parse(
            r#"
            (make-history h)
            (def sig (noise))
            (def previous (read-history h))
            (def mixed (+ sig previous))
            (write-history h sig)
            "#,
        );
        let history = patch
            .nodes
            .iter()
            .find(|node| node.kind == NodeKind::History)
            .expect("history node");
        assert_eq!(node_display_label(history), "history");
        assert_eq!(
            patch
                .nodes
                .iter()
                .filter(|node| matches!(
                    node.op.as_str(),
                    "make-history" | "read-history" | "write-history"
                ))
                .count(),
            1,
            "{:#?}",
            patch.nodes
        );
        assert!(
            patch
                .connections
                .iter()
                .any(|connection| connection.from_node == history.id
                    && connection.kind == ConnectionKind::Forward)
        );
        assert!(
            patch
                .connections
                .iter()
                .any(|connection| connection.to_node == history.id
                    && connection.to_input == 0
                    && connection.kind == ConnectionKind::Feedback)
        );
    }

    #[test]
    fn unsupported_forms_become_code_islands() {
        let patch = parse("(if gate (out 1 1 @name audio) (out 0 1 @name audio))");
        assert!(
            patch
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::CodeIsland)
        );
        assert!(!patch.diagnostics.is_empty());
    }

    #[test]
    fn literal_args_are_inlined_and_do_not_create_visible_ports() {
        let patch = parse(
            r#"
            (def signal (in 1 @name signal))
            (out (* signal 3) 1 @name audio)
            "#,
        );
        let multiply = patch
            .nodes
            .iter()
            .find(|node| node.op == "*")
            .expect("anonymous multiply node");

        assert_eq!(node_display_label(multiply), "* 3");

        let input_indices = patch_input_indices(&patch);
        assert_eq!(
            input_indices.get(&multiply.id).map(Vec::as_slice),
            Some(&[0][..])
        );
    }

    #[test]
    fn display_labels_omit_def_names_and_show_in_out_channels() {
        let patch = parse(
            r#"
            (def signal (in 1 @name pitch))
            (def scaled (* signal 3))
            (out scaled 1 @name audio)
            "#,
        );
        let input = patch
            .nodes
            .iter()
            .find(|node| node.kind == NodeKind::In)
            .unwrap();
        let multiply = patch.nodes.iter().find(|node| node.op == "*").unwrap();
        let output = patch
            .nodes
            .iter()
            .find(|node| node.kind == NodeKind::Out)
            .unwrap();

        assert_eq!(node_display_label(input), "in 1");
        assert_eq!(node_display_label(multiply), "* 3");
        assert_eq!(node_display_label(output), "out 1");
    }

    #[test]
    fn interaction_positions_override_auto_layout_positions() {
        let patch = parse(
            r#"
            (def pitch (in 1 @name pitch))
            (def sig (phasor pitch))
            "#,
        );
        let pitch = patch.nodes.iter().find(|node| node.id == "pitch").unwrap();
        assert_ne!(pitch.position, (22.0, 7.0));

        let mut state = PatcherInteractionState::default();
        state
            .node_positions
            .insert(scoped_node_position_key("root", "pitch"), (22.0, 7.0));
        let patch = patch_with_interaction_positions(patch, &state, "root");
        let pitch = patch.nodes.iter().find(|node| node.id == "pitch").unwrap();
        assert_eq!(pitch.position, (22.0, 7.0));
    }

    #[test]
    fn patcher_hit_testing_uses_node_rects_after_pan() {
        let patch = parse("(def pitch (in 1 @name pitch))");
        let pan = PatcherPanState::default();
        let rect = Rect {
            row: 0.0,
            col: 0.0,
            width: 80.0,
            height: 30.0,
        };
        let hit = hit_patcher_node(&patch, rect, &pan, 7.0, 6.8);
        assert_eq!(hit.as_deref(), Some("pitch"));
    }

    #[test]
    fn pan_state_allows_overscroll_and_clamps_to_finite_canvas_bounds() {
        let mut state = PatcherPanState {
            offset_x: 100.0,
            offset_y: 100.0,
            content_width: 50.0,
            content_height: 30.0,
            viewport_width: 20.0,
            viewport_height: 10.0,
        };
        clamp_patcher_pan_state(&mut state);
        assert_eq!(state.offset_x, 78.0);
        assert_eq!(state.offset_y, 68.0);

        state.offset_x = -200.0;
        state.offset_y = -200.0;
        clamp_patcher_pan_state(&mut state);
        assert_eq!(state.offset_x, -48.0);
        assert_eq!(state.offset_y, -48.0);
    }

    #[test]
    fn touchpad_horizontal_pan_matches_canvas_drag_direction() {
        let node = LayoutNode {
            widget_id: 987_655,
            stable_widget_id: Some(987_655),
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "patcher".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 20.0,
                height: 10.0,
            },
            props: HashMap::new(),
            children: Vec::new(),
            focusable: true,
        };
        let key = patcher_state_key(&node);

        PATCHER_WIDGET.scroll_gesture_event(&node, 10.0, 5.0, 100.0, 0.0);

        let state = get_patcher_pan_state(key);
        assert!(
            state.offset_x < 0.0,
            "positive horizontal gesture delta should move canvas right, got {}",
            state.offset_x
        );
    }

    #[test]
    fn defmacro_becomes_read_only_subpatch() {
        let patch = parse(
            r#"
            (defmacro ap (x)
              (def y (allpass x 100 0.5))
              y)
            (def z (ap input))
            "#,
        );
        assert_eq!(patch.macros.len(), 1);
        assert_eq!(patch.macros[0].name, "ap");
        let macro_patch = &patch.macros[0].patch;
        assert!(
            macro_patch
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::In && node_display_label(node) == "in 1"),
            "{:#?}",
            macro_patch.nodes
        );
        assert!(
            macro_patch
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::Out && node_display_label(node) == "out 1"),
            "{:#?}",
            macro_patch.nodes
        );
        assert!(
            patch
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::MacroInstance)
        );
        assert!(
            !patch
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::MacroDefinition),
            "{:#?}",
            patch.nodes
        );
    }

    #[test]
    fn double_clicking_macro_instance_opens_macro_view_and_breadcrumb_returns_to_root() {
        let source = r#"
            (defmacro ap (x)
              (def y (allpass x 100 0.5)))
            (def z (ap input))
        "#;
        let path = std::env::temp_dir().join(format!(
            "eseqlisp-patcher-macro-nav-{}.lisp",
            std::process::id()
        ));
        std::fs::write(&path, source).unwrap();
        let root_patch = parse_patch_source(source, PatcherIntent::Effect).unwrap();
        let macro_node = root_patch
            .nodes
            .iter()
            .find(|node| node.kind == NodeKind::MacroInstance)
            .unwrap();

        let mut props = HashMap::new();
        props.insert(
            "path".to_string(),
            Value::String(path.to_string_lossy().to_string()),
        );
        let node = LayoutNode {
            widget_id: 112_233,
            stable_widget_id: Some(112_233),
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "patcher".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 80.0,
                height: 30.0,
            },
            props,
            children: Vec::new(),
            focusable: true,
        };
        let key = patcher_state_key(&node);
        set_patcher_interaction_state(key, PatcherInteractionState::default());

        let rects = patch_node_rects(&root_patch, node.rect, &PatcherPanState::default());
        let macro_rect = rects.get(&macro_node.id).unwrap();
        assert!(handle_patcher_double_click(
            &node,
            macro_rect.col + macro_rect.width * 0.5,
            macro_rect.row + macro_rect.height * 0.5
        ));
        assert_eq!(
            get_patcher_interaction_state(key).active_macro.as_deref(),
            Some("ap")
        );

        let mut state = get_patcher_interaction_state(key);
        state.active_macro = Some("ap".to_string());
        set_patcher_interaction_state(key, state);
        handle_patcher_pointer_moved(&node, 1.2, 0.8);
        assert!(get_patcher_interaction_state(key).hover_back_button);

        handle_patcher_pointer_down(&node, 1.2, 0.8, KeyModifiers::empty());
        assert_eq!(get_patcher_interaction_state(key).active_macro, None);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn layout_assigns_finite_nonzero_node_positions() {
        let patch = parse(
            r#"
            (def pitch (in 1 @name pitch))
            (def sig (triangle (phasor pitch)))
            (out sig 1 @name audio)
            "#,
        );
        for node in &patch.nodes {
            assert!(node.position.0.is_finite());
            assert!(node.position.1.is_finite());
            assert!(node.position.0 >= 0.0);
            assert!(node.position.1 >= 0.0);
        }
    }

    #[test]
    fn fixture_videogame_arp_projects_without_parse_failure() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../sequencer/instruments/arcade/videogame-arp/dsp.lisp");
        let source = std::fs::read_to_string(path).unwrap();
        let patch = parse_patch_source(&source, PatcherIntent::Instrument).unwrap();
        assert!(!patch.nodes.is_empty());
    }

    #[test]
    fn fixture_lexilush_projects_without_parse_failure() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../sequencer/effects/lexilush/dsp.lisp");
        let source = std::fs::read_to_string(path).unwrap();
        let patch = parse_patch_source(&source, PatcherIntent::Effect).unwrap();
        assert!(!patch.nodes.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn metal_render_emits_nodes_and_cables() {
        let patch = parse(
            r#"
            (def pitch (in 1 @name pitch))
            (def sig (triangle (phasor pitch)))
            (out sig 1 @name audio)
            "#,
        );
        let mut prims = Vec::new();
        draw_patch(
            &mut prims,
            &patch,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 100.0,
                height: 40.0,
            },
            WidgetViewport {
                cell_w: 10.0,
                cell_h: 20.0,
                vp_w: 1000.0,
                vp_h: 800.0,
                time_seconds: 0.0,
                focused_widget_id: None,
                focused_branch: false,
                tile_content_rows: 40.0,
                scroll_top: 0.0,
                scroll_left: 0.0,
                inherited_hover: false,
            },
            &PatcherPanState::default(),
            &PatcherInteractionState::default(),
        );
        let text_count = prims
            .iter()
            .filter(|prim| matches!(prim, MetalPrimitive::ProportionalText(_)))
            .count();
        let rect_count = prims
            .iter()
            .filter(|prim| matches!(prim, MetalPrimitive::Rect(_)))
            .count();
        let rounded_count = prims
            .iter()
            .filter(|prim| matches!(prim, MetalPrimitive::WidgetInstance { .. }))
            .count();
        let cable_count = prims
            .iter()
            .filter(|prim| matches!(prim, MetalPrimitive::PatchCable(_)))
            .count();
        assert!(text_count >= patch.nodes.len(), "{text_count}");
        assert!(rounded_count >= patch.nodes.len() * 2, "{rounded_count}");
        assert!(cable_count >= patch.connections.len(), "{cable_count}");
        assert!(
            rect_count == 0,
            "patcher node chrome should use rounded widget instances, got {rect_count} rects"
        );
    }
}
