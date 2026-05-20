use std::collections::HashMap;

use crate::parser::{ASTParser, Expression, Parser, format_expression};

use super::display::node_display_label;
use super::lisp::{parse_patch_source, symbol_at};
use super::model::{
    BindingTarget, ExprPathSegment, InputPortRef, NodeKind, OutputPortRef, Patch, PatchConnection,
    PatchNode, PatcherIntent, SourceExprId, SourceFormId, SourceOwner, SourceScopeId,
};
use super::project::dgenlisp_operator_names;
use super::state::{
    PatcherConnectionEdit, PatcherConnectionOrigin, PatcherInteractionState, PatcherNodeOrigin,
    active_patcher_view_key, connection_edit_key, source_connection_id,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WriteBackError {
    Parse(String),
    MissingSourceOwner {
        view_key: String,
        node_id: String,
    },
    EditedCodeIsland {
        view_key: String,
        node_id: String,
    },
    UnknownOperator {
        view_key: String,
        node_id: String,
        operator: String,
    },
    UnsupportedCreatedNode {
        view_key: String,
        node_id: String,
    },
    UnsupportedCreatedConnection {
        view_key: String,
        connection_id: String,
    },
    UnsupportedDeletedNode {
        view_key: String,
        node_id: String,
    },
    UnsupportedDeletedConnection {
        view_key: String,
        connection_id: String,
    },
    MultipleHistoryWrites {
        view_key: String,
        history_id: String,
    },
    UnsupportedHistoryEdit {
        view_key: String,
        history_id: String,
        reason: String,
    },
    UnsupportedSourceOwner {
        view_key: String,
        node_id: String,
        owner: String,
    },
    InvalidEdit {
        view_key: String,
        node_id: String,
        reason: String,
    },
}

pub(super) fn emit_patch_writeback(
    source: &str,
    intent: PatcherIntent,
    interaction_state: &PatcherInteractionState,
) -> Result<String, WriteBackError> {
    let mut document = SourceDocument::parse(source)?;
    let root_patch = parse_patch_source(source, intent).map_err(WriteBackError::Parse)?;

    validate_connection_edits(&root_patch, interaction_state)?;
    reject_unsupported_deletions(interaction_state)?;

    let active_view_key = active_patcher_view_key(interaction_state);
    if let Some(text_edit) = interaction_state.text_edit.as_ref() {
        let patch = patch_for_view(&root_patch, &active_view_key).ok_or_else(|| {
            WriteBackError::InvalidEdit {
                view_key: active_view_key.clone(),
                node_id: text_edit.node_id.clone(),
                reason: "active text edit targets a missing patch view".to_string(),
            }
        })?;
        let node =
            patch_node(patch, &text_edit.node_id).ok_or_else(|| WriteBackError::InvalidEdit {
                view_key: active_view_key.clone(),
                node_id: text_edit.node_id.clone(),
                reason: "active text edit targets a missing source node".to_string(),
            })?;
        if text_edit.text.trim() != node_display_label(node).trim() {
            apply_node_text_edit(&mut document, &active_view_key, node, &text_edit.text)?;
        }
    }

    let active_text_key = interaction_state
        .text_edit
        .as_ref()
        .map(|text_edit| (active_view_key.as_str(), text_edit.node_id.as_str()));
    let mut node_edits = interaction_state
        .edit_state
        .nodes
        .values()
        .collect::<Vec<_>>();
    node_edits.sort_by_key(|edit| {
        source_edit_sort_key(&root_patch, edit).unwrap_or_else(|| {
            (
                edit.view_key.clone(),
                usize::MAX,
                usize::MAX,
                edit.id.clone(),
            )
        })
    });

    for edit in node_edits {
        match &edit.origin {
            PatcherNodeOrigin::Created { .. } => {
                if !created_history_edit(edit) {
                    return Err(WriteBackError::UnsupportedCreatedNode {
                        view_key: edit.view_key.clone(),
                        node_id: edit.id.clone(),
                    });
                }
            }
            PatcherNodeOrigin::Source { source_node_id } => {
                if active_text_key == Some((edit.view_key.as_str(), source_node_id.as_str())) {
                    continue;
                }
                let patch = patch_for_view(&root_patch, &edit.view_key).ok_or_else(|| {
                    WriteBackError::InvalidEdit {
                        view_key: edit.view_key.clone(),
                        node_id: edit.id.clone(),
                        reason: "node edit targets a missing patch view".to_string(),
                    }
                })?;
                let node = patch_node(patch, source_node_id).ok_or_else(|| {
                    WriteBackError::InvalidEdit {
                        view_key: edit.view_key.clone(),
                        node_id: edit.id.clone(),
                        reason: "node edit targets a missing source node".to_string(),
                    }
                })?;
                if edit.text.trim() != node_display_label(node).trim() {
                    apply_node_text_edit(&mut document, &edit.view_key, node, &edit.text)?;
                }
            }
        }
    }

    apply_history_writeback(&mut document, &root_patch, interaction_state)?;

    Ok(document.emit())
}

fn validate_connection_edits(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> Result<(), WriteBackError> {
    for edit in interaction_state.edit_state.connections.values() {
        match edit.origin {
            PatcherConnectionOrigin::Created { .. } => {
                if !connection_edit_touches_history(root_patch, interaction_state, edit) {
                    return Err(WriteBackError::UnsupportedCreatedConnection {
                        view_key: edit.view_key.clone(),
                        connection_id: edit.id.clone(),
                    });
                }
            }
            PatcherConnectionOrigin::Source { .. } => {
                return Err(WriteBackError::UnsupportedDeletedConnection {
                    view_key: edit.view_key.clone(),
                    connection_id: edit.id.clone(),
                });
            }
        }
    }

    for key in interaction_state.edit_state.deleted_connections.iter() {
        let (view_key, connection_id) = split_scoped_key(key);
        if !deleted_connection_has_history_replacement(
            root_patch,
            interaction_state,
            &view_key,
            &connection_id,
        ) {
            return Err(WriteBackError::UnsupportedDeletedConnection {
                view_key,
                connection_id,
            });
        }
    }

    Ok(())
}

fn reject_unsupported_deletions(
    interaction_state: &PatcherInteractionState,
) -> Result<(), WriteBackError> {
    if let Some(key) = interaction_state.edit_state.deleted_nodes.iter().next() {
        let (view_key, node_id) = split_scoped_key(key);
        return Err(WriteBackError::UnsupportedDeletedNode { view_key, node_id });
    }
    Ok(())
}

fn split_scoped_key(key: &str) -> (String, String) {
    key.split_once("::")
        .map(|(view, id)| (view.to_string(), id.to_string()))
        .unwrap_or_else(|| ("root".to_string(), key.to_string()))
}

fn patch_for_view<'a>(root_patch: &'a Patch, view_key: &str) -> Option<&'a Patch> {
    let Some(macro_name) = view_key.strip_prefix("macro:") else {
        return Some(root_patch);
    };
    root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == macro_name)
        .map(|macro_patch| &macro_patch.patch)
}

