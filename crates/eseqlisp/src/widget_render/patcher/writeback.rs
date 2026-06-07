use std::collections::{BTreeMap, HashMap, HashSet};

use crate::defmacro_library::{DefmacroLibrary, parse_use_defmacro};
use crate::parser::{ASTParser, Expression, Parser, format_expression};

use super::display::{node_display_input_slots, node_display_label};
use super::lisp::{
    node_kind_for_op, parse_patch_source, parse_patch_source_with_library, positional_args,
    symbol_at,
};
use super::model::{
    ArgValue, BindingId, BindingKind, BindingTarget, ExprPathSegment, InputPortRef, NodeKind,
    OutputPortRef, Patch, PatchConnection, PatchNode, PatcherIntent, SourceArgValue, SourceExprId,
    SourceFormId, SourceOwner, SourceScopeId,
};
use super::project::{dgenlisp_operator_names, dgenlisp_operator_required_input_counts};
use super::state::{
    PatcherConnectionEdit, PatcherConnectionOrigin, PatcherInteractionState, PatcherNodeOrigin,
    active_patcher_view_key, connection_edit_key, debug_log_writeback_event,
    default_created_macro_source, node_edit_key, patch_with_created_macros, source_connection_id,
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
    BindingRenameCollision {
        view_key: String,
        node_id: String,
        name: String,
    },
    BindingRenameBlockedByCodeIsland {
        view_key: String,
        node_id: String,
        name: String,
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
    emit_patch_writeback_result(source, intent, interaction_state).map(|result| result.source)
}

pub(super) fn replace_macro_source(
    source: &str,
    macro_name: &str,
    macro_source: &str,
) -> Result<String, WriteBackError> {
    let mut document = SourceDocument::parse(source)?;
    let expr =
        parse_single_expression(macro_source).map_err(|reason| WriteBackError::InvalidEdit {
            view_key: "root".to_string(),
            node_id: String::new(),
            reason,
        })?;
    document.replace_macro(macro_name, expr)?;
    Ok(document.emit())
}

#[derive(Debug, Clone)]
pub(super) struct PatchWritebackResult {
    pub(super) source: String,
    pub(super) generated_node_ids: HashMap<(String, String), String>,
}

pub(super) fn emit_patch_writeback_result(
    source: &str,
    intent: PatcherIntent,
    interaction_state: &PatcherInteractionState,
) -> Result<PatchWritebackResult, WriteBackError> {
    let root_patch = parse_patch_source(source, intent).map_err(WriteBackError::Parse)?;
    emit_patch_writeback_result_for_root_patch(source, intent, interaction_state, root_patch, None)
}

pub(super) fn emit_patch_writeback_result_with_library(
    source: &str,
    intent: PatcherIntent,
    interaction_state: &PatcherInteractionState,
    library: &DefmacroLibrary,
) -> Result<PatchWritebackResult, WriteBackError> {
    let root_patch =
        parse_patch_source_with_library(source, intent, library).map_err(WriteBackError::Parse)?;
    emit_patch_writeback_result_for_root_patch(
        source,
        intent,
        interaction_state,
        root_patch,
        Some(library),
    )
}

fn emit_patch_writeback_result_for_root_patch(
    source: &str,
    intent: PatcherIntent,
    interaction_state: &PatcherInteractionState,
    root_patch: Patch,
    library: Option<&DefmacroLibrary>,
) -> Result<PatchWritebackResult, WriteBackError> {
    let mut document = SourceDocument::parse(source)?;
    if let Some(library) = library {
        document.register_external_macros(library.packages().keys().cloned());
    }
    register_created_modulatable_params(&mut document, interaction_state);
    apply_created_macro_writeback(&mut document, &root_patch, interaction_state)?;
    let effective_root_patch = patch_with_created_macros(root_patch.clone(), interaction_state);
    apply_created_macro_parameter_writeback(&mut document, interaction_state)?;

    validate_connection_edits(&effective_root_patch, interaction_state)?;
    apply_node_deletions(&mut document, &effective_root_patch, interaction_state)?;

    let active_view_key = active_patcher_view_key(interaction_state);
    if let Some(text_edit) = interaction_state.text_edit.as_ref() {
        let patch = patch_for_view(&effective_root_patch, &active_view_key).ok_or_else(|| {
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
            apply_node_text_edit(
                &mut document,
                &effective_root_patch,
                &active_view_key,
                node,
                &text_edit.text,
            )?;
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
        source_edit_sort_key(&effective_root_patch, edit).unwrap_or_else(|| {
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
                if !created_history_edit(edit)
                    && !created_value_edit(edit)
                    && !created_out_edit(edit)
                    && !created_macro_parameter_edit(edit)
                    && !created_macro_instance_edit(interaction_state, edit)
                {
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
                let patch =
                    patch_for_view(&effective_root_patch, &edit.view_key).ok_or_else(|| {
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
                if edit.text.trim() != node_display_label(node).trim()
                    || node_requires_param_form_normalization(node)
                {
                    apply_node_text_edit(
                        &mut document,
                        &effective_root_patch,
                        &edit.view_key,
                        node,
                        &edit.text,
                    )?;
                }
            }
        }
    }

    ensure_host_modulator_inputs_if_needed(&mut document, intent, interaction_state)?;

    let history_bindings =
        resolve_history_bindings(&document, &effective_root_patch, interaction_state);
    let generated = apply_generated_binding_writeback(
        &mut document,
        &effective_root_patch,
        interaction_state,
        &history_bindings,
    )?;
    apply_created_out_writeback(
        &mut document,
        &effective_root_patch,
        interaction_state,
        &generated,
        &history_bindings,
        CreatedOutWritebackScope::Root,
    )?;
    apply_created_out_writeback(
        &mut document,
        &effective_root_patch,
        interaction_state,
        &generated,
        &history_bindings,
        CreatedOutWritebackScope::Macro,
    )?;
    apply_cable_writeback(
        &mut document,
        &effective_root_patch,
        interaction_state,
        &generated,
        &history_bindings,
    )?;
    apply_history_writeback(
        &mut document,
        &effective_root_patch,
        interaction_state,
        &generated,
        &history_bindings,
    )?;
    let macro_prune_candidates =
        macro_prune_candidate_names(&effective_root_patch, interaction_state);
    document.remove_unreferenced_candidate_macros(&macro_prune_candidates);
    if let Some(library) = library {
        document.add_imports_for_used_library_macros(&effective_root_patch, library);
    }

    let mut generated_node_ids = generated.node_id_map();
    generated_node_ids.extend(history_bindings.node_id_map());
    Ok(PatchWritebackResult {
        source: document.emit(),
        generated_node_ids,
    })
}

pub(super) fn discard_stale_semantic_edits(
    source: &str,
    intent: PatcherIntent,
    interaction_state: &mut PatcherInteractionState,
) -> Result<bool, WriteBackError> {
    let document = SourceDocument::parse(source)?;
    let root_patch = parse_patch_source(source, intent).map_err(WriteBackError::Parse)?;
    let effective_root_patch = patch_with_created_macros(root_patch, interaction_state);

    let stale_node_keys = interaction_state
        .edit_state
        .nodes
        .iter()
        .filter_map(|(key, edit)| {
            node_edit_is_stale(&document, &effective_root_patch, edit).then(|| key.clone())
        })
        .collect::<HashSet<_>>();
    let stale_connection_keys = interaction_state
        .edit_state
        .connections
        .iter()
        .filter_map(|(key, edit)| {
            connection_edit_is_stale(
                &document,
                &effective_root_patch,
                interaction_state,
                &stale_node_keys,
                edit,
            )
            .then(|| key.clone())
        })
        .collect::<HashSet<_>>();
    let stale_deleted_nodes = interaction_state
        .edit_state
        .deleted_nodes
        .iter()
        .filter_map(|key| {
            let (view_key, node_id) = split_scoped_key(key);
            patch_for_view(&effective_root_patch, &view_key)
                .and_then(|patch| patch_node(patch, &node_id))
                .is_none()
                .then(|| key.clone())
        })
        .collect::<HashSet<_>>();
    let stale_deleted_connections = interaction_state
        .edit_state
        .deleted_connections
        .iter()
        .filter_map(|key| {
            let (view_key, connection_id) = split_scoped_key(key);
            source_connection(&effective_root_patch, &view_key, &connection_id)
                .is_none()
                .then(|| key.clone())
        })
        .collect::<HashSet<_>>();

    let changed = !stale_node_keys.is_empty()
        || !stale_connection_keys.is_empty()
        || !stale_deleted_nodes.is_empty()
        || !stale_deleted_connections.is_empty();
    if !changed {
        return Ok(false);
    }

    for key in &stale_node_keys {
        interaction_state.edit_state.nodes.remove(key);
    }
    for key in &stale_connection_keys {
        interaction_state.edit_state.connections.remove(key);
    }
    for key in &stale_deleted_nodes {
        interaction_state.edit_state.deleted_nodes.remove(key);
    }
    for key in &stale_deleted_connections {
        interaction_state.edit_state.deleted_connections.remove(key);
    }
    interaction_state.selected_nodes.retain(|node_id| {
        !stale_node_keys
            .iter()
            .any(|key| split_scoped_key(key).1 == *node_id)
    });
    if interaction_state.text_edit.as_ref().is_some_and(|edit| {
        stale_node_keys.contains(&node_edit_key(
            &active_patcher_view_key(interaction_state),
            &edit.node_id,
        ))
    }) {
        interaction_state.text_edit = None;
    }

    Ok(true)
}

fn node_edit_is_stale(
    document: &SourceDocument,
    root_patch: &Patch,
    edit: &super::state::PatcherNodeEdit,
) -> bool {
    match &edit.origin {
        PatcherNodeOrigin::Created { .. } => false,
        PatcherNodeOrigin::Source { source_node_id } => {
            let Some(node) = patch_for_view(root_patch, &edit.view_key)
                .and_then(|patch| patch_node(patch, source_node_id))
            else {
                return true;
            };
            node_source_is_stale(document, node)
        }
    }
}

fn connection_edit_is_stale(
    document: &SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    stale_node_keys: &HashSet<String>,
    edit: &PatcherConnectionEdit,
) -> bool {
    match &edit.origin {
        PatcherConnectionOrigin::Source {
            source_connection_id,
        } => {
            source_connection(root_patch, &edit.view_key, source_connection_id).is_none()
                || connection_endpoint_is_stale(
                    document,
                    root_patch,
                    interaction_state,
                    &edit.view_key,
                    &edit.from.node_id,
                    stale_node_keys,
                )
                || connection_endpoint_is_stale(
                    document,
                    root_patch,
                    interaction_state,
                    &edit.view_key,
                    &edit.to.node_id,
                    stale_node_keys,
                )
        }
        PatcherConnectionOrigin::Created { .. } => {
            connection_endpoint_is_stale(
                document,
                root_patch,
                interaction_state,
                &edit.view_key,
                &edit.from.node_id,
                stale_node_keys,
            ) || connection_endpoint_is_stale(
                document,
                root_patch,
                interaction_state,
                &edit.view_key,
                &edit.to.node_id,
                stale_node_keys,
            )
        }
    }
}

fn connection_endpoint_is_stale(
    document: &SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    node_id: &str,
    stale_node_keys: &HashSet<String>,
) -> bool {
    if stale_node_keys.contains(&node_edit_key(view_key, node_id)) {
        return true;
    }
    if interaction_state
        .edit_state
        .nodes
        .contains_key(&node_edit_key(view_key, node_id))
    {
        return false;
    }
    let Some(node) =
        patch_for_view(root_patch, view_key).and_then(|patch| patch_node(patch, node_id))
    else {
        return true;
    };
    node_source_is_stale(document, node)
}

fn node_source_is_stale(document: &SourceDocument, node: &PatchNode) -> bool {
    let Some(source) = node.source.as_ref() else {
        return true;
    };
    source
        .expr
        .as_ref()
        .is_some_and(|expr| document.expr(expr).is_none())
        || source.call_shape.as_ref().is_some_and(|shape| {
            document.expr(&shape.call).is_none()
                || shape
                    .positional_args
                    .iter()
                    .any(|arg| document.expr(&arg.expr).is_none())
                || shape
                    .attributes
                    .iter()
                    .any(|attr| document.expr(&attr.value).is_none())
        })
}

fn validate_connection_edits(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> Result<(), WriteBackError> {
    for edit in interaction_state.edit_state.connections.values() {
        match edit.origin {
            PatcherConnectionOrigin::Created { .. } => {
                let touches_history =
                    connection_edit_touches_history(root_patch, interaction_state, edit);
                let touches_created_value =
                    connection_edit_touches_created_value(interaction_state, edit);
                let touches_created_out =
                    connection_edit_touches_created_out(interaction_state, edit);
                let has_source_destination =
                    connection_edit_has_source_destination(root_patch, edit);
                if !touches_history
                    && !touches_created_value
                    && !touches_created_out
                    && !has_source_destination
                {
                    debug_log_writeback_event(
                        "validation-failed-created-connection",
                        created_connection_validation_details(
                            root_patch,
                            interaction_state,
                            edit,
                            touches_history,
                            touches_created_value,
                            has_source_destination,
                        ),
                    );
                    return Err(WriteBackError::UnsupportedCreatedConnection {
                        view_key: edit.view_key.clone(),
                        connection_id: edit.id.clone(),
                    });
                }
            }
            PatcherConnectionOrigin::Source { .. } => {
                if source_connection_edit_is_layout_only(root_patch, edit) {
                    continue;
                }
                let original_is_deletable =
                    source_connection_edit_original_is_deletable(root_patch, edit);
                let has_source_destination =
                    connection_edit_has_source_destination(root_patch, edit);
                if !original_is_deletable || !has_source_destination {
                    debug_log_writeback_event(
                        "validation-failed-source-connection-edit",
                        format!(
                            "source connection edit cannot be decomposed into delete/create writeback\n{}\nchecks: original_is_deletable={original_is_deletable} has_source_destination={has_source_destination}",
                            connection_edit_details(edit),
                        ),
                    );
                    return Err(WriteBackError::UnsupportedDeletedConnection {
                        view_key: edit.view_key.clone(),
                        connection_id: edit.id.clone(),
                    });
                }
            }
        }
    }

    for key in interaction_state.edit_state.deleted_connections.iter() {
        let (view_key, connection_id) = split_scoped_key(key);
        let has_history_replacement = deleted_connection_has_history_replacement(
            root_patch,
            interaction_state,
            &view_key,
            &connection_id,
        );
        let has_created_value_replacement = deleted_connection_has_created_value_replacement(
            root_patch,
            interaction_state,
            &view_key,
            &connection_id,
        );
        let has_created_connection_replacement =
            deleted_connection_has_created_connection_replacement(
                root_patch,
                interaction_state,
                &view_key,
                &connection_id,
            );
        let is_deletable = source_connection_is_deletable(root_patch, &view_key, &connection_id);
        let incident_to_deleted_node = deleted_connection_is_incident_to_deleted_node(
            root_patch,
            interaction_state,
            &view_key,
            &connection_id,
        );
        if !has_history_replacement
            && !has_created_value_replacement
            && !has_created_connection_replacement
            && !is_deletable
            && !incident_to_deleted_node
        {
            debug_log_writeback_event(
                "validation-failed-deleted-connection",
                deleted_connection_validation_details(
                    root_patch,
                    interaction_state,
                    &view_key,
                    &connection_id,
                    has_history_replacement,
                    has_created_value_replacement,
                    is_deletable,
                    incident_to_deleted_node,
                ),
            );
            return Err(WriteBackError::UnsupportedDeletedConnection {
                view_key,
                connection_id,
            });
        }
    }

    Ok(())
}

fn created_connection_validation_details(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    edit: &PatcherConnectionEdit,
    touches_history: bool,
    touches_created_value: bool,
    has_source_destination: bool,
) -> String {
    format!(
        "{}\nchecks: touches_history={touches_history} touches_created_value={touches_created_value} has_source_destination={has_source_destination}\nfrom_node={}\nto_node={}\ncreated_from={}\ncreated_to={}\nsource_destination={}",
        connection_edit_details(edit),
        node_debug_details(
            root_patch,
            interaction_state,
            &edit.view_key,
            &edit.from.node_id
        ),
        node_debug_details(
            root_patch,
            interaction_state,
            &edit.view_key,
            &edit.to.node_id
        ),
        created_value_node(interaction_state, &edit.view_key, &edit.from.node_id)
            .map(|node| format!("{:?}", node))
            .unwrap_or_else(|| "none".to_string()),
        created_value_node(interaction_state, &edit.view_key, &edit.to.node_id)
            .map(|node| format!("{:?}", node))
            .unwrap_or_else(|| "none".to_string()),
        patch_for_view(root_patch, &edit.view_key)
            .and_then(|patch| patch_node(patch, &edit.to.node_id))
            .and_then(|node| node.source.as_ref())
            .map(|source| format!("{:?}", source.owner))
            .unwrap_or_else(|| "none".to_string()),
    )
}

fn deleted_connection_validation_details(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    connection_id: &str,
    has_history_replacement: bool,
    has_created_value_replacement: bool,
    is_deletable: bool,
    incident_to_deleted_node: bool,
) -> String {
    let source_connection = source_connection(root_patch, view_key, connection_id);
    let destination_from_id = connection_destination_from_id(connection_id);
    let replacements = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|edit| edit.view_key == view_key)
        .filter(|edit| {
            destination_from_id
                .as_ref()
                .is_some_and(|to| edit.to == *to)
                || source_connection.is_some_and(|connection| {
                    edit.to.node_id == connection.to_node
                        && edit.to.input_index == connection.to_input
                })
        })
        .map(connection_edit_details)
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "view={view_key} connection_id={connection_id}\nchecks: has_history_replacement={has_history_replacement} has_created_value_replacement={has_created_value_replacement} is_deletable={is_deletable} incident_to_deleted_node={incident_to_deleted_node}\nsource_connection={}\ndestination_from_id={:?}\nreplacement_candidates:\n{}",
        source_connection
            .map(source_connection_details)
            .unwrap_or_else(|| "none".to_string()),
        destination_from_id,
        if replacements.is_empty() {
            "none".to_string()
        } else {
            replacements
        },
    )
}

fn connection_edit_details(edit: &PatcherConnectionEdit) -> String {
    format!(
        "edit view={} id={} origin={:?} from={}:{} to={}:{} kind={:?} segment={:?}",
        edit.view_key,
        edit.id,
        edit.origin,
        edit.from.node_id,
        edit.from.output_index,
        edit.to.node_id,
        edit.to.input_index,
        edit.kind,
        edit.segment
    )
}

fn source_connection_details(connection: &PatchConnection) -> String {
    format!(
        "source from={}:{} to={}:{} kind={:?} source={:?}",
        connection.from_node,
        connection.from_output,
        connection.to_node,
        connection.to_input,
        connection.kind,
        connection.source
    )
}

fn node_debug_details(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    node_id: &str,
) -> String {
    let patch_node = patch_for_view(root_patch, view_key)
        .and_then(|patch| patch_node(patch, node_id))
        .map(|node| {
            format!(
                "patch-node id={} op={} kind={:?} label={:?} source={:?}",
                node.id, node.op, node.kind, node.label, node.source
            )
        })
        .unwrap_or_else(|| "patch-node none".to_string());
    let edit_node = interaction_state
        .edit_state
        .nodes
        .get(&node_edit_key(view_key, node_id))
        .map(|edit| format!("edit-node {:?}", edit))
        .unwrap_or_else(|| "edit-node none".to_string());
    format!("{patch_node}; {edit_node}")
}

fn apply_created_macro_writeback(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> Result<(), WriteBackError> {
    let existing_names = root_patch
        .macros
        .iter()
        .map(|macro_patch| macro_patch.name.as_str())
        .collect::<HashSet<_>>();
    let mut edits = interaction_state
        .edit_state
        .created_macros
        .values()
        .filter(|edit| !existing_names.contains(edit.name.as_str()))
        .collect::<Vec<_>>();
    edits.sort_by(|a, b| a.name.cmp(&b.name));

    for edit in edits.into_iter().rev() {
        let source = edit
            .source
            .as_deref()
            .map(str::to_string)
            .unwrap_or_else(|| default_created_macro_source(&edit.name));
        let expr =
            parse_single_expression(&source).map_err(|reason| WriteBackError::InvalidEdit {
                view_key: "root".to_string(),
                node_id: edit.instance_node_id.clone(),
                reason,
            })?;
        document.prepend_macro(expr)?;
    }

    Ok(())
}

fn apply_created_macro_parameter_writeback(
    document: &mut SourceDocument,
    interaction_state: &PatcherInteractionState,
) -> Result<(), WriteBackError> {
    let mut params = interaction_state
        .edit_state
        .nodes
        .values()
        .filter(|edit| created_macro_parameter_edit(edit))
        .map(|edit| {
            created_macro_parameter(edit)
                .map(|param| (edit.view_key.clone(), edit.id.clone(), param))
        })
        .collect::<Result<Vec<_>, WriteBackError>>()?;
    params.sort_by(|(_, _, left), (_, _, right)| {
        left.macro_name
            .cmp(&right.macro_name)
            .then(left.index.cmp(&right.index))
    });

    for (view_key, node_id, param) in params {
        document.ensure_macro_param(
            &SourceScopeId::Macro {
                name: param.macro_name,
            },
            param.index,
            param.name.as_deref(),
            &view_key,
            &node_id,
        )?;
    }
    Ok(())
}

fn apply_created_out_writeback(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
    scope_filter: CreatedOutWritebackScope,
) -> Result<(), WriteBackError> {
    let mut edits = interaction_state
        .edit_state
        .nodes
        .values()
        .filter(|edit| created_out_edit(edit))
        .filter(|edit| scope_filter.includes(&edit.view_key))
        .collect::<Vec<_>>();
    edits.sort_by_key(|edit| (edit.view_key.clone(), edit.id.clone()));
    let mut macro_returns: HashMap<String, BTreeMap<usize, Expression>> = HashMap::new();
    for edit in edits {
        let inbound = interaction_state
            .edit_state
            .connections
            .values()
            .filter(|connection| {
                connection.view_key == edit.view_key
                    && connection.to.node_id == edit.id
                    && connection.to.input_index == 0
            })
            .min_by_key(|connection| connection.id.clone());
        let value = if let Some(connection) = inbound {
            created_out_source_expression(
                document,
                root_patch,
                interaction_state,
                generated,
                history_bindings,
                &edit.view_key,
                &connection.from,
            )?
        } else {
            Expression::Symbol(MISSING_INPUT_SENTINEL.to_string())
        };
        if edit.view_key.starts_with("macro:") {
            let channel = created_out_channel(edit)?;
            macro_returns
                .entry(edit.view_key.clone())
                .or_default()
                .insert(channel, value);
            continue;
        }
        let expr = created_out_expression(edit, value)?;
        document.insert_form(&scope_for_view_key(&edit.view_key), usize::MAX, expr)?;
    }
    for (view_key, mut outputs) in macro_returns {
        add_live_source_macro_outputs(
            document,
            root_patch,
            interaction_state,
            generated,
            history_bindings,
            &view_key,
            &mut outputs,
        )?;
        let scope = scope_for_view_key(&view_key);
        let SourceScopeId::Macro { .. } = &scope else {
            continue;
        };
        let max_channel = outputs.keys().copied().max().unwrap_or(0);
        let values = (1..=max_channel)
            .map(|channel| {
                outputs
                    .remove(&channel)
                    .unwrap_or_else(|| Expression::Symbol(MISSING_INPUT_SENTINEL.to_string()))
            })
            .collect::<Vec<_>>();
        let replacement = macro_return_expression(values);
        document.replace_macro_visual_return(&scope, replacement)?;
    }
    Ok(())
}

fn add_live_source_macro_outputs(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
    view_key: &str,
    outputs: &mut BTreeMap<usize, Expression>,
) -> Result<(), WriteBackError> {
    let Some(patch) = patch_for_view(root_patch, view_key) else {
        return Ok(());
    };
    for node in patch.nodes.iter().filter(|node| node.kind == NodeKind::Out) {
        if interaction_state
            .edit_state
            .deleted_nodes
            .contains(&node_edit_key(view_key, &node.id))
        {
            continue;
        }
        let Some(channel) = live_source_out_channel(interaction_state, view_key, node)? else {
            continue;
        };
        if outputs.contains_key(&channel) {
            continue;
        }
        let value = source_out_value_expression(
            document,
            root_patch,
            interaction_state,
            generated,
            history_bindings,
            view_key,
            node,
            patch,
        )?;
        outputs.insert(channel, value);
    }
    Ok(())
}

fn live_source_out_channel(
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    node: &PatchNode,
) -> Result<Option<usize>, WriteBackError> {
    if let Some(edit) = interaction_state
        .edit_state
        .nodes
        .get(&node_edit_key(view_key, &node.id))
    {
        return out_channel_from_edit(edit).map(Some);
    }
    Ok(output_channel_from_node(node).or(Some(1)))
}

fn source_out_value_expression(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
    view_key: &str,
    out_node: &PatchNode,
    patch: &Patch,
) -> Result<Expression, WriteBackError> {
    if let Some(connection) = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| {
            connection.view_key == view_key
                && connection.to.node_id == out_node.id
                && connection.to.input_index == 0
        })
        .min_by_key(|connection| connection.id.clone())
    {
        return created_out_source_expression(
            document,
            root_patch,
            interaction_state,
            generated,
            history_bindings,
            view_key,
            &connection.from,
        );
    }
    let Some(connection) = patch
        .connections
        .iter()
        .find(|connection| connection.to_node == out_node.id && connection.to_input == 0)
        .filter(|connection| {
            !interaction_state
                .edit_state
                .deleted_connections
                .contains(&connection_edit_key(
                    view_key,
                    &source_connection_id(connection),
                ))
        })
    else {
        return Ok(Expression::Symbol(MISSING_INPUT_SENTINEL.to_string()));
    };
    value_reference_expr(
        document,
        root_patch,
        interaction_state,
        generated,
        history_bindings,
        view_key,
        &OutputPortRef {
            node_id: connection.from_node.clone(),
            output_index: connection.from_output,
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreatedOutWritebackScope {
    Root,
    Macro,
}

impl CreatedOutWritebackScope {
    fn includes(self, view_key: &str) -> bool {
        match self {
            Self::Root => !view_key.starts_with("macro:"),
            Self::Macro => view_key.starts_with("macro:"),
        }
    }
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
            let mut targets = Vec::new();
            source_deletion_targets_for_owner(&view_key, &node_id, &source.owner, &mut targets)?;
            Ok(targets
                .into_iter()
                .map(move |target| (view_key.clone(), node_id.clone(), target))
                .collect::<Vec<_>>())
        })
        .collect::<Result<Vec<_>, WriteBackError>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

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

fn source_deletion_targets_for_owner(
    view_key: &str,
    node_id: &str,
    owner: &SourceOwner,
    targets: &mut Vec<SourceDeletionTarget>,
) -> Result<(), WriteBackError> {
    match owner {
        SourceOwner::TopLevelForm { form_id } | SourceOwner::BindingValue { form_id, .. } => {
            targets.push(SourceDeletionTarget::Form(form_id.clone()));
            Ok(())
        }
        SourceOwner::NestedExpr { expr } => {
            targets.push(SourceDeletionTarget::Expr(expr.clone()));
            Ok(())
        }
        SourceOwner::ArgumentSlot { arg, .. } | SourceOwner::SymbolReference { arg, .. } => {
            targets.push(SourceDeletionTarget::Expr(arg.expr.clone()));
            Ok(())
        }
        SourceOwner::Compound { parts } => {
            for part in parts {
                source_deletion_targets_for_owner(view_key, node_id, part, targets)?;
            }
            Ok(())
        }
        SourceOwner::CodeIsland { .. } => Err(WriteBackError::EditedCodeIsland {
            view_key: view_key.to_string(),
            node_id: node_id.to_string(),
        }),
        SourceOwner::MacroParameter { .. } | SourceOwner::Created { .. } => {
            Err(WriteBackError::UnsupportedDeletedNode {
                view_key: view_key.to_string(),
                node_id: node_id.to_string(),
            })
        }
    }
}

fn macro_prune_candidate_names(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> HashSet<String> {
    if interaction_state.edit_state.deleted_nodes.is_empty() {
        return HashSet::new();
    }
    let newly_created = interaction_state
        .edit_state
        .created_macros
        .keys()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    root_patch
        .macros
        .iter()
        .filter(|macro_patch| !newly_created.contains(macro_patch.name.as_str()))
        .map(|macro_patch| macro_patch.name.clone())
        .collect()
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

fn source_connection_edit_is_layout_only(root_patch: &Patch, edit: &PatcherConnectionEdit) -> bool {
    let PatcherConnectionOrigin::Source {
        source_connection_id,
    } = &edit.origin
    else {
        return false;
    };
    let Some(connection) = source_connection(root_patch, &edit.view_key, source_connection_id)
    else {
        return false;
    };
    edit.from.node_id == connection.from_node
        && edit.from.output_index == connection.from_output
        && edit.to.node_id == connection.to_node
        && edit.to.input_index == connection.to_input
        && edit.kind == connection.kind
}

fn source_connection_edit_original_is_deletable(
    root_patch: &Patch,
    edit: &PatcherConnectionEdit,
) -> bool {
    let PatcherConnectionOrigin::Source {
        source_connection_id,
    } = &edit.origin
    else {
        return false;
    };
    source_connection_is_deletable(root_patch, &edit.view_key, source_connection_id)
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

fn used_macro_instance_names(root_patch: &Patch) -> HashSet<String> {
    let mut names = root_patch
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::MacroInstance)
        .map(|node| node.op.clone())
        .collect::<HashSet<_>>();
    for macro_patch in &root_patch.macros {
        names.extend(used_macro_instance_names(&macro_patch.patch));
    }
    names
}

fn collect_library_macro_calls(
    expr: &Expression,
    library: &DefmacroLibrary,
    names: &mut HashSet<String>,
) {
    let Expression::List(items) = expr else {
        return;
    };
    if let Some(op) = symbol_at(items, 0)
        && library.package(op).is_some()
    {
        names.insert(op.to_string());
    }
    for item in items {
        collect_library_macro_calls(item, library, names);
    }
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
        && !created_out_edit(edit)
        && !created_macro_parameter_edit(edit)
}

fn created_out_edit(edit: &super::state::PatcherNodeEdit) -> bool {
    matches!(&edit.origin, PatcherNodeOrigin::Created { .. })
        && matches!(created_node_operator(edit).as_deref(), Ok("out"))
}

fn created_macro_parameter_edit(edit: &super::state::PatcherNodeEdit) -> bool {
    matches!(&edit.origin, PatcherNodeOrigin::Created { .. })
        && edit.view_key.starts_with("macro:")
        && created_macro_parameter_candidate(edit)
}

fn created_macro_parameter_candidate(edit: &super::state::PatcherNodeEdit) -> bool {
    let Ok(Expression::List(items)) = parse_created_node_text(edit) else {
        return false;
    };
    symbol_at(&items, 0) == Some("in")
}

#[derive(Debug, Clone)]
struct CreatedMacroParameter {
    macro_name: String,
    index: usize,
    name: Option<String>,
}

fn created_macro_parameter(
    edit: &super::state::PatcherNodeEdit,
) -> Result<CreatedMacroParameter, WriteBackError> {
    let Some(macro_name) = edit.view_key.strip_prefix("macro:") else {
        return Err(WriteBackError::InvalidEdit {
            view_key: edit.view_key.clone(),
            node_id: edit.id.clone(),
            reason: "created macro parameter must be in a macro view".to_string(),
        });
    };
    let expr = parse_created_node_text(edit)?;
    let Expression::List(items) = expr else {
        return Err(WriteBackError::InvalidEdit {
            view_key: edit.view_key.clone(),
            node_id: edit.id.clone(),
            reason: "created macro parameter must parse as an in form".to_string(),
        });
    };
    if symbol_at(&items, 0) != Some("in") {
        return Err(WriteBackError::InvalidEdit {
            view_key: edit.view_key.clone(),
            node_id: edit.id.clone(),
            reason: "created macro parameter must use the in operator".to_string(),
        });
    }
    let Some(Expression::Number(channel)) = items.get(1) else {
        return Err(WriteBackError::InvalidEdit {
            view_key: edit.view_key.clone(),
            node_id: edit.id.clone(),
            reason: "created macro parameter must include an input channel".to_string(),
        });
    };
    if *channel < 1.0 || channel.fract() != 0.0 {
        return Err(WriteBackError::InvalidEdit {
            view_key: edit.view_key.clone(),
            node_id: edit.id.clone(),
            reason: "created macro parameter channel must be a positive integer".to_string(),
        });
    }
    let name =
        symbol_attribute_value(&items, "@name").map_err(|reason| WriteBackError::InvalidEdit {
            view_key: edit.view_key.clone(),
            node_id: edit.id.clone(),
            reason,
        })?;
    Ok(CreatedMacroParameter {
        macro_name: macro_name.to_string(),
        index: (*channel as usize) - 1,
        name,
    })
}

fn created_macro_instance_edit(
    interaction_state: &PatcherInteractionState,
    edit: &super::state::PatcherNodeEdit,
) -> bool {
    edit.view_key == "root"
        && interaction_state
            .edit_state
            .created_macros
            .get(edit.text.trim())
            .is_some_and(|macro_edit| macro_edit.instance_node_id == edit.id)
}

fn created_macro_instance_is_connected(
    interaction_state: &PatcherInteractionState,
    edit: &super::state::PatcherNodeEdit,
) -> bool {
    interaction_state
        .edit_state
        .connections
        .values()
        .any(|connection| {
            connection.view_key == edit.view_key
                && (connection.from.node_id == edit.id || connection.to.node_id == edit.id)
        })
}

fn register_created_modulatable_params(
    document: &mut SourceDocument,
    interaction_state: &PatcherInteractionState,
) {
    for edit in interaction_state
        .edit_state
        .nodes
        .values()
        .filter(|edit| created_param_edit(edit))
    {
        let Ok(Expression::List(items)) = parse_created_node_text(edit) else {
            continue;
        };
        let Some(name) = symbol_at(&items, 1) else {
            continue;
        };
        if expression_has_true_attribute(&items, "@mod") {
            document.register_virtual_modulatable_param(
                scope_for_view_key(&edit.view_key),
                name.to_string(),
            );
        }
    }
}

fn connection_edit_touches_created_value(
    interaction_state: &PatcherInteractionState,
    edit: &PatcherConnectionEdit,
) -> bool {
    created_value_node(interaction_state, &edit.view_key, &edit.from.node_id).is_some()
        || created_value_node(interaction_state, &edit.view_key, &edit.to.node_id).is_some()
}

fn connection_edit_touches_created_out(
    interaction_state: &PatcherInteractionState,
    edit: &PatcherConnectionEdit,
) -> bool {
    interaction_state
        .edit_state
        .nodes
        .get(&node_edit_key(&edit.view_key, &edit.to.node_id))
        .is_some_and(created_out_edit)
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
    let Some(to) = source_connection(root_patch, view_key, connection_id)
        .map(|connection| InputPortRef {
            node_id: connection.to_node.clone(),
            input_index: connection.to_input,
        })
        .or_else(|| connection_destination_from_id(connection_id))
    else {
        return false;
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

fn deleted_connection_has_created_connection_replacement(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    connection_id: &str,
) -> bool {
    let Some(to) = source_connection(root_patch, view_key, connection_id)
        .map(|connection| InputPortRef {
            node_id: connection.to_node.clone(),
            input_index: connection.to_input,
        })
        .or_else(|| connection_destination_from_id(connection_id))
    else {
        return false;
    };
    interaction_state
        .edit_state
        .connections
        .values()
        .any(|edit| {
            edit.view_key == view_key
                && matches!(edit.origin, PatcherConnectionOrigin::Created { .. })
                && edit.to == to
        })
}

fn connection_destination_from_id(connection_id: &str) -> Option<InputPortRef> {
    let (_, to) = connection_id.split_once("->")?;
    let (node_id, input_index) = to.rsplit_once(':')?;
    Some(InputPortRef {
        node_id: node_id.to_string(),
        input_index: input_index.parse().ok()?,
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
    names: HashMap<(String, String, usize), String>,
}

#[derive(Debug, Clone)]
enum GeneratedFormInsertion {
    OriginalIndex(usize),
    CurrentPosition(usize),
    AfterCurrentForms(Vec<SourceFormId>),
}

impl GeneratedFormInsertion {
    fn sort_index(&self) -> usize {
        match self {
            GeneratedFormInsertion::OriginalIndex(index) => *index,
            GeneratedFormInsertion::CurrentPosition(position) => *position,
            GeneratedFormInsertion::AfterCurrentForms(forms) => {
                forms.iter().map(|form| form.index + 1).max().unwrap_or(0)
            }
        }
    }
}

#[derive(Debug, Clone)]
struct PendingGeneratedForm {
    scope: SourceScopeId,
    insertion: GeneratedFormInsertion,
    dependency_depth: usize,
    order: usize,
    defined_names: Vec<String>,
    expr: Expression,
}

impl GeneratedBindings {
    fn insert(&mut self, view_key: &str, node_id: &str, name: String) {
        self.insert_output(view_key, node_id, 0, name);
    }

    fn insert_output(&mut self, view_key: &str, node_id: &str, output_index: usize, name: String) {
        self.names.insert(
            (view_key.to_string(), node_id.to_string(), output_index),
            name,
        );
    }

    fn get(&self, view_key: &str, node_id: &str) -> Option<&str> {
        self.get_output(view_key, node_id, 0)
    }

    fn get_output(&self, view_key: &str, node_id: &str, output_index: usize) -> Option<&str> {
        self.names
            .get(&(view_key.to_string(), node_id.to_string(), output_index))
            .map(String::as_str)
    }

    fn node_id_map(&self) -> HashMap<(String, String), String> {
        self.names
            .iter()
            .filter_map(|((view_key, node_id, output_index), name)| {
                (*output_index == 0).then(|| ((view_key.clone(), node_id.clone()), name.clone()))
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
struct HistoryBindings {
    names: HashMap<(String, String), String>,
    created_names: HashMap<(String, String), String>,
    pending_make_forms: Vec<(SourceScopeId, String)>,
}

impl HistoryBindings {
    fn insert(&mut self, view_key: &str, node_id: &str, name: String) {
        self.names
            .insert((view_key.to_string(), node_id.to_string()), name);
    }

    fn insert_created(&mut self, view_key: &str, node_id: &str, name: String) {
        self.insert(view_key, node_id, name.clone());
        self.created_names
            .insert((view_key.to_string(), node_id.to_string()), name);
    }

    fn get(&self, view_key: &str, node_id: &str) -> Option<&str> {
        self.names
            .get(&(view_key.to_string(), node_id.to_string()))
            .map(String::as_str)
    }

    fn read_expr(&self, view_key: &str, node_id: &str) -> Option<Expression> {
        self.get(view_key, node_id).map(|name| {
            Expression::List(vec![
                Expression::Symbol("read-history".to_string()),
                Expression::Symbol(name.to_string()),
            ])
        })
    }

    fn node_id_map(&self) -> HashMap<(String, String), String> {
        self.created_names.clone()
    }
}

fn resolve_history_bindings(
    document: &SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> HistoryBindings {
    let mut bindings = HistoryBindings::default();
    let mut allocator = HistoryNameAllocator::new(document);
    for view_key in writeback_views(root_patch, interaction_state) {
        let Some(patch) = patch_for_view(root_patch, &view_key) else {
            continue;
        };
        let scope = scope_for_view_key(&view_key);
        for node in patch
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::History)
        {
            bindings.insert(&view_key, &node.id, node.id.clone());
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
            bindings.insert_created(&view_key, &node_id, name.clone());
            bindings.pending_make_forms.push((scope.clone(), name));
        }
    }
    bindings
}

fn writeback_views(root_patch: &Patch, interaction_state: &PatcherInteractionState) -> Vec<String> {
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
    views
}

fn apply_generated_binding_writeback(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    history_bindings: &HistoryBindings,
) -> Result<GeneratedBindings, WriteBackError> {
    let mut generated = GeneratedBindings::default();
    let mut allocator = GeneratedNameAllocator::new(document);

    let mut pending_forms: Vec<PendingGeneratedForm> = Vec::new();
    let mut pending_consumer_rewrites = Vec::new();
    let mut next_generated_def_order = 0usize;
    for view_key in writeback_views(root_patch, interaction_state) {
        let scope = scope_for_view_key(&view_key);
        let mut created_nodes = interaction_state
            .edit_state
            .nodes
            .values()
            .filter(|edit| edit.view_key == view_key)
            .filter(|edit| created_generated_value_edit(edit))
            .filter(|edit| {
                !created_macro_instance_edit(interaction_state, edit)
                    || created_macro_instance_is_connected(interaction_state, edit)
            })
            .filter(|edit| {
                !created_value_only_feeds_created_out(interaction_state, &view_key, &edit.id)
            })
            .collect::<Vec<_>>();
        created_nodes.sort_by(|a, b| a.id.cmp(&b.id));

        let mut materialized_nodes = HashSet::new();
        for edit in &created_nodes {
            if !created_param_edit(edit) {
                continue;
            }
            let generated_expr = created_value_expression(
                document,
                root_patch,
                interaction_state,
                &generated,
                history_bindings,
                &view_key,
                edit,
            )?;
            let name = created_param_name(&generated_expr, &view_key, &edit.id)?;
            allocator.reserve(&scope, &name).map_err(|name| {
                WriteBackError::BindingRenameCollision {
                    view_key: view_key.clone(),
                    node_id: edit.id.clone(),
                    name,
                }
            })?;
            generated.insert(&view_key, &edit.id, name.clone());
            materialized_nodes.insert(edit.id.clone());
            pending_consumer_rewrites.push((view_key.clone(), (*edit).clone()));
            let insertion_index = generated_def_insertion_index(
                document,
                root_patch,
                interaction_state,
                &generated,
                history_bindings,
                &view_key,
                edit,
                &generated_expr,
            )?;
            pending_forms.push(PendingGeneratedForm {
                scope: scope.clone(),
                insertion: insertion_index,
                dependency_depth: 0,
                order: next_generated_def_order,
                defined_names: vec![name],
                expr: generated_expr,
            });
            next_generated_def_order += 1;
        }
        loop {
            let mut changed = false;
            for edit in &created_nodes {
                if materialized_nodes.contains(&edit.id) {
                    continue;
                }
                if let Some(name) = materialized_created_node_binding(
                    document,
                    root_patch,
                    interaction_state,
                    &generated,
                    history_bindings,
                    &view_key,
                    edit,
                )? {
                    generated.insert(&view_key, &edit.id, name);
                    materialized_nodes.insert(edit.id.clone());
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        created_nodes.retain(|edit| !materialized_nodes.contains(&edit.id));

        for edit in &created_nodes {
            let op = created_generated_binding_stem(edit)?;
            let output_count =
                created_generated_output_count(root_patch, interaction_state, &view_key, edit);
            for output_index in 0..output_count {
                let name = allocator.allocate(&scope, &op);
                generated.insert_output(&view_key, &edit.id, output_index, name);
            }
        }

        for edit in created_nodes {
            let output_count =
                created_generated_output_count(root_patch, interaction_state, &view_key, &edit);
            let names = (0..output_count)
                .map(|output_index| {
                    generated
                        .get_output(&view_key, &edit.id, output_index)
                        .expect("created node name allocated before emission")
                        .to_string()
                })
                .collect::<Vec<_>>();
            let generated_expr = created_value_expression(
                document,
                root_patch,
                interaction_state,
                &generated,
                history_bindings,
                &view_key,
                edit,
            )?;
            let insertion_index = generated_def_insertion_index(
                document,
                root_patch,
                interaction_state,
                &generated,
                history_bindings,
                &view_key,
                edit,
                &generated_expr,
            )?;
            let dependency_depth =
                generated_binding_dependency_depth(interaction_state, &view_key, &edit.id);
            pending_forms.push(PendingGeneratedForm {
                scope: scope.clone(),
                insertion: insertion_index,
                dependency_depth,
                order: next_generated_def_order,
                defined_names: names.clone(),
                expr: generated_def_expression(names, generated_expr),
            });
            next_generated_def_order += 1;
            pending_consumer_rewrites.push((view_key.clone(), edit.clone()));
        }
        rewrite_created_literal_consumers(
            document,
            root_patch,
            interaction_state,
            &generated,
            &view_key,
        )?;
    }

    pending_consumer_rewrites.sort_by(|(view_a, edit_a), (view_b, edit_b)| {
        generated_consumer_rewrite_source_depth(root_patch, interaction_state, view_b, &edit_b.id)
            .cmp(&generated_consumer_rewrite_source_depth(
                root_patch,
                interaction_state,
                view_a,
                &edit_a.id,
            ))
            .then(view_a.cmp(view_b))
            .then(edit_a.id.cmp(&edit_b.id))
    });
    for (view_key, edit) in pending_consumer_rewrites {
        rewrite_created_value_consumers(
            document,
            root_patch,
            interaction_state,
            &generated,
            &view_key,
            &edit,
        )?;
    }

    sort_pending_generated_forms(&mut pending_forms);
    for form in pending_forms {
        document.insert_generated_form(&form.scope, &form.insertion, form.expr)?;
    }
    Ok(generated)
}

fn sort_pending_generated_forms(forms: &mut [PendingGeneratedForm]) {
    let generated_name_to_order = forms
        .iter()
        .flat_map(|form| {
            form.defined_names
                .iter()
                .map(|name| (name.clone(), form.order))
        })
        .collect::<HashMap<_, _>>();
    let mut memo = HashMap::new();
    let dependency_rank_by_order = forms
        .iter()
        .map(|form| {
            (
                form.order,
                pending_generated_dependency_rank(
                    form.order,
                    forms,
                    &generated_name_to_order,
                    &mut memo,
                    &mut HashSet::new(),
                ),
            )
        })
        .collect::<HashMap<_, _>>();
    forms.sort_by(|a, b| {
        view_key_for_scope(&b.scope)
            .cmp(&view_key_for_scope(&a.scope))
            .then(b.insertion.sort_index().cmp(&a.insertion.sort_index()))
            .then(
                dependency_rank_by_order
                    .get(&a.order)
                    .copied()
                    .unwrap_or(0)
                    .cmp(&dependency_rank_by_order.get(&b.order).copied().unwrap_or(0)),
            )
            .then(a.dependency_depth.cmp(&b.dependency_depth))
            .then(b.order.cmp(&a.order))
    });
}

fn pending_generated_dependency_rank(
    order: usize,
    forms: &[PendingGeneratedForm],
    generated_name_to_order: &HashMap<String, usize>,
    memo: &mut HashMap<usize, usize>,
    visiting: &mut HashSet<usize>,
) -> usize {
    if let Some(rank) = memo.get(&order) {
        return *rank;
    }
    if !visiting.insert(order) {
        return 0;
    }
    let Some(form) = forms.iter().find(|form| form.order == order) else {
        visiting.remove(&order);
        return 0;
    };
    let mut dependencies = HashSet::new();
    collect_generated_symbol_dependencies(&form.expr, generated_name_to_order, &mut dependencies);
    dependencies.remove(&order);
    let rank = dependencies
        .into_iter()
        .map(|dependency| {
            1 + pending_generated_dependency_rank(
                dependency,
                forms,
                generated_name_to_order,
                memo,
                visiting,
            )
        })
        .max()
        .unwrap_or(0);
    visiting.remove(&order);
    memo.insert(order, rank);
    rank
}

fn collect_generated_symbol_dependencies(
    expr: &Expression,
    generated_name_to_order: &HashMap<String, usize>,
    out: &mut HashSet<usize>,
) {
    match expr {
        Expression::Symbol(symbol) => {
            if let Some(order) = generated_name_to_order.get(symbol) {
                out.insert(*order);
            }
        }
        Expression::List(items) | Expression::QuoteList(items) => {
            for (idx, item) in items.iter().enumerate() {
                if matches!(item, Expression::Symbol(symbol) if symbol.starts_with('@'))
                    || matches!(items.get(idx.saturating_sub(1)), Some(Expression::Symbol(symbol)) if symbol.starts_with('@'))
                {
                    continue;
                }
                collect_generated_symbol_dependencies(item, generated_name_to_order, out);
            }
        }
        Expression::Quasiquote(inner) | Expression::Unquote(inner) => {
            collect_generated_symbol_dependencies(inner, generated_name_to_order, out);
        }
        _ => {}
    }
}

fn generated_consumer_rewrite_source_depth(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    node_id: &str,
) -> usize {
    interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.from.node_id == node_id)
        .filter_map(|connection| {
            patch_for_view(root_patch, view_key)
                .and_then(|patch| patch_node(patch, &connection.to.node_id))
                .and_then(source_owner_location_for_node)
                .map(|(_, depth)| depth)
        })
        .max()
        .unwrap_or(0)
}

fn materialized_created_node_binding(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
    view_key: &str,
    edit: &super::state::PatcherNodeEdit,
) -> Result<Option<String>, WriteBackError> {
    if created_literal_expr(edit).is_some() {
        return Ok(None);
    }
    let expr = match created_value_expression(
        document,
        root_patch,
        interaction_state,
        generated,
        history_bindings,
        view_key,
        edit,
    ) {
        Ok(expr) => expr,
        Err(WriteBackError::UnsupportedGeneratedBinding { reason, .. })
            if reason == "generated binding source must be source-backed or generated" =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let Some(patch) = patch_for_view(root_patch, view_key) else {
        return Ok(None);
    };
    for node in &patch.nodes {
        let Some(source) = node.source.as_ref() else {
            continue;
        };
        let SourceOwner::BindingValue {
            binding: BindingTarget::Symbol(name),
            form_id,
            value_path,
        } = &source.owner
        else {
            continue;
        };
        let source_expr = SourceExprId {
            form_id: form_id.clone(),
            path: value_path.clone(),
        };
        if document.expr(&source_expr) == Some(&expr)
            && materialized_created_node_outputs_match_source(
                root_patch,
                interaction_state,
                view_key,
                edit,
                node,
                generated,
            )
        {
            return Ok(Some(name.clone()));
        }
    }
    Ok(None)
}

fn created_generated_output_count(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    edit: &super::state::PatcherNodeEdit,
) -> usize {
    let macro_outputs = created_node_operator(edit)
        .ok()
        .and_then(|op| {
            root_patch
                .macros
                .iter()
                .find(|macro_patch| macro_patch.name == op)
                .map(|macro_patch| macro_patch.outputs.len())
        })
        .unwrap_or(1);
    let used_outputs = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.from.node_id == edit.id)
        .map(|connection| connection.from.output_index + 1)
        .max()
        .unwrap_or(1);
    macro_outputs.max(used_outputs).max(1)
}

fn created_value_only_feeds_created_out(
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    node_id: &str,
) -> bool {
    let outgoing = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.from.node_id == node_id)
        .collect::<Vec<_>>();
    !outgoing.is_empty()
        && outgoing
            .iter()
            .all(|connection| connection_edit_touches_created_out(interaction_state, connection))
}

fn generated_def_expression(names: Vec<String>, value: Expression) -> Expression {
    let target = if names.len() == 1 {
        Expression::Symbol(names.into_iter().next().unwrap())
    } else {
        Expression::List(names.into_iter().map(Expression::Symbol).collect())
    };
    Expression::List(vec![Expression::Symbol("def".to_string()), target, value])
}

fn scalar_binding_destructuring_names(
    document: &SourceDocument,
    form_id: &SourceFormId,
    first_name: &str,
    count: usize,
) -> Vec<String> {
    let mut used = document.reserved_names(&form_id.scope);
    used.remove(first_name);
    let mut names = vec![first_name.to_string()];
    for output_index in 1..count {
        let stem = format!("{first_name}{}", output_index + 1);
        names.push(allocate_unique_name(&mut used, &stem));
    }
    names
}

fn allocate_unique_name(used: &mut HashSet<String>, stem: &str) -> String {
    if used.insert(stem.to_string()) {
        return stem.to_string();
    }
    let mut suffix = 1usize;
    loop {
        let candidate = format!("{stem}_{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn materialized_created_node_outputs_match_source(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    edit: &super::state::PatcherNodeEdit,
    source_node: &PatchNode,
    generated: &GeneratedBindings,
) -> bool {
    let Some(patch) = patch_for_view(root_patch, view_key) else {
        return false;
    };
    interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.from.node_id == edit.id)
        .all(|connection| {
            if generated.get(view_key, &connection.to.node_id).is_some()
                || created_value_node(interaction_state, view_key, &connection.to.node_id).is_some()
            {
                return true;
            }
            patch.connections.iter().any(|source_connection| {
                source_connection.from_node == source_node.id
                    && source_connection.from_output == connection.from.output_index
                    && source_connection.to_node == connection.to.node_id
                    && source_connection.to_input == connection.to.input_index
            })
        })
}

fn created_generated_value_edit(edit: &super::state::PatcherNodeEdit) -> bool {
    created_value_edit(edit)
}

fn created_param_edit(edit: &super::state::PatcherNodeEdit) -> bool {
    matches!(created_node_operator(edit).as_deref(), Ok("param"))
}

fn created_param_name(
    expr: &Expression,
    view_key: &str,
    node_id: &str,
) -> Result<String, WriteBackError> {
    let Expression::List(items) = expr else {
        return Err(WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: node_id.to_string(),
            reason: "created param must parse as a call".to_string(),
        });
    };
    if symbol_at(items, 0) != Some("param") {
        return Err(WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: node_id.to_string(),
            reason: "created param expression must start with `param`".to_string(),
        });
    }
    let Some(name) = symbol_at(items, 1) else {
        return Err(WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: node_id.to_string(),
            reason: "created param must provide a symbolic name".to_string(),
        });
    };
    Ok(name.to_string())
}

fn created_node_requires_host_modulator_inputs(edit: &super::state::PatcherNodeEdit) -> bool {
    let Ok(Expression::List(items)) = parse_created_node_text(edit) else {
        return false;
    };
    match symbol_at(&items, 0) {
        Some("mod") => true,
        Some("param") => expression_has_true_attribute(&items, "@mod"),
        _ => false,
    }
}

fn expression_has_true_attribute(items: &[Expression], attr: &str) -> bool {
    items.windows(2).any(|window| {
        matches!(
            window,
            [Expression::Symbol(key), Expression::Symbol(value)]
                if key == attr && value == "true"
        )
    })
}

fn ensure_host_modulator_inputs_if_needed(
    document: &mut SourceDocument,
    intent: PatcherIntent,
    interaction_state: &PatcherInteractionState,
) -> Result<(), WriteBackError> {
    if !document_requires_host_modulation(document)
        && !interaction_requires_host_modulation(interaction_state)
    {
        return Ok(());
    }
    document.ensure_root_host_modulator_inputs(intent)
}

fn interaction_requires_host_modulation(interaction_state: &PatcherInteractionState) -> bool {
    interaction_state
        .edit_state
        .nodes
        .values()
        .any(created_node_requires_host_modulator_inputs)
}

fn document_requires_host_modulation(document: &SourceDocument) -> bool {
    document
        .scope_forms(&SourceScopeId::Root)
        .into_iter()
        .any(expression_requires_host_modulation)
}

fn expression_requires_host_modulation(expr: &Expression) -> bool {
    match expr {
        Expression::List(items) | Expression::QuoteList(items) => {
            if symbol_at(items, 0) == Some("param") && expression_has_true_attribute(items, "@mod")
            {
                return true;
            }
            if symbol_at(items, 0) == Some("mod") {
                return true;
            }
            items.iter().any(expression_requires_host_modulation)
        }
        Expression::Quasiquote(inner) | Expression::Unquote(inner) => {
            expression_requires_host_modulation(inner)
        }
        _ => false,
    }
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

fn created_generated_binding_stem(
    edit: &super::state::PatcherNodeEdit,
) -> Result<String, WriteBackError> {
    if created_literal_expr(edit).is_some() {
        return Ok("value".to_string());
    }
    created_node_operator(edit)
}

fn generated_binding_dependency_depth(
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    node_id: &str,
) -> usize {
    let mut visiting = HashSet::new();
    generated_binding_dependency_depth_inner(interaction_state, view_key, node_id, &mut visiting)
}

fn generated_binding_dependency_depth_inner(
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    node_id: &str,
    visiting: &mut HashSet<String>,
) -> usize {
    if !visiting.insert(node_id.to_string()) {
        return 0;
    }
    let depth = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.to.node_id == node_id)
        .filter(|connection| {
            interaction_state
                .edit_state
                .nodes
                .get(&node_edit_key(view_key, &connection.from.node_id))
                .is_some_and(created_generated_value_edit)
        })
        .map(|connection| {
            1 + generated_binding_dependency_depth_inner(
                interaction_state,
                view_key,
                &connection.from.node_id,
                visiting,
            )
        })
        .max()
        .unwrap_or(0);
    visiting.remove(node_id);
    depth
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

fn created_literal_expr(edit: &super::state::PatcherNodeEdit) -> Option<Expression> {
    let text = edit.text.trim();
    if text.is_empty() {
        return None;
    }
    let expr = parse_single_expression(text).ok()?;
    match &expr {
        Expression::Number(_) => Some(expr),
        Expression::Symbol(symbol) => {
            let known_macros = HashSet::new();
            (node_kind_for_op(symbol, &known_macros) == NodeKind::Constant).then_some(expr)
        }
        _ => None,
    }
}

fn created_value_expression(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
    view_key: &str,
    edit: &super::state::PatcherNodeEdit,
) -> Result<Expression, WriteBackError> {
    if let Some(literal) = created_literal_expr(edit) {
        return Ok(literal);
    }
    created_node_expression(
        document,
        root_patch,
        interaction_state,
        generated,
        history_bindings,
        view_key,
        edit,
    )
}

fn created_out_expression(
    edit: &super::state::PatcherNodeEdit,
    value: Expression,
) -> Result<Expression, WriteBackError> {
    let expr = parse_created_node_text(edit)?;
    let Expression::List(items) = expr else {
        return Err(WriteBackError::InvalidEdit {
            view_key: edit.view_key.clone(),
            node_id: edit.id.clone(),
            reason: "created out node text must parse as an out form".to_string(),
        });
    };
    if symbol_at(&items, 0) != Some("out") {
        return Err(WriteBackError::InvalidEdit {
            view_key: edit.view_key.clone(),
            node_id: edit.id.clone(),
            reason: "created out node must use the out operator".to_string(),
        });
    }
    let mut rebuilt = vec![Expression::Symbol("out".to_string()), value];
    rebuilt.extend(items.into_iter().skip(1));
    Ok(Expression::List(rebuilt))
}

fn created_out_source_expression(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
    view_key: &str,
    from: &OutputPortRef,
) -> Result<Expression, WriteBackError> {
    if let Some(name) = generated.get_output(view_key, &from.node_id, from.output_index) {
        return Ok(Expression::Symbol(name.to_string()));
    }
    if let Some(edit) = created_value_node(interaction_state, view_key, &from.node_id)
        && created_literal_expr(edit).is_none()
    {
        return created_value_expression(
            document,
            root_patch,
            interaction_state,
            generated,
            history_bindings,
            view_key,
            edit,
        );
    }
    value_reference_expr(
        document,
        root_patch,
        interaction_state,
        generated,
        history_bindings,
        view_key,
        from,
    )
}

fn created_out_channel(edit: &super::state::PatcherNodeEdit) -> Result<usize, WriteBackError> {
    out_channel_from_edit(edit)
}

fn out_channel_from_edit(edit: &super::state::PatcherNodeEdit) -> Result<usize, WriteBackError> {
    let expr = parse_created_node_text(edit)?;
    let Expression::List(items) = expr else {
        return Err(WriteBackError::InvalidEdit {
            view_key: edit.view_key.clone(),
            node_id: edit.id.clone(),
            reason: "created out node text must parse as an out form".to_string(),
        });
    };
    if symbol_at(&items, 0) != Some("out") {
        return Err(WriteBackError::InvalidEdit {
            view_key: edit.view_key.clone(),
            node_id: edit.id.clone(),
            reason: "created out node must use the out operator".to_string(),
        });
    }
    match items.get(1) {
        Some(Expression::Number(value)) if value.fract() == 0.0 && *value > 0.0 => {
            Ok(*value as usize)
        }
        Some(Expression::Symbol(value)) => {
            value
                .parse::<usize>()
                .map_err(|_| WriteBackError::InvalidEdit {
                    view_key: edit.view_key.clone(),
                    node_id: edit.id.clone(),
                    reason: "created out node channel must be a positive integer".to_string(),
                })
        }
        _ => Err(WriteBackError::InvalidEdit {
            view_key: edit.view_key.clone(),
            node_id: edit.id.clone(),
            reason: "created out node must provide an output channel".to_string(),
        }),
    }
}

fn output_channel_from_node(node: &PatchNode) -> Option<usize> {
    node.args
        .first()
        .and_then(|arg| match arg {
            ArgValue::Literal(value) => value.parse::<usize>().ok(),
            _ => None,
        })
        .filter(|channel| *channel > 0)
}

fn macro_return_expression(values: Vec<Expression>) -> Expression {
    if values.len() == 1 {
        return values.into_iter().next().unwrap();
    }
    let mut items = Vec::with_capacity(values.len() + 1);
    items.push(Expression::Symbol("tuple".to_string()));
    items.extend(values);
    Expression::List(items)
}

fn created_node_expression(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
    view_key: &str,
    edit: &super::state::PatcherNodeEdit,
) -> Result<Expression, WriteBackError> {
    let mut expr = parse_created_node_text(edit)?;
    let macro_arities = root_patch
        .macros
        .iter()
        .map(|macro_patch| (macro_patch.name.clone(), macro_patch.params.len()))
        .collect::<HashMap<_, _>>();
    let mut inbound = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.to.node_id == edit.id)
        .collect::<Vec<_>>();
    inbound.sort_by_key(|connection| connection.to.input_index);
    let max_input_index = inbound
        .iter()
        .map(|connection| connection.to.input_index)
        .max();

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

    normalize_created_node_inline_args(items, &macro_arities, max_input_index);
    for connection in inbound {
        let value = value_reference_expr(
            document,
            root_patch,
            interaction_state,
            generated,
            history_bindings,
            view_key,
            &connection.from,
        )?;
        let item_index = connection.to.input_index + 1;
        while items.len() <= item_index {
            items.push(Expression::Symbol(MISSING_INPUT_SENTINEL.to_string()));
        }
        items[item_index] = value;
    }
    expand_editor_mod_shorthand_expr(expr, document, &scope_for_view_key(view_key)).map_err(
        |reason| WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: edit.id.clone(),
            reason,
        },
    )
}

fn normalize_created_node_inline_args(
    items: &mut Vec<Expression>,
    macro_arities: &HashMap<String, usize>,
    max_input_index: Option<usize>,
) {
    let Some(op) = symbol_at(items, 0).map(str::to_string) else {
        return;
    };
    let known_macros = macro_arities.keys().cloned().collect::<HashSet<_>>();
    let kind = node_kind_for_op(&op, &known_macros);
    if matches!(
        kind,
        NodeKind::In | NodeKind::Out | NodeKind::Param | NodeKind::Constant
    ) {
        return;
    }
    let required_input_count = created_node_required_input_count(&op, kind, macro_arities);
    let inline_args = positional_args(items, 1)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let mut attributes = Vec::new();
    let mut idx = 1usize;
    while idx < items.len() {
        if matches!(&items[idx], Expression::Symbol(symbol) if symbol.starts_with('@')) {
            attributes.push(items[idx].clone());
            if let Some(value) = items.get(idx + 1) {
                attributes.push(value.clone());
            }
            idx += 2;
        } else {
            idx += 1;
        }
    }

    let mut input_count = required_input_count.max(inline_args.len() + 1);
    if let Some(max_input_index) = max_input_index {
        input_count = input_count.max(max_input_index + 1);
    }
    let mut rebuilt = Vec::with_capacity(1 + input_count + attributes.len());
    rebuilt.push(Expression::Symbol(op));
    for input_index in 0..input_count {
        if input_index == 0 {
            rebuilt.push(Expression::Symbol(MISSING_INPUT_SENTINEL.to_string()));
        } else if let Some(inline_arg) = inline_args.get(input_index - 1) {
            rebuilt.push(inline_arg.clone());
        } else {
            rebuilt.push(Expression::Symbol(MISSING_INPUT_SENTINEL.to_string()));
        }
    }
    rebuilt.extend(attributes);
    *items = rebuilt;
}

fn created_node_required_input_count(
    op: &str,
    kind: NodeKind,
    macro_arities: &HashMap<String, usize>,
) -> usize {
    match kind {
        NodeKind::In | NodeKind::Param | NodeKind::Constant => 0,
        NodeKind::Out | NodeKind::History => 1,
        NodeKind::MacroInstance => macro_arities.get(op).copied().unwrap_or(1),
        _ => dgenlisp_operator_required_input_counts()
            .get(op)
            .copied()
            .unwrap_or(1),
    }
}

fn value_reference_expr(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
    view_key: &str,
    from: &OutputPortRef,
) -> Result<Expression, WriteBackError> {
    if let Some(read_expr) = history_bindings.read_expr(view_key, &from.node_id) {
        return Ok(read_expr);
    }
    if let Some(name) = generated.get_output(view_key, &from.node_id, from.output_index) {
        return Ok(Expression::Symbol(name.to_string()));
    }
    if let Some(edit) = created_value_node(interaction_state, view_key, &from.node_id)
        && let Some(literal) = created_literal_expr(edit)
    {
        return Ok(literal);
    }
    if let Some(edit) = interaction_state
        .edit_state
        .nodes
        .get(&node_edit_key(view_key, &from.node_id))
        .filter(|edit| created_macro_parameter_edit(edit))
    {
        let param = created_macro_parameter(edit)?;
        return Ok(Expression::Symbol(document.macro_param_name(
            &SourceScopeId::Macro {
                name: param.macro_name,
            },
            param.index,
            view_key,
            &from.node_id,
        )?));
    }
    if let Some(name) = edited_source_param_name(interaction_state, view_key, &from.node_id) {
        return Ok(Expression::Symbol(name));
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
    node_reference_expr_with_pending_source_input_rewrites(
        document,
        root_patch,
        interaction_state,
        generated,
        history_bindings,
        node,
        view_key,
        from.output_index,
    )
}

fn node_reference_expr_with_pending_source_input_rewrites(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
    node: &PatchNode,
    view_key: &str,
    output_index: usize,
) -> Result<Expression, WriteBackError> {
    let mut expr =
        node_reference_expr(document, node, view_key, output_index).or_else(|error| {
            source_node_original_expression(document, node, output_index).ok_or(error)
        })?;
    if output_index != 0 || !source_node_accepts_positional_input_rewrites(node) {
        return Ok(expr);
    }
    let Expression::List(items) = &mut expr else {
        return Ok(expr);
    };
    for (input_index, value) in pending_source_input_rewrites_for_node(
        document,
        root_patch,
        interaction_state,
        generated,
        history_bindings,
        view_key,
        &node.id,
    )? {
        replace_or_insert_positional_arg(items, input_index, value);
    }
    Ok(expr)
}

fn source_node_original_expression(
    document: &SourceDocument,
    node: &PatchNode,
    output_index: usize,
) -> Option<Expression> {
    if output_index != 0 {
        return None;
    }
    let source = node.source.as_ref()?;
    match &source.owner {
        SourceOwner::NestedExpr { .. } | SourceOwner::TopLevelForm { .. } => source
            .expr
            .as_ref()
            .and_then(|expr| document.original_expr(expr))
            .cloned(),
        SourceOwner::BindingValue {
            binding: BindingTarget::Symbol(name),
            ..
        } => Some(Expression::Symbol(name.clone())),
        _ => None,
    }
}

fn source_node_accepts_positional_input_rewrites(node: &PatchNode) -> bool {
    node.source
        .as_ref()
        .and_then(|source| source.call_shape.as_ref())
        .is_some()
}

fn pending_source_input_rewrites_for_node(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
    view_key: &str,
    node_id: &str,
) -> Result<Vec<(usize, Expression)>, WriteBackError> {
    let mut rewrites = Vec::new();
    let mut created_connections = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| {
            connection.view_key == view_key
                && matches!(connection.origin, PatcherConnectionOrigin::Created { .. })
                && connection.to.node_id == node_id
        })
        .collect::<Vec<_>>();
    created_connections.sort_by(|a, b| a.id.cmp(&b.id));
    for connection in created_connections {
        if connection_edit_touches_history(root_patch, interaction_state, connection)
            || connection_edit_touches_created_out(interaction_state, connection)
        {
            continue;
        }
        let value = value_reference_expr(
            document,
            root_patch,
            interaction_state,
            generated,
            history_bindings,
            view_key,
            &connection.from,
        )?;
        rewrites.push((connection.to.input_index, value));
    }

    let mut source_connections = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| {
            connection.view_key == view_key
                && matches!(connection.origin, PatcherConnectionOrigin::Source { .. })
                && connection.to.node_id == node_id
                && !source_connection_edit_is_layout_only(root_patch, connection)
        })
        .collect::<Vec<_>>();
    source_connections.sort_by(|a, b| a.id.cmp(&b.id));
    for connection in source_connections {
        let value = value_reference_expr(
            document,
            root_patch,
            interaction_state,
            generated,
            history_bindings,
            view_key,
            &connection.from,
        )?;
        rewrites.push((connection.to.input_index, value));
    }

    let deleted = interaction_state
        .edit_state
        .deleted_connections
        .iter()
        .map(|key| split_scoped_key(key))
        .filter(|(deleted_view_key, _)| deleted_view_key == view_key)
        .collect::<Vec<_>>();
    for (_, connection_id) in deleted {
        if deleted_connection_has_history_replacement(
            root_patch,
            interaction_state,
            view_key,
            &connection_id,
        ) || deleted_connection_has_created_value_replacement(
            root_patch,
            interaction_state,
            view_key,
            &connection_id,
        ) || deleted_connection_has_created_connection_replacement(
            root_patch,
            interaction_state,
            view_key,
            &connection_id,
        ) {
            continue;
        }
        let Some(connection) = source_connection(root_patch, view_key, &connection_id) else {
            continue;
        };
        if connection.to_node == node_id {
            rewrites.push((
                connection.to_input,
                Expression::Symbol(MISSING_INPUT_SENTINEL.to_string()),
            ));
        }
    }

    rewrites.sort_by_key(|(input_index, _)| *input_index);
    Ok(rewrites)
}

fn node_reference_expr(
    document: &mut SourceDocument,
    node: &PatchNode,
    view_key: &str,
    output_index: usize,
) -> Result<Expression, WriteBackError> {
    let source = node
        .source
        .as_ref()
        .ok_or_else(|| WriteBackError::MissingSourceOwner {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
        })?;
    match &source.owner {
        SourceOwner::TopLevelForm { .. } | SourceOwner::BindingValue { .. }
            if node.kind == NodeKind::Param =>
        {
            let Some(name) = param_node_name(node) else {
                return Err(WriteBackError::UnsupportedGeneratedBinding {
                    view_key: view_key.to_string(),
                    node_id: node.id.clone(),
                    reason: "param source has no parameter name".to_string(),
                });
            };
            Ok(Expression::Symbol(name.to_string()))
        }
        SourceOwner::BindingValue {
            binding: BindingTarget::Symbol(name),
            ..
        } if output_index == 0 => Ok(Expression::Symbol(name.clone())),
        SourceOwner::BindingValue {
            binding: BindingTarget::Symbol(name),
            form_id,
            ..
        } => {
            let names =
                scalar_binding_destructuring_names(document, form_id, name, output_index + 1);
            document.replace_def_target(form_id, BindingTarget::Destructuring(names.clone()))?;
            Ok(Expression::Symbol(names[output_index].clone()))
        }
        SourceOwner::BindingValue {
            binding: BindingTarget::Destructuring(names),
            ..
        } => names
            .get(output_index)
            .cloned()
            .map(Expression::Symbol)
            .ok_or_else(|| WriteBackError::UnsupportedGeneratedBinding {
                view_key: view_key.to_string(),
                node_id: node.id.clone(),
                reason: "destructuring source does not have the requested output".to_string(),
            }),
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

fn edited_source_param_name(
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    node_id: &str,
) -> Option<String> {
    let edit = interaction_state
        .edit_state
        .nodes
        .get(&node_edit_key(view_key, node_id))?;
    let PatcherNodeOrigin::Source { source_node_id } = &edit.origin else {
        return None;
    };
    if source_node_id != node_id {
        return None;
    }
    let expr = parse_single_expression(&format!("({})", edit.text.trim())).ok()?;
    param_name(&expr).map(str::to_string)
}

fn generated_def_insertion_index(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
    view_key: &str,
    edit: &super::state::PatcherNodeEdit,
    generated_expr: &Expression,
) -> Result<GeneratedFormInsertion, WriteBackError> {
    let scope = scope_for_view_key(view_key);
    let mut dependency_index =
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
    dependency_index = dependency_index.max(generated_expression_source_dependency_index(
        document,
        &scope,
        generated_expr,
    ));
    dependency_index = dependency_index.max(generated_chain_source_dependency_index(
        document,
        root_patch,
        interaction_state,
        generated,
        history_bindings,
        view_key,
        &edit.id,
    )?);
    let host_modulator_insert_position =
        if generated_node_requires_host_modulator_inputs(interaction_state, view_key, &edit.id) {
            document
                .latest_host_modulator_input_position(&scope)
                .map(|position| position + 1)
        } else {
            None
        };
    if generated_node_requires_host_modulator_inputs(interaction_state, view_key, &edit.id) {
        dependency_index = dependency_index.max(
            document
                .latest_host_modulator_input_index(&scope)
                .map(|index| index + 1)
                .unwrap_or(0),
        );
    }
    let consumers =
        generated_node_consumers(root_patch, interaction_state, view_key, edit, generated);
    let consumer_index = consumers
        .iter()
        .filter_map(|node_id| {
            patch_for_view(root_patch, view_key)
                .and_then(|patch| patch_node(patch, node_id))
                .and_then(|node| source_owner_location_for_node(node))
                .filter(|(form_id, _)| form_id.scope == scope)
                .map(|(form_id, _)| form_id.index)
        })
        .min()
        .unwrap_or_else(|| document.scope_len(&scope));
    if dependency_index > consumer_index {
        let Some(dependency_original_index) = dependency_index.checked_sub(1) else {
            return Ok(GeneratedFormInsertion::OriginalIndex(dependency_index));
        };
        let dependency_form = SourceFormId {
            scope: scope.clone(),
            index: dependency_original_index,
        };
        let moved_forms = generated_consumer_form_closure(root_patch, view_key, &scope, &consumers);
        if moved_forms.iter().any(|form| form == &dependency_form) {
            debug_log_writeback_event(
                "generated-binding-invalid",
                format!(
                    "view={view_key}\nnode={}\nreason=generated binding dependencies appear after its consumers\nattempted-source:\n{}",
                    edit.id,
                    document.emit()
                ),
            );
            return Err(WriteBackError::UnsupportedGeneratedBinding {
                view_key: view_key.to_string(),
                node_id: edit.id.clone(),
                reason: "generated binding dependencies appear after its consumers".to_string(),
            });
        }
        document.move_forms_after_dependency_if_needed(&moved_forms, &dependency_form)?;
        let mut insertion_dependencies = generated_source_dependency_forms(
            root_patch,
            interaction_state,
            view_key,
            &scope,
            edit,
        );
        if !insertion_dependencies
            .iter()
            .any(|form| form == &dependency_form)
        {
            insertion_dependencies.push(dependency_form);
        }
        return Ok(GeneratedFormInsertion::AfterCurrentForms(
            insertion_dependencies,
        ));
    }
    if let Some(host_position) = host_modulator_insert_position {
        let original_position =
            document.insertion_position_for_original_index(&scope, dependency_index);
        if host_position > original_position {
            let moved_forms =
                generated_consumer_form_closure(root_patch, view_key, &scope, &consumers);
            let insertion_dependencies = generated_source_dependency_forms(
                root_patch,
                interaction_state,
                view_key,
                &scope,
                edit,
            );
            if !insertion_dependencies.is_empty() {
                document.move_forms_after_current_position_if_needed(
                    &insertion_dependencies,
                    &scope,
                    host_position,
                )?;
                if let Some(last_dependency) = insertion_dependencies
                    .iter()
                    .filter(|form| form.scope == scope)
                    .max_by_key(|form| document.form_position(form).unwrap_or(0))
                {
                    document
                        .move_forms_after_dependency_if_needed(&moved_forms, last_dependency)?;
                }
                return Ok(GeneratedFormInsertion::AfterCurrentForms(
                    insertion_dependencies,
                ));
            }
            let insertion_position = document.move_forms_after_current_position_if_needed(
                &moved_forms,
                &scope,
                host_position,
            )?;
            return Ok(GeneratedFormInsertion::CurrentPosition(insertion_position));
        }
    }
    Ok(GeneratedFormInsertion::OriginalIndex(dependency_index))
}

fn generated_source_dependency_forms(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    scope: &SourceScopeId,
    edit: &super::state::PatcherNodeEdit,
) -> Vec<SourceFormId> {
    let mut forms = Vec::new();
    let mut seen = HashSet::new();
    for node_id in generated_node_dependencies(root_patch, interaction_state, view_key, edit) {
        if let Some((form_id, _)) = patch_for_view(root_patch, view_key)
            .and_then(|patch| patch_node(patch, &node_id))
            .and_then(|node| source_owner_location_for_node(node))
            && &form_id.scope == scope
            && seen.insert(form_id.clone())
        {
            forms.push(form_id.clone());
        }
    }
    forms
}

fn generated_consumer_form_closure(
    root_patch: &Patch,
    view_key: &str,
    scope: &SourceScopeId,
    consumers: &[String],
) -> Vec<SourceFormId> {
    let Some(patch) = patch_for_view(root_patch, view_key) else {
        return Vec::new();
    };
    let mut forms = Vec::new();
    let mut seen = HashSet::new();
    for consumer in consumers {
        for form in dependent_form_closure(patch, consumer, scope) {
            if seen.insert(form.clone()) {
                forms.push(form);
            }
        }
    }
    forms
}

fn generated_expression_source_dependency_index(
    document: &SourceDocument,
    scope: &SourceScopeId,
    expr: &Expression,
) -> usize {
    let source_bindings = document.source_binding_dependency_indices(scope);
    expression_source_dependency_index(expr, &source_bindings)
}

fn generated_chain_source_dependency_index(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
    view_key: &str,
    node_id: &str,
) -> Result<usize, WriteBackError> {
    let mut visiting = HashSet::new();
    generated_chain_source_dependency_index_inner(
        document,
        root_patch,
        interaction_state,
        generated,
        history_bindings,
        view_key,
        node_id,
        &mut visiting,
    )
}

fn generated_chain_source_dependency_index_inner(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
    view_key: &str,
    node_id: &str,
    visiting: &mut HashSet<String>,
) -> Result<usize, WriteBackError> {
    if !visiting.insert(node_id.to_string()) {
        return Ok(0);
    }
    let Some(edit) = created_value_node(interaction_state, view_key, node_id) else {
        visiting.remove(node_id);
        return Ok(0);
    };

    let scope = scope_for_view_key(view_key);
    let expr = created_value_expression(
        document,
        root_patch,
        interaction_state,
        generated,
        history_bindings,
        view_key,
        edit,
    )?;
    let mut dependency_index =
        generated_expression_source_dependency_index(document, &scope, &expr);

    for connection in interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.to.node_id == node_id)
        .filter(|connection| {
            created_value_node(interaction_state, view_key, &connection.from.node_id).is_some()
        })
    {
        dependency_index = dependency_index.max(generated_chain_source_dependency_index_inner(
            document,
            root_patch,
            interaction_state,
            generated,
            history_bindings,
            view_key,
            &connection.from.node_id,
            visiting,
        )?);
    }

    visiting.remove(node_id);
    Ok(dependency_index)
}

fn expression_source_dependency_index(
    expr: &Expression,
    source_bindings: &HashMap<String, usize>,
) -> usize {
    match expr {
        Expression::Symbol(symbol) => source_bindings.get(symbol.as_str()).copied().unwrap_or(0),
        Expression::List(items) => items
            .iter()
            .enumerate()
            .filter(|(idx, item)| {
                *idx != 0
                    && !matches!(item, Expression::Symbol(symbol) if symbol.starts_with('@'))
                    && !matches!(items.get(idx.saturating_sub(1)), Some(Expression::Symbol(symbol)) if symbol.starts_with('@'))
            })
            .map(|(_, item)| expression_source_dependency_index(item, source_bindings))
            .max()
            .unwrap_or(0),
        _ => 0,
    }
}

fn macro_body_side_effect_form(expr: &Expression) -> bool {
    let Expression::List(items) = expr else {
        return false;
    };
    matches!(
        symbol_at(items, 0),
        Some("def" | "make-history" | "write-history")
    )
}

fn generated_node_dependencies(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    edit: &super::state::PatcherNodeEdit,
) -> Vec<String> {
    let Some(patch) = patch_for_view(root_patch, view_key) else {
        return Vec::new();
    };
    let mut visited = HashSet::new();
    let mut dependencies = Vec::new();
    for connection in interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.to.node_id == edit.id)
    {
        collect_generated_node_prior_form_dependencies(
            patch,
            interaction_state,
            view_key,
            &connection.from.node_id,
            &mut visited,
            &mut dependencies,
        );
    }
    dependencies
}

fn generated_node_requires_host_modulator_inputs(
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    node_id: &str,
) -> bool {
    let mut visited = HashSet::new();
    generated_node_requires_host_modulator_inputs_inner(
        interaction_state,
        view_key,
        node_id,
        &mut visited,
    )
}

fn generated_node_requires_host_modulator_inputs_inner(
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    node_id: &str,
    visited: &mut HashSet<String>,
) -> bool {
    if !visited.insert(node_id.to_string()) {
        return false;
    }
    if let Some(edit) = created_value_node(interaction_state, view_key, node_id) {
        if created_node_requires_host_modulator_inputs(edit) {
            return true;
        }
    } else {
        return false;
    }
    interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.to.node_id == node_id)
        .any(|connection| {
            generated_node_requires_host_modulator_inputs_inner(
                interaction_state,
                view_key,
                &connection.from.node_id,
                visited,
            )
        })
}

fn rewrite_created_literal_consumers(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    view_key: &str,
) -> Result<(), WriteBackError> {
    let mut consumers = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key)
        .filter_map(|connection| {
            created_value_node(interaction_state, view_key, &connection.from.node_id)
                .and_then(created_literal_expr)?;
            (generated.get(view_key, &connection.to.node_id).is_none()).then_some(connection)
        })
        .collect::<Vec<_>>();
    consumers.sort_by_key(|connection| (connection.to.node_id.clone(), connection.to.input_index));
    for connection in consumers {
        let Some(dest) = patch_for_view(root_patch, view_key)
            .and_then(|patch| patch_node(patch, &connection.to.node_id))
        else {
            return Err(WriteBackError::UnsupportedGeneratedBinding {
                view_key: view_key.to_string(),
                node_id: connection.from.node_id.clone(),
                reason: "created literal consumer must be source-backed or generated".to_string(),
            });
        };
        let value = value_reference_expr(
            document,
            root_patch,
            interaction_state,
            generated,
            &HistoryBindings::default(),
            view_key,
            &connection.from,
        )?;
        rewrite_node_input(document, view_key, dest, connection.to.input_index, value)?;
    }
    Ok(())
}

fn generated_source_reference_requires_prior_form(node: &PatchNode) -> bool {
    let Some(source) = node.source.as_ref() else {
        return false;
    };
    match &source.owner {
        SourceOwner::BindingValue { .. } => true,
        SourceOwner::TopLevelForm { .. } if node.kind == NodeKind::Param => true,
        _ => false,
    }
}

fn collect_generated_node_prior_form_dependencies(
    patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    node_id: &str,
    visited: &mut HashSet<String>,
    dependencies: &mut Vec<String>,
) {
    if !visited.insert(node_id.to_string()) {
        return;
    }
    if let Some(edit) = created_value_node(interaction_state, view_key, node_id) {
        if created_literal_expr(edit).is_none() {
            for connection in
                interaction_state
                    .edit_state
                    .connections
                    .values()
                    .filter(|connection| {
                        connection.view_key == view_key && connection.to.node_id == node_id
                    })
            {
                collect_generated_node_prior_form_dependencies(
                    patch,
                    interaction_state,
                    view_key,
                    &connection.from.node_id,
                    visited,
                    dependencies,
                );
            }
        }
        return;
    }
    let Some(node) = patch_node(patch, node_id) else {
        return;
    };
    if generated_source_reference_requires_prior_form(node) {
        dependencies.push(node_id.to_string());
    }
    for connection in patch.connections.iter().filter(|connection| {
        connection.to_node == node_id
            && created_value_node(interaction_state, view_key, &connection.from_node).is_none()
    }) {
        collect_generated_node_prior_form_dependencies(
            patch,
            interaction_state,
            view_key,
            &connection.from_node,
            visited,
            dependencies,
        );
    }
}

fn generated_node_consumers(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    edit: &super::state::PatcherNodeEdit,
    generated: &GeneratedBindings,
) -> Vec<String> {
    let mut consumers = Vec::new();
    let mut seen = HashSet::new();
    let mut visiting = HashSet::new();
    collect_generated_node_consumers(
        root_patch,
        interaction_state,
        generated,
        view_key,
        &edit.id,
        &mut visiting,
        &mut seen,
        &mut consumers,
    );
    consumers
}

fn collect_generated_node_consumers(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    view_key: &str,
    node_id: &str,
    visiting: &mut HashSet<String>,
    seen: &mut HashSet<String>,
    consumers: &mut Vec<String>,
) {
    if !visiting.insert(node_id.to_string()) {
        return;
    }
    let mut outgoing = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.from.node_id == node_id)
        .collect::<Vec<_>>();
    outgoing.sort_by_key(|connection| (connection.to.node_id.clone(), connection.to.input_index));
    for connection in outgoing {
        if connection_edit_touches_created_out(interaction_state, connection)
            || node_is_history(
                root_patch,
                interaction_state,
                view_key,
                &connection.to.node_id,
            )
        {
            continue;
        }
        if created_value_node(interaction_state, view_key, &connection.to.node_id).is_some() {
            collect_generated_node_consumers(
                root_patch,
                interaction_state,
                generated,
                view_key,
                &connection.to.node_id,
                visiting,
                seen,
                consumers,
            );
            continue;
        }
        if generated.get(view_key, &connection.to.node_id).is_some() {
            continue;
        }
        if patch_for_view(root_patch, view_key)
            .and_then(|patch| patch_node(patch, &connection.to.node_id))
            .is_some()
            && seen.insert(connection.to.node_id.clone())
        {
            consumers.push(connection.to.node_id.clone());
        }
    }
    visiting.remove(node_id);
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
    if generated.get(view_key, &edit.id).is_none() {
        return Ok(());
    }
    let mut consumers = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.from.node_id == edit.id)
        .filter(|connection| !connection_edit_touches_created_out(interaction_state, connection))
        .filter(|connection| generated.get(view_key, &connection.to.node_id).is_none())
        .filter(|connection| {
            created_value_node(interaction_state, view_key, &connection.to.node_id).is_none()
        })
        .filter(|connection| {
            !node_is_history(
                root_patch,
                interaction_state,
                view_key,
                &connection.to.node_id,
            )
        })
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
            Expression::Symbol(
                generated
                    .get_output(view_key, &edit.id, connection.from.output_index)
                    .unwrap_or_else(|| {
                        generated
                            .get(view_key, &edit.id)
                            .expect("generated binding has output zero")
                    })
                    .to_string(),
            ),
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
        if let Some(call_shape) = source.call_shape.as_ref() {
            return document
                .replace_or_insert_call_positional_arg(&call_shape.call, input_index, value)
                .map_err(|error| writeback_error_with_node(error, view_key, &node.id));
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
                return document
                    .replace_expr(&arg.expr, value)
                    .map_err(|error| writeback_error_with_node(error, view_key, &node.id));
            }
            return document.replace_or_insert_macro_return(
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

fn reorder_destination_after_new_dependency(
    document: &mut SourceDocument,
    root_patch: &Patch,
    view_key: &str,
    from: &OutputPortRef,
    dest: &PatchNode,
) -> Result<(), WriteBackError> {
    let Some(patch) = patch_for_view(root_patch, view_key) else {
        return Ok(());
    };
    let Some(source) = patch_node(patch, from.node_id.as_str()) else {
        return Ok(());
    };

    let Some((source_form, _)) = source_owner_location_for_node(source) else {
        return Ok(());
    };
    let Some((dest_form, _)) = source_owner_location_for_node(dest) else {
        return Ok(());
    };
    if source_form == dest_form {
        return Ok(());
    }
    if source_form.scope != dest_form.scope {
        return Ok(());
    }

    let moved_forms = dependent_form_closure(patch, &dest.id, &dest_form.scope);
    if moved_forms.iter().any(|form| form == source_form) {
        return Err(WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: dest.id.clone(),
            reason: "connection would move a node after one of its own dependents".to_string(),
        });
    }
    document.move_forms_after_dependency_if_needed(&moved_forms, source_form)
}

fn dependent_form_closure(
    patch: &Patch,
    root_node_id: &str,
    scope: &SourceScopeId,
) -> Vec<SourceFormId> {
    let mut stack = vec![root_node_id.to_string()];
    let mut visited_nodes = HashSet::new();
    let mut forms = Vec::new();
    let mut seen_forms = HashSet::new();

    while let Some(node_id) = stack.pop() {
        if !visited_nodes.insert(node_id.clone()) {
            continue;
        }
        if let Some(node) = patch_node(patch, &node_id)
            && let Some((form_id, _)) = source_owner_location_for_node(node)
            && &form_id.scope == scope
            && seen_forms.insert(form_id.clone())
        {
            forms.push(form_id.clone());
        }
        for connection in patch
            .connections
            .iter()
            .filter(|connection| connection.from_node == node_id)
        {
            stack.push(connection.to_node.clone());
        }
    }

    forms
}

fn apply_cable_writeback(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
) -> Result<(), WriteBackError> {
    // Pass 1: source-backed cables that were deleted outright.
    //
    // A cable deletion means "this destination inlet no longer has a source".
    // For ordinary source-owned destination nodes we preserve the inlet slot by
    // writing a missing-input sentinel into the destination call. Some deleted
    // cables are intentionally ignored here because another pass owns the
    // semantic replacement, e.g. deleting a cable as part of replacing it with
    // a history or created-value connection.
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
        ) || deleted_connection_has_created_connection_replacement(
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

    // Pass 2: source-backed cables whose endpoints changed.
    //
    // Editing an existing source cable is handled as "clear the old destination
    // input, then write the source expression into the new destination input".
    // Layout-only edits, such as changing a segmented cable's bend row, do not
    // touch Lisp source and are skipped.
    let mut source_edits = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| matches!(connection.origin, PatcherConnectionOrigin::Source { .. }))
        .filter(|connection| !source_connection_edit_is_layout_only(root_patch, connection))
        .collect::<Vec<_>>();
    source_edits.sort_by(|a, b| a.id.cmp(&b.id));
    for connection in source_edits {
        apply_source_connection_edit_writeback(
            document,
            root_patch,
            interaction_state,
            generated,
            history_bindings,
            connection,
        )?;
    }

    // Pass 3: new cables created by dragging an outlet onto an inlet.
    //
    // The interaction layer has already turned the editor gesture into a
    // PatcherConnectionEdit. Writeback only needs to resolve the outlet into a
    // Lisp expression, find the destination node, and replace/insert the
    // destination input expression.
    let mut created = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| matches!(connection.origin, PatcherConnectionOrigin::Created { .. }))
        .collect::<Vec<_>>();
    created.sort_by(|a, b| a.id.cmp(&b.id));
    for connection in created {
        if connection_edit_touches_history(root_patch, interaction_state, connection)
            || connection_edit_touches_created_value(interaction_state, connection)
            || connection_edit_touches_created_out(interaction_state, connection)
        {
            continue;
        }
        // Convert the source outlet into the expression that should appear in
        // the destination call: a binding symbol, generated binding name,
        // literal value, nested expression, history read, etc.
        let value = value_reference_expr(
            document,
            root_patch,
            interaction_state,
            generated,
            history_bindings,
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
        if source_node_call_path_was_replaced_by_created_rewrite(
            document,
            root_patch,
            interaction_state,
            generated,
            &connection.view_key,
            dest,
        ) {
            continue;
        }
        // Apply the edit to the destination node's source-owned call shape.
        // The helper preserves semantic input indexes and inserts sentinel
        // values for any missing positional gaps before attributes.
        rewrite_node_input(
            document,
            &connection.view_key,
            dest,
            connection.to.input_index,
            value,
        )?;
        reorder_destination_after_new_dependency(
            document,
            root_patch,
            &connection.view_key,
            &connection.from,
            dest,
        )?;
    }
    Ok(())
}

fn source_node_call_path_was_replaced_by_created_rewrite(
    document: &SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    view_key: &str,
    node: &PatchNode,
) -> bool {
    let Some(call_shape) = node
        .source
        .as_ref()
        .and_then(|source| source.call_shape.as_ref())
    else {
        return false;
    };
    if matches!(document.expr(&call_shape.call), Some(Expression::List(_))) {
        return false;
    }
    source_node_feeds_generated_binding(
        root_patch,
        interaction_state,
        generated,
        view_key,
        &node.id,
    ) || source_node_feeds_created_value_rewrite(root_patch, interaction_state, view_key, &node.id)
}

fn source_node_feeds_generated_binding(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    view_key: &str,
    node_id: &str,
) -> bool {
    let source_connections = patch_for_view(root_patch, view_key)
        .into_iter()
        .flat_map(|patch| patch.connections.iter())
        .filter(|connection| connection.from_node == node_id)
        .filter(|connection| {
            !interaction_state
                .edit_state
                .deleted_connections
                .contains(&connection_edit_key(
                    view_key,
                    &source_connection_id(connection),
                ))
        })
        .any(|connection| generated.get(view_key, &connection.to_node).is_some());
    let created_connections = interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key)
        .filter(|connection| connection.from.node_id == node_id)
        .any(|connection| generated.get(view_key, &connection.to.node_id).is_some());
    source_connections || created_connections
}

fn source_node_feeds_created_value_rewrite(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    node_id: &str,
) -> bool {
    interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key)
        .filter(|connection| connection.from.node_id == node_id)
        .filter(|connection| {
            created_value_node(interaction_state, view_key, &connection.to.node_id).is_some()
        })
        .any(|connection| {
            created_value_has_source_backed_consumer(
                root_patch,
                interaction_state,
                view_key,
                &connection.to.node_id,
            )
        })
}

fn created_value_has_source_backed_consumer(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    node_id: &str,
) -> bool {
    interaction_state
        .edit_state
        .connections
        .values()
        .filter(|connection| connection.view_key == view_key && connection.from.node_id == node_id)
        .any(|connection| {
            connection_edit_touches_created_out(interaction_state, connection)
                || patch_for_view(root_patch, view_key)
                    .and_then(|patch| patch_node(patch, &connection.to.node_id))
                    .and_then(|node| node.source.as_ref())
                    .is_some()
        })
}

fn apply_source_connection_edit_writeback(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
    edit: &PatcherConnectionEdit,
) -> Result<(), WriteBackError> {
    let PatcherConnectionOrigin::Source {
        source_connection_id,
    } = &edit.origin
    else {
        return Ok(());
    };
    let Some(original) = source_connection(root_patch, &edit.view_key, source_connection_id) else {
        return Err(WriteBackError::UnsupportedDeletedConnection {
            view_key: edit.view_key.clone(),
            connection_id: edit.id.clone(),
        });
    };
    let Some(original_dest) = patch_for_view(root_patch, &edit.view_key)
        .and_then(|patch| patch_node(patch, &original.to_node))
    else {
        return Err(WriteBackError::UnsupportedDeletedConnection {
            view_key: edit.view_key.clone(),
            connection_id: edit.id.clone(),
        });
    };
    let Some(new_dest) = patch_for_view(root_patch, &edit.view_key)
        .and_then(|patch| patch_node(patch, &edit.to.node_id))
    else {
        return Err(WriteBackError::UnsupportedCreatedConnection {
            view_key: edit.view_key.clone(),
            connection_id: edit.id.clone(),
        });
    };
    let same_destination =
        original.to_node == edit.to.node_id && original.to_input == edit.to.input_index;
    if !same_destination {
        rewrite_node_input(
            document,
            &edit.view_key,
            original_dest,
            original.to_input,
            Expression::Symbol(MISSING_INPUT_SENTINEL.to_string()),
        )?;
    }
    let value = value_reference_expr(
        document,
        root_patch,
        interaction_state,
        generated,
        history_bindings,
        &edit.view_key,
        &edit.from,
    )?;
    rewrite_node_input(
        document,
        &edit.view_key,
        new_dest,
        edit.to.input_index,
        value,
    )?;
    reorder_destination_after_new_dependency(
        document,
        root_patch,
        &edit.view_key,
        &edit.from,
        new_dest,
    )
}

fn apply_history_writeback(
    document: &mut SourceDocument,
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    generated: &GeneratedBindings,
    history_bindings: &HistoryBindings,
) -> Result<(), WriteBackError> {
    let mut pending_write_forms: Vec<(SourceScopeId, String, Expression)> = Vec::new();

    for view_key in writeback_views(root_patch, interaction_state) {
        if patch_for_view(root_patch, &view_key).is_none() {
            continue;
        };
        validate_single_history_writes(root_patch, interaction_state, &view_key)?;

        let scope = scope_for_view_key(&view_key);
        let mut created_connections = interaction_state
            .edit_state
            .connections
            .values()
            .filter(|edit| edit.view_key == view_key)
            .filter(|edit| matches!(edit.origin, PatcherConnectionOrigin::Created { .. }))
            .collect::<Vec<_>>();
        created_connections.sort_by(|a, b| a.id.cmp(&b.id));
        for connection in created_connections {
            if let Some(history_name) = history_bindings.get(&view_key, &connection.from.node_id) {
                if generated.get(&view_key, &connection.to.node_id).is_some()
                    || created_value_node(interaction_state, &view_key, &connection.to.node_id)
                        .is_some()
                {
                    continue;
                }
                apply_history_read_connection(
                    document,
                    root_patch,
                    &view_key,
                    connection,
                    history_name,
                )?;
            } else if let Some(history_name) =
                history_bindings.get(&view_key, &connection.to.node_id)
            {
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
                    pending_write_forms.push((scope.clone(), history_name.to_string(), value));
                }
            } else {
                if generated.get(&view_key, &connection.from.node_id).is_some()
                    || generated.get(&view_key, &connection.to.node_id).is_some()
                    || connection_edit_touches_created_value(interaction_state, connection)
                    || connection_edit_touches_created_out(interaction_state, connection)
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

    for (scope, name) in history_bindings.pending_make_forms.iter().rev() {
        document.prepend_form(
            scope,
            Expression::List(vec![
                Expression::Symbol("make-history".to_string()),
                Expression::Symbol(name.clone()),
            ]),
        )?;
    }
    for (scope, name, value) in pending_write_forms {
        document.insert_history_write(&scope, name.to_string(), value)?;
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
        if source_connection_has_semantic_edit(root_patch, interaction_state, view_key, connection)
        {
            continue;
        }
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
        if source_connection_edit_is_layout_only(root_patch, connection) {
            continue;
        }
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

fn source_connection_has_semantic_edit(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    connection: &PatchConnection,
) -> bool {
    let connection_id = source_connection_id(connection);
    interaction_state
        .edit_state
        .connections
        .values()
        .any(|edit| {
            edit.view_key == view_key
                && matches!(
                    &edit.origin,
                    PatcherConnectionOrigin::Source {
                        source_connection_id
                    } if source_connection_id == &connection_id
                )
                && !source_connection_edit_is_layout_only(root_patch, edit)
        })
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
        if let Some(call_shape) = source.call_shape.as_ref() {
            return document
                .replace_or_insert_call_positional_arg(
                    &call_shape.call,
                    connection.to.input_index,
                    read_expr,
                )
                .map_err(|error| {
                    writeback_error_with_node(error, view_key, &connection.to.node_id)
                });
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

fn writeback_error_with_node(
    error: WriteBackError,
    view_key: &str,
    node_id: &str,
) -> WriteBackError {
    match error {
        WriteBackError::InvalidEdit { reason, .. } => WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: node_id.to_string(),
            reason,
        },
        other => other,
    }
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
    if let Some(name) = generated.get_output(view_key, &from.node_id, from.output_index) {
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
        SourceOwner::TopLevelForm { .. } | SourceOwner::BindingValue { .. }
            if node.kind == NodeKind::Param =>
        {
            let Some(name) = param_node_name(node) else {
                return Err(WriteBackError::UnsupportedHistoryEdit {
                    view_key: view_key.to_string(),
                    history_id: String::new(),
                    reason: "param history write source has no param name".to_string(),
                });
            };
            Ok((Expression::Symbol(name.to_string()), scope))
        }
        SourceOwner::BindingValue {
            binding: BindingTarget::Symbol(name),
            ..
        } => Ok((Expression::Symbol(name.clone()), scope)),
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

    fn reserve(&mut self, scope: &SourceScopeId, name: &str) -> Result<(), String> {
        let used = self.used_by_scope.entry(scope.clone()).or_default();
        if used.insert(name.to_string()) {
            Ok(())
        } else {
            Err(name.to_string())
        }
    }
}

fn generated_binding_stem(operator: &str) -> String {
    let mapped = match operator {
        "*" => "mul",
        "+" => "add",
        "-" => "sub",
        "/" => "div",
        "mod" => "modulated",
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
    root_patch: &Patch,
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
    if let SourceOwner::MacroParameter { binding, index } = &source.owner {
        return apply_macro_parameter_text_edit(
            document, root_patch, view_key, node, binding, *index, text,
        );
    }
    let replacement =
        edited_expression_for_node(text, node, source.expr.as_ref(), document, root_patch)
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

    if node.kind == NodeKind::Param
        && let SourceOwner::TopLevelForm { form_id } = &source.owner
    {
        return apply_param_text_edit(document, root_patch, view_key, node, form_id, replacement);
    }

    match &source.owner {
        SourceOwner::BindingValue {
            form_id,
            binding,
            value_path,
        } => {
            if param_name(&replacement).is_some() {
                return apply_binding_value_to_param_text_edit(
                    document,
                    root_patch,
                    view_key,
                    node,
                    form_id,
                    binding,
                    replacement,
                );
            }
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

fn node_requires_param_form_normalization(node: &PatchNode) -> bool {
    node.kind == NodeKind::Param
        && node
            .source
            .as_ref()
            .is_some_and(|source| matches!(source.owner, SourceOwner::BindingValue { .. }))
}

fn param_node_name(node: &PatchNode) -> Option<&str> {
    (node.kind == NodeKind::Param)
        .then(|| node.label.split_whitespace().nth(1))
        .flatten()
}

fn reorder_modulatable_param_after_host_modulators(
    document: &mut SourceDocument,
    root_patch: &Patch,
    view_key: &str,
    node: &PatchNode,
    form_id: &SourceFormId,
    replacement: &Expression,
) -> Result<(), WriteBackError> {
    if view_key != "root" {
        return Ok(());
    }
    let Expression::List(items) = replacement else {
        return Ok(());
    };
    if symbol_at(items, 0) != Some("param") || !expression_has_true_attribute(items, "@mod") {
        return Ok(());
    }
    let Some(dependency_original_index) =
        document.latest_host_modulator_input_index(&form_id.scope)
    else {
        return Ok(());
    };
    let dependency_form = SourceFormId {
        scope: form_id.scope.clone(),
        index: dependency_original_index,
    };
    let Some(patch) = patch_for_view(root_patch, view_key) else {
        return Ok(());
    };
    let moved_forms = dependent_form_closure(patch, &node.id, &form_id.scope);
    if moved_forms.iter().any(|form| form == &dependency_form) {
        return Err(WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            reason: "modulatable param would move after one of its own dependents".to_string(),
        });
    }
    document.move_forms_after_dependency_if_needed(&moved_forms, &dependency_form)
}

fn apply_param_text_edit(
    document: &mut SourceDocument,
    root_patch: &Patch,
    view_key: &str,
    node: &PatchNode,
    form_id: &SourceFormId,
    replacement: Expression,
) -> Result<(), WriteBackError> {
    let old_name = document
        .form_expr(&form_id.scope, form_id.index)
        .and_then(param_name)
        .ok_or_else(|| WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            reason: "param source has no symbolic name".to_string(),
        })?
        .to_string();
    let new_name = param_name(&replacement).map(str::to_string);

    let Some(new_name) = new_name else {
        let expr_id = SourceExprId {
            form_id: form_id.clone(),
            path: Default::default(),
        };
        return document.replace_expr(
            &expr_id,
            Expression::List(vec![
                Expression::Symbol("def".to_string()),
                Expression::Symbol(old_name),
                replacement,
            ]),
        );
    };

    if new_name != old_name {
        let reserved = document.reserved_names(&form_id.scope);
        if reserved.contains(&new_name) {
            return Err(WriteBackError::BindingRenameCollision {
                view_key: view_key.to_string(),
                node_id: node.id.clone(),
                name: new_name,
            });
        }
        if patch_for_view(root_patch, view_key).is_some_and(|patch| {
            patch
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::CodeIsland)
        }) {
            return Err(WriteBackError::BindingRenameBlockedByCodeIsland {
                view_key: view_key.to_string(),
                node_id: node.id.clone(),
                name: old_name,
            });
        }
    }

    let expr_id = SourceExprId {
        form_id: form_id.clone(),
        path: Default::default(),
    };
    document.replace_expr(&expr_id, replacement.clone())?;
    reorder_modulatable_param_after_host_modulators(
        document,
        root_patch,
        view_key,
        node,
        form_id,
        &replacement,
    )?;

    if new_name == old_name {
        return Ok(());
    }

    let binding = BindingId {
        scope: form_id.scope.clone(),
        name: old_name,
        kind: BindingKind::Param,
    };
    let patch =
        patch_for_view(root_patch, view_key).ok_or_else(|| WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            reason: "param rename targets a missing patch view".to_string(),
        })?;
    for expr in resolved_binding_references(patch, &binding) {
        document.replace_expr(&expr, Expression::Symbol(new_name.clone()))?;
    }
    Ok(())
}

fn apply_binding_value_to_param_text_edit(
    document: &mut SourceDocument,
    root_patch: &Patch,
    view_key: &str,
    node: &PatchNode,
    form_id: &SourceFormId,
    binding: &BindingTarget,
    replacement: Expression,
) -> Result<(), WriteBackError> {
    let BindingTarget::Symbol(old_name) = binding else {
        return Err(WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            reason: "destructuring bindings cannot be converted to params".to_string(),
        });
    };
    let new_name = param_name(&replacement)
        .ok_or_else(|| WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            reason: "param edit must provide a symbolic parameter name".to_string(),
        })?
        .to_string();

    if new_name != *old_name {
        let reserved = document.reserved_names(&form_id.scope);
        if reserved.contains(&new_name) {
            return Err(WriteBackError::BindingRenameCollision {
                view_key: view_key.to_string(),
                node_id: node.id.clone(),
                name: new_name,
            });
        }
        if patch_for_view(root_patch, view_key).is_some_and(|patch| {
            patch
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::CodeIsland)
        }) {
            return Err(WriteBackError::BindingRenameBlockedByCodeIsland {
                view_key: view_key.to_string(),
                node_id: node.id.clone(),
                name: old_name.clone(),
            });
        }
    }

    let expr_id = SourceExprId {
        form_id: form_id.clone(),
        path: Default::default(),
    };
    document.replace_expr(&expr_id, replacement.clone())?;
    reorder_modulatable_param_after_host_modulators(
        document,
        root_patch,
        view_key,
        node,
        form_id,
        &replacement,
    )?;

    if new_name == *old_name {
        return Ok(());
    }

    let binding = BindingId {
        scope: form_id.scope.clone(),
        name: old_name.clone(),
        kind: BindingKind::Def,
    };
    let patch =
        patch_for_view(root_patch, view_key).ok_or_else(|| WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            reason: "binding-to-param edit targets a missing patch view".to_string(),
        })?;
    for expr in resolved_binding_references(patch, &binding) {
        document.replace_expr(&expr, Expression::Symbol(new_name.clone()))?;
    }
    Ok(())
}

fn apply_macro_parameter_text_edit(
    document: &mut SourceDocument,
    root_patch: &Patch,
    view_key: &str,
    node: &PatchNode,
    binding: &BindingId,
    index: usize,
    text: &str,
) -> Result<(), WriteBackError> {
    let old_name = binding.name.clone();
    let new_name = macro_parameter_edit_name(text, index, &old_name).map_err(|reason| {
        WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            reason,
        }
    })?;
    if new_name == old_name {
        return Ok(());
    }
    let reserved = document.reserved_names(&binding.scope);
    if reserved.contains(&new_name) {
        return Err(WriteBackError::BindingRenameCollision {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            name: new_name,
        });
    }
    if patch_for_view(root_patch, view_key).is_some_and(|patch| {
        patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::CodeIsland)
    }) {
        return Err(WriteBackError::BindingRenameBlockedByCodeIsland {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            name: old_name,
        });
    }

    document.replace_macro_param(&binding.scope, index, new_name.clone())?;
    let patch =
        patch_for_view(root_patch, view_key).ok_or_else(|| WriteBackError::InvalidEdit {
            view_key: view_key.to_string(),
            node_id: node.id.clone(),
            reason: "macro parameter rename targets a missing patch view".to_string(),
        })?;
    for expr in resolved_binding_references(patch, binding) {
        document.replace_expr(&expr, Expression::Symbol(new_name.clone()))?;
    }
    Ok(())
}

fn macro_parameter_edit_name(text: &str, index: usize, old_name: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("macro parameter edit cannot be empty".to_string());
    }

    if let Ok(Expression::Symbol(name)) = parse_single_expression(trimmed) {
        return Ok(name);
    }

    let edited = parse_single_expression(&format!("({trimmed})"))?;
    let Expression::List(items) = edited else {
        return Err("macro parameter edit must be a symbol or in form".to_string());
    };
    if symbol_at(&items, 0) != Some("in") {
        return Err("macro parameter edit must be a symbol or in form".to_string());
    }
    match items.get(1) {
        Some(Expression::Number(channel))
            if (*channel - (index + 1) as f64).abs() < f64::EPSILON => {}
        Some(_) => {
            return Err(format!(
                "macro parameter edit must keep input channel {}",
                index + 1
            ));
        }
        None => return Err("macro parameter in form must include an input channel".to_string()),
    }
    Ok(symbol_attribute_value(&items, "@name")?.unwrap_or_else(|| old_name.to_string()))
}

fn symbol_attribute_value(items: &[Expression], attr: &str) -> Result<Option<String>, String> {
    for pair in items.windows(2) {
        if let Expression::Symbol(key) = &pair[0]
            && key == attr
        {
            return match &pair[1] {
                Expression::Symbol(value) => Ok(Some(value.clone())),
                _ => Err(format!("{attr} must be followed by a symbol")),
            };
        }
    }
    Ok(None)
}

fn resolved_binding_references(patch: &Patch, binding: &BindingId) -> Vec<SourceExprId> {
    let mut refs = patch
        .connections
        .iter()
        .filter_map(|connection| connection.source.as_ref())
        .filter_map(|source| match &source.previous_arg {
            SourceArgValue::SymbolReference {
                expr,
                resolved_binding: Some(resolved_binding),
                ..
            } if resolved_binding == binding => Some(expr.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    refs.sort_by(|left, right| {
        left.form_id
            .index
            .cmp(&right.form_id.index)
            .then_with(|| expr_path_indexes(left).cmp(&expr_path_indexes(right)))
    });
    refs.dedup();
    refs
}

fn param_name(expr: &Expression) -> Option<&str> {
    let Expression::List(items) = expr else {
        return None;
    };
    if symbol_at(items, 0) == Some("param") {
        symbol_at(items, 1)
    } else {
        None
    }
}

fn edited_expression_for_node(
    text: &str,
    node: &PatchNode,
    source_expr: Option<&SourceExprId>,
    document: &SourceDocument,
    root_patch: &Patch,
) -> Result<Expression, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("node text cannot be empty for write-back".to_string());
    }

    if node.kind == NodeKind::Constant {
        if let Ok(Expression::Symbol(symbol)) = parse_single_expression(trimmed)
            && document.is_known_operator(&symbol)
        {
            return expand_editor_mod_shorthand_for_node(
                Expression::List(vec![Expression::Symbol(symbol)]),
                source_expr,
                document,
            );
        }
        return parse_single_expression(trimmed)
            .or_else(|_| parse_single_expression(&format!("({trimmed})")))
            .and_then(|expr| expand_editor_mod_shorthand_for_node(expr, source_expr, document));
    }

    if node.kind == NodeKind::Param
        && !trimmed.starts_with("param")
        && let Ok(expr) = parse_single_expression(trimmed)
    {
        return expand_editor_mod_shorthand_for_node(expr, source_expr, document);
    }

    let edited = parse_single_expression(&format!("({trimmed})"))?;
    let Expression::List(edited_items) = edited else {
        return Err("node text must parse as a list expression".to_string());
    };
    if edited_items.is_empty() {
        return Err("node text must include an operator".to_string());
    }

    if node_display_omits_first_input(node)
        && let Some(source_expr) = source_expr
        && let Some(Expression::List(original_items)) = document.expr(source_expr)
        && let Some(first_input_item) =
            positional_item_index(original_items, 0).and_then(|idx| original_items.get(idx))
    {
        if let Some(merged) = merge_operator_only_edit_with_required_inputs(
            node,
            root_patch,
            original_items,
            &edited_items,
        ) {
            return expand_editor_mod_shorthand_for_node(
                Expression::List(merged),
                Some(source_expr),
                document,
            );
        }
        if let Some(merged) =
            merge_operator_rename_preserving_original_inputs(original_items, &edited_items)
        {
            return expand_editor_mod_shorthand_for_node(
                Expression::List(merged),
                Some(source_expr),
                document,
            );
        }
        if let Some(merged) = merge_edited_inline_inputs(node, original_items, &edited_items) {
            return expand_editor_mod_shorthand_for_node(
                Expression::List(merged),
                Some(source_expr),
                document,
            );
        }
        let mut merged = Vec::with_capacity(edited_items.len() + 1);
        merged.push(edited_items[0].clone());
        merged.push(first_input_item.clone());
        merged.extend(edited_items.iter().skip(1).cloned());
        append_missing_original_attributes(original_items, &mut merged);
        return expand_editor_mod_shorthand_for_node(
            Expression::List(merged),
            Some(source_expr),
            document,
        );
    }

    if edited_items.len() == 1
        && let Some(source_expr) = source_expr
        && let Some(Expression::List(original_items)) = document.expr(source_expr)
    {
        let mut merged = original_items.clone();
        merged[0] = edited_items[0].clone();
        return expand_editor_mod_shorthand_for_node(
            Expression::List(merged),
            Some(source_expr),
            document,
        );
    }

    expand_editor_mod_shorthand_for_node(Expression::List(edited_items), source_expr, document)
}

fn merge_operator_rename_preserving_original_inputs(
    original_items: &[Expression],
    edited_items: &[Expression],
) -> Option<Vec<Expression>> {
    if edited_items.len() != 1 || symbol_at(edited_items, 0)? == symbol_at(original_items, 0)? {
        return None;
    }
    let mut merged = original_items.to_vec();
    merged[0] = edited_items[0].clone();
    Some(merged)
}

fn merge_operator_only_edit_with_required_inputs(
    node: &PatchNode,
    root_patch: &Patch,
    original_items: &[Expression],
    edited_items: &[Expression],
) -> Option<Vec<Expression>> {
    if edited_items.len() != 1 || node_display_input_slots(node).is_empty() {
        return None;
    }
    let edited_op = symbol_at(edited_items, 0)?;
    let original_op = symbol_at(original_items, 0)?;
    if edited_op != original_op {
        return None;
    }

    let input_count = required_input_count_for_existing_node_edit(node, root_patch, edited_op);
    let mut merged = Vec::with_capacity(1 + input_count + original_items.len());
    merged.push(edited_items[0].clone());
    for semantic_index in 0..input_count {
        let Some(item_index) = positional_item_index(original_items, semantic_index) else {
            merged.push(Expression::Symbol(MISSING_INPUT_SENTINEL.to_string()));
            continue;
        };
        merged.push(original_items[item_index].clone());
    }
    append_original_attributes(original_items, &mut merged);
    Some(merged)
}

fn required_input_count_for_existing_node_edit(
    node: &PatchNode,
    root_patch: &Patch,
    op: &str,
) -> usize {
    match node.kind {
        NodeKind::In | NodeKind::Param | NodeKind::Constant => 0,
        NodeKind::Out | NodeKind::History => 1,
        NodeKind::MacroInstance => root_patch
            .macros
            .iter()
            .find(|macro_patch| macro_patch.name == op)
            .map(|macro_patch| macro_patch.params.len())
            .unwrap_or_else(|| node.args.len()),
        _ => dgenlisp_operator_required_input_counts()
            .get(op)
            .copied()
            .unwrap_or_else(|| node.args.len()),
    }
}

fn append_original_attributes(original_items: &[Expression], merged: &mut Vec<Expression>) {
    let mut idx = 1usize;
    while idx < original_items.len() {
        if matches!(&original_items[idx], Expression::Symbol(symbol) if symbol.starts_with('@')) {
            merged.push(original_items[idx].clone());
            if let Some(value) = original_items.get(idx + 1) {
                merged.push(value.clone());
            }
            idx += 2;
        } else {
            idx += 1;
        }
    }
}

fn append_missing_original_attributes(original_items: &[Expression], merged: &mut Vec<Expression>) {
    let existing = attribute_keys(merged);
    let mut idx = 1usize;
    while idx < original_items.len() {
        if matches!(&original_items[idx], Expression::Symbol(symbol) if symbol.starts_with('@')) {
            if let Expression::Symbol(symbol) = &original_items[idx]
                && !existing.contains(symbol)
            {
                merged.push(original_items[idx].clone());
                if let Some(value) = original_items.get(idx + 1) {
                    merged.push(value.clone());
                }
            }
            idx += 2;
        } else {
            idx += 1;
        }
    }
}

fn attribute_keys(items: &[Expression]) -> HashSet<String> {
    let mut keys = HashSet::new();
    let mut idx = 1usize;
    while idx < items.len() {
        if let Expression::Symbol(symbol) = &items[idx]
            && symbol.starts_with('@')
        {
            keys.insert(symbol.clone());
            idx += 2;
            continue;
        }
        idx += 1;
    }
    keys
}

fn merge_edited_inline_inputs(
    node: &PatchNode,
    original_items: &[Expression],
    edited_items: &[Expression],
) -> Option<Vec<Expression>> {
    let input_slots = node_display_input_slots(node);
    if input_slots.is_empty() || edited_items.len().saturating_sub(1) != input_slots.len() {
        return None;
    }
    let mut merged = original_items.to_vec();
    if merged.is_empty() {
        return None;
    }
    merged[0] = edited_items[0].clone();
    for (token_idx, input_index) in input_slots.into_iter().enumerate() {
        let replacement = &edited_items[token_idx + 1];
        if editor_placeholder_expr(replacement)
            && node
                .args
                .get(input_index)
                .is_some_and(|arg| matches!(arg, ArgValue::SymbolRef(_) | ArgValue::ConnectedExpr))
        {
            continue;
        }
        let item_index = positional_item_index(&merged, input_index)?;
        merged[item_index] = replacement.clone();
    }
    Some(merged)
}

fn editor_placeholder_expr(expr: &Expression) -> bool {
    matches!(expr, Expression::Symbol(symbol) if symbol == "?")
}

fn expand_editor_mod_shorthand_for_node(
    expr: Expression,
    source_expr: Option<&SourceExprId>,
    document: &SourceDocument,
) -> Result<Expression, String> {
    let scope = source_expr
        .map(|expr| expr.form_id.scope.clone())
        .unwrap_or(SourceScopeId::Root);
    expand_editor_mod_shorthand_expr(expr, document, &scope)
}

fn expand_editor_mod_shorthand_expr(
    expr: Expression,
    document: &SourceDocument,
    scope: &SourceScopeId,
) -> Result<Expression, String> {
    match expr {
        Expression::Symbol(symbol) => expand_editor_mod_shorthand_symbol(symbol, document, scope),
        Expression::List(items) => {
            let mut expanded = Vec::with_capacity(items.len());
            for (idx, item) in items.into_iter().enumerate() {
                if idx == 0 {
                    expanded.push(item);
                } else {
                    expanded.push(expand_editor_mod_shorthand_expr(item, document, scope)?);
                }
            }
            Ok(Expression::List(expanded))
        }
        other => Ok(other),
    }
}

fn expand_editor_mod_shorthand_symbol(
    symbol: String,
    document: &SourceDocument,
    scope: &SourceScopeId,
) -> Result<Expression, String> {
    let Some(name) = symbol.strip_suffix('~') else {
        return Ok(Expression::Symbol(symbol));
    };
    if name.is_empty() || !is_symbol_name(name) {
        return Err(format!("invalid mod shorthand `{symbol}`"));
    }
    if !document.param_is_modulatable(scope, name) {
        return Err(format!(
            "`{symbol}` requires `{name}` to be declared as a modulatable param"
        ));
    }
    Ok(Expression::List(vec![
        Expression::Symbol("mod".to_string()),
        Expression::Symbol(name.to_string()),
    ]))
}

fn node_display_omits_first_input(node: &PatchNode) -> bool {
    matches!(
        node.args.first(),
        Some(ArgValue::SymbolRef(_) | ArgValue::ConnectedExpr)
    )
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
    forms: Vec<DocumentForm>,
    macros: HashMap<String, MacroDocument>,
    original_forms: Vec<DocumentForm>,
    original_macros: HashMap<String, MacroDocument>,
    external_macros: HashSet<String>,
    virtual_modulatable_params: HashSet<(SourceScopeId, String)>,
}

#[derive(Debug, Clone)]
struct DocumentForm {
    original_index: Option<usize>,
    form: SourceForm,
}

#[derive(Debug, Clone)]
enum SourceForm {
    Expr(Expression),
    Macro(String),
}

#[derive(Debug, Clone)]
struct MacroBodyForm {
    original_index: Option<usize>,
    expr: Expression,
}

#[derive(Debug, Clone)]
struct MacroDocument {
    name: String,
    params: Vec<Expression>,
    body: Vec<MacroBodyForm>,
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
        for (index, expr) in exprs.into_iter().enumerate() {
            if let Some(macro_doc) = MacroDocument::from_expr(&expr) {
                forms.push(DocumentForm {
                    original_index: Some(index),
                    form: SourceForm::Macro(macro_doc.name.clone()),
                });
                macros.insert(macro_doc.name.clone(), macro_doc);
            } else {
                forms.push(DocumentForm {
                    original_index: Some(index),
                    form: SourceForm::Expr(expr),
                });
            }
        }
        Ok(Self {
            original_forms: forms.clone(),
            original_macros: macros.clone(),
            forms,
            macros,
            external_macros: HashSet::new(),
            virtual_modulatable_params: HashSet::new(),
        })
    }

    fn register_external_macros(&mut self, names: impl IntoIterator<Item = String>) {
        self.external_macros.extend(names);
    }

    fn expr(&self, expr_id: &SourceExprId) -> Option<&Expression> {
        let form = self.form_expr(&expr_id.form_id.scope, expr_id.form_id.index)?;
        expr_at_path(form, &expr_id.path.0)
    }

    fn original_expr(&self, expr_id: &SourceExprId) -> Option<&Expression> {
        let form = Self::form_expr_in(
            &self.original_forms,
            &self.original_macros,
            &expr_id.form_id.scope,
            expr_id.form_id.index,
        )?;
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

    fn replace_def_target(
        &mut self,
        form_id: &SourceFormId,
        target: BindingTarget,
    ) -> Result<(), WriteBackError> {
        let Some(form) = self.form_expr_mut(&form_id.scope, form_id.index) else {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key_for_scope(&form_id.scope),
                node_id: String::new(),
                reason: format!("missing source form {}", form_id.index),
            });
        };
        let Expression::List(items) = form else {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key_for_scope(&form_id.scope),
                node_id: String::new(),
                reason: "binding source form is not a list".to_string(),
            });
        };
        if symbol_at(items, 0) != Some("def") || items.len() < 3 {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key_for_scope(&form_id.scope),
                node_id: String::new(),
                reason: "binding source form is not a def".to_string(),
            });
        }
        items[1] = match target {
            BindingTarget::Symbol(name) => Expression::Symbol(name),
            BindingTarget::Destructuring(names) => {
                Expression::List(names.into_iter().map(Expression::Symbol).collect())
            }
        };
        Ok(())
    }

    fn replace_or_insert_call_positional_arg(
        &mut self,
        call: &SourceExprId,
        semantic_index: usize,
        value: Expression,
    ) -> Result<(), WriteBackError> {
        let Some(form) = self.form_expr_mut(&call.form_id.scope, call.form_id.index) else {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key_for_scope(&call.form_id.scope),
                node_id: String::new(),
                reason: format!("missing source form {}", call.form_id.index),
            });
        };
        let Some(expr) = expr_at_path_mut(form, &call.path.0) else {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key_for_scope(&call.form_id.scope),
                node_id: String::new(),
                reason: "source expression path does not resolve".to_string(),
            });
        };
        let Expression::List(items) = expr else {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key_for_scope(&call.form_id.scope),
                node_id: String::new(),
                reason: "source expression path does not point to a call".to_string(),
            });
        };
        replace_or_insert_positional_arg(items, semantic_index, value);
        Ok(())
    }

    fn replace_or_insert_macro_return(
        &mut self,
        expr_id: &SourceExprId,
        replacement: Expression,
    ) -> Result<(), WriteBackError> {
        if self
            .form_expr(&expr_id.form_id.scope, expr_id.form_id.index)
            .is_some()
        {
            return self.replace_expr(expr_id, replacement);
        }
        let SourceScopeId::Macro { name } = &expr_id.form_id.scope else {
            return self.replace_expr(expr_id, replacement);
        };
        let Some(macro_doc) = self.macros.get_mut(name) else {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key_for_scope(&expr_id.form_id.scope),
                node_id: String::new(),
                reason: "missing macro scope for return replacement".to_string(),
            });
        };
        if macro_doc.body.len() == 1
            && macro_doc.body[0].original_index.is_none()
            && macro_doc.body[0].expr == macro_doc.default_return_expression()
        {
            macro_doc.body.clear();
        }
        let index = macro_doc
            .body
            .iter()
            .position(|form| {
                form.original_index
                    .is_some_and(|original| original >= expr_id.form_id.index)
            })
            .unwrap_or(macro_doc.body.len());
        macro_doc.body.insert(
            index,
            MacroBodyForm {
                original_index: Some(expr_id.form_id.index),
                expr: replacement,
            },
        );
        Ok(())
    }

    fn replace_macro_visual_return(
        &mut self,
        scope: &SourceScopeId,
        replacement: Expression,
    ) -> Result<(), WriteBackError> {
        let SourceScopeId::Macro { name } = scope else {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key_for_scope(scope),
                node_id: String::new(),
                reason: "visual macro return replacement requires a macro scope".to_string(),
            });
        };
        let Some(macro_doc) = self.macros.get_mut(name) else {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key_for_scope(scope),
                node_id: String::new(),
                reason: "missing macro scope for visual return replacement".to_string(),
            });
        };
        if let Some(index) = macro_doc
            .body
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, form)| (!macro_body_side_effect_form(&form.expr)).then_some(index))
        {
            macro_doc.body.remove(index);
        }
        macro_doc.body.push(MacroBodyForm {
            original_index: None,
            expr: replacement,
        });
        Ok(())
    }

    fn remove_form(&mut self, form_id: &SourceFormId) -> Result<(), WriteBackError> {
        match &form_id.scope {
            SourceScopeId::Root => {
                let Some(position) = self
                    .forms
                    .iter()
                    .position(|form| form.original_index == Some(form_id.index))
                else {
                    return Err(WriteBackError::InvalidEdit {
                        view_key: view_key_for_scope(&form_id.scope),
                        node_id: String::new(),
                        reason: format!("missing root source form {}", form_id.index),
                    });
                };
                self.forms.remove(position);
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
                let Some(position) = macro_doc
                    .body
                    .iter()
                    .position(|form| form.original_index == Some(form_id.index))
                else {
                    return Err(WriteBackError::InvalidEdit {
                        view_key: view_key_for_scope(&form_id.scope),
                        node_id: String::new(),
                        reason: format!("missing macro source form {}", form_id.index),
                    });
                };
                macro_doc.body.remove(position);
                macro_doc.ensure_valid_body();
                Ok(())
            }
        }
    }

    fn replace_macro_param(
        &mut self,
        scope: &SourceScopeId,
        index: usize,
        name: String,
    ) -> Result<(), WriteBackError> {
        let SourceScopeId::Macro { name: macro_name } = scope else {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key_for_scope(scope),
                node_id: String::new(),
                reason: "macro parameter rename requires a macro scope".to_string(),
            });
        };
        let Some(macro_doc) = self.macros.get_mut(macro_name) else {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key_for_scope(scope),
                node_id: String::new(),
                reason: "missing macro scope for parameter rename".to_string(),
            });
        };
        if index >= macro_doc.params.len() {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key_for_scope(scope),
                node_id: String::new(),
                reason: format!("missing macro parameter {}", index),
            });
        }
        macro_doc.params[index] = Expression::Symbol(name);
        Ok(())
    }

    fn ensure_macro_param(
        &mut self,
        scope: &SourceScopeId,
        index: usize,
        preferred_name: Option<&str>,
        view_key: &str,
        node_id: &str,
    ) -> Result<String, WriteBackError> {
        let SourceScopeId::Macro { name: macro_name } = scope else {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key.to_string(),
                node_id: node_id.to_string(),
                reason: "created macro parameter requires a macro scope".to_string(),
            });
        };
        let Some(macro_doc) = self.macros.get_mut(macro_name) else {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key.to_string(),
                node_id: node_id.to_string(),
                reason: "missing macro scope for created parameter".to_string(),
            });
        };
        if let Some(existing) = macro_doc.param_name(index) {
            return Ok(existing.to_string());
        }

        let mut used = macro_doc.reserved_names();
        while macro_doc.params.len() <= index {
            let slot = macro_doc.params.len();
            let preferred = if slot == index {
                preferred_name
                    .map(str::to_string)
                    .unwrap_or_else(|| default_macro_param_name(slot))
            } else {
                default_macro_param_name(slot)
            };
            if !is_symbol_name(&preferred) {
                return Err(WriteBackError::InvalidEdit {
                    view_key: view_key.to_string(),
                    node_id: node_id.to_string(),
                    reason: format!("invalid macro parameter name `{preferred}`"),
                });
            }
            let name = if slot == index && preferred_name.is_some() {
                if used.contains(&preferred) {
                    return Err(WriteBackError::BindingRenameCollision {
                        view_key: view_key.to_string(),
                        node_id: node_id.to_string(),
                        name: preferred,
                    });
                }
                preferred
            } else {
                unique_symbol_name(&used, &preferred)
            };
            used.insert(name.clone());
            macro_doc.params.push(Expression::Symbol(name));
        }
        macro_doc
            .param_name(index)
            .map(str::to_string)
            .ok_or_else(|| WriteBackError::InvalidEdit {
                view_key: view_key.to_string(),
                node_id: node_id.to_string(),
                reason: format!("missing macro parameter {}", index),
            })
    }

    fn macro_param_name(
        &self,
        scope: &SourceScopeId,
        index: usize,
        view_key: &str,
        node_id: &str,
    ) -> Result<String, WriteBackError> {
        let SourceScopeId::Macro { name } = scope else {
            return Err(WriteBackError::InvalidEdit {
                view_key: view_key.to_string(),
                node_id: node_id.to_string(),
                reason: "macro parameter reference requires a macro scope".to_string(),
            });
        };
        self.macros
            .get(name)
            .and_then(|macro_doc| macro_doc.param_name(index))
            .map(str::to_string)
            .ok_or_else(|| WriteBackError::InvalidEdit {
                view_key: view_key.to_string(),
                node_id: node_id.to_string(),
                reason: format!("missing macro parameter {}", index),
            })
    }

    fn form_expr(&self, scope: &SourceScopeId, index: usize) -> Option<&Expression> {
        Self::form_expr_in(&self.forms, &self.macros, scope, index)
    }

    fn form_expr_in<'a>(
        forms: &'a [DocumentForm],
        macros: &'a HashMap<String, MacroDocument>,
        scope: &SourceScopeId,
        index: usize,
    ) -> Option<&'a Expression> {
        match scope {
            SourceScopeId::Root => match &forms
                .iter()
                .find(|form| form.original_index == Some(index))?
                .form
            {
                SourceForm::Expr(expr) => Some(expr),
                SourceForm::Macro(_) => None,
            },
            SourceScopeId::Macro { name } => macros
                .get(name)?
                .body
                .iter()
                .find(|form| form.original_index == Some(index))
                .map(|form| &form.expr),
        }
    }

    fn form_is_host_modulator_input(&self, form_id: &SourceFormId) -> bool {
        self.form_expr(&form_id.scope, form_id.index)
            .and_then(host_modulator_def_name)
            .is_some()
    }

    fn param_is_modulatable(&self, scope: &SourceScopeId, name: &str) -> bool {
        if self
            .virtual_modulatable_params
            .contains(&(scope.clone(), name.to_string()))
        {
            return true;
        }
        self.scope_forms(scope).iter().any(|expr| {
            let Expression::List(items) = expr else {
                return false;
            };
            symbol_at(items, 0) == Some("param")
                && symbol_at(items, 1) == Some(name)
                && expression_has_true_attribute(items, "@mod")
        })
    }

    fn register_virtual_modulatable_param(&mut self, scope: SourceScopeId, name: String) {
        self.virtual_modulatable_params.insert((scope, name));
    }

    fn form_expr_mut(&mut self, scope: &SourceScopeId, index: usize) -> Option<&mut Expression> {
        match scope {
            SourceScopeId::Root => match &mut self
                .forms
                .iter_mut()
                .find(|form| form.original_index == Some(index))?
                .form
            {
                SourceForm::Expr(expr) => Some(expr),
                SourceForm::Macro(_) => None,
            },
            SourceScopeId::Macro { name } => self
                .macros
                .get_mut(name)?
                .body
                .iter_mut()
                .find(|form| form.original_index == Some(index))
                .map(|form| &mut form.expr),
        }
    }

    fn form_position(&self, form_id: &SourceFormId) -> Option<usize> {
        match &form_id.scope {
            SourceScopeId::Root => self
                .forms
                .iter()
                .position(|form| form.original_index == Some(form_id.index)),
            SourceScopeId::Macro { name } => self
                .macros
                .get(name)?
                .body
                .iter()
                .position(|form| form.original_index == Some(form_id.index)),
        }
    }

    fn insertion_position_for_original_index(&self, scope: &SourceScopeId, index: usize) -> usize {
        match scope {
            SourceScopeId::Root => self
                .forms
                .iter()
                .position(|form| {
                    form.original_index
                        .is_some_and(|original| original >= index)
                })
                .unwrap_or(self.forms.len()),
            SourceScopeId::Macro { name } => self
                .macros
                .get(name)
                .and_then(|macro_doc| {
                    macro_doc.body.iter().position(|form| {
                        form.original_index
                            .is_some_and(|original| original >= index)
                    })
                })
                .unwrap_or_else(|| {
                    self.macros
                        .get(name)
                        .map(|macro_doc| macro_doc.body.len())
                        .unwrap_or(0)
                }),
        }
    }

    fn move_forms_after_dependency_if_needed(
        &mut self,
        moved: &[SourceFormId],
        dependency: &SourceFormId,
    ) -> Result<(), WriteBackError> {
        let Some(dependency_position) = self.form_position(dependency) else {
            return Ok(());
        };
        let moved_indexes = moved
            .iter()
            .filter(|form| form.scope == dependency.scope)
            .filter(|form| form.index != dependency.index)
            .filter_map(|form| {
                let position = self.form_position(form)?;
                (position < dependency_position).then_some(form.index)
            })
            .collect::<HashSet<_>>();
        if moved_indexes.is_empty() {
            return Ok(());
        }

        match &dependency.scope {
            SourceScopeId::Root => {
                move_forms_after_dependency_in_scope(
                    &mut self.forms,
                    &moved_indexes,
                    dependency.index,
                    |form| form.original_index,
                );
            }
            SourceScopeId::Macro { name } => {
                let Some(macro_doc) = self.macros.get_mut(name) else {
                    return Ok(());
                };
                move_forms_after_dependency_in_scope(
                    &mut macro_doc.body,
                    &moved_indexes,
                    dependency.index,
                    |form| form.original_index,
                );
            }
        }

        Ok(())
    }

    fn move_forms_after_current_position_if_needed(
        &mut self,
        moved: &[SourceFormId],
        scope: &SourceScopeId,
        position: usize,
    ) -> Result<usize, WriteBackError> {
        let moved_indexes = moved
            .iter()
            .filter(|form| &form.scope == scope)
            .filter_map(|form| {
                let current_position = self.form_position(form)?;
                (current_position < position).then_some(form.index)
            })
            .collect::<HashSet<_>>();
        if moved_indexes.is_empty() {
            return Ok(position);
        }

        let insertion_position = match scope {
            SourceScopeId::Root => move_forms_after_current_position_in_scope(
                &mut self.forms,
                &moved_indexes,
                position,
                |form| form.original_index,
            ),
            SourceScopeId::Macro { name } => {
                let Some(macro_doc) = self.macros.get_mut(name) else {
                    return Ok(position);
                };
                move_forms_after_current_position_in_scope(
                    &mut macro_doc.body,
                    &moved_indexes,
                    position,
                    |form| form.original_index,
                )
            }
        };

        Ok(insertion_position)
    }

    fn emit(&self) -> String {
        self.forms_in_emit_order()
            .into_iter()
            .filter_map(|form| self.emit_form(form))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn add_imports_for_used_library_macros(
        &mut self,
        root_patch: &Patch,
        library: &DefmacroLibrary,
    ) {
        let local_macros = self.macros.keys().cloned().collect::<HashSet<_>>();
        let existing_imports = self.imported_macro_names();
        let mut needed = used_macro_instance_names(root_patch)
            .into_iter()
            .chain(self.library_macro_call_names(library))
            .filter(|name| library.package(name).is_some())
            .filter(|name| !local_macros.contains(name))
            .filter(|name| !existing_imports.contains(name))
            .collect::<Vec<_>>();
        needed.sort();
        for name in needed.into_iter().rev() {
            self.forms.insert(
                self.import_insert_position(),
                DocumentForm {
                    original_index: None,
                    form: SourceForm::Expr(Expression::List(vec![
                        Expression::Symbol("use-defmacro".to_string()),
                        Expression::Symbol(name),
                    ])),
                },
            );
        }
    }

    fn imported_macro_names(&self) -> HashSet<String> {
        self.forms
            .iter()
            .filter_map(|form| match &form.form {
                SourceForm::Expr(expr) => parse_use_defmacro(expr).ok().flatten(),
                SourceForm::Macro(_) => None,
            })
            .collect()
    }

    fn import_insert_position(&self) -> usize {
        self.forms
            .iter()
            .position(|form| match &form.form {
                SourceForm::Expr(expr) => parse_use_defmacro(expr).ok().flatten().is_none(),
                SourceForm::Macro(_) => true,
            })
            .unwrap_or(self.forms.len())
    }

    fn library_macro_call_names(&self, library: &DefmacroLibrary) -> HashSet<String> {
        let mut names = HashSet::new();
        for form in &self.forms {
            match &form.form {
                SourceForm::Expr(expr) => collect_library_macro_calls(expr, library, &mut names),
                SourceForm::Macro(name) => {
                    if let Some(macro_doc) = self.macros.get(name) {
                        for form in &macro_doc.body {
                            collect_library_macro_calls(&form.expr, library, &mut names);
                        }
                    }
                }
            }
        }
        names
    }

    fn emit_form(&self, form: &DocumentForm) -> Option<String> {
        match &form.form {
            SourceForm::Expr(expr) => Some(format_writeback_expression(expr)),
            SourceForm::Macro(name) => self.macros.get(name).map(MacroDocument::emit),
        }
    }

    fn forms_in_emit_order(&self) -> Vec<&DocumentForm> {
        let macro_names = self
            .forms
            .iter()
            .filter_map(|form| match &form.form {
                SourceForm::Macro(name) if self.macros.contains_key(name) => Some(name.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        if macro_names.is_empty() {
            return self.forms.iter().collect();
        }

        let macro_name_set = macro_names.iter().cloned().collect::<HashSet<_>>();
        let macro_position = macro_names
            .iter()
            .enumerate()
            .map(|(index, name)| (name.as_str(), index))
            .collect::<HashMap<_, _>>();
        let macro_form_by_name = self
            .forms
            .iter()
            .filter_map(|form| match &form.form {
                SourceForm::Macro(name) => Some((name.as_str(), form)),
                _ => None,
            })
            .collect::<HashMap<_, _>>();

        let mut ordered_macro_names = Vec::new();
        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        for name in &macro_names {
            self.visit_macro_for_emit_order(
                name,
                &macro_name_set,
                &macro_position,
                &mut visiting,
                &mut visited,
                &mut ordered_macro_names,
            );
        }

        let mut ordered = ordered_macro_names
            .into_iter()
            .filter_map(|name| macro_form_by_name.get(name.as_str()).copied())
            .collect::<Vec<_>>();
        ordered.extend(
            self.forms
                .iter()
                .filter(|form| !matches!(form.form, SourceForm::Macro(_))),
        );
        ordered
    }

    fn visit_macro_for_emit_order<'a>(
        &'a self,
        name: &'a str,
        macro_names: &HashSet<String>,
        macro_position: &HashMap<&'a str, usize>,
        visiting: &mut HashSet<String>,
        visited: &mut HashSet<String>,
        ordered: &mut Vec<String>,
    ) {
        if visited.contains(name) {
            return;
        }
        if !visiting.insert(name.to_string()) {
            return;
        }

        let mut references = HashSet::new();
        if let Some(macro_doc) = self.macros.get(name) {
            for form in &macro_doc.body {
                collect_macro_call_references(&form.expr, macro_names, &mut references);
            }
        }
        let mut references = references.into_iter().collect::<Vec<_>>();
        references.sort_by_key(|reference| {
            macro_position
                .get(reference.as_str())
                .copied()
                .unwrap_or(usize::MAX)
        });
        for reference in references {
            if reference != name {
                self.visit_macro_for_emit_order(
                    &reference,
                    macro_names,
                    macro_position,
                    visiting,
                    visited,
                    ordered,
                );
            }
        }

        visiting.remove(name);
        visited.insert(name.to_string());
        ordered.push(name.to_string());
    }

    fn prepend_form(
        &mut self,
        scope: &SourceScopeId,
        expr: Expression,
    ) -> Result<(), WriteBackError> {
        match scope {
            SourceScopeId::Root => {
                self.forms.insert(
                    0,
                    DocumentForm {
                        original_index: None,
                        form: SourceForm::Expr(expr),
                    },
                );
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
                macro_doc.body.insert(
                    0,
                    MacroBodyForm {
                        original_index: None,
                        expr,
                    },
                );
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
                let index = self
                    .forms
                    .iter()
                    .position(|form| {
                        form.original_index
                            .is_some_and(|original| original >= index)
                    })
                    .unwrap_or(self.forms.len());
                self.forms.insert(
                    index,
                    DocumentForm {
                        original_index: None,
                        form: SourceForm::Expr(expr),
                    },
                );
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
                let index = macro_doc
                    .body
                    .iter()
                    .position(|form| {
                        form.original_index
                            .is_some_and(|original| original >= index)
                    })
                    .unwrap_or(macro_doc.body.len());
                macro_doc.body.insert(
                    index,
                    MacroBodyForm {
                        original_index: None,
                        expr,
                    },
                );
                Ok(())
            }
        }
    }

    fn insert_generated_form(
        &mut self,
        scope: &SourceScopeId,
        insertion: &GeneratedFormInsertion,
        expr: Expression,
    ) -> Result<(), WriteBackError> {
        match insertion {
            GeneratedFormInsertion::OriginalIndex(index) => self.insert_form(scope, *index, expr),
            GeneratedFormInsertion::CurrentPosition(position) => {
                self.insert_form_at_current_position(scope, *position, expr)
            }
            GeneratedFormInsertion::AfterCurrentForms(forms) => {
                self.insert_form_after_current_forms(scope, forms, expr)
            }
        }
    }

    fn insert_form_at_current_position(
        &mut self,
        scope: &SourceScopeId,
        position: usize,
        expr: Expression,
    ) -> Result<(), WriteBackError> {
        match scope {
            SourceScopeId::Root => {
                let mut insert_at = position.min(self.forms.len());
                while self
                    .forms
                    .get(insert_at)
                    .is_some_and(|form| form.original_index.is_none())
                {
                    insert_at += 1;
                }
                self.forms.insert(
                    insert_at,
                    DocumentForm {
                        original_index: None,
                        form: SourceForm::Expr(expr),
                    },
                );
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
                let mut insert_at = position.min(macro_doc.body.len());
                while macro_doc
                    .body
                    .get(insert_at)
                    .is_some_and(|form| form.original_index.is_none())
                {
                    insert_at += 1;
                }
                macro_doc.body.insert(
                    insert_at,
                    MacroBodyForm {
                        original_index: None,
                        expr,
                    },
                );
                Ok(())
            }
        }
    }

    fn insert_form_after_current_forms(
        &mut self,
        scope: &SourceScopeId,
        dependencies: &[SourceFormId],
        expr: Expression,
    ) -> Result<(), WriteBackError> {
        match scope {
            SourceScopeId::Root => {
                let mut insert_at = dependencies
                    .iter()
                    .filter(|dependency| dependency.scope == *scope)
                    .filter_map(|dependency| self.form_position(dependency))
                    .max()
                    .map(|position| position + 1)
                    .unwrap_or(self.forms.len());
                while self
                    .forms
                    .get(insert_at)
                    .is_some_and(|form| form.original_index.is_none())
                {
                    insert_at += 1;
                }
                self.forms.insert(
                    insert_at,
                    DocumentForm {
                        original_index: None,
                        form: SourceForm::Expr(expr),
                    },
                );
                Ok(())
            }
            SourceScopeId::Macro { name } => {
                let positions = dependencies
                    .iter()
                    .filter(|dependency| dependency.scope == *scope)
                    .filter_map(|dependency| self.form_position(dependency))
                    .collect::<Vec<_>>();
                let Some(macro_doc) = self.macros.get_mut(name) else {
                    return Err(WriteBackError::InvalidEdit {
                        view_key: view_key_for_scope(scope),
                        node_id: String::new(),
                        reason: "missing macro scope for generated binding".to_string(),
                    });
                };
                let insert_at = positions
                    .into_iter()
                    .max()
                    .map(|position| position + 1)
                    .unwrap_or(macro_doc.body.len());
                let mut insert_at = insert_at;
                while macro_doc
                    .body
                    .get(insert_at)
                    .is_some_and(|form| form.original_index.is_none())
                {
                    insert_at += 1;
                }
                macro_doc.body.insert(
                    insert_at,
                    MacroBodyForm {
                        original_index: None,
                        expr,
                    },
                );
                Ok(())
            }
        }
    }

    fn prepend_macro(&mut self, expr: Expression) -> Result<(), WriteBackError> {
        let Some(macro_doc) = MacroDocument::from_expr(&expr) else {
            return Err(WriteBackError::InvalidEdit {
                view_key: "root".to_string(),
                node_id: String::new(),
                reason: "created macro source did not parse as defmacro".to_string(),
            });
        };
        if self.macros.contains_key(&macro_doc.name) {
            return Ok(());
        }
        self.forms.insert(
            0,
            DocumentForm {
                original_index: None,
                form: SourceForm::Macro(macro_doc.name.clone()),
            },
        );
        self.macros.insert(macro_doc.name.clone(), macro_doc);
        Ok(())
    }

    fn replace_macro(&mut self, name: &str, expr: Expression) -> Result<(), WriteBackError> {
        let Some(macro_doc) = MacroDocument::from_expr(&expr) else {
            return Err(WriteBackError::InvalidEdit {
                view_key: "root".to_string(),
                node_id: String::new(),
                reason: "replacement source did not parse as defmacro".to_string(),
            });
        };
        if macro_doc.name != name {
            return Err(WriteBackError::InvalidEdit {
                view_key: "root".to_string(),
                node_id: String::new(),
                reason: format!(
                    "replacement macro name `{}` does not match `{name}`",
                    macro_doc.name
                ),
            });
        }
        if !self.macros.contains_key(name) {
            return Err(WriteBackError::InvalidEdit {
                view_key: "root".to_string(),
                node_id: String::new(),
                reason: format!("missing macro `{name}`"),
            });
        }
        self.macros.insert(name.to_string(), macro_doc);
        Ok(())
    }

    fn remove_unreferenced_candidate_macros(&mut self, candidates: &HashSet<String>) {
        if candidates.is_empty() {
            return;
        }
        let macro_names = self.macros.keys().cloned().collect::<HashSet<_>>();
        let candidates = candidates
            .intersection(&macro_names)
            .cloned()
            .collect::<HashSet<_>>();
        if candidates.is_empty() {
            return;
        }

        let live = self.live_macro_references_excluding_candidates(&macro_names, &candidates);
        for name in candidates.difference(&live) {
            self.macros.remove(name);
            self.forms.retain(
                |form| !matches!(&form.form, SourceForm::Macro(macro_name) if macro_name == name),
            );
        }
    }

    fn live_macro_references_excluding_candidates(
        &self,
        macro_names: &HashSet<String>,
        candidates: &HashSet<String>,
    ) -> HashSet<String> {
        let mut live = HashSet::new();
        for form in &self.forms {
            if let SourceForm::Expr(expr) = &form.form {
                collect_macro_call_references(expr, macro_names, &mut live);
            }
        }
        for (name, macro_doc) in &self.macros {
            if candidates.contains(name) {
                continue;
            }
            for form in &macro_doc.body {
                collect_macro_call_references(&form.expr, macro_names, &mut live);
            }
        }

        let mut stack = live.iter().cloned().collect::<Vec<_>>();
        while let Some(name) = stack.pop() {
            let Some(macro_doc) = self.macros.get(&name) else {
                continue;
            };
            let mut refs = HashSet::new();
            for form in &macro_doc.body {
                collect_macro_call_references(&form.expr, macro_names, &mut refs);
            }
            for referenced in refs {
                if live.insert(referenced.clone()) {
                    stack.push(referenced);
                }
            }
        }
        live
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
                self.forms.push(DocumentForm {
                    original_index: None,
                    form: SourceForm::Expr(expr),
                });
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
                macro_doc.body.insert(
                    index,
                    MacroBodyForm {
                        original_index: None,
                        expr,
                    },
                );
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
                    match &form.form {
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
                    for form in &macro_doc.body {
                        collect_scope_binding_names(&form.expr, &mut names);
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
                .filter_map(|form| match &form.form {
                    SourceForm::Expr(expr) => Some(expr),
                    SourceForm::Macro(_) => None,
                })
                .collect(),
            SourceScopeId::Macro { name } => self
                .macros
                .get(name)
                .map(|macro_doc| macro_doc.body.iter().map(|form| &form.expr).collect())
                .unwrap_or_default(),
        }
    }

    fn source_binding_dependency_indices(&self, scope: &SourceScopeId) -> HashMap<String, usize> {
        let mut indices = HashMap::new();
        match scope {
            SourceScopeId::Root => {
                for form in &self.forms {
                    let Some(original_index) = form.original_index else {
                        continue;
                    };
                    let SourceForm::Expr(expr) = &form.form else {
                        continue;
                    };
                    collect_source_binding_dependency_indices(
                        expr,
                        original_index + 1,
                        &mut indices,
                    );
                }
            }
            SourceScopeId::Macro { name } => {
                if let Some(macro_doc) = self.macros.get(name) {
                    for form in &macro_doc.body {
                        let Some(original_index) = form.original_index else {
                            continue;
                        };
                        collect_source_binding_dependency_indices(
                            &form.expr,
                            original_index + 1,
                            &mut indices,
                        );
                    }
                }
            }
        }
        indices
    }

    fn latest_host_modulator_input_index(&self, scope: &SourceScopeId) -> Option<usize> {
        match scope {
            SourceScopeId::Root => self
                .forms
                .iter()
                .filter_map(|form| {
                    host_modulator_def_name(match &form.form {
                        SourceForm::Expr(expr) => expr,
                        SourceForm::Macro(_) => return None,
                    })?;
                    form.original_index
                })
                .max(),
            SourceScopeId::Macro { name } => self.macros.get(name).and_then(|macro_doc| {
                macro_doc
                    .body
                    .iter()
                    .filter_map(host_modulator_body_form_index)
                    .max()
            }),
        }
    }

    fn latest_host_modulator_input_position(&self, scope: &SourceScopeId) -> Option<usize> {
        match scope {
            SourceScopeId::Root => self
                .forms
                .iter()
                .enumerate()
                .filter_map(|(position, form)| {
                    host_modulator_def_name(match &form.form {
                        SourceForm::Expr(expr) => expr,
                        SourceForm::Macro(_) => return None,
                    })?;
                    Some(position)
                })
                .max(),
            SourceScopeId::Macro { name } => self.macros.get(name).and_then(|macro_doc| {
                macro_doc
                    .body
                    .iter()
                    .enumerate()
                    .filter_map(|(position, form)| {
                        host_modulator_def_name(&form.expr)?;
                        Some(position)
                    })
                    .max()
            }),
        }
    }

    fn ensure_root_host_modulator_inputs(
        &mut self,
        intent: PatcherIntent,
    ) -> Result<(), WriteBackError> {
        let mut present_slots = HashSet::new();
        let mut used_input_channels = HashSet::new();
        let mut root_names = HashSet::new();

        for form in &self.forms {
            let SourceForm::Expr(expr) = &form.form else {
                continue;
            };
            collect_scope_binding_names(expr, &mut root_names);
            if let Some(channel) = input_form_channel(expr) {
                used_input_channels.insert(channel);
            }
            if let Some(slot) = host_modulator_def_slot_for_intent(expr, intent) {
                present_slots.insert(slot);
            }
        }

        let insertions = (1..=4)
            .filter(|slot| !present_slots.contains(slot))
            .map(|slot| {
                let name = format!("mod{slot}");
                if root_names.contains(&name) {
                    return Err(WriteBackError::InvalidEdit {
                        view_key: "root".to_string(),
                        node_id: name.clone(),
                        reason: "host modulation requires the reserved modulator input names mod1..mod4".to_string(),
                    });
                }
                let channel = host_modulator_input_channel(intent, slot);
                if used_input_channels.contains(&channel) {
                    return Err(WriteBackError::InvalidEdit {
                        view_key: "root".to_string(),
                        node_id: name.clone(),
                        reason: format!(
                            "host modulation requires input channel {channel} for {name}, but that channel is already used"
                        ),
                    });
                }
                used_input_channels.insert(channel);
                root_names.insert(name.clone());
                Ok(host_modulator_def_expr(slot, channel))
            })
            .collect::<Result<Vec<_>, _>>()?;

        if insertions.is_empty() {
            return Ok(());
        }

        let mut insert_at = self
            .forms
            .iter()
            .rposition(|form| {
                matches!(&form.form, SourceForm::Expr(expr) if input_form_channel(expr).is_some())
            })
            .map(|index| index + 1)
            .unwrap_or(0);
        for expr in insertions {
            self.forms.insert(
                insert_at,
                DocumentForm {
                    original_index: None,
                    form: SourceForm::Expr(expr),
                },
            );
            insert_at += 1;
        }
        Ok(())
    }

    fn is_known_operator(&self, operator: &str) -> bool {
        matches!(
            operator,
            "def"
                | "defmacro"
                | "param"
                | "in"
                | "out"
                | "use-defmacro"
                | "make-history"
                | "read-history"
                | "write-history"
        ) || dgenlisp_operator_names().contains(operator)
            || self.macros.contains_key(operator)
            || self.external_macros.contains(operator)
    }
}

fn move_forms_after_dependency_in_scope<T>(
    forms: &mut Vec<T>,
    moved_original_indexes: &HashSet<usize>,
    dependency_original_index: usize,
    original_index: impl Fn(&T) -> Option<usize>,
) {
    // SourceFormId::index is the original parse index, not the current vector
    // position. Resolve both ids against the current scope vector before moving.
    if !forms
        .iter()
        .any(|form| original_index(form) == Some(dependency_original_index))
    {
        return;
    }

    let mut kept = Vec::with_capacity(forms.len());
    let mut moved = Vec::new();
    for form in forms.drain(..) {
        if original_index(&form).is_some_and(|index| {
            moved_original_indexes.contains(&index) && index != dependency_original_index
        }) {
            moved.push(form);
        } else {
            kept.push(form);
        }
    }

    let Some(insert_at) = kept
        .iter()
        .position(|form| original_index(form) == Some(dependency_original_index))
        .map(|position| position + 1)
    else {
        *forms = kept;
        return;
    };
    kept.splice(insert_at..insert_at, moved);
    *forms = kept;
}

fn move_forms_after_current_position_in_scope<T>(
    forms: &mut Vec<T>,
    moved_original_indexes: &HashSet<usize>,
    position: usize,
    original_index: impl Fn(&T) -> Option<usize>,
) -> usize {
    let mut kept = Vec::with_capacity(forms.len());
    let mut moved = Vec::new();
    let mut removed_before_position = 0usize;

    for (current_position, form) in forms.drain(..).enumerate() {
        if current_position < position
            && original_index(&form).is_some_and(|index| moved_original_indexes.contains(&index))
        {
            removed_before_position += 1;
            moved.push(form);
        } else {
            kept.push(form);
        }
    }

    if moved.is_empty() {
        let insertion_position = position.min(kept.len());
        *forms = kept;
        return insertion_position;
    }

    let insert_at = position
        .saturating_sub(removed_before_position)
        .min(kept.len());
    kept.splice(insert_at..insert_at, moved);
    *forms = kept;
    insert_at
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
            body: items
                .iter()
                .skip(3)
                .cloned()
                .enumerate()
                .map(|(index, expr)| MacroBodyForm {
                    original_index: Some(index),
                    expr,
                })
                .collect(),
        })
    }

    fn emit(&self) -> String {
        let body = if self.body.is_empty() {
            vec![self.default_return_expression()]
        } else {
            self.body.iter().map(|form| form.expr.clone()).collect()
        };
        if body.len() <= 1 {
            let mut items = vec![
                Expression::Symbol("defmacro".to_string()),
                Expression::Symbol(self.name.clone()),
                Expression::List(self.params.clone()),
            ];
            items.extend(body);
            return format_writeback_expression(&Expression::List(items));
        }

        let params = format_writeback_expression(&Expression::List(self.params.clone()));
        let body = body
            .iter()
            .map(|expr| format!("  {}", format_writeback_expression(expr)))
            .collect::<Vec<_>>()
            .join("\n");
        format!("(defmacro {} {params}\n{body})", self.name)
    }

    fn ensure_valid_body(&mut self) {
        if self.body.is_empty() {
            self.body.push(MacroBodyForm {
                original_index: None,
                expr: self.default_return_expression(),
            });
        }
    }

    fn default_return_expression(&self) -> Expression {
        self.params
            .iter()
            .find_map(|param| match param {
                Expression::Symbol(name) => Some(Expression::Symbol(name.clone())),
                _ => None,
            })
            .unwrap_or(Expression::Number(0.0))
    }

    fn param_name(&self, index: usize) -> Option<&str> {
        match self.params.get(index) {
            Some(Expression::Symbol(name)) => Some(name),
            _ => None,
        }
    }

    fn reserved_names(&self) -> HashSet<String> {
        let mut names = HashSet::new();
        for param in &self.params {
            if let Expression::Symbol(name) = param {
                names.insert(name.clone());
            }
        }
        for form in &self.body {
            collect_scope_binding_names(&form.expr, &mut names);
        }
        names
    }
}

