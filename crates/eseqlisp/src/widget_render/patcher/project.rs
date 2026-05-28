use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::parser::{Expression, format_expression};

use super::display::preview;
use super::layout;
use super::lisp::{
    attribute_symbol_value, attribute_value, connection_kind_for_op, default_outputs,
    format_patch_literal, is_numeric_literal, is_unsupported_call_head, node_kind_for_op,
    node_label, symbol_at,
};
use super::model::{
    ArgSource, ArgValue, AttributeSource, BindingId, BindingKind, BindingTarget, CallSourceShape,
    ConnectionKind, ConnectionSource, ExprPath, ExprPathSegment, InputPresentation, MacroPatch,
    MacroSignature, NodeKind, NodeSource, OperatorPortShape, ParamNodeInfo, Patch, PatchConnection,
    PatchNode, PatcherIntent, SourceArgValue, SourceExprId, SourceFormId, SourceOwner,
    SourceScopeId, refresh_patch_inline_inputs,
};

pub(super) struct Projector {
    patch: Patch,
    symbol_sources: HashMap<String, (String, usize)>,
    symbol_bindings: HashMap<String, BindingId>,
    history_nodes: HashMap<String, String>,
    op_occurrences: HashMap<String, usize>,
    used_ids: HashSet<String>,
    known_ops: &'static HashSet<String>,
    macro_signatures: HashMap<String, MacroSignature>,
    scope: SourceScopeId,
    intent: PatcherIntent,
}

impl Projector {
    pub(super) fn new(
        macro_signatures: HashMap<String, MacroSignature>,
        intent: PatcherIntent,
    ) -> Self {
        Self::new_in_scope(macro_signatures, SourceScopeId::Root, intent)
    }

    fn new_in_scope(
        macro_signatures: HashMap<String, MacroSignature>,
        scope: SourceScopeId,
        intent: PatcherIntent,
    ) -> Self {
        Self {
            patch: Patch::default(),
            symbol_sources: HashMap::new(),
            symbol_bindings: HashMap::new(),
            history_nodes: HashMap::new(),
            op_occurrences: HashMap::new(),
            used_ids: HashSet::new(),
            known_ops: dgenlisp_operator_names(),
            macro_signatures,
            scope,
            intent,
        }
    }

    pub(super) fn project(mut self, exprs: &[Expression]) -> Patch {
        for (idx, expr) in exprs.iter().enumerate() {
            let form_id = self.form_id(idx);
            self.project_top_level(expr, form_id);
        }
        refresh_patch_inline_inputs(&mut self.patch);
        assign_layout(&mut self.patch);
        self.patch
    }

    fn form_id(&self, index: usize) -> SourceFormId {
        SourceFormId {
            scope: self.scope.clone(),
            index,
        }
    }

    fn root_expr(&self, form_id: SourceFormId) -> SourceExprId {
        SourceExprId {
            form_id,
            path: ExprPath::default(),
        }
    }

    fn child_expr(&self, parent: &SourceExprId, item_index: usize) -> SourceExprId {
        let mut path = parent.path.clone();
        path.0.push(ExprPathSegment::ListItem(item_index));
        SourceExprId {
            form_id: parent.form_id.clone(),
            path,
        }
    }

    fn binding_id(&self, name: &str, kind: BindingKind) -> BindingId {
        BindingId {
            scope: self.scope.clone(),
            name: name.to_string(),
            kind,
        }
    }

    fn project_top_level(&mut self, expr: &Expression, form_id: SourceFormId) {
        let Expression::List(items) = expr else {
            self.add_code_island(expr, "top-level atom is not visual patch syntax", form_id);
            return;
        };
        let Some(head) = symbol_at(items, 0) else {
            self.add_code_island(expr, "top-level form has no symbolic operator", form_id);
            return;
        };
        let source_expr = self.root_expr(form_id.clone());
        match head {
            "def" => self.project_def(items, expr, source_expr),
            "defmacro" => self.project_defmacro(items, expr, source_expr),
            "param" => self.project_param(items, expr, source_expr),
            "make-history" => self.project_make_history(items, expr, source_expr),
            "write-history" => {
                let _ = self.project_write_history(items, expr, source_expr);
            }
            "out" => {
                let id = self.stable_id_for_call(items, None);
                let _ = self.project_call(
                    items,
                    Some(id),
                    None,
                    source_expr.clone(),
                    SourceOwner::TopLevelForm { form_id },
                );
            }
            _ => self.add_code_island(
                expr,
                "top-level form is not a supported patch form",
                form_id,
            ),
        }
    }