fn patch_node<'a>(patch: &'a Patch, node_id: &str) -> Option<&'a PatchNode> {
    patch.nodes.iter().find(|node| node.id == node_id)
}

fn created_history_edit(edit: &super::state::PatcherNodeEdit) -> bool {
    edit.text.trim() == "history"
}

fn connection_edit_touches_history(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    edit: &PatcherConnectionEdit,
) -> bool {
    node_is_history(
        root_patch,
        interaction_state,
        &edit.view_key,
        &edit.from.node_id,
    ) || node_is_history(
        root_patch,
        interaction_state,
        &edit.view_key,
        &edit.to.node_id,
    )
}

fn deleted_connection_has_history_replacement(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    connection_id: &str,
) -> bool {
    let Some(connection) = source_connection(root_patch, view_key, connection_id) else {
        return false;
    };
    let to = InputPortRef {
        node_id: connection.to_node.clone(),
        input_index: connection.to_input,
    };
    interaction_state
        .edit_state
        .connections
        .values()
        .filter(|edit| edit.view_key == view_key)
        .any(|edit| {
            node_is_history(root_patch, interaction_state, view_key, &edit.from.node_id)
                && edit.to == to
                || node_is_history(root_patch, interaction_state, view_key, &edit.to.node_id)
                    && edit.to == to
        })
}

fn source_connection<'a>(
    root_patch: &'a Patch,
    view_key: &str,
    connection_id: &str,
) -> Option<&'a PatchConnection> {
    patch_for_view(root_patch, view_key)?
        .connections
        .iter()
        .find(|connection| source_connection_id(connection) == connection_id)
}

