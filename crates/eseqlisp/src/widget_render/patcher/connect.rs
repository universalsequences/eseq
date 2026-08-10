//! Agentic connect: wiring one already-placed node into the surrounding patch
//! from a plan the agent proposes (docs/patcher-agentic-connect-spec.md).
//!
//! Nothing here emits or rewrites source. A plan is validated against the patch
//! at the current view level and then applied through the same primitives a
//! cable drag and a node retype drive, so the result is projectable by
//! construction.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use super::display::node_display_label;
use super::geometry::patch_input_indices;
use super::lisp::{is_numeric_literal, parse_editor_node_text};
use super::model::{
    ArgValue, InputPortRef, InputPresentation, MacroSignature, OutputPortRef, Patch, PatchNode,
    hidden_inline_node_ids,
};
use super::project::dgenlisp_operator_documentation;
use super::state::{
    ConnectSubject, PatcherInteractionState, allocate_created_connection, ensure_source_node_edit,
    macro_signatures_with_visual_edits, node_edit_key, node_from_editor_text,
};

/// One tagged edit from a connection plan (spec §6). Arguments are addressed by
/// index, never by drawn port position: setting a literal removes a port and
/// shifts the rest, but indices are stable.
#[derive(Clone, Debug, PartialEq)]
pub enum PatcherConnectOp {
    Connect {
        from_node: String,
        from_outlet: usize,
        to_node: String,
        to_arg: usize,
        why: String,
    },
    Inline {
        value: String,
        to_node: String,
        to_arg: usize,
        why: String,
    },
}

impl PatcherConnectOp {
    fn to_node(&self) -> &str {
        match self {
            PatcherConnectOp::Connect { to_node, .. }
            | PatcherConnectOp::Inline { to_node, .. } => to_node.as_str(),
        }
    }

    fn to_arg(&self) -> usize {
        match self {
            PatcherConnectOp::Connect { to_arg, .. } | PatcherConnectOp::Inline { to_arg, .. } => {
                *to_arg
            }
        }
    }

    fn why(&self) -> &str {
        match self {
            PatcherConnectOp::Connect { why, .. } | PatcherConnectOp::Inline { why, .. } => {
                why.as_str()
            }
        }
    }

    fn describe(&self) -> String {
        match self {
            PatcherConnectOp::Connect {
                from_node,
                from_outlet,
                to_node,
                to_arg,
                ..
            } => format!("connect {from_node}:{from_outlet} -> {to_node}:{to_arg}"),
            PatcherConnectOp::Inline {
                value,
                to_node,
                to_arg,
                ..
            } => format!("inline {value} -> {to_node}:{to_arg}"),
        }
    }
}

/// What a plan did, for the status line (spec §9): silently dropping skipped
/// ops would read as "it wired everything" when it did not.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PatcherConnectReport {
    pub applied: Vec<String>,
    pub skipped: Vec<String>,
}

/// Whether an argument slot can take a plan op (spec §5.4).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ArgOccupancy {
    /// A port is drawn here and nothing is cabled to it.
    Free,
    Cabled {
        from_node: String,
        from_output: usize,
    },
    Literal(String),
    /// A named param rendered inside the node, backed by a hidden param node.
    InlineParam(String),
    /// No port is drawn and the argument holds a nested expression.
    Occupied,
}

impl ArgOccupancy {
    fn describe(&self) -> String {
        match self {
            ArgOccupancy::Free => "free".to_string(),
            ArgOccupancy::Cabled {
                from_node,
                from_output,
            } => format!("cabled from {from_node}:{from_output}"),
            ArgOccupancy::Literal(value) => format!("literal \"{value}\""),
            ArgOccupancy::InlineParam(name) => format!("inline param {name}"),
            ArgOccupancy::Occupied => "occupied".to_string(),
        }
    }
}

pub(super) fn arg_occupancies(
    patch: &Patch,
    node: &PatchNode,
    drawn_ports: &HashMap<String, Vec<usize>>,
) -> Vec<ArgOccupancy> {
    let drawn = drawn_ports.get(&node.id).cloned().unwrap_or_default();
    (0..node.args.len())
        .map(|idx| {
            if let Some(connection) = patch.connections.iter().find(|connection| {
                connection.to_node == node.id
                    && connection.to_input == idx
                    && connection.presentation == InputPresentation::Cable
            }) {
                return ArgOccupancy::Cabled {
                    from_node: connection.from_node.clone(),
                    from_output: connection.from_output,
                };
            }
            if let Some(inline) = node.inline_inputs.get(idx).and_then(|input| input.as_ref()) {
                return ArgOccupancy::InlineParam(inline.label());
            }
            if let Some(ArgValue::Literal(value)) = node.args.get(idx) {
                return ArgOccupancy::Literal(value.clone());
            }
            if drawn.contains(&idx) {
                ArgOccupancy::Free
            } else {
                ArgOccupancy::Occupied
            }
        })
        .collect()
}

