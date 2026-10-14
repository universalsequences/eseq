//! What the patch editor's inspector reads and writes
//! (docs/patcher-workspace-spec.md §6).
//!
//! The render pass mirrors the selected boxes' editable text per patcher key
//! (it is the one place holding both the effective patch and the selection
//! every frame), the host publishes that, and an inspector edit rewrites box
//! text through the same node edits a typed edit makes: one undo step, then
//! the patcher's `:on-change`, queued like a Lisp-run command's.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::parser::{ASTParser, Expression, Parser};
use crate::layout::LayoutNode;

use super::display::editable_node_text;
use super::interaction::inbound_slots_for_node;
use super::lisp::{attribute_value_len, format_patch_literal, is_attribute_key};
use super::model::Patch;
use super::state::{
    PatcherInteractionState, ensure_source_node_edit, get_patcher_interaction_state,
    node_edit_key, set_patcher_interaction_state,
};

thread_local! {
    /// `(id, editable text)` of each selected box, in patch order, per
    /// patcher key.
    static SELECTED_NODES: RefCell<HashMap<u64, Vec<(String, String)>>> =
        RefCell::new(HashMap::new());
    /// The patcher the inspector edits: the last one rendered.
    static INSPECTED_PATCHER: RefCell<Option<(u64, LayoutNode)>> = const { RefCell::new(None) };
    /// The selected boxes' output shapes, index-aligned with SELECTED_NODES.
    static SELECTED_NODE_SHAPES: RefCell<HashMap<u64, Vec<String>>> =
        RefCell::new(HashMap::new());
    /// The selected cable, described for the inspector.
    static SELECTED_CABLE: RefCell<HashMap<u64, SelectedCable>> = RefCell::new(HashMap::new());
}

/// The selected cable as the inspector shows it
/// (docs/patcher-tensor-cables-spec.md §6).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelectedCable {
    /// What flows through it: `signal-tensor [8 1] · 8 lanes`, `signal (scalar)`,
    /// or `shape unknown`.
    pub shape: String,
    /// `phasor · outlet 1`
    pub from: String,
    /// `* · inlet 2`
    pub to: String,
}

/// A compiled shape for the inspector as `kind|dims|lanes`, three parts the
/// UI draws as a label, a badge and a dim note: `signal-tensor|[8]|8 lanes`,
/// `signal|scalar|`, or `shape unknown||` when the latest compile did not
/// reach it.
pub fn shape_label(shape: Option<&super::shapes::BindingShape>) -> String {
    use super::shapes::BindingShapeKind as K;
    let Some(shape) = shape else {
        return "shape unknown||".to_string();
    };
    let kind = match shape.kind {
        K::Signal => "signal",
        K::Constant => "constant",
        K::Tensor => "tensor",
        K::SignalTensor => "signal-tensor",
        K::TensorHistory => "tensor-history",
        K::Tuple => "tuple",
        K::None => "no value",
        K::Unknown => "value",
    };
    if shape.is_multichannel() {
        let lanes = shape.lane_count();
        format!(
            "{kind}|{}|{lanes} lane{}",
            shape.dims_label(),
            if lanes == 1 { "" } else { "s" }
        )
    } else if matches!(shape.kind, K::Signal | K::Constant) {
        format!("{kind}|scalar|")
    } else {
        format!("{kind}||")
    }
}