fn node_is_history(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    node_id: &str,
) -> bool {
    patch_for_view(root_patch, view_key)
        .and_then(|patch| patch_node(patch, node_id))
        .is_some_and(|node| node.kind == NodeKind::History)
        || interaction_state
            .edit_state
            .nodes
            .get(&super::state::node_edit_key(view_key, node_id))
            .is_some_and(created_history_edit)
}

fn source_edit_sort_key(
    root_patch: &Patch,
    edit: &super::state::PatcherNodeEdit,
) -> Option<(String, usize, usize, String)> {
    let PatcherNodeOrigin::Source { source_node_id } = &edit.origin else {
        return None;
    };
    let patch = patch_for_view(root_patch, &edit.view_key)?;
    let node = patch_node(patch, source_node_id)?;
    let source = node.source.as_ref()?;
    let (form_id, path_len) = source_owner_location(&source.owner, source.expr.as_ref())?;
    Some((
        view_key_for_scope(&form_id.scope),
        form_id.index,
        path_len,
        node.id.clone(),
    ))
}

fn source_owner_location<'a>(
    owner: &'a SourceOwner,
    expr: Option<&'a SourceExprId>,
) -> Option<(&'a SourceFormId, usize)> {
    match owner {
        SourceOwner::BindingValue {
            form_id,
            value_path,
            ..
        } => Some((form_id, value_path.0.len())),
        SourceOwner::TopLevelForm { form_id } => Some((form_id, 0)),
        SourceOwner::NestedExpr { expr } => Some((&expr.form_id, expr.path.0.len())),
        _ => expr.map(|expr| (&expr.form_id, expr.path.0.len())),
    }
}

fn apply_history_writeback(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> Result<(), WriteBackError> {
    let mut allocator = HistoryNameAllocator::new(document);
    let mut pending_make_forms = Vec::new();
    let mut pending_write_forms = Vec::new();
    let mut views = vec!["root".to_string()];
    views.extend(
        root_patch
            .macros
            .iter()
            .map(|macro_patch| format!("macro:{}", macro_patch.name)),
    );
    for edit in interaction_state.edit_state.nodes.values() {
        if !views.contains(&edit.view_key) {
            views.push(edit.view_key.clone());
        }
    }
    for edit in interaction_state.edit_state.connections.values() {
        if !views.contains(&edit.view_key) {
            views.push(edit.view_key.clone());
        }
    }

    for view_key in views {
        let Some(patch) = patch_for_view(root_patch, &view_key) else {
            continue;
        };
        validate_single_history_writes(root_patch, interaction_state, &view_key)?;

        let scope = scope_for_view_key(&view_key);
        let mut history_names = HashMap::new();
        for node in patch
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::History)
        {
            history_names.insert(node.id.clone(), node.id.clone());
        }
        let mut created_history_ids = interaction_state
            .edit_state
            .nodes
            .values()
            .filter(|edit| edit.view_key == view_key)
            .filter(|edit| created_history_edit(edit))
            .map(|edit| edit.id.clone())
            .collect::<Vec<_>>();
        created_history_ids.sort();
        for node_id in created_history_ids {
            let name = allocator.allocate(&scope);
            history_names.insert(node_id, name.clone());
            pending_make_forms.push((scope.clone(), name));
        }

        let mut created_connections = interaction_state
            .edit_state
            .connections
            .values()
            .filter(|edit| edit.view_key == view_key)
            .collect::<Vec<_>>();
        created_connections.sort_by(|a, b| a.id.cmp(&b.id));
        for connection in created_connections {
            if let Some(history_name) = history_names.get(&connection.from.node_id) {
                apply_history_read_connection(
                    document,
                    root_patch,
                    &view_key,
                    connection,
                    history_name,
                )?;
            } else if let Some(history_name) = history_names.get(&connection.to.node_id) {
                let (value, value_scope) = connection_source_expr(
                    document,
                    root_patch,
                    interaction_state,
                    &view_key,
                    &connection.from,
                )?;
                if value_scope != scope {
                    return Err(WriteBackError::UnsupportedHistoryEdit {
                        view_key: view_key.clone(),
                        history_id: connection.to.node_id.clone(),
                        reason: "history write source crosses source scopes".to_string(),
                    });
                }
                if let Some(write_value_expr) = deleted_write_value_expr(
                    root_patch,
                    interaction_state,
                    &view_key,
                    &connection.to,
                ) {
                    document.replace_expr(&write_value_expr, value)?;
                } else {
                    pending_write_forms.push((scope.clone(), history_name.clone(), value));
                }
            } else {
                return Err(WriteBackError::UnsupportedCreatedConnection {
                    view_key: view_key.clone(),
                    connection_id: connection.id.clone(),
                });
            }
        }
    }

    for (scope, name) in pending_make_forms.into_iter().rev() {
        document.prepend_form(
            &scope,
            Expression::List(vec![
                Expression::Symbol("make-history".to_string()),
                Expression::Symbol(name),
            ]),
        )?;
    }
    for (scope, name, value) in pending_write_forms {
        document.insert_history_write(&scope, name, value)?;
    }
    Ok(())
}

