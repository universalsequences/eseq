use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::parser::{Expression, format_expression};

use super::display::{node_size, preview};
use super::lisp::{
    attribute_value, call_input_args, connection_kind_for_op, default_outputs,
    format_patch_literal, is_numeric_literal, is_unsupported_call_head, node_kind_for_op,
    node_label, positional_args, symbol_at,
};
use super::metrics::{LAYER_SPACING, NODE_COLUMN_GAP, VIEW_PADDING_X, VIEW_PADDING_Y};
use super::model::{
    ArgValue, ConnectionKind, MacroPatch, NodeKind, OperatorPortShape, Patch, PatchConnection,
    PatchNode,
};

pub(super) struct Projector {
    patch: Patch,
    symbol_sources: HashMap<String, (String, usize)>,
    history_nodes: HashMap<String, String>,
    op_occurrences: HashMap<String, usize>,
    used_ids: HashSet<String>,
    known_ops: &'static HashSet<String>,
    known_macros: HashSet<String>,
}

impl Projector {
    pub(super) fn new(known_macros: HashSet<String>) -> Self {
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

    pub(super) fn project(mut self, exprs: &[Expression]) -> Patch {
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
            Expression::Symbol(name) => {
                if let Some(source) = self.symbol_sources.get(name).cloned() {
                    Some(source)
                } else if dgenlisp_constant_names().contains(name) {
                    Some((self.add_constant_node(name), 0))
                } else {
                    None
                }
            }
            Expression::Number(_) => Some((self.add_constant_node(&format_patch_literal(expr)), 0)),
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

        let input_args = call_input_args(&op, items);
        let mut arg_slots = vec![None; input_args.len()];
        let mut pending_constants = Vec::new();
        for (idx, arg) in input_args.into_iter().enumerate() {
            match arg {
                Expression::Symbol(name) => {
                    if let Some((from_node, from_output)) = self.symbol_sources.get(name).cloned() {
                        self.flush_pending_constant_args(
                            &mut node,
                            &mut arg_slots,
                            &mut pending_constants,
                        );
                        self.patch.connections.push(PatchConnection {
                            from_node,
                            from_output,
                            to_node: node.id.clone(),
                            to_input: idx,
                            kind: connection_kind_for_op(&op),
                        });
                        arg_slots[idx] = Some(ArgValue::SymbolRef(name.clone()));
                    } else if dgenlisp_constant_names().contains(name) {
                        pending_constants.push((idx, name.clone()));
                    } else {
                        arg_slots[idx] = Some(ArgValue::Literal(name.clone()));
                    }
                }
                Expression::List(nested) => {
                    if let Some((from_node, from_output)) =
                        self.project_value(&Expression::List(nested.clone()), None, None)
                    {
                        self.flush_pending_constant_args(
                            &mut node,
                            &mut arg_slots,
                            &mut pending_constants,
                        );
                        self.patch.connections.push(PatchConnection {
                            from_node,
                            from_output,
                            to_node: node.id.clone(),
                            to_input: idx,
                            kind: connection_kind_for_op(&op),
                        });
                        arg_slots[idx] = Some(ArgValue::ConnectedExpr);
                    } else {
                        arg_slots[idx] = Some(ArgValue::Literal("<expr>".to_string()));
                    }
                }
                other => {
                    let value = format_patch_literal(other);
                    if is_numeric_literal(&value) {
                        pending_constants.push((idx, value));
                    } else {
                        arg_slots[idx] = Some(ArgValue::Literal(value));
                    }
                }
            }
        }
        for (idx, value) in pending_constants {
            arg_slots[idx] = Some(ArgValue::Literal(value));
        }
        node.args = arg_slots
            .into_iter()
            .map(|arg| arg.unwrap_or_else(|| ArgValue::Literal("<expr>".to_string())))
            .collect();
        let id = node.id.clone();
        self.patch.nodes.push(node);
        Some(id)
    }

    fn flush_pending_constant_args(
        &mut self,
        node: &mut PatchNode,
        arg_slots: &mut [Option<ArgValue>],
        pending_constants: &mut Vec<(usize, String)>,
    ) {
        for (idx, value) in pending_constants.drain(..) {
            let constant_id = self.add_constant_node(&value);
            self.patch.connections.push(PatchConnection {
                from_node: constant_id,
                from_output: 0,
                to_node: node.id.clone(),
                to_input: idx,
                kind: connection_kind_for_op(&node.op),
            });
            arg_slots[idx] = Some(ArgValue::ConnectedExpr);
        }
    }

    fn add_constant_node(&mut self, value: &str) -> String {
        let id = self.unique_id(value);
        self.patch.nodes.push(PatchNode {
            id: id.clone(),
            op: value.to_string(),
            kind: NodeKind::Constant,
            label: value.to_string(),
            args: Vec::new(),
            outputs: vec!["out".to_string()],
            position: (0.0, 0.0),
            diagnostic: None,
        });
        id
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
                | NodeKind::Constant
                | NodeKind::History
        ) || self.known_ops.contains(op)
        {
            None
        } else {
            Some(format!("unknown DGenLisp operator `{op}`"))
        }
    }
}

pub(super) fn assign_layout(patch: &mut Patch) {
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

pub(super) fn dgenlisp_constant_names() -> &'static HashSet<String> {
    static CONSTANT_NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    CONSTANT_NAMES.get_or_init(|| {
        let metadata: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../sequencer/tools/dgenlisp-operators.json"
        ))
        .expect("bundled dgenlisp-operators.json must be valid JSON");
        let constants = metadata
            .get("constants")
            .and_then(serde_json::Value::as_array)
            .expect("bundled dgenlisp-operators.json must contain a constants array");

        constants
            .iter()
            .filter_map(|constant| constant.get("name").and_then(serde_json::Value::as_str))
            .map(str::to_string)
            .collect()
    })
}

pub(super) fn dgenlisp_operator_names() -> &'static HashSet<String> {
    static OPERATOR_NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    OPERATOR_NAMES.get_or_init(|| {
        let metadata: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../sequencer/tools/dgenlisp-operators.json"
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

pub(super) fn dgenlisp_operator_port_shapes() -> &'static HashMap<String, OperatorPortShape> {
    static OPERATOR_PORT_SHAPES: OnceLock<HashMap<String, OperatorPortShape>> = OnceLock::new();
    OPERATOR_PORT_SHAPES.get_or_init(|| {
        let metadata: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../sequencer/tools/dgenlisp-operators.json"
        ))
        .expect("bundled dgenlisp-operators.json must be valid JSON");
        let operators = metadata
            .get("operators")
            .and_then(serde_json::Value::as_array)
            .expect("bundled dgenlisp-operators.json must contain an operators array");
        let mut shapes = HashMap::new();
        for operator in operators {
            let Some(name) = operator.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let shape = OperatorPortShape {
                input_count: documented_port_count(operator, "inputs", "input_count"),
                output_count: documented_port_count(operator, "outputs", "output_count"),
            };
            shapes.insert(name.to_string(), shape);
            if let Some(aliases) = operator
                .get("aliases")
                .and_then(serde_json::Value::as_array)
            {
                for alias in aliases {
                    if let Some(alias) = alias.as_str() {
                        shapes.insert(alias.to_string(), shape);
                    }
                }
            }
        }
        shapes
    })
}

fn documented_port_count(operator: &serde_json::Value, array_key: &str, count_key: &str) -> usize {
    let array_count = operator
        .get(array_key)
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    if array_count > 0 {
        return array_count;
    }
    operator
        .get(count_key)
        .and_then(|count| count.get("maximum").or_else(|| count.get("minimum")))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize
}