/// Mirror `state`'s selection in `patch` for the inspector (render pass).
pub(super) fn mirror_selection(
    key: u64,
    node: &LayoutNode,
    patch: &Patch,
    state: &PatcherInteractionState,
) {
    // A box whose text is being typed is not a real node yet: the inspector
    // shows it once the edit commits.
    let editing = state.text_edit.as_ref().map(|edit| edit.node_id.as_str());
    let selected: Vec<(String, String)> = patch
        .nodes
        .iter()
        .filter(|patch_node| state.selected_nodes.contains(&patch_node.id))
        .filter(|patch_node| Some(patch_node.id.as_str()) != editing)
        .map(|patch_node| {
            let text =
                editable_node_text(patch_node, &inbound_slots_for_node(patch, &patch_node.id));
            (patch_node.id.clone(), text)
        })
        .collect();
    // Shapes come from the latest compile, keyed by the patch path; only the
    // root view has them.
    let path = super::prop_str(&node.props, "path").or_else(|| super::prop_str(&node.props, "file"));
    let root_view = super::state::active_patcher_view_key(state) == "root";
    let shape_of = |id: &str| {
        path.as_deref()
            .filter(|_| root_view)
            .and_then(|path| super::shapes::node_output_shape(path, id))
    };
    let shapes: Vec<String> = selected.iter().map(|(id, _)| shape_label(shape_of(id).as_ref())).collect();
    SELECTED_NODE_SHAPES.with(|all| {
        let mut all = all.borrow_mut();
        if all.get(&key) != Some(&shapes) {
            all.insert(key, shapes);
        }
    });
    let cable = state.selected_cable.as_deref().and_then(|cable_id| {
        let connection = patch.connections.iter().find(|connection| {
            format!(
                "{}:{}->{}:{}",
                connection.from_node, connection.from_output, connection.to_node, connection.to_input
            ) == cable_id
        })?;
        let op = |id: &str| {
            patch
                .nodes
                .iter()
                .find(|n| n.id == id)
                .map(|n| if n.op.is_empty() { n.label.clone() } else { n.op.clone() })
                .unwrap_or_else(|| id.to_string())
        };
        let shape = path
            .as_deref()
            .filter(|_| root_view)
            .and_then(|path| super::shapes::connection_shape(path, connection));
        Some(SelectedCable {
            shape: shape_label(shape.as_ref()),
            from: format!("{} · outlet {}", op(&connection.from_node), connection.from_output + 1),
            to: format!("{} · inlet {}", op(&connection.to_node), connection.to_input + 1),
        })
    });
    SELECTED_CABLE.with(|all| {
        let mut all = all.borrow_mut();
        match cable {
            Some(cable) => {
                if all.get(&key) != Some(&cable) {
                    all.insert(key, cable);
                }
            }
            None => {
                all.remove(&key);
            }
        }
    });
    SELECTED_NODES.with(|nodes| {
        let mut nodes = nodes.borrow_mut();
        if nodes.get(&key) != Some(&selected) {
            nodes.insert(key, selected);
        }
    });
    INSPECTED_PATCHER.with(|cell| {
        let mut cell = cell.borrow_mut();
        let stale = match cell.as_ref() {
            Some((inspected, inspected_node)) => {
                *inspected != key || inspected_node.props != node.props
            }
            None => true,
        };
        if stale {
            *cell = Some((key, node.clone()));
        }
    });
}

/// The selected boxes of the patcher for `path`: `(id, editable text)`.
pub fn selected_nodes_for_path(path: impl AsRef<std::path::Path>) -> Vec<(String, String)> {
    super::state::patcher_keys_for_path(path.as_ref())
        .into_iter()
        .find_map(|key| SELECTED_NODES.with(|nodes| nodes.borrow().get(&key).cloned()))
        .unwrap_or_default()
}

/// The selected boxes' output shapes for `path`, index-aligned with
/// [`selected_nodes_for_path`] (see [`shape_label`]).
pub fn selected_node_shapes_for_path(path: impl AsRef<std::path::Path>) -> Vec<String> {
    super::state::patcher_keys_for_path(path.as_ref())
        .into_iter()
        .find_map(|key| SELECTED_NODE_SHAPES.with(|all| all.borrow().get(&key).cloned()))
        .unwrap_or_default()
}

/// The selected cable of the patcher for `path`, if one is selected.
pub fn selected_cable_for_path(path: impl AsRef<std::path::Path>) -> Option<SelectedCable> {
    super::state::patcher_keys_for_path(path.as_ref())
        .into_iter()
        .find_map(|key| SELECTED_CABLE.with(|all| all.borrow().get(&key).cloned()))
}