fn validate_single_history_writes(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
) -> Result<(), WriteBackError> {
    let Some(patch) = patch_for_view(root_patch, view_key) else {
        return Ok(());
    };
    let mut counts: HashMap<String, usize> = HashMap::new();
    for connection in &patch.connections {
        if interaction_state
            .edit_state
            .deleted_connections
            .contains(&connection_edit_key(
                view_key,
                &source_connection_id(connection),
            ))
        {
            continue;
        }
        if node_is_history(root_patch, interaction_state, view_key, &connection.to_node) {
            *counts.entry(connection.to_node.clone()).or_default() += 1;
        }
    }
    for connection in interaction_state
        .edit_state
        .connections
        .values()
        .filter(|edit| edit.view_key == view_key)
    {
        if node_is_history(
            root_patch,
            interaction_state,
            view_key,
            &connection.to.node_id,
        ) {
            *counts.entry(connection.to.node_id.clone()).or_default() += 1;
        }
    }
    if let Some((history_id, _)) = counts.into_iter().find(|(_, count)| *count > 1) {
        return Err(WriteBackError::MultipleHistoryWrites {
            view_key: view_key.to_string(),
            history_id,
        });
    }
    Ok(())
}

fn apply_history_read_connection(
    document: &mut SourceDocument,
    root_patch: &Patch,
    view_key: &str,
    connection: &PatcherConnectionEdit,
    history_name: &str,
) -> Result<(), WriteBackError> {
    let read_expr = Expression::List(vec![
        Expression::Symbol("read-history".to_string()),
        Expression::Symbol(history_name.to_string()),
    ]);
    let Some(dest) = patch_for_view(root_patch, view_key)
        .and_then(|patch| patch_node(patch, &connection.to.node_id))
    else {
        return Err(WriteBackError::UnsupportedHistoryEdit {
            view_key: view_key.to_string(),
            history_id: connection.from.node_id.clone(),
            reason: "history read destination must be source-backed in Phase 3".to_string(),
        });
    };
    if dest.kind == NodeKind::History {
        return Err(WriteBackError::UnsupportedHistoryEdit {
            view_key: view_key.to_string(),
            history_id: connection.from.node_id.clone(),
            reason: "history nodes cannot consume history reads".to_string(),
        });
    }
    if let Some(source) = dest.source.as_ref() {
        if let Some(call_shape) = source.call_shape.as_ref()
            && let Some(arg) = call_shape
                .positional_args
                .iter()
                .find(|arg| arg.semantic_index == connection.to.input_index)
        {
            return document.replace_expr(&arg.expr, read_expr);
        }
        if dest.kind == NodeKind::Out
            && connection.to.input_index == 0
            && let SourceOwner::TopLevelForm { form_id } = &source.owner
        {
            return document.replace_expr(
                &SourceExprId {
                    form_id: form_id.clone(),
                    path: Default::default(),
                },
                read_expr,
            );
        }
    }
    Err(WriteBackError::UnsupportedHistoryEdit {
        view_key: view_key.to_string(),
        history_id: connection.from.node_id.clone(),
        reason: "history read destination has no source-owned positional argument".to_string(),
    })
}