/// The §5 payload: what the subject is, plus one line per node and per argument
/// at the current view level. Names go out so the agent can reason about
/// intent; the plan comes back addressing indices.
pub(super) fn connect_context(
    patch: &Patch,
    subject_node_id: &str,
    subject: &ConnectSubject,
) -> String {
    let mut out = String::new();
    match subject {
        ConnectSubject::Macro {
            name,
            params,
            source,
        } => {
            let _ = writeln!(
                out,
                "subject: node {subject_node_id} is an instance of macro {name} ({})",
                params.join(" ")
            );
            let _ = writeln!(out, "{source}");
        }
        ConnectSubject::Operator { op } => {
            let _ = writeln!(out, "subject: node {subject_node_id} is the operator {op}");
            if let Some(docs) = dgenlisp_operator_documentation().get(op) {
                if let Some(summary) = &docs.summary {
                    let _ = writeln!(out, "{summary}");
                }
                for signature in &docs.signatures {
                    let _ = writeln!(out, "{signature}");
                }
            }
        }
    }
    let _ = writeln!(out, "\npatch:");
    let drawn_ports = patch_input_indices(patch);
    // Inline parameter accessors are not drawn, and render.rs skips any cable
    // touching one — offering them as wire endpoints would produce an
    // applied-but-invisible edge.
    let hidden_node_ids = hidden_inline_node_ids(patch);
    for node in &patch.nodes {
        if hidden_node_ids.contains(&node.id) {
            continue;
        }
        let _ = writeln!(
            out,
            "{}  {}  kind={:?}",
            node.id,
            node_display_label(node),
            node.kind
        );
        for (idx, occupancy) in arg_occupancies(patch, node, &drawn_ports)
            .into_iter()
            .enumerate()
        {
            let _ = writeln!(
                out,
                "  in  {idx}  {}  {}",
                inlet_name(patch, node, idx),
                occupancy.describe()
            );
        }
        for (idx, name) in node.outputs.iter().enumerate() {
            let _ = writeln!(out, "  out {idx}  {name}");
        }
    }
    out
}

/// The inlet's name as the port tooltip would show it: a macro parameter, an
/// operator's documented port, or the symbol the argument already reads.
fn inlet_name(patch: &Patch, node: &PatchNode, index: usize) -> String {
    let named = if node.kind == super::model::NodeKind::MacroInstance {
        patch
            .macros
            .iter()
            .find(|macro_patch| macro_patch.name == node.op)
            .and_then(|macro_patch| macro_patch.params.get(index))
            .cloned()
    } else {
        dgenlisp_operator_documentation()
            .get(&node.op)
            .and_then(|docs| docs.inputs.get(index))
            .and_then(|input| input.name.clone())
    };
    named
        .or_else(|| match node.args.get(index) {
            Some(ArgValue::SymbolRef(name)) => Some(name.clone()),
            _ => None,
        })
        .unwrap_or_else(|| format!("in{index}"))
}