/// A box's text split for the inspector.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeTextParts {
    pub op: String,
    /// Positional arguments, each formatted as written.
    pub args: Vec<String>,
    /// `@key value` pairs in order; the key keeps its `@`.
    pub attrs: Vec<(String, String)>,
    /// The box is a number literal (`5`), not an operator call.
    pub constant: bool,
}

/// Split box text into operator, positional arguments and attributes.
pub fn parse_node_text(text: &str) -> Result<NodeTextParts, String> {
    let source = format!("({})", super::lisp::normalize_editor_node_text(text));
    let tokens = Parser::new(source)
        .parse()
        .map_err(|error| format!("failed to tokenize node text: {error:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("failed to parse node text: {error:?}"))?;
    let Some(Expression::List(items)) = exprs.first() else {
        return Err("node text must be a call".to_string());
    };
    let Some(head) = items.first() else {
        return Ok(NodeTextParts::default());
    };
    let mut parts = NodeTextParts {
        op: format_patch_literal(head),
        constant: items.len() == 1 && matches!(head, Expression::Number(_)),
        ..NodeTextParts::default()
    };
    let mut idx = 1;
    while idx < items.len() {
        if is_attribute_key(&items[idx]) {
            let key = format_patch_literal(&items[idx]);
            let len = attribute_value_len(items, idx + 1);
            let value = items[idx + 1..idx + 1 + len]
                .iter()
                .map(format_patch_literal)
                .collect::<Vec<_>>()
                .join(" ");
            parts.attrs.push((key, value));
            idx += 1 + len;
        } else {
            parts.args.push(format_patch_literal(&items[idx]));
            idx += 1;
        }
    }
    Ok(parts)
}

/// Rebuild box text from its parts; an attribute with an empty value is
/// dropped.
pub fn format_node_text(parts: &NodeTextParts) -> String {
    let mut out = parts.op.clone();
    for arg in &parts.args {
        out.push(' ');
        out.push_str(arg);
    }
    for (key, value) in &parts.attrs {
        if value.trim().is_empty() {
            continue;
        }
        out.push(' ');
        out.push_str(key);
        out.push(' ');
        out.push_str(value.trim());
    }
    out
}

/// `text` with attribute `key` set to `value` (appended when absent;
/// removed when `value` is empty).
pub fn set_node_text_attribute(text: &str, key: &str, value: &str) -> Result<String, String> {
    let mut parts = parse_node_text(text)?;
    match parts.attrs.iter_mut().find(|(existing, _)| existing == key) {
        Some((_, existing)) => value.clone_into(existing),
        None => parts.attrs.push((key.to_string(), value.to_string())),
    }
    Ok(format_node_text(&parts))
}

/// The attributes the dgenlisp manifest declares for operator `op`, `@`
/// included.
pub fn operator_attribute_names(op: &str) -> Vec<String> {
    super::project::dgenlisp_operator_attributes()
        .get(op)
        .map(|names| {
            names
                .iter()
                .map(|name| {
                    if name.starts_with('@') {
                        name.clone()
                    } else {
                        format!("@{name}")
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The dgenlisp manifest's documentation for operator `op` (its summary,
/// signatures and inlets / outlets), for the inspector; `None` for an
/// undocumented operator or a macro.
pub fn operator_documentation(op: &str) -> Option<OperatorDocs> {
    let docs = super::project::dgenlisp_operator_documentation().get(op)?;
    let port = |port: &super::project::OperatorPortDocumentation| PortDocs {
        name: port.name.clone().unwrap_or_default(),
        kind: port.kind.clone().unwrap_or_default(),
        required: port.required.unwrap_or(false),
        summary: port.summary.clone().unwrap_or_default(),
    };
    Some(OperatorDocs {
        summary: docs.summary.clone().unwrap_or_default(),
        signatures: docs.signatures.clone(),
        inputs: docs.inputs.iter().map(port).collect(),
        outputs: docs.outputs.iter().map(port).collect(),
    })
}

/// See [`operator_documentation`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OperatorDocs {
    pub summary: String,
    pub signatures: Vec<String>,
    pub inputs: Vec<PortDocs>,
    pub outputs: Vec<PortDocs>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PortDocs {
    pub name: String,
    pub kind: String,
    pub required: bool,
    pub summary: String,
}

/// Rewrite the text of boxes in the inspected patcher, `(id, text)` each, as
/// one undo step, and queue the patcher's `:on-change` so the host previews
/// it. `false` when nothing changed or no patcher is open.
pub fn set_inspected_node_texts(edits: &[(String, String)]) -> bool {
    let Some((key, node)) = INSPECTED_PATCHER.with(|cell| cell.borrow().clone()) else {
        return false;
    };
    let mut state = get_patcher_interaction_state(key);
    if state.text_edit.is_some() || state.drag.is_some() {
        return false;
    }
    let view_key = super::state::active_patcher_view_key(&state);
    let Some(patch) = super::debug_patch_for_state(&node, &state, &view_key) else {
        return false;
    };
    let mut changed = false;
    for (id, text) in edits {
        let Some(patch_node) = patch.nodes.iter().find(|patch_node| &patch_node.id == id) else {
            continue;
        };
        let current = editable_node_text(patch_node, &inbound_slots_for_node(&patch, id));
        let text = text.trim();
        if text.is_empty() || text == current {
            continue;
        }
        ensure_source_node_edit(&mut state, &view_key, patch_node, current);
        if let Some(edit) = state.edit_state.nodes.get_mut(&node_edit_key(&view_key, id)) {
            text.clone_into(&mut edit.text);
            changed = true;
        }
    }
    if !changed {
        return false;
    }
    set_patcher_interaction_state(key, state);
    super::queue_patcher_command_output(&node, super::patcher_semantic_event(true));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_text_splits_into_op_args_and_attributes_and_back() {
        let text = "param attack @group amp @default 5 @min 0 @max 1000 @unit ms";
        let parts = parse_node_text(text).expect("parse");
        assert_eq!(parts.op, "param");
        assert_eq!(parts.args, vec!["attack".to_string()]);
        assert_eq!(
            parts.attrs,
            vec![
                ("@group".to_string(), "amp".to_string()),
                ("@default".to_string(), "5".to_string()),
                ("@min".to_string(), "0".to_string()),
                ("@max".to_string(), "1000".to_string()),
                ("@unit".to_string(), "ms".to_string()),
            ]
        );
        assert_eq!(format_node_text(&parts), text);
    }

    #[test]
    fn a_number_box_is_a_constant() {
        let parts = parse_node_text("5").expect("parse");
        assert!(parts.constant);
        assert_eq!(parts.op, "5");
        assert!(!parse_node_text("phasor 5").expect("parse").constant);
    }

    #[test]
    fn setting_an_attribute_replaces_appends_or_removes_it() {
        let text = "param gain @default 0.5 @min 0";
        assert_eq!(
            set_node_text_attribute(text, "@min", "-1").unwrap(),
            "param gain @default 0.5 @min -1"
        );
        assert_eq!(
            set_node_text_attribute(text, "@max", "2").unwrap(),
            "param gain @default 0.5 @min 0 @max 2"
        );
        assert_eq!(
            set_node_text_attribute(text, "@min", "").unwrap(),
            "param gain @default 0.5"
        );
    }

    #[test]
    fn a_documented_operator_lists_its_inlets() {
        let docs = operator_documentation("phasor").expect("phasor is documented");
        assert!(!docs.summary.is_empty());
        assert!(!docs.inputs.is_empty(), "phasor has a frequency inlet");
        assert!(operator_documentation("no-such-operator").is_none());
    }

    #[test]
    fn a_bracketed_attribute_value_stays_one_value() {
        let parts = parse_node_text("tensor @shape [3 3] @default 0").expect("parse");
        assert_eq!(parts.args, Vec::<String>::new());
        assert_eq!(parts.attrs[0].0, "@shape");
        assert_eq!(parts.attrs[1], ("@default".to_string(), "0".to_string()));
    }
}