fn default_macro_param_name(index: usize) -> String {
    if index == 0 {
        "input".to_string()
    } else {
        format!("input{}", index + 1)
    }
}

fn is_symbol_name(name: &str) -> bool {
    matches!(parse_single_expression(name), Ok(Expression::Symbol(symbol)) if symbol == name)
}

fn unique_symbol_name(used: &HashSet<String>, preferred: &str) -> String {
    if !used.contains(preferred) {
        return preferred.to_string();
    }
    let mut suffix = next_suffix_for_stem(used, preferred);
    loop {
        let candidate = format!("{preferred}{suffix}");
        if !used.contains(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

fn format_writeback_expression(expr: &Expression) -> String {
    match expr {
        Expression::List(items) => format_writeback_list(items),
        _ => format_expression(expr),
    }
}

fn format_writeback_list(items: &[Expression]) -> String {
    let inner = items
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            if idx > 0
                && matches!(items.get(idx - 1), Some(Expression::Symbol(attr)) if attr == "@modulator")
                && let Expression::Number(value) = item
                && value.fract() == 0.0
                && *value > 0.0
            {
                return format!("{value:.0}");
            }
            if expression_slot_requires_integer_token(items, idx)
                && let Expression::Number(value) = item
                && value.fract() == 0.0
                && *value > 0.0
            {
                return format!("{value:.0}");
            }
            format_writeback_expression(item)
        })
        .collect::<Vec<_>>();
    format!("({})", inner.join(" "))
}

fn expression_slot_requires_integer_token(items: &[Expression], idx: usize) -> bool {
    match symbol_at(items, 0) {
        Some("in") => idx == 1,
        Some("out") => positional_item_index(items, 1) == Some(idx),
        _ => false,
    }
}

fn positional_item_index(items: &[Expression], semantic_index: usize) -> Option<usize> {
    let mut current = 0;
    let mut idx = 1;
    while idx < items.len() {
        if matches!(&items[idx], Expression::Symbol(symbol) if symbol.starts_with('@')) {
            idx += 2;
            continue;
        }
        if current == semantic_index {
            return Some(idx);
        }
        current += 1;
        idx += 1;
    }
    None
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

fn expr_at_path_mut<'a>(
    mut expr: &'a mut Expression,
    path: &[ExprPathSegment],
) -> Option<&'a mut Expression> {
    for segment in path {
        match (expr, segment) {
            (Expression::List(items), ExprPathSegment::ListItem(index)) => {
                expr = items.get_mut(*index)?;
            }
            _ => return None,
        }
    }
    Some(expr)
}