/// Validate every op against `patch`, then apply the survivors. `patch` must
/// already carry the interaction state's uncommitted edits.
pub(super) fn apply_connect_plan(
    state: &mut PatcherInteractionState,
    patch: &Patch,
    view_key: &str,
    ops: &[PatcherConnectOp],
) -> PatcherConnectReport {
    let drawn_ports = patch_input_indices(patch);
    let hidden_node_ids = hidden_inline_node_ids(patch);
    let signatures = macro_signatures_with_visual_edits(patch, state);
    let mut claimed: HashMap<(String, usize), ArgClaim> = HashMap::new();
    // Reported in plan order, whatever order the work happens in.
    let mut applied: Vec<(usize, String)> = Vec::new();
    let mut skipped: Vec<(usize, String)> = Vec::new();
    let mut valid: Vec<(usize, &PatcherConnectOp)> = Vec::new();
    // Every inline op for a node collapses into that node's single text edit,
    // so they are composed together before any of them is accepted — building
    // each one from the original node would let the last write erase the rest.
    let mut inlined: HashMap<&str, Vec<InlineSet>> = HashMap::new();
    for (index, op) in ops.iter().enumerate() {
        match validate_op(patch, &drawn_ports, &hidden_node_ids, &mut claimed, op) {
            Ok(None) => valid.push((index, op)),
            Ok(Some((arg, value))) => {
                inlined
                    .entry(op.to_node())
                    .or_default()
                    .push(InlineSet { index, arg, value });
                valid.push((index, op));
            }
            Err(reason) => skipped.push((index, format!("{}: {reason}", op.describe()))),
        }
    }

    let mut node_texts: HashMap<&str, (&PatchNode, String)> = HashMap::new();
    for (node_id, sets) in &inlined {
        let Some(node) = patch.nodes.iter().find(|node| &node.id == node_id) else {
            continue;
        };
        match inlined_node_text(node, sets, &signatures) {
            Ok(text) => {
                node_texts.insert(node_id, (node, text));
            }
            Err(reason) => {
                // The composed text is all-or-nothing: reporting one of these
                // as applied would describe a node state that never existed.
                for set in sets {
                    skipped.push((
                        set.index,
                        format!("{}: {reason}", ops[set.index].describe()),
                    ));
                }
                valid.retain(|(index, _)| !sets.iter().any(|set| set.index == *index));
            }
        }
    }

    let mut written: HashSet<&str> = HashSet::new();
    for (index, op) in valid {
        match op {
            PatcherConnectOp::Connect {
                from_node,
                from_outlet,
                to_node,
                to_arg,
                ..
            } => {
                allocate_created_connection(
                    state,
                    view_key,
                    OutputPortRef {
                        node_id: from_node.clone(),
                        output_index: *from_outlet,
                    },
                    InputPortRef {
                        node_id: to_node.clone(),
                        input_index: *to_arg,
                    },
                );
            }
            PatcherConnectOp::Inline { to_node, .. } => {
                let Some((node, text)) = node_texts.get(to_node.as_str()) else {
                    skipped.push((index, format!("{}: node text went missing", op.describe())));
                    continue;
                };
                if written.insert(to_node.as_str()) {
                    ensure_source_node_edit(state, view_key, node, node_display_label(node));
                    if let Some(edit) = state
                        .edit_state
                        .nodes
                        .get_mut(&node_edit_key(view_key, to_node))
                    {
                        edit.text = text.clone();
                    }
                }
            }
        }
        applied.push((index, format!("{} ({})", op.describe(), op.why())));
    }

    applied.sort_by_key(|(index, _)| *index);
    skipped.sort_by_key(|(index, _)| *index);
    PatcherConnectReport {
        applied: applied.into_iter().map(|(_, entry)| entry).collect(),
        skipped: skipped.into_iter().map(|(_, entry)| entry).collect(),
    }
}

/// One accepted `inline` op, waiting to be composed with its node's others.
struct InlineSet {
    /// Position in the plan, so the report stays in plan order.
    index: usize,
    arg: usize,
    value: String,
}

/// How an accepted op holds its argument slot. An inlet sums its cables, so
/// several `connect` ops may share one; an `inline` op rewrites the slot's
/// literal and cannot share with anything.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ArgClaim {
    Cable,
    Exclusive,
}