fn deleted_write_value_expr(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    to: &InputPortRef,
) -> Option<SourceExprId> {
    let patch = patch_for_view(root_patch, view_key)?;
    for deleted in &interaction_state.edit_state.deleted_connections {
        let (deleted_view, connection_id) = split_scoped_key(deleted);
        if deleted_view != view_key {
            continue;
        }
        let Some(connection) = patch
            .connections
            .iter()
            .find(|connection| source_connection_id(connection) == connection_id)
        else {
            continue;
        };
        if connection.to_node == to.node_id && connection.to_input == to.input_index {
            return connection
                .source
                .as_ref()
                .map(|source| source.to_arg.expr.clone());
        }
    }
    None
}

fn connection_source_expr(
    document: &SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    from: &OutputPortRef,
) -> Result<(Expression, SourceScopeId), WriteBackError> {
    let Some(node) =
        patch_for_view(root_patch, view_key).and_then(|patch| patch_node(patch, &from.node_id))
    else {
        return Err(WriteBackError::UnsupportedHistoryEdit {
            view_key: view_key.to_string(),
            history_id: String::new(),
            reason: "history write source must be source-backed in Phase 3".to_string(),
        });
    };
    if node.kind == NodeKind::History
        || node_is_history(root_patch, interaction_state, view_key, &from.node_id)
    {
        return Err(WriteBackError::UnsupportedHistoryEdit {
            view_key: view_key.to_string(),
            history_id: node.id.clone(),
            reason: "history write source cannot be another history node".to_string(),
        });
    }
    let source = node
        .source
        .as_ref()
        .ok_or_else(|| WriteBackError::MissingSourceOwner {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
        })?;
    let scope = source
        .expr
        .as_ref()
        .map(|expr| expr.form_id.scope.clone())
        .or_else(|| {
            source_owner_location(&source.owner, source.expr.as_ref())
                .map(|(form_id, _)| form_id.scope.clone())
        })
        .unwrap_or_else(|| scope_for_view_key(view_key));
    match &source.owner {
        SourceOwner::BindingValue {
            binding: BindingTarget::Symbol(name),
            ..
        } => Ok((Expression::Symbol(name.clone()), scope)),
        SourceOwner::TopLevelForm { .. } if node.kind == NodeKind::Param => {
            let Some(name) = node.label.split_whitespace().nth(1) else {
                return Err(WriteBackError::UnsupportedHistoryEdit {
                    view_key: view_key.to_string(),
                    history_id: String::new(),
                    reason: "param history write source has no param name".to_string(),
                });
            };
            Ok((Expression::Symbol(name.to_string()), scope))
        }
        SourceOwner::MacroParameter { binding, .. } => Ok((
            Expression::Symbol(binding.name.clone()),
            binding.scope.clone(),
        )),
        _ if node.kind == NodeKind::Constant => parse_single_expression(&node.op)
            .map(|expr| (expr, scope))
            .map_err(|reason| WriteBackError::InvalidEdit {
                view_key: view_key.to_string(),
                node_id: node.id.clone(),
                reason,
            }),
        SourceOwner::TopLevelForm { .. } => {
            let Some(expr_id) = source.expr.as_ref() else {
                return Err(WriteBackError::UnsupportedHistoryEdit {
                    view_key: view_key.to_string(),
                    history_id: String::new(),
                    reason: "top-level history write source has no source expression".to_string(),
                });
            };
            let Some(expr) = document.expr(expr_id) else {
                return Err(WriteBackError::UnsupportedHistoryEdit {
                    view_key: view_key.to_string(),
                    history_id: String::new(),
                    reason: "top-level history write source expression is missing".to_string(),
                });
            };
            Ok((expr.clone(), scope))
        }
        _ => Err(WriteBackError::UnsupportedHistoryEdit {
            view_key: view_key.to_string(),
            history_id: String::new(),
            reason: "history write source requires generated binding allocation".to_string(),
        }),
    }
}

#[derive(Debug)]
struct HistoryNameAllocator {
    next_by_scope: HashMap<SourceScopeId, usize>,
}

impl HistoryNameAllocator {
    fn new(document: &SourceDocument) -> Self {
        let mut next_by_scope = HashMap::new();
        for scope in document.scopes() {
            next_by_scope.insert(scope.clone(), document.next_history_suffix(&scope));
        }
        Self { next_by_scope }
    }

    fn allocate(&mut self, scope: &SourceScopeId) -> String {
        let next = self.next_by_scope.entry(scope.clone()).or_insert(1);
        let name = format!("history{next}");
        *next += 1;
        name
    }
}

