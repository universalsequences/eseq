use std::collections::{HashMap, HashSet};

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
    active_patcher_view_key, connection_edit_key, node_edit_key, source_connection_id,
};

const MISSING_INPUT_SENTINEL: &str = "__patcher_missing_input__";

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
    UnsupportedGeneratedBinding {
        view_key: String,
        node_id: String,
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
    apply_node_deletions(&mut document, &root_patch, interaction_state)?;

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
                if !created_history_edit(edit) && !created_value_edit(edit) {
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

    let generated =
        apply_generated_binding_writeback(&mut document, &root_patch, interaction_state)?;
    apply_cable_writeback(&mut document, &root_patch, interaction_state, &generated)?;
    apply_history_writeback(&mut document, &root_patch, interaction_state, &generated)?;

    Ok(document.emit())
}

fn validate_connection_edits(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> Result<(), WriteBackError> {
    for edit in interaction_state.edit_state.connections.values() {
        match edit.origin {
            PatcherConnectionOrigin::Created { .. } => {
                if !connection_edit_touches_history(root_patch, interaction_state, edit)
                    && !connection_edit_touches_created_value(interaction_state, edit)
                    && !connection_edit_has_source_destination(root_patch, edit)
                {
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
        ) && !deleted_connection_has_created_value_replacement(
            root_patch,
            interaction_state,
            &view_key,
            &connection_id,
        ) && !source_connection_is_deletable(root_patch, &view_key, &connection_id)
            && !deleted_connection_is_incident_to_deleted_node(
                root_patch,
                interaction_state,
                &view_key,
                &connection_id,
            )
        {
            return Err(WriteBackError::UnsupportedDeletedConnection {
                view_key,
                connection_id,
            });
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
enum SourceDeletionTarget {
    Form(SourceFormId),
    Expr(SourceExprId),
}

fn apply_node_deletions(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> Result<(), WriteBackError> {
    let deletion_targets = interaction_state
        .edit_state
        .deleted_nodes
        .iter()
        .map(|key| split_scoped_key(key))
        .map(|(view_key, node_id)| {
            let Some(node) =
                patch_for_view(root_patch, &view_key).and_then(|patch| patch_node(patch, &node_id))
            else {
                return Err(WriteBackError::UnsupportedDeletedNode { view_key, node_id });
            };
            let Some(source) = node.source.as_ref() else {
                return Err(WriteBackError::MissingSourceOwner { view_key, node_id });
            };
            match &source.owner {
                SourceOwner::TopLevelForm { form_id } => Ok((
                    view_key,
                    node_id,
                    SourceDeletionTarget::Form(form_id.clone()),
                )),
                SourceOwner::BindingValue { form_id, .. } => Ok((
                    view_key,
                    node_id,
                    SourceDeletionTarget::Form(form_id.clone()),
                )),
                SourceOwner::NestedExpr { expr } => {
                    Ok((view_key, node_id, SourceDeletionTarget::Expr(expr.clone())))
                }
                SourceOwner::CodeIsland { .. } => {
                    return Err(WriteBackError::EditedCodeIsland { view_key, node_id });
                }
                _ => {
                    return Err(WriteBackError::UnsupportedDeletedNode { view_key, node_id });
                }
            }
        })
        .collect::<Result<Vec<_>, WriteBackError>>()?;

    let mut deleted_forms = deletion_targets
        .iter()
        .filter_map(|(_, _, target)| match target {
            SourceDeletionTarget::Form(form_id) => Some(form_id.clone()),
            SourceDeletionTarget::Expr(_) => None,
        })
        .collect::<Vec<_>>();
    deleted_forms.sort_by(|left, right| {
        source_scope_sort_key(&left.scope)
            .cmp(&source_scope_sort_key(&right.scope))
            .then_with(|| right.index.cmp(&left.index))
    });
    deleted_forms.dedup();

    let mut deleted_exprs = deletion_targets
        .iter()
        .filter_map(|(_, _, target)| match target {
            SourceDeletionTarget::Form(_) => None,
            SourceDeletionTarget::Expr(expr) => Some(expr.clone()),
        })
        .filter(|expr| !deleted_forms.iter().any(|form_id| form_id == &expr.form_id))
        .collect::<Vec<_>>();
    deleted_exprs.sort_by(|left, right| {
        source_scope_sort_key(&left.form_id.scope)
            .cmp(&source_scope_sort_key(&right.form_id.scope))
            .then_with(|| left.form_id.index.cmp(&right.form_id.index))
            .then_with(|| expr_path_indexes(left).cmp(&expr_path_indexes(right)))
    });
    deleted_exprs.dedup();
    deleted_exprs = deleted_exprs
        .iter()
        .filter(|expr| {
            !deleted_exprs
                .iter()
                .any(|candidate| candidate != *expr && source_expr_is_ancestor(candidate, expr))
        })
        .cloned()
        .collect();

    for expr in deleted_exprs {
        document.replace_expr(
            &expr,
            Expression::Symbol(MISSING_INPUT_SENTINEL.to_string()),
        )?;
    }

    for form_id in deleted_forms {
        document.remove_form(&form_id)?;
    }
    Ok(())
}

fn source_expr_is_ancestor(candidate: &SourceExprId, expr: &SourceExprId) -> bool {
    candidate.form_id == expr.form_id
        && candidate.path.0.len() < expr.path.0.len()
        && expr.path.0.starts_with(&candidate.path.0)
}

fn expr_path_indexes(expr: &SourceExprId) -> Vec<usize> {
    expr.path
        .0
        .iter()
        .map(|segment| match segment {
            ExprPathSegment::ListItem(index) => *index,
        })
        .collect()
}

fn source_scope_sort_key(scope: &SourceScopeId) -> (&str, &str) {
    match scope {
        SourceScopeId::Root => ("", ""),
        SourceScopeId::Macro { name } => ("macro", name.as_str()),
    }
}

fn deleted_connection_is_incident_to_deleted_node(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    connection_id: &str,
) -> bool {
    let Some(connection) = source_connection(root_patch, view_key, connection_id) else {
        return false;
    };
    interaction_state
        .edit_state
        .deleted_nodes
        .contains(&node_edit_key(view_key, &connection.from_node))
        || interaction_state
            .edit_state
            .deleted_nodes
            .contains(&node_edit_key(view_key, &connection.to_node))
}

fn connection_edit_has_source_destination(
    root_patch: &Patch,
    edit: &PatcherConnectionEdit,
) -> bool {
    patch_for_view(root_patch, &edit.view_key)
        .and_then(|patch| patch_node(patch, &edit.to.node_id))
        .and_then(|node| node.source.as_ref())
        .is_some()
}

fn source_connection_is_deletable(root_patch: &Patch, view_key: &str, connection_id: &str) -> bool {
    let Some(connection) = source_connection(root_patch, view_key, connection_id) else {
        return false;
    };
    patch_for_view(root_patch, view_key)
        .and_then(|patch| patch_node(patch, &connection.to_node))
        .and_then(|node| node.source.as_ref())
        .is_some()
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

fn created_value_edit(edit: &super::state::PatcherNodeEdit) -> bool {
    matches!(&edit.origin, PatcherNodeOrigin::Created { .. })
        && !edit.text.trim().is_empty()
        && !created_history_edit(edit)
}

fn connection_edit_touches_created_value(
    interaction_state: &PatcherInteractionState,
    edit: &PatcherConnectionEdit,
) -> bool {
    created_value_node(interaction_state, &edit.view_key, &edit.from.node_id).is_some()
        || created_value_node(interaction_state, &edit.view_key, &edit.to.node_id).is_some()
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

fn deleted_connection_has_created_value_replacement(
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
            created_value_node(interaction_state, view_key, &edit.from.node_id).is_some()
                && edit.to == to
        })
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

fn created_value_node<'a>(
    interaction_state: &'a PatcherInteractionState,
    view_key: &str,
    node_id: &str,
) -> Option<&'a super::state::PatcherNodeEdit> {
    interaction_state
        .edit_state
        .nodes
        .get(&node_edit_key(view_key, node_id))
        .filter(|edit| created_value_edit(edit))
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

#[derive(Debug, Clone, Default)]
struct GeneratedBindings {
    names: HashMap<(String, String), String>,
}

impl GeneratedBindings {
    fn insert(&mut self, view_key: &str, node_id: &str, name: String) {
        self.names
            .insert((view_key.to_string(), node_id.to_string()), name);
    }

    fn get(&self, view_key: &str, node_id: &str) -> Option<&str> {
        self.names
            .get(&(view_key.to_string(), node_id.to_string()))
            .map(String::as_str)
    }
}

fn apply_generated_binding_writeback(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> Result<GeneratedBindings, WriteBackError> {
    let mut generated = GeneratedBindings::default();
    let mut allocator = GeneratedNameAllocator::new(document);
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

    let mut pending_defs: Vec<(SourceScopeId, usize, usize, Expression)> = Vec::new();
    let mut next_generated_def_order = 0usize;
    for view_key in views {
        let scope = scope_for_view_key(&view_key);
        let mut created_nodes = interaction_state
            .edit_state
            .nodes
            .values()
            .filter(|edit| edit.view_key == view_key)
            .filter(|edit| created_value_edit(edit))
            .collect::<Vec<_>>();
        created_nodes.sort_by(|a, b| a.id.cmp(&b.id));

        for edit in &created_nodes {
            let op = created_node_operator(edit)?;
            let name = allocator.allocate(&scope, &op);
            generated.insert(&view_key, &edit.id, name);
        }

        for edit in created_nodes {
            let name = generated
                .get(&view_key, &edit.id)
                .expect("created node name allocated before emission")
                .to_string();
            let generated_expr = created_node_expression(
                document,
                root_patch,
                interaction_state,
                &generated,
                &view_key,
                edit,
            )?;
            let insertion_index = generated_def_insertion_index(
                document,
                root_patch,
                interaction_state,
                &generated,
                &view_key,
                edit,
            )?;
            pending_defs.push((
                scope.clone(),
                insertion_index,
                next_generated_def_order,
                Expression::List(vec![
                    Expression::Symbol("def".to_string()),
                    Expression::Symbol(name),
                    generated_expr,
                ]),
            ));
            next_generated_def_order += 1;
            rewrite_created_value_consumers(
                document,
                root_patch,
                interaction_state,
                &generated,
                &view_key,
                edit,
            )?;
        }
    }

    pending_defs.sort_by(
        |(scope_a, index_a, order_a, _), (scope_b, index_b, order_b, _)| {
            view_key_for_scope(scope_b)
                .cmp(&view_key_for_scope(scope_a))
                .then(index_b.cmp(index_a))
                .then(order_b.cmp(order_a))
        },
    );
    for (scope, index, _, expr) in pending_defs {
        document.insert_form(&scope, index, expr)?;
    }
    Ok(generated)
}

fn created_node_operator(edit: &super::state::PatcherNodeEdit) -> Result<String, WriteBackError> {
    let expr = parse_created_node_text(edit)?;
    let Some(op) = edited_operator(&expr) else {
        return Err(WriteBackError::UnsupportedGeneratedBinding {
            view_key: edit.view_key.clone(),
            node_id: edit.id.clone(),
            reason: "created value node text must start with an operator".to_string(),
        });
    };
    Ok(op.to_string())
}

fn parse_created_node_text(
    edit: &super::state::PatcherNodeEdit,
) -> Result<Expression, WriteBackError> {
    parse_single_expression(&format!("({})", edit.text.trim())).map_err(|reason| {
        WriteBackError::InvalidEdit {
            view_key: edit.view_key.clone(),
            node_id: edit.id.clone(),
            reason,
        }
    })
}

fn created_node_expression(
    document: &SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    view_key: &str,
    edit: &super::state::PatcherNodeEdit,
) -> Result<Expression, WriteBackError> {
    let mut expr = parse_created_node_text(edit)?;
    let Expression::List(items) = &mut expr else {
        return Err(WriteBackError::UnsupportedGeneratedBinding {
            view_key: view_key.to_string(),
            node_id: edit.id.clone(),
            reason: "created value node text must parse as a call".to_string(),
        });
    };
    let Some(op) = symbol_at(items, 0) else {
        return Err(WriteBackError::UnsupportedGeneratedBinding {
            view_key: view_key.to_string(),
            node_id: edit.id.clone(),
            reason: "created value node operator must be a symbol".to_string(),
        });
    };
    if !document.is_known_operator(op) {
        return Err(WriteBackError::UnknownOperator {
            view_key: view_key.to_string(),
            node_id: edit.id.clone(),
            operator: op.to_string(),
        });
    }

    let mut inbound = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.to.node_id == edit.id)
        .collect::<Vec<_>>();
    inbound.sort_by_key(|connection| connection.to.input_index);
    for connection in inbound {
        let value =
            value_reference_expr(document, root_patch, generated, view_key, &connection.from)?;
        let item_index = connection.to.input_index + 1;
        while items.len() <= item_index {
            items.push(Expression::Symbol(MISSING_INPUT_SENTINEL.to_string()));
        }
        items[item_index] = value;
    }
    Ok(expr)
}

fn value_reference_expr(
    document: &SourceDocument,
    root_patch: &Patch,
    generated: &GeneratedBindings,
    view_key: &str,
    from: &OutputPortRef,
) -> Result<Expression, WriteBackError> {
    if let Some(name) = generated.get(view_key, &from.node_id) {
        return Ok(Expression::Symbol(name.to_string()));
    }
    let Some(node) =
        patch_for_view(root_patch, view_key).and_then(|patch| patch_node(patch, &from.node_id))
    else {
        return Err(WriteBackError::UnsupportedGeneratedBinding {
            view_key: view_key.to_string(),
            node_id: from.node_id.clone(),
            reason: "generated binding source must be source-backed or generated".to_string(),
        });
    };
    node_reference_expr(document, node, view_key)
}

fn node_reference_expr(
    document: &SourceDocument,
    node: &PatchNode,
    view_key: &str,
) -> Result<Expression, WriteBackError> {
    let source = node
        .source
        .as_ref()
        .ok_or_else(|| WriteBackError::MissingSourceOwner {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
        })?;
    match &source.owner {
        SourceOwner::BindingValue {
            binding: BindingTarget::Symbol(name),
            ..
        } => Ok(Expression::Symbol(name.clone())),
        SourceOwner::TopLevelForm { .. } if node.kind == NodeKind::Param => {
            let Some(name) = node.label.split_whitespace().nth(1) else {
                return Err(WriteBackError::UnsupportedGeneratedBinding {
                    view_key: view_key.to_string(),
                    node_id: node.id.clone(),
                    reason: "param source has no parameter name".to_string(),
                });
            };
            Ok(Expression::Symbol(name.to_string()))
        }
        SourceOwner::MacroParameter { binding, .. } => Ok(Expression::Symbol(binding.name.clone())),
        _ if node.kind == NodeKind::Constant => {
            parse_single_expression(&node.op).map_err(|reason| WriteBackError::InvalidEdit {
                view_key: view_key.to_string(),
                node_id: node.id.clone(),
                reason,
            })
        }
        SourceOwner::NestedExpr { expr } => document.expr(expr).cloned().ok_or_else(|| {
            WriteBackError::UnsupportedGeneratedBinding {
                view_key: view_key.to_string(),
                node_id: node.id.clone(),
                reason: "nested source expression is missing".to_string(),
            }
        }),
        SourceOwner::TopLevelForm { .. } => source
            .expr
            .as_ref()
            .and_then(|expr| document.expr(expr))
            .cloned()
            .ok_or_else(|| WriteBackError::UnsupportedGeneratedBinding {
                view_key: view_key.to_string(),
                node_id: node.id.clone(),
                reason: "top-level source expression is missing".to_string(),
            }),
        _ => Err(WriteBackError::UnsupportedGeneratedBinding {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            reason: "source owner cannot be referenced by generated binding".to_string(),
        }),
    }
}

fn generated_def_insertion_index(
    document: &SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    view_key: &str,
    edit: &super::state::PatcherNodeEdit,
) -> Result<usize, WriteBackError> {
    let scope = scope_for_view_key(view_key);
    let dependency_index =
        generated_node_dependencies(root_patch, interaction_state, view_key, edit)
            .into_iter()
            .filter_map(|node_id| {
                patch_for_view(root_patch, view_key)
                    .and_then(|patch| patch_node(patch, &node_id))
                    .and_then(|node| source_owner_location_for_node(node))
                    .filter(|(form_id, _)| form_id.scope == scope)
                    .map(|(form_id, _)| form_id.index)
            })
            .max()
            .map(|index| index + 1)
            .unwrap_or(0);
    let consumer_index =
        generated_node_consumers(root_patch, interaction_state, view_key, edit, generated)
            .into_iter()
            .filter_map(|node_id| {
                patch_for_view(root_patch, view_key)
                    .and_then(|patch| patch_node(patch, &node_id))
                    .and_then(|node| source_owner_location_for_node(node))
                    .filter(|(form_id, _)| form_id.scope == scope)
                    .map(|(form_id, _)| form_id.index)
            })
            .min()
            .unwrap_or_else(|| document.scope_len(&scope));
    if dependency_index > consumer_index {
        return Err(WriteBackError::UnsupportedGeneratedBinding {
            view_key: view_key.to_string(),
            node_id: edit.id.clone(),
            reason: "generated binding dependencies appear after its consumers".to_string(),
        });
    }
    Ok(dependency_index)
}

fn generated_node_dependencies(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    edit: &super::state::PatcherNodeEdit,
) -> Vec<String> {
    interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.to.node_id == edit.id)
        .filter(|connection| {
            created_value_node(interaction_state, view_key, &connection.from.node_id).is_none()
        })
        .filter(|connection| {
            patch_for_view(root_patch, view_key)
                .and_then(|patch| patch_node(patch, &connection.from.node_id))
                .is_some()
        })
        .map(|connection| connection.from.node_id.clone())
        .collect()
}

fn generated_node_consumers(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    edit: &super::state::PatcherNodeEdit,
    generated: &GeneratedBindings,
) -> Vec<String> {
    interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.from.node_id == edit.id)
        .filter(|connection| generated.get(view_key, &connection.to.node_id).is_none())
        .filter(|connection| {
            patch_for_view(root_patch, view_key)
                .and_then(|patch| patch_node(patch, &connection.to.node_id))
                .is_some()
        })
        .map(|connection| connection.to.node_id.clone())
        .collect()
}

fn source_owner_location_for_node(node: &PatchNode) -> Option<(&SourceFormId, usize)> {
    let source = node.source.as_ref()?;
    source_owner_location(&source.owner, source.expr.as_ref())
}

fn rewrite_created_value_consumers(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    view_key: &str,
    edit: &super::state::PatcherNodeEdit,
) -> Result<(), WriteBackError> {
    let Some(name) = generated.get(view_key, &edit.id) else {
        return Ok(());
    };
    let mut consumers = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.from.node_id == edit.id)
        .filter(|connection| generated.get(view_key, &connection.to.node_id).is_none())
        .collect::<Vec<_>>();
    consumers.sort_by_key(|connection| (connection.to.node_id.clone(), connection.to.input_index));
    for connection in consumers {
        let Some(dest) = patch_for_view(root_patch, view_key)
            .and_then(|patch| patch_node(patch, &connection.to.node_id))
        else {
            return Err(WriteBackError::UnsupportedGeneratedBinding {
                view_key: view_key.to_string(),
                node_id: edit.id.clone(),
                reason: "generated binding consumer must be source-backed".to_string(),
            });
        };
        rewrite_node_input(
            document,
            view_key,
            dest,
            connection.to.input_index,
            Expression::Symbol(name.to_string()),
        )?;
    }
    Ok(())
}

fn rewrite_node_input(
    document: &mut SourceDocument,
    view_key: &str,
    node: &PatchNode,
    input_index: usize,
    value: Expression,
) -> Result<(), WriteBackError> {
    if let Some(source) = node.source.as_ref() {
        if let Some(call_shape) = source.call_shape.as_ref()
            && let Some(arg) = call_shape
                .positional_args
                .iter()
                .find(|arg| arg.semantic_index == input_index)
        {
            return document.replace_expr(&arg.expr, value);
        }
        if node.kind == NodeKind::Out
            && input_index == 0
            && let SourceOwner::TopLevelForm { form_id } = &source.owner
        {
            if let Some(call_shape) = source.call_shape.as_ref()
                && let Some(arg) = call_shape
                    .positional_args
                    .iter()
                    .find(|arg| arg.semantic_index == input_index)
            {
                return document.replace_expr(&arg.expr, value);
            }
            return document.replace_expr(
                &SourceExprId {
                    form_id: form_id.clone(),
                    path: Default::default(),
                },
                value,
            );
        }
    }
    Err(WriteBackError::UnsupportedGeneratedBinding {
        view_key: view_key.to_string(),
        node_id: node.id.clone(),
        reason: "generated binding consumer has no source-owned positional argument".to_string(),
    })
}

fn apply_cable_writeback(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
) -> Result<(), WriteBackError> {
    let mut deleted = interaction_state
        .edit_state
        .deleted_connections
        .iter()
        .map(|key| split_scoped_key(key))
        .collect::<Vec<_>>();
    deleted.sort();
    for (view_key, connection_id) in deleted {
        if deleted_connection_is_incident_to_deleted_node(
            root_patch,
            interaction_state,
            &view_key,
            &connection_id,
        ) || deleted_connection_has_history_replacement(
            root_patch,
            interaction_state,
            &view_key,
            &connection_id,
        ) || deleted_connection_has_created_value_replacement(
            root_patch,
            interaction_state,
            &view_key,
            &connection_id,
        ) {
            continue;
        }
        let Some(connection) = source_connection(root_patch, &view_key, &connection_id) else {
            return Err(WriteBackError::UnsupportedDeletedConnection {
                view_key,
                connection_id,
            });
        };
        let Some(dest) = patch_for_view(root_patch, &view_key)
            .and_then(|patch| patch_node(patch, &connection.to_node))
        else {
            return Err(WriteBackError::UnsupportedDeletedConnection {
                view_key,
                connection_id,
            });
        };
        rewrite_node_input(
            document,
            &view_key,
            dest,
            connection.to_input,
            Expression::Symbol(MISSING_INPUT_SENTINEL.to_string()),
        )?;
    }

    let mut created = interaction_state
        .edit_state
        .connections
        .values()
        .collect::<Vec<_>>();
    created.sort_by(|a, b| a.id.cmp(&b.id));
    for connection in created {
        if connection_edit_touches_history(root_patch, interaction_state, connection)
            || connection_edit_touches_created_value(interaction_state, connection)
        {
            continue;
        }
        let value = value_reference_expr(
            document,
            root_patch,
            generated,
            &connection.view_key,
            &connection.from,
        )?;
        let Some(dest) = patch_for_view(root_patch, &connection.view_key)
            .and_then(|patch| patch_node(patch, &connection.to.node_id))
        else {
            return Err(WriteBackError::UnsupportedCreatedConnection {
                view_key: connection.view_key.clone(),
                connection_id: connection.id.clone(),
            });
        };
        rewrite_node_input(
            document,
            &connection.view_key,
            dest,
            connection.to.input_index,
            value,
        )?;
    }
    Ok(())
}

fn apply_history_writeback(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
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
                    generated,
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
                if generated.get(&view_key, &connection.from.node_id).is_some()
                    || generated.get(&view_key, &connection.to.node_id).is_some()
                    || connection_edit_has_source_destination(root_patch, connection)
                {
                    continue;
                }
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
    generated: &GeneratedBindings,
    view_key: &str,
    from: &OutputPortRef,
) -> Result<(Expression, SourceScopeId), WriteBackError> {
    if let Some(name) = generated.get(view_key, &from.node_id) {
        return Ok((
            Expression::Symbol(name.to_string()),
            scope_for_view_key(view_key),
        ));
    }
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

#[derive(Debug)]
struct GeneratedNameAllocator {
    used_by_scope: HashMap<SourceScopeId, HashSet<String>>,
    next_by_scope_stem: HashMap<(SourceScopeId, String), usize>,
}

impl GeneratedNameAllocator {
    fn new(document: &SourceDocument) -> Self {
        let mut used_by_scope = HashMap::new();
        for scope in document.scopes() {
            used_by_scope.insert(scope.clone(), document.reserved_names(&scope));
        }
        Self {
            used_by_scope,
            next_by_scope_stem: HashMap::new(),
        }
    }

    fn allocate(&mut self, scope: &SourceScopeId, operator: &str) -> String {
        let stem = generated_binding_stem(operator);
        let used = self.used_by_scope.entry(scope.clone()).or_default();
        let key = (scope.clone(), stem.clone());
        let mut next = self
            .next_by_scope_stem
            .remove(&key)
            .unwrap_or_else(|| next_suffix_for_stem(used, &stem));
        loop {
            let candidate = format!("{stem}{next}");
            next += 1;
            if used.insert(candidate.clone()) {
                self.next_by_scope_stem.insert(key, next);
                return candidate;
            }
        }
    }
}

fn generated_binding_stem(operator: &str) -> String {
    let mapped = match operator {
        "*" => "mul",
        "+" => "add",
        "-" => "sub",
        "/" => "div",
        _ => operator,
    };
    let stem = mapped
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        .collect::<String>();
    if stem.is_empty() || stem.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        "generated".to_string()
    } else {
        stem
    }
}

fn next_suffix_for_stem(used: &HashSet<String>, stem: &str) -> usize {
    used.iter()
        .filter_map(|name| name.strip_prefix(stem))
        .filter(|suffix| !suffix.is_empty())
        .filter_map(|suffix| suffix.parse::<usize>().ok())
        .max()
        .map(|value| value + 1)
        .unwrap_or(1)
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

    fn remove_form(&mut self, form_id: &SourceFormId) -> Result<(), WriteBackError> {
        match &form_id.scope {
            SourceScopeId::Root => {
                if form_id.index >= self.forms.len() {
                    return Err(WriteBackError::InvalidEdit {
                        view_key: view_key_for_scope(&form_id.scope),
                        node_id: String::new(),
                        reason: format!("missing root source form {}", form_id.index),
                    });
                }
                self.forms.remove(form_id.index);
                Ok(())
            }
            SourceScopeId::Macro { name } => {
                let Some(macro_doc) = self.macros.get_mut(name) else {
                    return Err(WriteBackError::InvalidEdit {
                        view_key: view_key_for_scope(&form_id.scope),
                        node_id: String::new(),
                        reason: "missing macro scope for form removal".to_string(),
                    });
                };
                if form_id.index >= macro_doc.body.len() {
                    return Err(WriteBackError::InvalidEdit {
                        view_key: view_key_for_scope(&form_id.scope),
                        node_id: String::new(),
                        reason: format!("missing macro source form {}", form_id.index),
                    });
                }
                macro_doc.body.remove(form_id.index);
                Ok(())
            }
        }
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

    fn insert_form(
        &mut self,
        scope: &SourceScopeId,
        index: usize,
        expr: Expression,
    ) -> Result<(), WriteBackError> {
        match scope {
            SourceScopeId::Root => {
                let index = index.min(self.forms.len());
                self.forms.insert(index, SourceForm::Expr(expr));
                Ok(())
            }
            SourceScopeId::Macro { name } => {
                let Some(macro_doc) = self.macros.get_mut(name) else {
                    return Err(WriteBackError::InvalidEdit {
                        view_key: view_key_for_scope(scope),
                        node_id: String::new(),
                        reason: "missing macro scope for generated binding".to_string(),
                    });
                };
                let index = index.min(macro_doc.body.len());
                macro_doc.body.insert(index, expr);
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

    fn scope_len(&self, scope: &SourceScopeId) -> usize {
        match scope {
            SourceScopeId::Root => self.forms.len(),
            SourceScopeId::Macro { name } => self
                .macros
                .get(name)
                .map(|macro_doc| macro_doc.body.len())
                .unwrap_or(0),
        }
    }

    fn reserved_names(&self, scope: &SourceScopeId) -> HashSet<String> {
        let mut names = HashSet::new();
        match scope {
            SourceScopeId::Root => {
                for form in &self.forms {
                    match form {
                        SourceForm::Expr(expr) => collect_scope_binding_names(expr, &mut names),
                        SourceForm::Macro(name) => {
                            names.insert(name.clone());
                        }
                    }
                }
            }
            SourceScopeId::Macro { name } => {
                if let Some(macro_doc) = self.macros.get(name) {
                    for param in &macro_doc.params {
                        if let Expression::Symbol(name) = param {
                            names.insert(name.clone());
                        }
                    }
                    for expr in &macro_doc.body {
                        collect_scope_binding_names(expr, &mut names);
                    }
                }
            }
        }
        names
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

fn collect_scope_binding_names(expr: &Expression, names: &mut HashSet<String>) {
    let Expression::List(items) = expr else {
        return;
    };
    match symbol_at(items, 0) {
        Some("def") => match items.get(1) {
            Some(Expression::Symbol(name)) => {
                names.insert(name.clone());
            }
            Some(Expression::List(outputs)) => {
                for output in outputs {
                    if let Expression::Symbol(name) = output {
                        names.insert(name.clone());
                    }
                }
            }
            _ => {}
        },
        Some("param" | "make-history") => {
            if let Some(name) = symbol_at(items, 1) {
                names.insert(name.to_string());
            }
        }
        _ => {}
    }
}
