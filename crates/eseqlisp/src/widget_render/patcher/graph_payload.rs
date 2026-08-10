//! Serialized patch graph (spec §4.1b/§4.1c, rev 3).
//!
//! The generated `dsp.lisp` is a lossy encoding of the patch model: distinct
//! models print to identical text (an inline numeric literal and a wired
//! constant node being the canonical example), so reconstructing the model by
//! re-parsing the file is a guess. This module serializes the model itself into
//! the layout sidecar, making the payload authoritative on load and letting the
//! generator emit natural source instead of distorting it to smuggle model
//! facts through the parser.
//!
//! Deliberately NOT serialized:
//! - `NodeSource` / `ConnectionSource` — source positions are dead for editing
//!   (§4.1a); the payload exists precisely so nothing has to map back to text.
//! - `position` / `width` / cable segments — already carried by the sidecar's
//!   layout maps, which stay the single home for geometry.
//! - `inline_inputs` — derived from connections by `refresh_patch_inline_inputs`.
//! - `diagnostics` and per-node `diagnostic` — derived from operator/macro
//!   resolution, recomputed on load by `resolve_node_operators`. Freezing a
//!   diagnostic would make "unknown operator" permanent for a call whose
//!   library simply was not loaded when the payload was written.

use serde::{Deserialize, Serialize};