fn scope_for_view_key(view_key: &str) -> SourceScopeId {
    view_key
        .strip_prefix("macro:")
        .map(|name| SourceScopeId::Macro {
            name: name.to_string(),
        })
        .unwrap_or(SourceScopeId::Root)
}

fn apply_node_text_edit(
    document: &mut SourceDocument,
    view_key: &str,
    node: &PatchNode,
    text: &str,
) -> Result<(), WriteBackError> {
    if node.kind == NodeKind::CodeIsland {
        return Err(WriteBackError::EditedCodeIsland {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
        });
    }
    let source = node
        .source
        .as_ref()
        .ok_or_else(|| WriteBackError::MissingSourceOwner {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
        })?;
    let replacement = edited_expression_for_node(text, node, source.expr.as_ref(), document)
        .map_err(|reason| WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            reason,
        })?;
    if let Some(operator) = edited_operator(&replacement)
        && !document.is_known_operator(operator)
    {
        return Err(WriteBackError::UnknownOperator {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            operator: operator.to_string(),
        });
    }

    match &source.owner {
        SourceOwner::BindingValue {
            form_id,
            value_path,
            ..
        } => {
            let expr_id = SourceExprId {
                form_id: form_id.clone(),
                path: value_path.clone(),
            };
            document.replace_expr(&expr_id, replacement)
        }
        SourceOwner::NestedExpr { expr } => document.replace_expr(expr, replacement),
        SourceOwner::TopLevelForm { form_id } => {
            let expr_id = SourceExprId {
                form_id: form_id.clone(),
                path: Default::default(),
            };
            document.replace_expr(&expr_id, replacement)
        }
        SourceOwner::MacroParameter { .. } => Err(WriteBackError::UnsupportedSourceOwner {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            owner: "macro parameter rename belongs to Phase 6".to_string(),
        }),
        SourceOwner::CodeIsland { .. } => Err(WriteBackError::EditedCodeIsland {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
        }),
        other => Err(WriteBackError::UnsupportedSourceOwner {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            owner: format!("{other:?}"),
        }),
    }
}

fn edited_expression_for_node(
    text: &str,
    node: &PatchNode,
    source_expr: Option<&SourceExprId>,
    document: &SourceDocument,
) -> Result<Expression, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("node text cannot be empty for write-back".to_string());
    }

    if node.kind == NodeKind::Constant {
        return parse_single_expression(trimmed);
    }

    let edited = parse_single_expression(&format!("({trimmed})"))?;
    let Expression::List(edited_items) = edited else {
        return Err("node text must parse as a list expression".to_string());
    };
    if edited_items.is_empty() {
        return Err("node text must include an operator".to_string());
    }

    if edited_items.len() == 1
        && let Some(source_expr) = source_expr
        && let Some(Expression::List(original_items)) = document.expr(source_expr)
    {
        let mut merged = original_items.clone();
        merged[0] = edited_items[0].clone();
        return Ok(Expression::List(merged));
    }

    Ok(Expression::List(edited_items))
}