fn replace_or_insert_positional_arg(
    items: &mut Vec<Expression>,
    semantic_index: usize,
    value: Expression,
) {
    if let Some(item_index) = positional_item_index(items, semantic_index) {
        items[item_index] = value;
        return;
    }

    let existing_count = positional_arg_count(items);
    let insert_index = positional_insert_index(items);
    let mut inserted = Vec::new();
    for _ in existing_count..semantic_index {
        inserted.push(Expression::Symbol(MISSING_INPUT_SENTINEL.to_string()));
    }
    inserted.push(value);
    items.splice(insert_index..insert_index, inserted);
}

fn positional_arg_count(items: &[Expression]) -> usize {
    let mut count = 0;
    let mut idx = 1;
    while idx < items.len() {
        if matches!(&items[idx], Expression::Symbol(symbol) if symbol.starts_with('@')) {
            idx += 2;
            continue;
        }
        count += 1;
        idx += 1;
    }
    count
}

fn positional_insert_index(items: &[Expression]) -> usize {
    let mut idx = 1;
    while idx < items.len() {
        if matches!(&items[idx], Expression::Symbol(symbol) if symbol.starts_with('@')) {
            return idx;
        }
        idx += 1;
    }
    items.len()
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

fn collect_source_binding_dependency_indices(
    expr: &Expression,
    dependency_index: usize,
    indices: &mut HashMap<String, usize>,
) {
    let mut names = HashSet::new();
    collect_scope_binding_names(expr, &mut names);
    for name in names {
        indices
            .entry(name)
            .and_modify(|existing| *existing = (*existing).max(dependency_index))
            .or_insert(dependency_index);
    }
}

fn collect_macro_call_references(
    expr: &Expression,
    macro_names: &HashSet<String>,
    references: &mut HashSet<String>,
) {
    match expr {
        Expression::List(items) | Expression::QuoteList(items) => {
            if let Some(head) = symbol_at(items, 0)
                && macro_names.contains(head)
            {
                references.insert(head.to_string());
            }
            for item in items {
                collect_macro_call_references(item, macro_names, references);
            }
        }
        Expression::Quasiquote(inner) | Expression::Unquote(inner) => {
            collect_macro_call_references(inner, macro_names, references);
        }
        _ => {}
    }
}

fn host_modulator_body_form_index(form: &MacroBodyForm) -> Option<usize> {
    host_modulator_def_name(&form.expr)?;
    form.original_index
}

fn host_modulator_def_name(expr: &Expression) -> Option<&str> {
    let (slot, name, _) = host_modulator_def_signature(expr)?;
    (expected_host_modulator_slot(name) == Some(slot)).then_some(name)
}

fn host_modulator_def_slot_for_intent(expr: &Expression, _intent: PatcherIntent) -> Option<usize> {
    let (slot, name, _) = host_modulator_def_signature(expr)?;
    if expected_host_modulator_slot(name) != Some(slot) {
        return None;
    }
    Some(slot)
}

fn host_modulator_def_signature(expr: &Expression) -> Option<(usize, &str, usize)> {
    let Expression::List(items) = expr else {
        return None;
    };
    if symbol_at(items, 0) != Some("def") {
        return None;
    }
    let name = symbol_at(items, 1)?;
    let (channel, input_name, modulator) = items.get(2).and_then(host_modulator_input_signature)?;
    (input_name == name).then_some((modulator, name, channel))
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

fn host_modulator_input_channel(intent: PatcherIntent, slot: usize) -> usize {
    match intent {
        PatcherIntent::Instrument => slot + 5,
        PatcherIntent::Effect => slot + 2,
    }
}

fn host_modulator_def_expr(slot: usize, channel: usize) -> Expression {
    let name = format!("mod{slot}");
    Expression::List(vec![
        Expression::Symbol("def".to_string()),
        Expression::Symbol(name.clone()),
        Expression::List(vec![
            Expression::Symbol("in".to_string()),
            Expression::Number(channel as f64),
            Expression::Symbol("@name".to_string()),
            Expression::Symbol(name),
            Expression::Symbol("@modulator".to_string()),
            Expression::Number(slot as f64),
        ]),
    ])
}

fn input_form_channel(expr: &Expression) -> Option<usize> {
    let Expression::List(items) = expr else {
        return None;
    };
    if symbol_at(items, 0) != Some("def") {
        return None;
    }
    let Expression::List(value_items) = items.get(2)? else {
        return None;
    };
    if symbol_at(value_items, 0) != Some("in") {
        return None;
    }
    match value_items.get(1) {
        Some(Expression::Number(value)) if value.fract() == 0.0 && *value > 0.0 => {
            Some(*value as usize)
        }
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
    let name = items
        .windows(2)
        .find_map(|pair| match (&pair[0], &pair[1]) {
            (Expression::Symbol(key), Expression::Symbol(value)) if key == "@name" => {
                Some(value.as_str())
            }
            _ => None,
        })?;
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