use super::lisp::connection_kind_for_op;
use super::model::{
    ArgValue, ConnectionKind, HostModulatorInput, InputPresentation, MacroOrigin, MacroPatch,
    NodeKind, ParamNodeInfo, Patch, PatchConnection, PatchNode, SourceOwner,
    refresh_patch_inline_inputs,
};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphPayload {
    #[serde(default)]
    nodes: Vec<NodeEntry>,
    #[serde(default)]
    connections: Vec<ConnectionEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    macros: Vec<MacroEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    host_modulators: Vec<HostModulatorEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    imports: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeEntry {
    id: String,
    op: String,
    kind: NodeKindEntry,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    args: Vec<ArgEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    param: Option<ParamEntry>,
    /// Projector-synthesized helper (the hidden `(mod p)` accessor). Implied by
    /// a `NestedExpr` source owner on the parse path; carried explicitly here
    /// because payloads hold no source data at all.
    #[serde(default, skip_serializing_if = "is_false")]
    synthesized: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum NodeKindEntry {
    Builtin,
    Param,
    In,
    Out,
    History,
    Constant,
    MacroDefinition,
    MacroInstance,
    CodeIsland,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum ArgEntry {
    Literal { value: String },
    SymbolRef { name: String },
    Connected,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ParamEntry {
    name: String,
    modulatable: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionEntry {
    from_node: String,
    #[serde(default)]
    from_output: usize,
    to_node: String,
    #[serde(default)]
    to_input: usize,
    #[serde(default = "forward_kind")]
    kind: ConnectionKindEntry,
    #[serde(default = "cable_presentation")]
    presentation: InputPresentation,
}

fn forward_kind() -> ConnectionKindEntry {
    ConnectionKindEntry::Forward
}

fn cable_presentation() -> InputPresentation {
    InputPresentation::Cable
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ConnectionKindEntry {
    Forward,
    Feedback,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MacroEntry {
    name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    params: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    outputs: Vec<String>,
    origin: MacroOriginEntry,
    #[serde(default)]
    graph: GraphPayload,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum MacroOriginEntry {
    Local,
    #[serde(rename_all = "camelCase")]
    Library {
        source_path: String,
        layout_path: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct HostModulatorEntry {
    name: String,
    channel: usize,
    slot: usize,
}

pub(super) fn payload_from_patch(patch: &Patch) -> GraphPayload {
    // Normalize before serializing, not only after loading: a node created by
    // typing `phasor trigger` records the edge as a bare argument with no
    // `PatchConnection`, while the same node parsed back from source has both.
    // Canonicalizing on the way out keeps save → load → save byte-identical.
    let mut normalized = patch.clone();
    materialize_symbol_ref_connections(&mut normalized);
    for macro_patch in &mut normalized.macros {
        materialize_symbol_ref_connections(&mut macro_patch.patch);
    }
    let patch = &normalized;
    GraphPayload {
        nodes: patch.nodes.iter().map(node_entry).collect(),
        connections: patch.connections.iter().map(connection_entry).collect(),
        macros: patch.macros.iter().map(macro_entry).collect(),
        host_modulators: patch
            .host_modulators
            .iter()
            .map(|modulator| HostModulatorEntry {
                name: modulator.name.clone(),
                channel: modulator.channel,
                slot: modulator.slot,
            })
            .collect(),
        imports: patch.imports.clone(),
    }
}

pub(super) fn patch_from_payload(payload: &GraphPayload) -> Patch {
    let mut patch = Patch {
        nodes: payload.nodes.iter().map(patch_node).collect(),
        connections: payload.connections.iter().map(patch_connection).collect(),
        macros: payload.macros.iter().map(macro_patch).collect(),
        diagnostics: Vec::new(),
        host_modulators: payload
            .host_modulators
            .iter()
            .map(|modulator| HostModulatorInput {
                name: modulator.name.clone(),
                channel: modulator.channel,
                slot: modulator.slot,
            })
            .collect(),
        imports: payload.imports.clone(),
    };
    materialize_symbol_ref_connections(&mut patch);
    refresh_patch_inline_inputs(&mut patch);
    for macro_patch in &mut patch.macros {
        materialize_symbol_ref_connections(&mut macro_patch.patch);
        refresh_patch_inline_inputs(&mut macro_patch.patch);
    }
    patch
}

/// A bare symbol argument (`phasor trigger` typed into a node box) is a real
/// edge in the graph, but the live model records only the argument — the
/// matching `PatchConnection` is something the projector synthesizes when it
/// reads the emitted source back. Loading from a payload skips that parse, so
/// the same normalization has to happen here or the cable is missing from the
/// canvas even though the source and the arg slot both describe it.
///
/// Both arg spellings have to be considered: parsed nodes carry `SymbolRef`,
/// while nodes created by typing carry a plain `Literal` whose text happens to
/// name a binding. The generator already resolves them the same way
/// (`collect_value_deps`, generate.rs:648), so this matches its rule.
fn materialize_symbol_ref_connections(patch: &mut Patch) {
    let resolved = patch
        .nodes
        .iter()
        .enumerate()
        .flat_map(|(node_index, node)| {
            node.args
                .iter()
                .enumerate()
                .filter_map(move |(arg_index, arg)| match arg {
                    ArgValue::SymbolRef(name) | ArgValue::Literal(name) => {
                        Some((node_index, arg_index, name.clone()))
                    }
                    ArgValue::ConnectedExpr => None,
                })
        })
        .filter(|(node_index, arg_index, _)| {
            let node = &patch.nodes[*node_index];
            !patch
                .connections
                .iter()
                .any(|connection| {
                    connection.to_node == node.id && connection.to_input == *arg_index
                })
        })
        .filter_map(|(node_index, arg_index, name)| {
            let node = &patch.nodes[node_index];
            let (from_node, from_output) = resolve_symbol_output(patch, &node.id, &name)?;
            Some(PatchConnection {
                from_node,
                from_output,
                to_node: node.id.clone(),
                to_input: arg_index,
                kind: connection_kind_for_op(&node.op),
                segment: None,
                presentation: InputPresentation::Cable,
                presentation_override: None,
                source: None,
            })
        })
        .collect::<Vec<_>>();
    patch.connections.extend(resolved);
}

/// A symbol names either a node's binding directly, or one output of a
/// multi-output `(def (a b) …)` node. Self-references never resolve: a node
/// cannot cable into itself, and treating its own binding name as an edge
/// would fabricate a self-loop.
fn resolve_symbol_output(patch: &Patch, consumer_id: &str, name: &str) -> Option<(String, usize)> {
    if name == consumer_id {
        return None;
    }
    // Constant nodes are keyed by their own text (`add_constant_node` ids a
    // `0.5` node "0.5"), so a numeric argument elsewhere in the patch would
    // otherwise "resolve" to an unrelated constant and fabricate a cable.
    // Attribute tokens (`@name`) are never bindings either.
    if name.parse::<f64>().is_ok() || name.starts_with('@') {
        return None;
    }
    if let Some(node) = patch.nodes.iter().find(|node| node.id == name) {
        return Some((node.id.clone(), 0));
    }
    patch
        .nodes
        .iter()
        .filter(|node| node.id != consumer_id)
        .find_map(|node| {
            node.outputs
                .iter()
                .position(|output| output == name)
                .map(|index| (node.id.clone(), index))
        })
}

fn node_entry(node: &PatchNode) -> NodeEntry {
    NodeEntry {
        id: node.id.clone(),
        op: node.op.clone(),
        kind: match node.kind {
            NodeKind::Builtin => NodeKindEntry::Builtin,
            NodeKind::Param => NodeKindEntry::Param,
            NodeKind::In => NodeKindEntry::In,
            NodeKind::Out => NodeKindEntry::Out,
            NodeKind::History => NodeKindEntry::History,
            NodeKind::Constant => NodeKindEntry::Constant,
            NodeKind::MacroDefinition => NodeKindEntry::MacroDefinition,
            NodeKind::MacroInstance => NodeKindEntry::MacroInstance,
            NodeKind::CodeIsland => NodeKindEntry::CodeIsland,
        },
        label: node.label.clone(),
        args: node
            .args
            .iter()
            .map(|arg| match arg {
                ArgValue::Literal(value) => ArgEntry::Literal {
                    value: value.clone(),
                },
                ArgValue::SymbolRef(name) => ArgEntry::SymbolRef { name: name.clone() },
                ArgValue::ConnectedExpr => ArgEntry::Connected,
            })
            .collect(),
        outputs: node.outputs.clone(),
        param: node.param.as_ref().map(|param| ParamEntry {
            name: param.name.clone(),
            modulatable: param.modulatable,
        }),
        synthesized: node.synthesized
            || matches!(
                node.source.as_ref().map(|source| &source.owner),
                Some(SourceOwner::NestedExpr { .. })
            ),
    }
}

fn patch_node(entry: &NodeEntry) -> PatchNode {
    let args = entry
        .args
        .iter()
        .map(|arg| match arg {
            ArgEntry::Literal { value } => ArgValue::Literal(value.clone()),
            ArgEntry::SymbolRef { name } => ArgValue::SymbolRef(name.clone()),
            ArgEntry::Connected => ArgValue::ConnectedExpr,
        })
        .collect::<Vec<_>>();
    PatchNode {
        id: entry.id.clone(),
        op: entry.op.clone(),
        kind: match entry.kind {
            NodeKindEntry::Builtin => NodeKind::Builtin,
            NodeKindEntry::Param => NodeKind::Param,
            NodeKindEntry::In => NodeKind::In,
            NodeKindEntry::Out => NodeKind::Out,
            NodeKindEntry::History => NodeKind::History,
            NodeKindEntry::Constant => NodeKind::Constant,
            NodeKindEntry::MacroDefinition => NodeKind::MacroDefinition,
            NodeKindEntry::MacroInstance => NodeKind::MacroInstance,
            NodeKindEntry::CodeIsland => NodeKind::CodeIsland,
        },
        label: entry.label.clone(),
        inline_inputs: vec![None; args.len()],
        args,
        outputs: entry.outputs.clone(),
        // Geometry comes from the sidecar's layout maps, applied right after
        // deserialization; unmatched ids fall back to auto-placement as always.
        position: (0.0, 0.0),
        width: None,
        param: entry.param.as_ref().map(|param| ParamNodeInfo {
            name: param.name.clone(),
            modulatable: param.modulatable,
        }),
        // Recomputed by `resolve_node_operators` once the macro set is
        // settled; a serialized diagnostic would outlive the condition that
        // produced it (a library that was merely unavailable at save time).
        diagnostic: None,
        source: None,
        synthesized: entry.synthesized,
    }
}

fn connection_entry(connection: &PatchConnection) -> ConnectionEntry {
    ConnectionEntry {
        from_node: connection.from_node.clone(),
        from_output: connection.from_output,
        to_node: connection.to_node.clone(),
        to_input: connection.to_input,
        kind: match connection.kind {
            ConnectionKind::Forward => ConnectionKindEntry::Forward,
            ConnectionKind::Feedback => ConnectionKindEntry::Feedback,
        },
        // `presentation_override` is deliberately not serialized here: it is
        // the layout-level "user pinned this inlet inline" record, already
        // carried by the sidecar's `inputPresentation` map and restored from
        // there on load. Storing it twice made a save-load-save cycle
        // non-idempotent (the map sets the override, the payload then reported
        // an override the original write did not have).
        presentation: connection.presentation,
    }
}

fn patch_connection(entry: &ConnectionEntry) -> PatchConnection {
    PatchConnection {
        from_node: entry.from_node.clone(),
        from_output: entry.from_output,
        to_node: entry.to_node.clone(),
        to_input: entry.to_input,
        kind: match entry.kind {
            ConnectionKindEntry::Forward => ConnectionKind::Forward,
            ConnectionKindEntry::Feedback => ConnectionKind::Feedback,
        },
        segment: None,
        presentation: entry.presentation,
        presentation_override: None,
        source: None,
    }
}

fn macro_entry(macro_patch: &MacroPatch) -> MacroEntry {
    MacroEntry {
        name: macro_patch.name.clone(),
        params: macro_patch.params.clone(),
        outputs: macro_patch.outputs.clone(),
        origin: match &macro_patch.origin {
            MacroOrigin::Local => MacroOriginEntry::Local,
            MacroOrigin::Library {
                source_path,
                layout_path,
            } => MacroOriginEntry::Library {
                source_path: source_path.clone(),
                layout_path: layout_path.clone(),
            },
        },
        // A macro body's own `macros` list is a projection artifact (the
        // scope re-collects the defmacros visible to it, including itself).
        // Serializing it would nest one level deeper on every save — the file
        // grows without bound — and nothing reads a macro body's macro list:
        // both consumers (`generate.rs:396`, `project.rs:418`) work off the
        // root patch, where the flat list is already complete.
        graph: GraphPayload {
            macros: Vec::new(),
            ..payload_from_patch(&macro_patch.patch)
        },
    }
}

fn macro_patch(entry: &MacroEntry) -> MacroPatch {
    MacroPatch {
        name: entry.name.clone(),
        params: entry.params.clone(),
        outputs: entry.outputs.clone(),
        patch: patch_from_payload(&entry.graph),
        origin: match &entry.origin {
            MacroOriginEntry::Local => MacroOrigin::Local,
            MacroOriginEntry::Library {
                source_path,
                layout_path,
            } => MacroOrigin::Library {
                source_path: source_path.clone(),
                layout_path: layout_path.clone(),
            },
        },
    }
}
