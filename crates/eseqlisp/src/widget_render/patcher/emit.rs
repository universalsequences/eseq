use std::collections::{HashMap, HashSet};
use std::env;

use super::model::{
    ArgValue, BindingTarget, ConnectionKind, NodeKind, Patch, PatchConnection, PatchNode,
    SourceArgValue, SourceOwner, SourceScopeId,
};

const MISSING_INPUT_SENTINEL: &str = "__patcher_missing_input__";
const DEBUG_LISP_ENV: &str = "ESEQ_PATCHER_DEBUG_LISP";

pub(super) fn emit_patch_debug_lisp(patch: &Patch) -> String {
    emit_patch_debug_lisp_body(patch, EmitContext::Root)
}

pub(super) fn emit_patch_debug_lisp_for_view(view_key: &str, patch: &Patch) -> String {
    let Some(macro_name) = view_key.strip_prefix("macro:") else {
        return emit_patch_debug_lisp(patch);
    };
    emit_macro_debug_lisp(macro_name, patch)
}

fn emit_macro_debug_lisp(macro_name: &str, patch: &Patch) -> String {
    let params = macro_params(patch, macro_name);
    let body = emit_patch_debug_lisp_body(patch, EmitContext::Macro);
    if body.starts_with(";; patcher debug emit:") {
        return format!("(defmacro {macro_name} ({})\n  {body})", params.join(" "));
    }
    let indented_body = body
        .lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "(defmacro {macro_name} ({})\n{indented_body})",
        params.join(" ")
    )
}

fn emit_patch_debug_lisp_body(patch: &Patch, context: EmitContext) -> String {
    let inbound = inbound_connections(patch);
    let nodes = patch
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let mut emitted = Vec::new();
    let mut emitted_node_ids = HashSet::new();
    let mut top_level = patch.nodes.iter().collect::<Vec<_>>();
    top_level.sort_by_key(|node| source_order(node).unwrap_or((usize::MAX, usize::MAX)));
    for node in top_level {
        if !should_emit_top_level(node, context) {
            continue;
        }
        if emitted_node_ids.contains(node.id.as_str()) {
            continue;
        }
        emitted_node_ids.insert(node.id.as_str());
        emitted.push(emit_top_level_node(
            node,
            &nodes,
            &inbound,
            &mut HashSet::new(),
        ));
    }
    if emitted.is_empty() {
        return ";; patcher debug emit: no source-backed top-level forms".to_string();
    }
    emitted.join("\n")
}

pub(super) fn debug_log_patch_lisp(view_key: &str, patch: &Patch) {
    if !debug_lisp_logging_enabled() {
        return;
    }
    eprintln!(
        "[patcher writeback debug:{view_key}]\n{}\n[/patcher writeback debug]",
        emit_patch_debug_lisp_for_view(view_key, patch)
    );
}

fn debug_lisp_logging_enabled() -> bool {
    env::var(DEBUG_LISP_ENV)
        .ok()
        .is_some_and(|value| env_flag_enabled(&value))
}

fn env_flag_enabled(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
}

#[cfg(test)]
mod tests {
    use super::env_flag_enabled;