fn parse_single_expression(source: &str) -> Result<Expression, String> {
    let tokens = Parser::new(source.to_string())
        .parse()
        .map_err(|error| format!("failed to tokenize edited node text: {error:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("failed to parse edited node text: {error:?}"))?;
    match exprs.as_slice() {
        [expr] => Ok(expr.clone()),
        [] => Err("edited node text parsed to no expression".to_string()),
        _ => Err("edited node text must contain exactly one expression".to_string()),
    }
}

#[derive(Debug, Clone)]
struct SourceDocument {
    forms: Vec<SourceForm>,
    macros: HashMap<String, MacroDocument>,
}

#[derive(Debug, Clone)]
enum SourceForm {
    Expr(Expression),
    Macro(String),
}

#[derive(Debug, Clone)]
struct MacroDocument {
    name: String,
    params: Vec<Expression>,
    body: Vec<Expression>,
}

impl SourceDocument {
    fn parse(source: &str) -> Result<Self, WriteBackError> {
        let tokens = Parser::new(source.to_string()).parse().map_err(|error| {
            WriteBackError::Parse(format!("failed to tokenize source: {error:?}"))
        })?;
        let exprs = ASTParser::new(tokens)
            .parse()
            .map_err(|error| WriteBackError::Parse(format!("failed to parse source: {error:?}")))?;

        let mut forms = Vec::new();
        let mut macros = HashMap::new();
        for expr in exprs {
            if let Some(macro_doc) = MacroDocument::from_expr(&expr) {
                forms.push(SourceForm::Macro(macro_doc.name.clone()));
                macros.insert(macro_doc.name.clone(), macro_doc);
            } else {
                forms.push(SourceForm::Expr(expr));
            }
        }
        Ok(Self { forms, macros })
    }

    fn expr(&self, expr_id: &SourceExprId) -> Option<&Expression> {
        let form = self.form_expr(&expr_id.form_id.scope, expr_id.form_id.index)?;
        expr_at_path(form, &expr_id.path.0)
    }

    fn replace_expr(
        &mut self,
        expr_id: &SourceExprId,
        replacement: Expression,
    ) -> Result<(), WriteBackError> {
        let Some(form) = self.form_expr_mut(&expr_id.form_id.scope, expr_id.form_id.index) else {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key_for_scope(&expr_id.form_id.scope),
                node_id: String::new(),
                reason: format!("missing source form {}", expr_id.form_id.index),
            });
        };
        replace_expr_at_path(form, &expr_id.path.0, replacement).map_err(|reason| {
            WriteBackError::InvalidEdit {
                view_key: view_key_for_scope(&expr_id.form_id.scope),
                node_id: String::new(),
                reason,
            }
        })
    }

    fn form_expr(&self, scope: &SourceScopeId, index: usize) -> Option<&Expression> {
        match scope {
            SourceScopeId::Root => match self.forms.get(index)? {
                SourceForm::Expr(expr) => Some(expr),
                SourceForm::Macro(_) => None,
            },
            SourceScopeId::Macro { name } => self.macros.get(name)?.body.get(index),
        }
    }

    fn form_expr_mut(&mut self, scope: &SourceScopeId, index: usize) -> Option<&mut Expression> {
        match scope {
            SourceScopeId::Root => match self.forms.get_mut(index)? {
                SourceForm::Expr(expr) => Some(expr),
                SourceForm::Macro(_) => None,
            },
            SourceScopeId::Macro { name } => self.macros.get_mut(name)?.body.get_mut(index),
        }
    }

    fn emit(&self) -> String {
        self.forms
            .iter()
            .filter_map(|form| match form {
                SourceForm::Expr(expr) => Some(format_expression(expr)),
                SourceForm::Macro(name) => self.macros.get(name).map(MacroDocument::emit),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn prepend_form(
        &mut self,
        scope: &SourceScopeId,
        expr: Expression,
    ) -> Result<(), WriteBackError> {
        match scope {
            SourceScopeId::Root => {
                self.forms.insert(0, SourceForm::Expr(expr));
                Ok(())
            }
            SourceScopeId::Macro { name } => {
                let Some(macro_doc) = self.macros.get_mut(name) else {
                    return Err(WriteBackError::InvalidEdit {
                        view_key: view_key_for_scope(scope),
                        node_id: String::new(),
                        reason: "missing macro scope for history form".to_string(),
                    });
                };
                macro_doc.body.insert(0, expr);
                Ok(())
            }
        }
    }

    fn insert_history_write(
        &mut self,
        scope: &SourceScopeId,
        name: String,
        value: Expression,
    ) -> Result<(), WriteBackError> {
        let expr = Expression::List(vec![
            Expression::Symbol("write-history".to_string()),
            Expression::Symbol(name),
            value,
        ]);
        match scope {
            SourceScopeId::Root => {
                self.forms.push(SourceForm::Expr(expr));
                Ok(())
            }
            SourceScopeId::Macro { name } => {
                let Some(macro_doc) = self.macros.get_mut(name) else {
                    return Err(WriteBackError::InvalidEdit {
                        view_key: view_key_for_scope(scope),
                        node_id: String::new(),
                        reason: "missing macro scope for history write".to_string(),
                    });
                };
                let index = macro_doc.body.len().saturating_sub(1);
                macro_doc.body.insert(index, expr);
                Ok(())
            }
        }
    }

    fn scopes(&self) -> Vec<SourceScopeId> {
        let mut scopes = vec![SourceScopeId::Root];
        scopes.extend(
            self.macros
                .keys()
                .cloned()
                .map(|name| SourceScopeId::Macro { name }),
        );
        scopes
    }

    fn next_history_suffix(&self, scope: &SourceScopeId) -> usize {
        let mut max_suffix = 0usize;
        for expr in self.scope_forms(scope) {
            collect_history_suffixes(expr, &mut max_suffix);
        }
        max_suffix + 1
    }

    fn scope_forms(&self, scope: &SourceScopeId) -> Vec<&Expression> {
        match scope {
            SourceScopeId::Root => self
                .forms
                .iter()
                .filter_map(|form| match form {
                    SourceForm::Expr(expr) => Some(expr),
                    SourceForm::Macro(_) => None,
                })
                .collect(),
            SourceScopeId::Macro { name } => self
                .macros
                .get(name)
                .map(|macro_doc| macro_doc.body.iter().collect())
                .unwrap_or_default(),
        }
    }

    fn is_known_operator(&self, operator: &str) -> bool {
        matches!(
            operator,
            "def"
                | "defmacro"
                | "param"
                | "in"
                | "out"
                | "make-history"
                | "read-history"
                | "write-history"
        ) || dgenlisp_operator_names().contains(operator)
            || self.macros.contains_key(operator)
    }
}

impl MacroDocument {
    fn from_expr(expr: &Expression) -> Option<Self> {
        let Expression::List(items) = expr else {
            return None;
        };
        if symbol_at(items, 0) != Some("defmacro") {
            return None;
        }
        let name = symbol_at(items, 1)?.to_string();
        let params = match items.get(2)? {
            Expression::List(params) => params.clone(),
            _ => return None,
        };
        Some(Self {
            name,
            params,
            body: items.iter().skip(3).cloned().collect(),
        })
    }

    fn emit(&self) -> String {
        let mut items = vec![
            Expression::Symbol("defmacro".to_string()),
            Expression::Symbol(self.name.clone()),
            Expression::List(self.params.clone()),
        ];
        items.extend(self.body.clone());
        format_expression(&Expression::List(items))
    }
}

fn expr_at_path<'a>(mut expr: &'a Expression, path: &[ExprPathSegment]) -> Option<&'a Expression> {
    for segment in path {
        match (expr, segment) {
            (Expression::List(items), ExprPathSegment::ListItem(index)) => {
                expr = items.get(*index)?;
            }
            _ => return None,
        }
    }
    Some(expr)
}

fn replace_expr_at_path(
    expr: &mut Expression,
    path: &[ExprPathSegment],
    replacement: Expression,
) -> Result<(), String> {
    let Some((first, rest)) = path.split_first() else {
        *expr = replacement;
        return Ok(());
    };
    match (expr, first) {
        (Expression::List(items), ExprPathSegment::ListItem(index)) => {
            let Some(child) = items.get_mut(*index) else {
                return Err(format!(
                    "source expression path item {index} is out of bounds"
                ));
            };
            replace_expr_at_path(child, rest, replacement)
        }
        _ => Err("source expression path does not point into a list".to_string()),
    }
}

fn view_key_for_scope(scope: &SourceScopeId) -> String {
    match scope {
        SourceScopeId::Root => "root".to_string(),
        SourceScopeId::Macro { name } => format!("macro:{name}"),
    }
}

fn edited_operator(expr: &Expression) -> Option<&str> {
    let Expression::List(items) = expr else {
        return None;
    };
    symbol_at(items, 0)
}

fn collect_history_suffixes(expr: &Expression, max_suffix: &mut usize) {
    match expr {
        Expression::List(items) => {
            if matches!(
                symbol_at(items, 0),
                Some("make-history" | "read-history" | "write-history")
            ) && let Some(name) = symbol_at(items, 1)
                && let Some(suffix) = name.strip_prefix("history").and_then(|s| s.parse().ok())
            {
                *max_suffix = (*max_suffix).max(suffix);
            }
            for item in items {
                collect_history_suffixes(item, max_suffix);
            }
        }
        Expression::QuoteList(items) => {
            for item in items {
                collect_history_suffixes(item, max_suffix);
            }
        }
        Expression::Quasiquote(inner) | Expression::Unquote(inner) => {
            collect_history_suffixes(inner, max_suffix);
        }
        _ => {}
    }
}
