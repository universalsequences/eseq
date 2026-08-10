//! Cmd+E: collapse the selected nodes into a new local `defmacro` plus one
//! macro-instance node, inferring the macro's parameter list and return value
//! from the cables that crossed the selection boundary.
//!
//! See docs/patcher-encapsulate-spec.md. The whole operation is a single
//! `PatchEditState` mutation: the macro body becomes created-node edits in the
//! `macro:<name>` view, and the deterministic generator turns that model into
//! `(defmacro …)` source at save time. A macro's signature is *derived* from
//! its `in`/`out` nodes (`generate.rs` `macro_param_list` / `emit_macro_return`),
//! so creating those nodes is how the interface gets built.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::display::node_display_label;
use super::model::{
    ArgValue, ConnectionKind, InputPortRef, NodeKind, OutputPortRef, Patch, PatchConnection,
    PatchNode, hidden_inline_node_ids,
};
use super::state::{
    PatcherInteractionState, PatcherMacroEdit, allocate_created_connection,
    allocate_created_node_avoiding, bring_nodes_to_front, delete_selected_nodes,
    empty_created_macro_source, node_edit_key, note_touched_node,
};

const MISSING_INPUT_SENTINEL: &str = "__patcher_missing_input__";

/// Cells of vertical clearance between the `in` row and the top of the moved
/// nodes inside the macro, and between the moved nodes and the `out` row.
const INTERFACE_ROW_GAP_CELLS: f32 = 4.0;
/// Horizontal spacing between adjacent `in` (or `out`) nodes.
const INTERFACE_SPACING_CELLS: f32 = 8.0;
/// Left/top inset of the macro body inside its scope.
const BODY_INSET_CELLS: (f32, f32) = (2.0, 2.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EncapsulationRefusal {
    /// Nothing usable was selected.
    EmptySelection,
    /// `param` / `in` / `out` / macro-definition / code-island nodes are bound
    /// to their scope and cannot move into a macro.
    ScopeBoundNode,
    /// Collapsing the selection to one atomic call would create a cycle:
    /// some external node both consumes from and feeds back into the selection.
    NotConvex,
    /// A history node inside the selection is both written from outside and
    /// read from outside; its state cannot cross the instance boundary twice.
    HistoryStraddlesBoundary,
    /// Every candidate macro name was taken.
    NoAvailableName,
}

/// A node inside the macro body, keyed by where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum BodyKey {
    /// A node moved out of the enclosing view, keyed by its original id.
    Moved(String),
    /// A constant hoisted out of a moved node's slot-0 literal, so the
    /// recreated node's argument indices line up with the original's.
    /// See `encapsulated_node_text`.
    HoistedConstant(String),
    /// The macro's `in K` node for inlet index K (0-based).
    Inlet(usize),
    /// The macro's `out K` node for outlet index K (0-based).
    Outlet(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PlannedNode {
    pub(super) key: BodyKey,
    pub(super) text: String,
    pub(super) position: (f32, f32),
    pub(super) width: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PlannedCable {
    pub(super) from: BodyKey,
    pub(super) from_output: usize,
    pub(super) to: BodyKey,
    pub(super) to_input: usize,
}

/// One macro parameter: the external output port that feeds it, and every
/// internal slot it lands in. A single external source port fanning into three
/// internal nodes yields one inlet with three destinations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PlannedInlet {
    pub(super) external_source: OutputPortRef,
    pub(super) internal_destinations: Vec<InputPortRef>,
}

/// One macro return value: the internal output port that produces it, and
/// every external slot it lands in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PlannedOutlet {
    pub(super) internal_source: OutputPortRef,
    pub(super) external_destinations: Vec<InputPortRef>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct EncapsulationPlan {
    pub(super) macro_name: String,
    pub(super) instance_position: (f32, f32),
    pub(super) body_nodes: Vec<PlannedNode>,
    pub(super) body_cables: Vec<PlannedCable>,
    pub(super) inlets: Vec<PlannedInlet>,
    pub(super) outlets: Vec<PlannedOutlet>,
    /// Node ids removed from the enclosing view.
    pub(super) removed_node_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

/// Classify the selection against `patch` and produce the full plan, or refuse.
/// Pure: touches no interaction state.
pub(super) fn plan_encapsulation(
    patch: &Patch,
    selected_node_ids: &HashSet<String>,
    macro_name: String,
) -> Result<EncapsulationPlan, EncapsulationRefusal> {
    let hidden = hidden_inline_node_ids(patch);
    let nodes_by_id = patch
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();

    let mut selection = selected_node_ids
        .iter()
        .filter(|id| !hidden.contains(*id))
        .filter_map(|id| nodes_by_id.get(id.as_str()).copied())
        .filter(|node| !node_display_label(node).trim().is_empty())
        .collect::<Vec<_>>();
    selection.sort_by(|left, right| node_order_key(left).cmp(&node_order_key(right)));
    if selection.is_empty() {
        return Err(EncapsulationRefusal::EmptySelection);
    }
    if selection.iter().any(|node| {
        matches!(
            node.kind,
            NodeKind::Param
                | NodeKind::In
                | NodeKind::Out
                | NodeKind::MacroDefinition
                | NodeKind::CodeIsland
        )
    }) {
        return Err(EncapsulationRefusal::ScopeBoundNode);
    }

    let selected_ids = selection
        .iter()
        .map(|node| node.id.clone())
        .collect::<HashSet<_>>();

    check_convexity(patch, &selected_ids)?;
    check_history_boundaries(patch, &selection, &selected_ids)?;

    // --- classify every cable against the boundary --------------------------
    let mut internal: Vec<&PatchConnection> = Vec::new();
    let mut crossing_in: Vec<&PatchConnection> = Vec::new();
    let mut crossing_out: Vec<&PatchConnection> = Vec::new();
    for connection in &patch.connections {
        let from_selected = selected_ids.contains(&connection.from_node);
        let to_selected = selected_ids.contains(&connection.to_node);
        match (from_selected, to_selected) {
            (true, true) => internal.push(connection),
            (false, true) => crossing_in.push(connection),
            (true, false) => crossing_out.push(connection),
            (false, false) => {}
        }
    }

    // --- allocate ports, deduplicating by port not by cable -----------------
    // Inlets key on the external SOURCE port: one external node fanning into
    // several internal nodes is one parameter that fans out inside.
    // Outlets key on the internal SOURCE port: one internal node feeding two
    // external nodes is one return value with two parent cables. (The Swift
    // SubpatchEncapsulator keys outlets on the external destination instead,
    // which splits that case into two identical outlets.)
    let inlets = group_ports(
        &crossing_in,
        |connection| OutputPortRef {
            node_id: connection.from_node.clone(),
            output_index: connection.from_output,
        },
        &nodes_by_id,
        PortSide::Inlet,
    )
    .into_iter()
    .map(|(external_source, connections)| PlannedInlet {
        external_source,
        internal_destinations: connections
            .iter()
            .map(|connection| InputPortRef {
                node_id: connection.to_node.clone(),
                input_index: connection.to_input,
            })
            .collect(),
    })
    .collect::<Vec<_>>();

    let mut outlets = group_ports(
        &crossing_out,
        |connection| OutputPortRef {
            node_id: connection.from_node.clone(),
            output_index: connection.from_output,
        },
        &nodes_by_id,
        PortSide::Outlet,
    )
    .into_iter()
    .map(|(internal_source, connections)| PlannedOutlet {
        internal_source,
        external_destinations: connections
            .iter()
            .map(|connection| InputPortRef {
                node_id: connection.to_node.clone(),
                input_index: connection.to_input,
            })
            .collect(),
    })
    .collect::<Vec<_>>();

    // A macro must return something. With nothing leaving the selection, take
    // the terminal internal node so the body is still valid; the instance is
    // simply left unconnected.
    if outlets.is_empty() {
        let consumed = internal
            .iter()
            .map(|connection| connection.from_node.as_str())
            .collect::<HashSet<_>>();
        let terminal = selection
            .iter()
            .find(|node| !consumed.contains(node.id.as_str()))
            .or_else(|| selection.last())
            .expect("selection is non-empty");
        outlets.push(PlannedOutlet {
            internal_source: OutputPortRef {
                node_id: terminal.id.clone(),
                output_index: 0,
            },
            external_destinations: Vec::new(),
        });
    }

    // --- lay the body out ---------------------------------------------------
    let origin = selection_origin(&selection);
    let inbound_slots = inbound_slot_index(patch);
    let body_height = selection
        .iter()
        .map(|node| node.position.1 - origin.1)
        .fold(0.0_f32, f32::max);

    let mut body_nodes = Vec::new();
    let mut body_cables = Vec::new();

    for (index, _) in inlets.iter().enumerate() {
        body_nodes.push(PlannedNode {
            key: BodyKey::Inlet(index),
            // The `@name` is what the generator's `macro_param_name` reads for
            // the `(defmacro …)` parameter list. Without it the name falls back
            // to the node's output name `out`, which is reserved and comes out
            // of `claim_unique` as `(defmacro sub1 (out-2) …)`.
            text: format!("in {channel} @name input{channel}", channel = index + 1),
            position: (
                BODY_INSET_CELLS.0 + index as f32 * INTERFACE_SPACING_CELLS,
                BODY_INSET_CELLS.1,
            ),
            width: None,
        });
    }
    for (index, _) in outlets.iter().enumerate() {
        body_nodes.push(PlannedNode {
            key: BodyKey::Outlet(index),
            text: format!("out {}", index + 1),
            position: (
                BODY_INSET_CELLS.0 + index as f32 * INTERFACE_SPACING_CELLS,
                BODY_INSET_CELLS.1 + INTERFACE_ROW_GAP_CELLS * 2.0 + body_height,
            ),
            width: None,
        });
    }

    for node in &selection {
        let rebased = (
            node.position.0 - origin.0 + BODY_INSET_CELLS.0,
            node.position.1 - origin.1 + BODY_INSET_CELLS.1 + INTERFACE_ROW_GAP_CELLS,
        );
        let text = encapsulated_node_text(node, &inbound_slots);
        if let Some(hoisted) = text.hoisted_constant {
            body_nodes.push(PlannedNode {
                key: BodyKey::HoistedConstant(node.id.clone()),
                text: hoisted,
                position: (rebased.0, rebased.1 - INTERFACE_ROW_GAP_CELLS * 0.5),
                width: None,
            });
            body_cables.push(PlannedCable {
                from: BodyKey::HoistedConstant(node.id.clone()),
                from_output: 0,
                to: BodyKey::Moved(node.id.clone()),
                to_input: 0,
            });
        }
        body_nodes.push(PlannedNode {
            key: BodyKey::Moved(node.id.clone()),
            text: text.text,
            position: rebased,
            width: node.width,
        });
    }

    for connection in &internal {
        body_cables.push(PlannedCable {
            from: BodyKey::Moved(connection.from_node.clone()),
            from_output: connection.from_output,
            to: BodyKey::Moved(connection.to_node.clone()),
            to_input: connection.to_input,
        });
    }
    for (index, inlet) in inlets.iter().enumerate() {
        for destination in &inlet.internal_destinations {
            body_cables.push(PlannedCable {
                from: BodyKey::Inlet(index),
                from_output: 0,
                to: BodyKey::Moved(destination.node_id.clone()),
                to_input: destination.input_index,
            });
        }
    }
    for (index, outlet) in outlets.iter().enumerate() {
        body_cables.push(PlannedCable {
            from: BodyKey::Moved(outlet.internal_source.node_id.clone()),
            from_output: outlet.internal_source.output_index,
            to: BodyKey::Outlet(index),
            to_input: 0,
        });
    }

    Ok(EncapsulationPlan {
        macro_name,
        instance_position: origin,
        body_nodes,
        body_cables,
        inlets,
        outlets,
        removed_node_ids: selection.iter().map(|node| node.id.clone()).collect(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortSide {
    Inlet,
    Outlet,
}

/// Group crossing cables into ports keyed by `key_of`, ordering the ports the
/// way the canvas reads: by the position of the node on the *inside* of the
/// boundary, then by slot, then by node id.
///
/// The ordering must be a pure function of the model — the generator's whole
/// contract is deterministic regeneration — so it can never fall out of
/// `patch.connections` iteration order the way the Swift original's does.
fn group_ports<F>(
    connections: &[&PatchConnection],
    key_of: F,
    nodes_by_id: &HashMap<&str, &PatchNode>,
    side: PortSide,
) -> Vec<(OutputPortRef, Vec<PatchConnection>)>
where
    F: Fn(&PatchConnection) -> OutputPortRef,
{
    let mut grouped: BTreeMap<(String, usize), Vec<PatchConnection>> = BTreeMap::new();
    let mut key_refs: HashMap<(String, usize), OutputPortRef> = HashMap::new();
    for connection in connections {
        let port = key_of(connection);
        let map_key = (port.node_id.clone(), port.output_index);
        key_refs.entry(map_key.clone()).or_insert(port);
        grouped
            .entry(map_key)
            .or_default()
            .push((*connection).clone());
    }

    let mut ports = grouped.into_iter().collect::<Vec<_>>();
    ports.sort_by(|(left_key, left), (right_key, right)| {
        port_sort_key(left, nodes_by_id, side)
            .cmp(&port_sort_key(right, nodes_by_id, side))
            .then_with(|| left_key.cmp(right_key))
    });
    ports
        .into_iter()
        .map(|(map_key, mut connections)| {
            connections.sort_by(|left, right| {
                (
                    left.to_node.as_str(),
                    left.to_input,
                    left.from_node.as_str(),
                    left.from_output,
                )
                    .cmp(&(
                        right.to_node.as_str(),
                        right.to_input,
                        right.from_node.as_str(),
                        right.from_output,
                    ))
            });
            (
                key_refs.remove(&map_key).expect("key recorded"),
                connections,
            )
        })
        .collect()
}

/// Sort key for one port group: the canvas position of the node on the inside
/// of the boundary — the destination for inlets, the source for outlets.
fn port_sort_key(
    connections: &[PatchConnection],
    nodes_by_id: &HashMap<&str, &PatchNode>,
    side: PortSide,
) -> (i64, i64, usize, String) {
    connections
        .iter()
        .map(|connection| {
            let (node_id, slot) = match side {
                PortSide::Inlet => (connection.to_node.as_str(), connection.to_input),
                PortSide::Outlet => (connection.from_node.as_str(), connection.from_output),
            };
            let position = nodes_by_id
                .get(node_id)
                .map(|node| node.position)
                .unwrap_or((0.0, 0.0));
            (
                quantize(position.1),
                quantize(position.0),
                slot,
                node_id.to_string(),
            )
        })
        .min()
        .unwrap_or((0, 0, 0, String::new()))
}

/// Positions are floats a drag can leave at 12.499999; rounding to half cells
/// keeps the ordering stable across saves without collapsing distinct rows.
fn quantize(value: f32) -> i64 {
    (value * 2.0).round() as i64
}

fn node_order_key(node: &PatchNode) -> (i64, i64, String) {
    (
        quantize(node.position.1),
        quantize(node.position.0),
        node.id.clone(),
    )
}

fn selection_origin(selection: &[&PatchNode]) -> (f32, f32) {
    let min_x = selection
        .iter()
        .map(|node| node.position.0)
        .fold(f32::INFINITY, f32::min);
    let min_y = selection
        .iter()
        .map(|node| node.position.1)
        .fold(f32::INFINITY, f32::min);
    (
        if min_x.is_finite() { min_x } else { 0.0 },
        if min_y.is_finite() { min_y } else { 0.0 },
    )
}

fn inbound_slot_index(patch: &Patch) -> HashSet<(String, usize)> {
    patch
        .connections
        .iter()
        .map(|connection| (connection.to_node.clone(), connection.to_input))
        .collect()
}

struct EncapsulatedNodeText {
    text: String,
    /// Slot 0 held a plain literal. A created node's arguments always start
    /// with an implicit cable slot (`node_from_editor_text` places inline args
    /// at index+1), so that literal cannot survive as text — it is hoisted
    /// into its own constant node wired into slot 0, exactly the way the
    /// projector already normalizes a source-level `(- 5 x)`.
    hoisted_constant: Option<String>,
}

/// Editor text for a node being moved into the macro, chosen so the recreated
/// node's argument indices match the original's one for one.
///
/// `node_display_label` alone is not enough: it drops connected slots that sit
/// past the last literal, so a two-cable `(- a b)` renders as bare `-` and the
/// recreated node has a single input slot — the second cable would be dropped
/// at generation. Every slot from 1 up to the highest one that carries a
/// literal or a cable therefore gets an explicit token, `?` standing in for a
/// cable.
fn encapsulated_node_text(
    node: &PatchNode,
    inbound_slots: &HashSet<(String, usize)>,
) -> EncapsulatedNodeText {
    if !matches!(node.kind, NodeKind::Builtin | NodeKind::MacroInstance) {
        // Constants carry no input slots, and a history's single slot is its
        // implicit `write-history` cable — both round-trip through the display
        // label unchanged. (A history's `op` is `make-history`; its editable
        // header is `history`.)
        return EncapsulatedNodeText {
            text: node_display_label(node),
            hoisted_constant: None,
        };
    }

    let is_cabled = |index: usize| inbound_slots.contains(&(node.id.clone(), index));
    let literal_at = |index: usize| -> Option<String> {
        match node.args.get(index) {
            Some(ArgValue::Literal(value))
                if value != "<expr>" && value != MISSING_INPUT_SENTINEL =>
            {
                Some(value.clone())
            }
            _ => None,
        }
    };
    // An inline `gain~` / `gain` presentation is backed by a real cable, so the
    // slot is a cable slot here and the accessor node stays in the enclosing
    // scope as an ordinary macro parameter source.
    let is_inline = |index: usize| {
        node.inline_inputs
            .get(index)
            .and_then(|input| input.as_ref())
            .is_some()
    };

    let highest_used = (0..node.args.len())
        .filter(|index| is_cabled(*index) || is_inline(*index) || literal_at(*index).is_some())
        .next_back();

    let hoisted_constant = (!is_cabled(0) && !is_inline(0))
        .then(|| literal_at(0))
        .flatten();

    let mut text = node.op.clone();
    if let Some(highest_used) = highest_used {
        for index in 1..=highest_used {
            text.push(' ');
            match literal_at(index) {
                Some(value) if !is_cabled(index) && !is_inline(index) => text.push_str(&value),
                _ => text.push('?'),
            }
        }
    }

    EncapsulatedNodeText {
        text,
        hoisted_constant,
    }
}

/// A macro instance is one atomic call, so a path
/// `selected -> external -> selected` collapses into a genuine cycle that the
/// generator's topological ordering cannot emit. Feedback edges are the
/// `write-history` back-edges and are excluded by design: they are how a
/// legal cycle is expressed.
fn check_convexity(
    patch: &Patch,
    selected_ids: &HashSet<String>,
) -> Result<(), EncapsulationRefusal> {
    let mut forward: HashMap<&str, Vec<&str>> = HashMap::new();
    for connection in &patch.connections {
        if connection.kind == ConnectionKind::Feedback {
            continue;
        }
        forward
            .entry(connection.from_node.as_str())
            .or_default()
            .push(connection.to_node.as_str());
    }

    // Walk forward from the selection through external nodes only. Reaching a
    // selected node again means an external detour re-enters the selection.
    let mut stack = selected_ids
        .iter()
        .flat_map(|id| forward.get(id.as_str()).into_iter().flatten().copied())
        .filter(|id| !selected_ids.contains(*id))
        .collect::<Vec<_>>();
    let mut seen = stack.iter().copied().collect::<HashSet<_>>();
    while let Some(current) = stack.pop() {
        for next in forward.get(current).into_iter().flatten().copied() {
            if selected_ids.contains(next) {
                return Err(EncapsulationRefusal::NotConvex);
            }
            if seen.insert(next) {
                stack.push(next);
            }
        }
    }
    Ok(())
}

/// A history node carries state across the sample boundary: its feedback
/// in-edge is the `write-history`, its output the `read-history`. Moving one
/// into the macro is fine (macros own per-expansion histories — see
/// `latch_on_trigger` in instruments/core/triton/dsp.lisp), and so is leaving
/// one outside. What cannot work is a history inside the selection that is
/// *both* written from outside and read from outside: the state would have to
/// cross the instance boundary in both directions.
fn check_history_boundaries(
    patch: &Patch,
    selection: &[&PatchNode],
    selected_ids: &HashSet<String>,
) -> Result<(), EncapsulationRefusal> {
    for node in selection {
        if node.kind != NodeKind::History {
            continue;
        }
        let written_from_outside = patch.connections.iter().any(|connection| {
            connection.to_node == node.id && !selected_ids.contains(&connection.from_node)
        });
        let read_from_outside = patch.connections.iter().any(|connection| {
            connection.from_node == node.id && !selected_ids.contains(&connection.to_node)
        });
        if written_from_outside && read_from_outside {
            return Err(EncapsulationRefusal::HistoryStraddlesBoundary);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Materialization
// ---------------------------------------------------------------------------

/// Apply `plan` to the interaction state: build the macro body in the
/// `macro:<name>` view, drop the selection from `view_key`, and drop in the
/// instance node wired to the ports the plan allocated. Returns the instance
/// node id.
pub(super) fn apply_encapsulation_plan(
    plan: &EncapsulationPlan,
    state: &mut PatcherInteractionState,
    view_key: &str,
    taken_node_ids: &HashSet<String>,
) -> String {
    let macro_view_key = format!("macro:{}", plan.macro_name);

    let mut body_ids: HashMap<BodyKey, String> = HashMap::new();
    for planned in &plan.body_nodes {
        let created_id = allocate_created_node_avoiding(
            state,
            &macro_view_key,
            planned.position,
            &HashSet::new(),
        );
        if let Some(edit) = state
            .edit_state
            .nodes
            .get_mut(&node_edit_key(&macro_view_key, &created_id))
        {
            edit.text = planned.text.clone();
            edit.width = planned.width;
        }
        body_ids.insert(planned.key.clone(), created_id);
    }

    state.edit_state.created_macros.insert(
        plan.macro_name.clone(),
        PatcherMacroEdit {
            name: plan.macro_name.clone(),
            // Placeholder until the instance node exists; corrected below.
            instance_node_id: String::new(),
            source: Some(empty_created_macro_source(&plan.macro_name)),
        },
    );

    for cable in &plan.body_cables {
        let (Some(from), Some(to)) = (body_ids.get(&cable.from), body_ids.get(&cable.to)) else {
            continue;
        };
        allocate_created_connection(
            state,
            &macro_view_key,
            OutputPortRef {
                node_id: from.clone(),
                output_index: cable.from_output,
            },
            InputPortRef {
                node_id: to.clone(),
                input_index: cable.to_input,
            },
        );
    }

    // Drop the selection from the enclosing view. Source-backed cables into or
    // out of it need no explicit deletion: `patch_with_interaction_state`
    // prunes any connection whose endpoint is no longer live.
    state.selected_nodes = plan.removed_node_ids.iter().cloned().collect();
    delete_selected_nodes(state, view_key);

    let instance_id =
        allocate_created_node_avoiding(state, view_key, plan.instance_position, taken_node_ids);
    if let Some(edit) = state
        .edit_state
        .nodes
        .get_mut(&node_edit_key(view_key, &instance_id))
    {
        edit.text = plan.macro_name.clone();
    }
    if let Some(macro_edit) = state.edit_state.created_macros.get_mut(&plan.macro_name) {
        macro_edit.instance_node_id = instance_id.clone();
    }

    for (index, inlet) in plan.inlets.iter().enumerate() {
        allocate_created_connection(
            state,
            view_key,
            inlet.external_source.clone(),
            InputPortRef {
                node_id: instance_id.clone(),
                input_index: index,
            },
        );
    }
    for (index, outlet) in plan.outlets.iter().enumerate() {
        for destination in &outlet.external_destinations {
            allocate_created_connection(
                state,
                view_key,
                OutputPortRef {
                    node_id: instance_id.clone(),
                    output_index: index,
                },
                destination.clone(),
            );
        }
    }

    note_touched_node(state, &instance_id);
    bring_nodes_to_front(state, view_key, std::slice::from_ref(&instance_id));
    state.selected_nodes = std::iter::once(instance_id.clone()).collect();
    state.selected_cable = None;
    instance_id
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Cmd+E. Returns true when the selection was encapsulated; a refusal leaves
/// the interaction state untouched.
pub(super) fn encapsulate_patcher_selection(
    node: &crate::layout::LayoutNode,
    state: &mut PatcherInteractionState,
    view_key: &str,
) -> bool {
    encapsulate_patcher_selection_result(node, state, view_key).is_ok()
}

pub(super) fn encapsulate_patcher_selection_result(
    node: &crate::layout::LayoutNode,
    state: &mut PatcherInteractionState,
    view_key: &str,
) -> Result<String, EncapsulationRefusal> {
    let Ok((_, root_patch)) = super::load_patch_from_props(&node.props) else {
        return Err(EncapsulationRefusal::EmptySelection);
    };
    let patch = super::active_patcher_patch(&root_patch, state);
    let patch = super::patch_with_interaction_state(patch, state, view_key);

    let taken_names = super::autocomplete_macros_for_patch(&node.props, Some(&patch))
        .into_iter()
        .map(|macro_patch| macro_patch.name)
        .chain(state.edit_state.created_macros.keys().cloned())
        .collect::<HashSet<_>>();
    let macro_name = next_encapsulated_macro_name(&taken_names)?;

    let selected = state.selected_nodes.clone();
    let plan = plan_encapsulation(&patch, &selected, macro_name)?;

    let taken_node_ids = patch
        .nodes
        .iter()
        .map(|patch_node| patch_node.id.clone())
        .collect::<HashSet<_>>();
    let instance_id = apply_encapsulation_plan(&plan, state, view_key, &taken_node_ids);
    super::debug_log_edit_event(
        &format!(
            "encapsulate view={view_key} macro={} nodes={} inlets={} outlets={}",
            plan.macro_name,
            plan.removed_node_ids.len(),
            plan.inlets.len(),
            plan.outlets.len()
        ),
        state,
    );
    Ok(instance_id)
}

/// Retyping a created macro instance's header renames the macro rather than
/// turning the node into a call to an operator that does not exist. This is
/// how an encapsulated `sub1` gets a real name: double-click it, type, commit.
///
/// `previous_text` is the header as it stood before the edit — the macro name
/// we are renaming *from*.
pub(super) fn rename_created_macro_from_instance_text(
    node: &crate::layout::LayoutNode,
    state: &mut PatcherInteractionState,
    view_key: &str,
    node_id: &str,
    previous_text: &str,
) -> bool {
    let old = previous_text.trim();
    let Some(macro_edit) = state.edit_state.created_macros.get(old) else {
        return false;
    };
    if macro_edit.instance_node_id != node_id {
        return false;
    }
    let Some(edit) = state
        .edit_state
        .nodes
        .get(&node_edit_key(view_key, node_id))
    else {
        return false;
    };
    let new = edit.text.trim().to_string();
    if new == old || !super::interaction::is_valid_created_macro_name(&new) {
        return false;
    }
    let patch = super::load_patch_from_props(&node.props)
        .ok()
        .map(|(_, root_patch)| root_patch);
    let taken_names = super::autocomplete_macros_for_patch(&node.props, patch.as_ref())
        .into_iter()
        .map(|macro_patch| macro_patch.name)
        .chain(state.edit_state.created_macros.keys().cloned())
        .chain(super::project::dgenlisp_operator_names().iter().cloned())
        .collect::<HashSet<_>>();
    super::state::rename_created_macro(state, old, &new, &taken_names)
}

/// First free `sub1`, `sub2`, … Names must not collide with an existing macro
/// (local, created, or imported from the defmacro library) or with anything
/// the generator reserves.
pub(super) fn next_encapsulated_macro_name(
    taken: &HashSet<String>,
) -> Result<String, EncapsulationRefusal> {
    (1..=999)
        .map(|index| format!("sub{index}"))
        .find(|candidate| !taken.contains(candidate))
        .ok_or(EncapsulationRefusal::NoAvailableName)
}