    fn project_param(
        &mut self,
        items: &[Expression],
        original: &Expression,
        source_expr: SourceExprId,
    ) {
        let Some(name) = symbol_at(items, 1) else {
            self.add_code_island(
                original,
                "`param` forms must provide a symbolic name",
                source_expr.form_id,
            );
            return;
        };
        let id = self.stable_id_for_call(items, None);
        let binding = self.binding_id(name, BindingKind::Param);
        let Some(node_id) = self.project_call(
            items,
            Some(id),
            None,
            source_expr.clone(),
            SourceOwner::TopLevelForm {
                form_id: source_expr.form_id.clone(),
            },
        ) else {
            self.add_code_island(
                original,
                "`param` value could not be projected",
                source_expr.form_id,
            );
            return;
        };
        self.symbol_sources.insert(name.to_string(), (node_id, 0));
        self.symbol_bindings.insert(name.to_string(), binding);
    }

    fn project_make_history(
        &mut self,
        items: &[Expression],
        original: &Expression,
        source_expr: SourceExprId,
    ) {
        let Some(name) = symbol_at(items, 1) else {
            self.add_code_island(
                original,
                "`make-history` forms must provide a symbolic name",
                source_expr.form_id,
            );
            return;
        };
        if self.history_nodes.contains_key(name) {
            self.add_code_island(
                original,
                "`make-history` declared the same history twice",
                source_expr.form_id,
            );
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
            width: None,
            param: None,
            inline_inputs: Vec::new(),
            diagnostic: None,
            source: Some(NodeSource {
                owner: SourceOwner::Compound {
                    parts: vec![SourceOwner::TopLevelForm {
                        form_id: source_expr.form_id.clone(),
                    }],
                },
                expr: Some(source_expr.clone()),
                call_shape: Some(self.call_source_shape(&source_expr, items)),
            }),
        });
    }

    fn project_def(
        &mut self,
        items: &[Expression],
        original: &Expression,
        source_expr: SourceExprId,
    ) {
        if items.len() != 3 {
            self.add_code_island(
                original,
                "`def` forms must have exactly a target and value",
                source_expr.form_id,
            );
            return;
        }

        match &items[1] {
            Expression::Symbol(name) => {
                if self.is_hidden_host_modulator_def(name, &items[2]) {
                    return;
                }
                let binding = self.binding_id(name, BindingKind::Def);
                let value_expr = self.child_expr(&source_expr, 2);
                let Some((node_id, output_idx)) = self.project_value(
                    &items[2],
                    Some(name.clone()),
                    Some(name.clone()),
                    value_expr,
                    SourceOwner::BindingValue {
                        form_id: source_expr.form_id.clone(),
                        binding: BindingTarget::Symbol(name.clone()),
                        value_path: ExprPath(vec![ExprPathSegment::ListItem(2)]),
                    },
                ) else {
                    self.add_code_island(
                        original,
                        "`def` value could not be projected",
                        source_expr.form_id,
                    );
                    return;
                };
                self.symbol_sources
                    .insert(name.clone(), (node_id, output_idx));
                self.symbol_bindings.insert(name.clone(), binding);
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
                        source_expr.form_id,
                    );
                    return;
                };
                let stable = names.join("_");
                let value_expr = self.child_expr(&source_expr, 2);
                let Some((node_id, _)) = self.project_value(
                    &items[2],
                    Some(stable),
                    None,
                    value_expr,
                    SourceOwner::BindingValue {
                        form_id: source_expr.form_id.clone(),
                        binding: BindingTarget::Destructuring(names.clone()),
                        value_path: ExprPath(vec![ExprPathSegment::ListItem(2)]),
                    },
                ) else {
                    self.add_code_island(
                        original,
                        "destructuring `def` value could not be projected",
                        source_expr.form_id,
                    );
                    return;
                };
                if let Some(node) = self.patch.nodes.iter_mut().find(|node| node.id == node_id) {
                    node.outputs = names.clone();
                    node.label = format!("{} -> {}", node.op, names.join(" "));
                }
                for (idx, name) in names.into_iter().enumerate() {
                    let binding = self.binding_id(&name, BindingKind::Def);
                    self.symbol_bindings.insert(name.clone(), binding);
                    self.symbol_sources.insert(name, (node_id.clone(), idx));
                }
            }
            _ => self.add_code_island(
                original,
                "`def` target is not a symbol or symbol tuple",
                source_expr.form_id,
            ),
        }
    }

    fn project_defmacro(
        &mut self,
        items: &[Expression],
        original: &Expression,
        source_expr: SourceExprId,
    ) {
        let (Some(name), Some(Expression::List(params))) = (symbol_at(items, 1), items.get(2))
        else {
            self.add_code_island(
                original,
                "`defmacro` must provide a name and parameter list",
                source_expr.form_id,
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
            self.add_code_island(
                original,
                "`defmacro` parameters must be symbols",
                source_expr.form_id,
            );
            return;
        };

        let body = if items.len() > 3 { &items[3..] } else { &[] };
        let mut projector = Projector::new_in_scope(
            self.macro_signatures.clone(),
            SourceScopeId::Macro {
                name: name.to_string(),
            },
            self.intent,
        );
        for (idx, param) in param_names.iter().enumerate() {
            let id = projector.unique_id(param);
            let binding = projector.binding_id(param, BindingKind::MacroParam);
            projector
                .symbol_sources
                .insert(param.clone(), (id.clone(), 0));
            projector
                .symbol_bindings
                .insert(param.clone(), binding.clone());
            projector.patch.nodes.push(PatchNode {
                id,
                op: "in".to_string(),
                kind: NodeKind::In,
                label: format!("in {} @name {}", idx + 1, param),
                args: vec![
                    ArgValue::Literal((idx + 1).to_string()),
                    ArgValue::Literal("@name".to_string()),
                    ArgValue::Literal(param.clone()),
                ],
                outputs: vec![param.clone()],
                position: (0.0, 0.0),
                width: None,
                param: None,
                inline_inputs: Vec::new(),
                diagnostic: None,
                source: Some(NodeSource {
                    owner: SourceOwner::MacroParameter {
                        binding,
                        index: idx,
                    },
                    expr: None,
                    call_shape: None,
                }),
            });
        }
        let outputs = infer_macro_outputs(body);
        for (idx, expr) in body.iter().enumerate() {
            let form_id = projector.form_id(idx);
            if idx + 1 == body.len() {
                projector.project_macro_return(expr, form_id);
            } else {
                projector.project_top_level(expr, form_id);
            }
        }
        let mut patch = projector.patch;
        refresh_patch_inline_inputs(&mut patch);
        assign_layout(&mut patch);
        self.patch.macros.push(MacroPatch {
            name: name.to_string(),
            params: param_names,
            outputs: outputs.clone(),
            patch,
        });
        self.macro_signatures.insert(
            name.to_string(),
            MacroSignature {
                params: self
                    .patch
                    .macros
                    .last()
                    .map(|macro_patch| macro_patch.params.clone())
                    .unwrap_or_default(),
                outputs,
            },
        );
    }

    fn is_hidden_host_modulator_def(&self, name: &str, expr: &Expression) -> bool {
        if self.scope != SourceScopeId::Root {
            return false;
        }
        expected_host_modulator_slot(name).is_some_and(|slot| {
            host_modulator_input_signature(expr).is_some_and(|(channel, input_name, modulator)| {
                if input_name != name || modulator != slot {
                    return false;
                }
                match self.intent {
                    PatcherIntent::Instrument => channel == slot + 4,
                    PatcherIntent::Effect => channel >= 3,
                }
            })
        })
    }

    fn project_macro_return(&mut self, expr: &Expression, form_id: SourceFormId) {
        if let Expression::List(items) = expr
            && symbol_at(items, 0).is_some_and(|head| {
                matches!(
                    head,
                    "def" | "defmacro" | "param" | "out" | "make-history" | "write-history"
                )
            })
        {
            self.project_top_level(expr, form_id);
            return;
        }

        let source_expr = self.root_expr(form_id.clone());
        let return_values = tuple_return_items(expr).unwrap_or_else(|| vec![expr]);
        if return_values.is_empty() {
            self.add_code_island(
                expr,
                "macro return value is not visual patch syntax",
                form_id,
            );
            return;
        }
        let is_tuple_return = tuple_return_items(expr).is_some();
        for (idx, value) in return_values.into_iter().enumerate() {
            let value_expr = if is_tuple_return {
                self.child_expr(&source_expr, idx + 1)
            } else {
                source_expr.clone()
            };
            let Some((from_node, from_output)) = self.project_value(
                value,
                Some(if idx == 0 {
                    "return".to_string()
                } else {
                    format!("return{}", idx + 1)
                }),
                None,
                value_expr,
                SourceOwner::TopLevelForm {
                    form_id: form_id.clone(),
                },
            ) else {
                self.add_code_island(
                    expr,
                    "macro return value is not visual patch syntax",
                    form_id,
                );
                return;
            };
            let out_id_base = if idx == 0 {
                "out".to_string()
            } else {
                format!("out{}", idx + 1)
            };
            let id = self.unique_id(&out_id_base);
            self.patch.nodes.push(PatchNode {
                id: id.clone(),
                op: "out".to_string(),
                kind: NodeKind::Out,
                label: format!("out {}", idx + 1),
                args: vec![ArgValue::Literal((idx + 1).to_string())],
                outputs: Vec::new(),
                position: (0.0, 0.0),
                width: None,
                param: None,
                inline_inputs: Vec::new(),
                diagnostic: None,
                source: Some(NodeSource {
                    owner: SourceOwner::TopLevelForm {
                        form_id: form_id.clone(),
                    },
                    expr: None,
                    call_shape: None,
                }),
            });
            self.patch.connections.push(PatchConnection {
                from_node,
                from_output,
                to_node: id,
                to_input: 0,
                kind: ConnectionKind::Forward,
                segment: None,
                presentation: InputPresentation::Cable,
                presentation_override: None,
                source: None,
            });
        }
    }

    fn project_value(
        &mut self,
        expr: &Expression,
        stable_id: Option<String>,
        def_name: Option<String>,
        source_expr: SourceExprId,
        owner: SourceOwner,
    ) -> Option<(String, usize)> {
        match expr {
            Expression::Symbol(name) => {
                if let Some(source) = self.symbol_sources.get(name).cloned() {
                    Some(source)
                } else if dgenlisp_constant_names().contains(name) {
                    Some((
                        self.add_constant_node(
                            name,
                            stable_id.as_deref(),
                            Some(NodeSource {
                                owner,
                                expr: Some(source_expr),
                                call_shape: None,
                            }),
                        ),
                        0,
                    ))
                } else {
                    None
                }
            }
            Expression::Number(_) => Some((
                self.add_constant_node(
                    &format_patch_literal(expr),
                    stable_id.as_deref(),
                    Some(NodeSource {
                        owner,
                        expr: Some(source_expr),
                        call_shape: None,
                    }),
                ),
                0,
            )),
            Expression::List(items) => self
                .project_call(items, stable_id, def_name, source_expr, owner)
                .map(|id| (id, 0)),
            _ => None,
        }
    }

    fn project_read_history(
        &mut self,
        items: &[Expression],
        original: &Expression,
        source_expr: SourceExprId,
    ) -> Option<String> {
        let Some(name) = symbol_at(items, 1) else {
            self.add_code_island(
                original,
                "`read-history` forms must provide a symbolic history name",
                source_expr.form_id,
            );
            return None;
        };
        if let Some(node_id) = self.history_nodes.get(name).cloned() {
            self.append_history_owner(
                &node_id,
                SourceOwner::NestedExpr {
                    expr: source_expr.clone(),
                },
            );
            Some(node_id)
        } else {
            self.add_code_island(
                original,
                "`read-history` references a history with no `make-history`",
                source_expr.form_id,
            );
            None
        }
    }

    fn project_write_history(
        &mut self,
        items: &[Expression],
        original: &Expression,
        source_expr: SourceExprId,
    ) -> Option<(String, usize)> {
        let Some(name) = symbol_at(items, 1) else {
            self.add_code_island(
                original,
                "`write-history` forms must provide a symbolic history name",
                source_expr.form_id,
            );
            return None;
        };
        let Some(history_node) = self.history_nodes.get(name).cloned() else {
            self.add_code_island(
                original,
                "`write-history` references a history with no `make-history`",
                source_expr.form_id,
            );
            return None;
        };
        let arg_sources = self.positional_arg_sources(&source_expr, items, 1);
        let Some(value_arg) = arg_sources.get(1).cloned() else {
            self.add_code_island(
                original,
                "`write-history` forms must provide a value to store",
                source_expr.form_id,
            );
            return None;
        };
        let Some(value) = items.get(value_arg.item_index) else {
            self.add_code_island(
                original,
                "`write-history` value could not be projected",
                source_expr.form_id,
            );
            return None;
        };
        let Some((from_node, from_output)) = self.project_value(
            value,
            None,
            None,
            value_arg.expr.clone(),
            SourceOwner::ArgumentSlot {
                call: source_expr.clone(),
                arg: value_arg.clone(),
            },
        ) else {
            self.add_code_island(
                original,
                "`write-history` value could not be projected",
                source_expr.form_id,
            );
            return None;
        };
        self.append_history_owner(
            &history_node,
            SourceOwner::TopLevelForm {
                form_id: source_expr.form_id.clone(),
            },
        );
        self.patch.connections.push(PatchConnection {
            from_node: from_node.clone(),
            from_output,
            to_node: history_node,
            to_input: 0,
            kind: ConnectionKind::Feedback,
            segment: None,
            presentation: InputPresentation::Cable,
            presentation_override: None,
            source: Some(ConnectionSource {
                from_expr: Some(value_arg.expr.clone()),
                to_call: source_expr.clone(),
                to_arg: value_arg.clone(),
                previous_arg: self.source_arg_value(value, &value_arg),
            }),
        });
        Some((from_node, from_output))
    }

    fn project_call(
        &mut self,
        items: &[Expression],
        stable_id: Option<String>,
        def_name: Option<String>,
        source_expr: SourceExprId,
        owner: SourceOwner,
    ) -> Option<String> {
        let op = symbol_at(items, 0)?.to_string();
        if is_unsupported_call_head(&op) {
            let expr = Expression::List(items.to_vec());
            self.add_code_island(
                &expr,
                "control-flow and binding forms are not visualized in V1",
                source_expr.form_id,
            );
            return None;
        }
        let original = Expression::List(items.to_vec());
        if op == "read-history" {
            return self.project_read_history(items, &original, source_expr);
        }
        if op == "write-history" {
            return self
                .project_write_history(items, &original, source_expr)
                .map(|(node_id, _)| node_id);
        }

        let known_macros = self
            .macro_signatures
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        let kind = node_kind_for_op(&op, &known_macros);
        let id = stable_id.unwrap_or_else(|| self.stable_id_for_call(items, None));
        let call_shape = self.call_source_shape(&source_expr, items);
        let mut node = PatchNode {
            id: self.unique_id(&id),
            op: op.clone(),
            kind,
            label: node_label(&op, items, def_name.as_deref()),
            args: Vec::new(),
            outputs: self.default_outputs_for_node(&op, kind),
            position: (0.0, 0.0),
            width: None,
            param: param_node_info(&op, items),
            inline_inputs: Vec::new(),
            diagnostic: self.operator_diagnostic(&op, kind),
            source: Some(NodeSource {
                owner,
                expr: Some(source_expr.clone()),
                call_shape: Some(call_shape.clone()),
            }),
        };

        let input_arg_sources = self.call_input_arg_sources(&op, &source_expr, items);
        let mut arg_slots = vec![None; input_arg_sources.len()];
        let mut pending_constants = Vec::new();
        for arg_source in input_arg_sources.iter().cloned() {
            let idx = arg_source.semantic_index;
            let Some(arg) = items.get(arg_source.item_index) else {
                continue;
            };
            match arg {
                Expression::Symbol(name) => {
                    if let Some((from_node, from_output)) = self.symbol_sources.get(name).cloned() {
                        self.flush_pending_constant_args(
                            &mut node,
                            &mut arg_slots,
                            &mut pending_constants,
                        );
                        let resolved_binding = self.symbol_bindings.get(name).cloned();
                        let presentation =
                            self.default_symbol_connection_presentation(idx, &from_node);
                        self.patch.connections.push(PatchConnection {
                            from_node,
                            from_output,
                            to_node: node.id.clone(),
                            to_input: idx,
                            kind: connection_kind_for_op(&op),
                            segment: None,
                            presentation,
                            presentation_override: None,
                            source: Some(ConnectionSource {
                                from_expr: None,
                                to_call: source_expr.clone(),
                                to_arg: arg_source.clone(),
                                previous_arg: SourceArgValue::SymbolReference {
                                    expr: arg_source.expr.clone(),
                                    symbol: name.clone(),
                                    resolved_binding,
                                },
                            }),
                        });
                        arg_slots[idx] = Some(ArgValue::SymbolRef(name.clone()));
                    } else if dgenlisp_constant_names().contains(name) {
                        pending_constants.push((arg_source.clone(), name.clone()));
                    } else {
                        arg_slots[idx] = Some(ArgValue::Literal(name.clone()));
                    }
                }
                Expression::List(nested) => {
                    if let Some((from_node, from_output)) = self.project_value(
                        &Expression::List(nested.clone()),
                        None,
                        None,
                        arg_source.expr.clone(),
                        SourceOwner::NestedExpr {
                            expr: arg_source.expr.clone(),
                        },
                    ) {
                        self.flush_pending_constant_args(
                            &mut node,
                            &mut arg_slots,
                            &mut pending_constants,
                        );
                        let presentation =
                            self.default_nested_connection_presentation(idx, &from_node);
                        self.patch.connections.push(PatchConnection {
                            from_node,
                            from_output,
                            to_node: node.id.clone(),
                            to_input: idx,
                            kind: connection_kind_for_op(&op),
                            segment: None,
                            presentation,
                            presentation_override: None,
                            source: Some(ConnectionSource {
                                from_expr: Some(arg_source.expr.clone()),
                                to_call: source_expr.clone(),
                                to_arg: arg_source.clone(),
                                previous_arg: SourceArgValue::NestedExpression(
                                    arg_source.expr.clone(),
                                ),
                            }),
                        });
                        arg_slots[idx] = Some(ArgValue::ConnectedExpr);
                    } else {
                        arg_slots[idx] = Some(ArgValue::Literal("<expr>".to_string()));
                    }
                }
                other => {
                    let value = format_patch_literal(other);
                    if is_numeric_literal(&value) {
                        pending_constants.push((arg_source.clone(), value));
                    } else {
                        arg_slots[idx] = Some(ArgValue::Literal(value));
                    }
                }
            }
        }
        for (arg_source, value) in pending_constants {
            arg_slots[arg_source.semantic_index] = Some(ArgValue::Literal(value));
        }
        node.args = arg_slots
            .into_iter()
            .map(|arg| arg.unwrap_or_else(|| ArgValue::Literal("<expr>".to_string())))
            .collect();
        let id = node.id.clone();
        self.patch.nodes.push(node);
        Some(id)
    }

    fn default_outputs_for_node(&self, op: &str, kind: NodeKind) -> Vec<String> {
        if kind == NodeKind::MacroInstance {
            return self
                .macro_signatures
                .get(op)
                .map(|signature| signature.outputs.clone())
                .filter(|outputs| !outputs.is_empty())
                .unwrap_or_else(|| default_outputs(op));
        }
        default_outputs(op)
    }

    fn flush_pending_constant_args(
        &mut self,
        node: &mut PatchNode,
        arg_slots: &mut [Option<ArgValue>],
        pending_constants: &mut Vec<(ArgSource, String)>,
    ) {
        for (arg_source, value) in pending_constants.drain(..) {
            let to_call = node
                .source
                .as_ref()
                .and_then(|source| source.expr.clone())
                .expect("projected source node has a source expression");
            let constant_id = self.add_constant_node(
                &value,
                None,
                Some(NodeSource {
                    owner: SourceOwner::ArgumentSlot {
                        call: to_call.clone(),
                        arg: arg_source.clone(),
                    },
                    expr: Some(arg_source.expr.clone()),
                    call_shape: None,
                }),
            );
            self.patch.connections.push(PatchConnection {
                from_node: constant_id,
                from_output: 0,
                to_node: node.id.clone(),
                to_input: arg_source.semantic_index,
                kind: connection_kind_for_op(&node.op),
                segment: None,
                presentation: InputPresentation::Cable,
                presentation_override: None,
                source: Some(ConnectionSource {
                    from_expr: Some(arg_source.expr.clone()),
                    to_call,
                    to_arg: arg_source.clone(),
                    previous_arg: SourceArgValue::Literal(arg_source.expr.clone()),
                }),
            });
            arg_slots[arg_source.semantic_index] = Some(ArgValue::ConnectedExpr);
        }
    }

    fn add_constant_node(
        &mut self,
        value: &str,
        stable_id: Option<&str>,
        source: Option<NodeSource>,
    ) -> String {
        let id = self.unique_id(stable_id.unwrap_or(value));
        self.patch.nodes.push(PatchNode {
            id: id.clone(),
            op: value.to_string(),
            kind: NodeKind::Constant,
            label: value.to_string(),
            args: Vec::new(),
            outputs: vec!["out".to_string()],
            position: (0.0, 0.0),
            width: None,
            param: None,
            inline_inputs: Vec::new(),
            diagnostic: None,
            source,
        });
        id
    }

    fn call_source_shape(&self, call: &SourceExprId, items: &[Expression]) -> CallSourceShape {
        CallSourceShape {
            call: call.clone(),
            positional_args: self.positional_arg_sources(call, items, 1),
            attributes: self.attribute_sources(call, items),
        }
    }

    fn call_input_arg_sources(
        &self,
        op: &str,
        call: &SourceExprId,
        items: &[Expression],
    ) -> Vec<ArgSource> {
        let positional = self.positional_arg_sources(call, items, 1);
        match op {
            "in" => positional.into_iter().take(1).collect(),
            "param" | "make-history" => Vec::new(),
            "out" => positional.into_iter().take(2).collect(),
            "write-history" => positional.into_iter().nth(1).into_iter().collect(),
            _ => positional,
        }
    }

    fn positional_arg_sources(
        &self,
        call: &SourceExprId,
        items: &[Expression],
        start: usize,
    ) -> Vec<ArgSource> {
        let mut args = Vec::new();
        let mut item_index = start;
        while item_index < items.len() {
            if matches!(&items[item_index], Expression::Symbol(symbol) if symbol.starts_with('@')) {
                item_index += 2;
                continue;
            }
            args.push(ArgSource {
                semantic_index: args.len(),
                item_index,
                expr: self.child_expr(call, item_index),
            });
            item_index += 1;
        }
        args
    }

    fn attribute_sources(&self, call: &SourceExprId, items: &[Expression]) -> Vec<AttributeSource> {
        let mut attributes = Vec::new();
        let mut item_index = 1;
        while item_index + 1 < items.len() {
            if let Expression::Symbol(symbol) = &items[item_index]
                && symbol.starts_with('@')
            {
                attributes.push(AttributeSource {
                    key_item_index: item_index,
                    value_item_index: item_index + 1,
                    key: symbol.clone(),
                    value: self.child_expr(call, item_index + 1),
                });
                item_index += 2;
                continue;
            }
            item_index += 1;
        }
        attributes
    }

    fn source_arg_value(&self, expr: &Expression, arg: &ArgSource) -> SourceArgValue {
        match expr {
            Expression::Symbol(symbol) => SourceArgValue::SymbolReference {
                expr: arg.expr.clone(),
                symbol: symbol.clone(),
                resolved_binding: self.symbol_bindings.get(symbol).cloned(),
            },
            Expression::List(_) => SourceArgValue::NestedExpression(arg.expr.clone()),
            _ => SourceArgValue::Literal(arg.expr.clone()),
        }
    }

    fn append_history_owner(&mut self, node_id: &str, owner: SourceOwner) {
        let Some(node) = self.patch.nodes.iter_mut().find(|node| node.id == node_id) else {
            return;
        };
        let Some(source) = node.source.as_mut() else {
            return;
        };
        match &mut source.owner {
            SourceOwner::Compound { parts } => parts.push(owner),
            existing => {
                source.owner = SourceOwner::Compound {
                    parts: vec![existing.clone(), owner],
                };
            }
        }
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

    fn add_code_island(&mut self, expr: &Expression, reason: &str, form_id: SourceFormId) {
        let id = self.unique_id("code");
        self.patch.nodes.push(PatchNode {
            id,
            op: "code".to_string(),
            kind: NodeKind::CodeIsland,
            label: preview(&format_expression(expr), 48),
            args: Vec::new(),
            outputs: Vec::new(),
            position: (0.0, 0.0),
            width: None,
            param: None,
            inline_inputs: Vec::new(),
            diagnostic: Some(reason.to_string()),
            source: Some(NodeSource {
                owner: SourceOwner::CodeIsland {
                    form_id: form_id.clone(),
                },
                expr: Some(self.root_expr(form_id)),
                call_shape: None,
            }),
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

    fn default_symbol_connection_presentation(
        &self,
        input_index: usize,
        from_node: &str,
    ) -> InputPresentation {
        if input_index == 0 {
            return InputPresentation::Cable;
        }
        self.patch
            .nodes
            .iter()
            .find(|node| node.id == from_node)
            .and_then(|node| node.param.as_ref())
            .filter(|param| !param.modulatable)
            .map(|_| InputPresentation::InlineRawParam)
            .unwrap_or(InputPresentation::Cable)
    }

    fn default_nested_connection_presentation(
        &self,
        input_index: usize,
        from_node: &str,
    ) -> InputPresentation {
        if input_index == 0 {
            return InputPresentation::Cable;
        }
        let Some(mod_node) = self.patch.nodes.iter().find(|node| node.id == from_node) else {
            return InputPresentation::Cable;
        };
        if mod_node.op != "mod" {
            return InputPresentation::Cable;
        }
        let Some(inbound) = self
            .patch
            .connections
            .iter()
            .find(|connection| connection.to_node == mod_node.id && connection.to_input == 0)
        else {
            return InputPresentation::Cable;
        };
        self.patch
            .nodes
            .iter()
            .find(|node| node.id == inbound.from_node)
            .and_then(|node| node.param.as_ref())
            .filter(|param| param.modulatable)
            .map(|_| InputPresentation::InlineModParam)
            .unwrap_or(InputPresentation::Cable)
    }
}

fn param_node_info(op: &str, items: &[Expression]) -> Option<ParamNodeInfo> {
    if op != "param" {
        return None;
    }
    let name = symbol_at(items, 1)?;
    Some(ParamNodeInfo {
        name: name.to_string(),
        modulatable: param_is_modulatable(items),
    })
}

fn param_is_modulatable(items: &[Expression]) -> bool {
    items.windows(2).any(|pair| {
        matches!(
            (&pair[0], &pair[1]),
            (Expression::Symbol(key), Expression::Symbol(value))
                if key == "@mod" && value == "true"
        )
    })
}

fn expected_host_modulator_slot(name: &str) -> Option<usize> {
    match name {
        "mod1" => Some(1),
        "mod2" => Some(2),
        "mod3" => Some(3),
        "mod4" => Some(4),
        _ => None,
    }
}

fn host_modulator_input_signature(expr: &Expression) -> Option<(usize, &str, usize)> {
    let Expression::List(items) = expr else {
        return None;
    };
    if symbol_at(items, 0) != Some("in") {
        return None;
    }
    let channel = match items.get(1) {
        Some(Expression::Number(value)) if value.fract() == 0.0 && *value > 0.0 => *value as usize,
        _ => return None,
    };
    let name = attribute_symbol_value(items, "@name")?;
    let modulator = match items
        .windows(2)
        .find_map(|pair| match (&pair[0], &pair[1]) {
            (Expression::Symbol(key), Expression::Number(value)) if key == "@modulator" => {
                Some(*value)
            }
            _ => None,
        }) {
        Some(value) if value.fract() == 0.0 && value > 0.0 => value as usize,
        _ => return None,
    };
    Some((channel, name, modulator))
}

fn infer_macro_outputs(body: &[Expression]) -> Vec<String> {
    let Some(return_expr) = body.last() else {
        return Vec::new();
    };
    let count = tuple_return_items(return_expr)
        .map(|items| items.len())
        .unwrap_or(1);
    (0..count)
        .map(|idx| {
            if idx == 0 {
                "out".to_string()
            } else {
                format!("out{}", idx + 1)
            }
        })
        .collect()
}

fn tuple_return_items(expr: &Expression) -> Option<Vec<&Expression>> {
    let Expression::List(items) = expr else {
        return None;
    };
    (symbol_at(items, 0) == Some("tuple")).then(|| items.iter().skip(1).collect())
}

pub(super) fn assign_layout(patch: &mut Patch) {
    layout::assign_layout(patch);
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
        for special_form in expression_special_forms(&metadata) {
            if let Some(name) = special_form.get("name").and_then(serde_json::Value::as_str) {
                names.insert(name.to_string());
            }
        }
        names
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct OperatorDocumentation {
    pub(super) summary: Option<String>,
    pub(super) signatures: Vec<String>,
    pub(super) inputs: Vec<OperatorPortDocumentation>,
    pub(super) outputs: Vec<OperatorPortDocumentation>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct OperatorPortDocumentation {
    pub(super) name: Option<String>,
    pub(super) kind: Option<String>,
    pub(super) required: Option<bool>,
    pub(super) index: Option<usize>,
    pub(super) summary: Option<String>,
}

pub(super) fn dgenlisp_operator_documentation() -> &'static HashMap<String, OperatorDocumentation> {
    static OPERATOR_DOCUMENTATION: OnceLock<HashMap<String, OperatorDocumentation>> =
        OnceLock::new();
    OPERATOR_DOCUMENTATION.get_or_init(|| {
        let metadata: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../sequencer/tools/dgenlisp-operators.json"
        ))
        .expect("bundled dgenlisp-operators.json must be valid JSON");
        let operators = metadata
            .get("operators")
            .and_then(serde_json::Value::as_array)
            .expect("bundled dgenlisp-operators.json must contain an operators array");

        let mut docs = HashMap::new();
        for operator in operators {
            let Some(name) = operator.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let doc = OperatorDocumentation {
                summary: operator
                    .get("summary")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                signatures: operator
                    .get("signatures")
                    .and_then(serde_json::Value::as_array)
                    .map(|signatures| {
                        signatures
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                inputs: operator_port_documentation(operator, "inputs"),
                outputs: operator_port_documentation(operator, "outputs"),
            };
            docs.insert(name.to_string(), doc.clone());
            if let Some(aliases) = operator
                .get("aliases")
                .and_then(serde_json::Value::as_array)
            {
                for alias in aliases {
                    if let Some(alias) = alias.as_str() {
                        docs.insert(alias.to_string(), doc.clone());
                    }
                }
            }
        }
        docs
    })
}

fn operator_port_documentation(
    operator: &serde_json::Value,
    key: &str,
) -> Vec<OperatorPortDocumentation> {
    operator
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|ports| {
            ports
                .iter()
                .map(|port| OperatorPortDocumentation {
                    name: port
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    kind: port
                        .get("kind")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    required: port.get("required").and_then(serde_json::Value::as_bool),
                    index: port
                        .get("index")
                        .and_then(serde_json::Value::as_u64)
                        .map(|index| index as usize),
                    summary: port
                        .get("summary")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default()
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
        for special_form in expression_special_forms(&metadata) {
            let Some(name) = special_form.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            shapes.insert(
                name.to_string(),
                OperatorPortShape {
                    input_count: documented_special_form_input_count(special_form),
                    output_count: 1,
                },
            );
        }
        shapes
    })
}

pub(super) fn dgenlisp_operator_required_input_counts() -> &'static HashMap<String, usize> {
    static OPERATOR_REQUIRED_INPUT_COUNTS: OnceLock<HashMap<String, usize>> = OnceLock::new();
    OPERATOR_REQUIRED_INPUT_COUNTS.get_or_init(|| {
        let metadata: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../sequencer/tools/dgenlisp-operators.json"
        ))
        .expect("bundled dgenlisp-operators.json must be valid JSON");
        let operators = metadata
            .get("operators")
            .and_then(serde_json::Value::as_array)
            .expect("bundled dgenlisp-operators.json must contain an operators array");
        let mut counts = HashMap::new();
        for operator in operators {
            let Some(name) = operator.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let count = documented_required_input_count(operator);
            counts.insert(name.to_string(), count);
            if let Some(aliases) = operator
                .get("aliases")
                .and_then(serde_json::Value::as_array)
            {
                for alias in aliases {
                    if let Some(alias) = alias.as_str() {
                        counts.insert(alias.to_string(), count);
                    }
                }
            }
        }
        for special_form in expression_special_forms(&metadata) {
            let Some(name) = special_form.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            counts.insert(
                name.to_string(),
                documented_special_form_input_count(special_form),
            );
        }
        counts
    })
}

fn expression_special_forms(metadata: &serde_json::Value) -> Vec<&serde_json::Value> {
    metadata
        .get("special_forms")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|form| form.get("name").and_then(serde_json::Value::as_str) == Some("mod"))
        .collect()
}

fn documented_special_form_input_count(form: &serde_json::Value) -> usize {
    form.get("signatures")
        .and_then(serde_json::Value::as_array)
        .and_then(|signatures| signatures.first())
        .and_then(serde_json::Value::as_str)
        .and_then(|signature| {
            let inner = signature
                .strip_prefix('(')
                .and_then(|value| value.strip_suffix(')'))?;
            Some(inner.split_whitespace().count().saturating_sub(1))
        })
        .unwrap_or(1)
}

fn documented_required_input_count(operator: &serde_json::Value) -> usize {
    let required_count = operator
        .get("inputs")
        .and_then(serde_json::Value::as_array)
        .map(|inputs| {
            inputs
                .iter()
                .filter(|input| {
                    input
                        .get("required")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true)
                })
                .count()
        })
        .unwrap_or(0);
    if required_count > 0 {
        return required_count;
    }
    operator
        .get("input_count")
        .and_then(|count| count.get("minimum").or_else(|| count.get("maximum")))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize
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