/// Spec §7. Returns the argument and canonical literal an `inline` op will set,
/// which its node composes with the plan's other inline ops.
fn validate_op(
    patch: &Patch,
    drawn_ports: &HashMap<String, Vec<usize>>,
    hidden_node_ids: &HashSet<String>,
    claimed: &mut HashMap<(String, usize), ArgClaim>,
    op: &PatcherConnectOp,
) -> Result<Option<(usize, String)>, String> {
    let target = patch
        .nodes
        .iter()
        .find(|node| node.id == op.to_node())
        .ok_or_else(|| format!("no node `{}` at this level", op.to_node()))?;
    if hidden_node_ids.contains(&target.id) {
        return Err(format!(
            "node `{}` is an inline parameter accessor",
            target.id
        ));
    }
    if op.to_arg() >= target.args.len() {
        return Err(format!(
            "argument {} is out of range for `{}`",
            op.to_arg(),
            target.id
        ));
    }
    let slot = (target.id.clone(), op.to_arg());
    let claim = match op {
        PatcherConnectOp::Connect { .. } => ArgClaim::Cable,
        PatcherConnectOp::Inline { .. } => ArgClaim::Exclusive,
    };
    if let Some(existing) = claimed.get(&slot)
        && (claim == ArgClaim::Exclusive || *existing == ArgClaim::Exclusive)
    {
        return Err("another op already targets this argument".to_string());
    }
    // The claim is only recorded once the op is known to be valid: an op that
    // fails below must not poison the slot for a later valid op in the plan.
    let occupancy = arg_occupancies(patch, target, drawn_ports)
        .into_iter()
        .nth(op.to_arg())
        .unwrap_or(ArgOccupancy::Occupied);
    // A cabled inlet still accepts another cable — the slot sums. Everything
    // else (a literal, an inline param, a nested expression) has no drawn port
    // and is not a wiring target at all.
    let accepts = match (op, &occupancy) {
        (_, ArgOccupancy::Free) => true,
        (PatcherConnectOp::Connect { .. }, ArgOccupancy::Cabled { .. }) => true,
        _ => false,
    };
    if !accepts {
        return Err(format!("argument is not free ({})", occupancy.describe()));
    }
    match op {
        PatcherConnectOp::Connect {
            from_node,
            from_outlet,
            ..
        } => {
            if from_node == &target.id {
                return Err("self-connection".to_string());
            }
            let source = patch
                .nodes
                .iter()
                .find(|node| &node.id == from_node)
                .ok_or_else(|| format!("no node `{from_node}` at this level"))?;
            if hidden_node_ids.contains(&source.id) {
                return Err(format!(
                    "node `{from_node}` is an inline parameter accessor"
                ));
            }
            if *from_outlet >= source.outputs.len() {
                return Err(format!(
                    "outlet {from_outlet} is out of range for `{from_node}`"
                ));
            }
            // A slot sums its cables, so a duplicate edge would silently double
            // the signal rather than add anything.
            if connection_exists(patch, from_node, *from_outlet, &target.id, op.to_arg()) {
                return Err("already connected".to_string());
            }
            claimed.insert(slot, claim);
            Ok(None)
        }
        PatcherConnectOp::Inline { value, to_arg, .. } => {
            let value = canonical_inline_value(value)?;
            claimed.insert(slot, claim);
            Ok(Some((*to_arg, value)))
        }
    }
}

/// Whether `patch` already carries this exact edge. A re-drawn cable must be a
/// no-op: `slot_value_expr` sums an inlet's cables, so a duplicate doubles the
/// signal instead of changing nothing.
pub(super) fn connection_exists(
    patch: &Patch,
    from_node: &str,
    from_output: usize,
    to_node: &str,
    to_input: usize,
) -> bool {
    patch.connections.iter().any(|connection| {
        connection.from_node == from_node
            && connection.from_output == from_output
            && connection.to_node == to_node
            && connection.to_input == to_input
    })
}

/// The literal as the node editor would round-trip it.
///
/// Numbers only: an expression typed into an argument slot is new DSP, which is
/// what the create bubble is for, and a bare symbol would silently bind to
/// whatever happens to be in scope.
fn canonical_inline_value(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("inline value is empty".to_string());
    }
    match parse_editor_node_text(&format!("+ {value}")) {
        Ok((op, args)) if op == "+" && args.len() == 1 && is_numeric_literal(&args[0]) => {
            Ok(args[0].clone())
        }
        Ok(_) => Err(format!("`{value}` is not a number")),
        Err(error) => Err(format!("`{value}` is not a number: {error}")),
    }
}

/// The node's editor text with every `sets` argument set. Built by relabelling
/// the node rather than by splicing tokens: the display label omits undisplayed
/// arguments, so a token index is not an argument index.
fn inlined_node_text(
    node: &PatchNode,
    sets: &[InlineSet],
    signatures: &HashMap<String, MacroSignature>,
) -> Result<String, String> {
    let mut updated = node.clone();
    for set in sets {
        updated.args[set.arg] = ArgValue::Literal(set.value.clone());
    }
    let text = node_display_label(&updated);
    // Accept the text only if the canvas rebuilds every intended argument from
    // it. Not every argument is reachable as text — the first input slot is the
    // implicit signal inlet, which the editor's own round trip shifts past.
    let rebuilt = node_from_editor_text(&node.id, &text, node.position, signatures, false);
    if rebuilt.op != node.op {
        return Err(format!("`{}` cannot take an inline literal", node.op));
    }
    for set in sets {
        if rebuilt.args.get(set.arg) != Some(&ArgValue::Literal(set.value.clone())) {
            return Err(format!(
                "argument {} cannot hold an inline literal",
                set.arg
            ));
        }
    }
    Ok(text)
}