    #[test]
    fn debug_lisp_env_flag_accepts_only_explicit_truthy_values() {
        for value in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(env_flag_enabled(value), "{value:?} should enable logging");
        }
        for value in ["", "0", "false", "no", "off", "anything"] {
            assert!(
                !env_flag_enabled(value),
                "{value:?} should not enable logging"
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmitContext {
    Root,
    Macro,
}

fn macro_params(patch: &Patch, macro_name: &str) -> Vec<String> {
    let macro_scope = SourceScopeId::Macro {
        name: macro_name.to_string(),
    };
    let mut params = patch
        .nodes
        .iter()
        .filter_map(
            |node| match node.source.as_ref().map(|source| &source.owner) {
                Some(SourceOwner::MacroParameter { binding, index })
                    if binding.scope == macro_scope =>
                {
                    Some((*index, binding.name.clone()))
                }
                _ => None,
            },
        )
        .collect::<Vec<_>>();
    params.sort_by_key(|(index, _)| *index);
    params.into_iter().map(|(_, name)| name).collect()
}

fn inbound_connections(patch: &Patch) -> HashMap<(String, usize), &PatchConnection> {
    let mut inbound = HashMap::new();
    for connection in &patch.connections {
        inbound.insert(
            (connection.to_node.clone(), connection.to_input),
            connection,
        );
    }
    inbound
}

fn source_order(node: &PatchNode) -> Option<(usize, usize)> {
    let source = node.source.as_ref()?;
    let form_id = match &source.owner {
        SourceOwner::BindingValue { form_id, .. }
        | SourceOwner::TopLevelForm { form_id }
        | SourceOwner::CodeIsland { form_id } => form_id,
        SourceOwner::Compound { parts } => parts.iter().find_map(|part| match part {
            SourceOwner::TopLevelForm { form_id } => Some(form_id),
            _ => None,
        })?,
        _ => return None,
    };
    let path_len = source
        .expr
        .as_ref()
        .map(|expr| expr.path.0.len())
        .unwrap_or(0);
    Some((form_id.index, path_len))
}

fn should_emit_top_level(node: &PatchNode, context: EmitContext) -> bool {
    if context == EmitContext::Macro && node.kind == NodeKind::Out {
        return false;
    }
    let Some(source) = node.source.as_ref() else {
        return matches!(node.kind, NodeKind::Out);
    };
    matches!(
        source.owner,
        SourceOwner::BindingValue { .. }
            | SourceOwner::TopLevelForm { .. }
            | SourceOwner::CodeIsland { .. }
            | SourceOwner::Compound { .. }
    )
}

fn emit_top_level_node(
    node: &PatchNode,
    nodes: &HashMap<&str, &PatchNode>,
    inbound: &HashMap<(String, usize), &PatchConnection>,
    stack: &mut HashSet<String>,
) -> String {
    let Some(source) = node.source.as_ref() else {
        return emit_node_expr(node, nodes, inbound, stack);
    };
    match &source.owner {
        SourceOwner::BindingValue { binding, .. } => match binding {
            BindingTarget::Symbol(name) => {
                format!(
                    "(def {name} {})",
                    emit_node_expr(node, nodes, inbound, stack)
                )
            }
            BindingTarget::Destructuring(names) => format!(
                "(def ({}) {})",
                names.join(" "),
                emit_node_expr(node, nodes, inbound, stack)
            ),
        },
        SourceOwner::CodeIsland { .. } => {
            format!(";; unsupported code island: {}", node.label)
        }
        SourceOwner::Compound { .. } if node.kind == NodeKind::History => {
            format!("(make-history {})", node.id)
        }
        _ => emit_node_expr(node, nodes, inbound, stack),
    }
}

fn emit_node_expr(
    node: &PatchNode,
    nodes: &HashMap<&str, &PatchNode>,
    inbound: &HashMap<(String, usize), &PatchConnection>,
    stack: &mut HashSet<String>,
) -> String {
    if !stack.insert(node.id.clone()) {
        return format!("/*cycle:{}*/", node.id);
    }
    let expr = match node.kind {
        NodeKind::Constant => node.op.clone(),
        NodeKind::Param => emit_label_as_call(&node.label),
        NodeKind::History => format!("(read-history {})", node.id),
        NodeKind::CodeIsland => format!("/*unsupported:{}*/", node.label),
        _ => emit_call_expr(node, nodes, inbound, stack),
    };
    stack.remove(&node.id);
    expr
}

fn emit_call_expr(
    node: &PatchNode,
    nodes: &HashMap<&str, &PatchNode>,
    inbound: &HashMap<(String, usize), &PatchConnection>,
    stack: &mut HashSet<String>,
) -> String {
    let mut parts = vec![node.op.clone()];
    for (idx, arg) in node.args.iter().enumerate() {
        if let Some(connection) = inbound.get(&(node.id.clone(), idx))
            && connection.kind == ConnectionKind::Forward
            && let Some(source_node) = nodes.get(connection.from_node.as_str())
        {
            if let Some(symbol) = connection.source.as_ref().and_then(symbol_reference_arg) {
                parts.push(symbol.to_string());
                continue;
            }
            if let Some(symbol) = node_reference_name(source_node) {
                parts.push(symbol);
                continue;
            }
            parts.push(emit_node_expr(source_node, nodes, inbound, stack));
            continue;
        }
        parts.push(match arg {
            ArgValue::Literal(value) => value.clone(),
            ArgValue::SymbolRef(symbol) => symbol.clone(),
            ArgValue::ConnectedExpr => MISSING_INPUT_SENTINEL.to_string(),
        });
    }
    format!("({})", parts.join(" "))
}

fn emit_label_as_call(label: &str) -> String {
    format!("({})", label.trim())
}

fn node_reference_name(node: &PatchNode) -> Option<String> {
    let source = node.source.as_ref()?;
    match &source.owner {
        SourceOwner::BindingValue {
            binding: BindingTarget::Symbol(name),
            ..
        } => Some(name.clone()),
        SourceOwner::TopLevelForm { .. } if node.kind == NodeKind::Param => {
            node.label.split_whitespace().nth(1).map(str::to_string)
        }
        SourceOwner::Compound { .. } if node.kind == NodeKind::History => Some(node.id.clone()),
        _ => None,
    }
}

fn symbol_reference_arg(source: &super::model::ConnectionSource) -> Option<&str> {
    match &source.previous_arg {
        SourceArgValue::SymbolReference { symbol, .. } => Some(symbol),
        _ => None,
    }
}
