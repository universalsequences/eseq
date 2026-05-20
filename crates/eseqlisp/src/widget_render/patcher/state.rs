use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use super::super::bump_widget_state_generation;
use super::super::text_input::TextInputState;
use crate::layout::LayoutNode;
use crate::vm::Value;

use super::lisp::{editor_node_port_shape, node_kind_for_op, parse_editor_node_text};
use super::metrics::{
    DEFAULT_ZOOM, MAX_ZOOM, MIN_ZOOM, PAN_OVERSCROLL_MIN_CELLS, PAN_OVERSCROLL_VIEWPORT_FACTOR,
};
use super::model::{
    ArgValue, CableEndpoint, CableSegmentInfo, ConnectionKind, InputPortRef, NodeKind,
    OutputPortRef, Patch, PatchConnection, PatchNode,
};
use super::project::dgenlisp_operator_names;
use super::prop_str;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PatcherPanState {
    pub(super) offset_x: f32,
    pub(super) offset_y: f32,
    pub(super) zoom: f32,
    pub(super) content_width: f32,
    pub(super) content_height: f32,
    pub(super) viewport_width: f32,
    pub(super) viewport_height: f32,
}

impl Default for PatcherPanState {
    fn default() -> Self {
        Self {
            offset_x: 0.0,
            offset_y: 0.0,
            zoom: DEFAULT_ZOOM,
            content_width: 0.0,
            content_height: 0.0,
            viewport_width: 0.0,
            viewport_height: 0.0,
        }
    }
}

thread_local! {
    static PATCHER_PAN_STATES: RefCell<HashMap<u64, PatcherPanState>> =
        RefCell::new(HashMap::new());
    static PATCHER_INTERACTION_STATES: RefCell<HashMap<u64, PatcherInteractionState>> =
        RefCell::new(HashMap::new());
}

pub(super) fn patcher_state_key(node: &LayoutNode) -> u64 {
    patcher_state_key_from_parts(node.stable_widget_id, &node.props)
}

pub(super) fn patcher_state_key_from_parts(
    stable_widget_id: Option<u64>,
    props: &HashMap<String, Value>,
) -> u64 {
    stable_widget_id.unwrap_or_else(|| {
        let mut hasher = DefaultHasher::new();
        "patcher".hash(&mut hasher);
        prop_str(props, "path").hash(&mut hasher);
        prop_str(props, "file").hash(&mut hasher);
        prop_str(props, "intent").hash(&mut hasher);
        hasher.finish()
    })
}

pub(super) fn get_patcher_pan_state(key: u64) -> PatcherPanState {
    PATCHER_PAN_STATES.with(|states| states.borrow().get(&key).cloned().unwrap_or_default())
}

pub(super) fn set_patcher_pan_state(key: u64, mut state: PatcherPanState) {
    clamp_patcher_pan_state(&mut state);
    let changed = PATCHER_PAN_STATES.with(|states| {
        let mut states = states.borrow_mut();
        let old = states.insert(key, state.clone());
        old.as_ref() != Some(&state)
    });
    if changed {
        bump_widget_state_generation();
    }
}

pub(super) fn clamp_patcher_pan_state(state: &mut PatcherPanState) {
    state.zoom = state.zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    let max_x = (state.content_width - state.viewport_width).max(0.0);
    let max_y = (state.content_height - state.viewport_height).max(0.0);
    let overscroll_x =
        (state.viewport_width * PAN_OVERSCROLL_VIEWPORT_FACTOR).max(PAN_OVERSCROLL_MIN_CELLS);
    let overscroll_y =
        (state.viewport_height * PAN_OVERSCROLL_VIEWPORT_FACTOR).max(PAN_OVERSCROLL_MIN_CELLS);
    state.offset_x = state.offset_x.clamp(-overscroll_x, max_x + overscroll_x);
    state.offset_y = state.offset_y.clamp(-overscroll_y, max_y + overscroll_y);
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct PatcherInteractionState {
    pub(super) selected_nodes: HashSet<String>,
    pub(super) hovered_node: Option<String>,
    pub(super) hovered_macro_drill_in: Option<String>,
    pub(super) hover_back_button: bool,
    pub(super) selected_cable: Option<String>,
    pub(super) edit_state: PatchEditState,
    pub(super) text_edit: Option<PatcherTextEdit>,
    pub(super) active_macro: Option<String>,
    pub(super) drag: Option<PatcherDragState>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct PatchEditState {
    pub(super) nodes: HashMap<String, PatcherNodeEdit>,
    pub(super) deleted_nodes: HashSet<String>,
    pub(super) connections: HashMap<String, PatcherConnectionEdit>,
    pub(super) deleted_connections: HashSet<String>,
    pub(super) next_created_node: u64,
    pub(super) next_created_connection: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PatcherNodeEdit {
    pub(super) view_key: String,
    pub(super) id: String,
    pub(super) origin: PatcherNodeOrigin,
    pub(super) text: String,
    pub(super) position: (f32, f32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PatcherNodeOrigin {
    Source { source_node_id: String },
    Created { created_id: String },
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PatcherConnectionEdit {
    pub(super) view_key: String,
    pub(super) id: String,
    pub(super) origin: PatcherConnectionOrigin,
    pub(super) from: OutputPortRef,
    pub(super) to: InputPortRef,
    pub(super) kind: ConnectionKind,
    pub(super) segment: Option<CableSegmentInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PatcherConnectionOrigin {
    Source { source_connection_id: String },
    Created { created_id: String },
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PatcherTextEdit {
    pub(super) node_id: String,
    pub(super) text: String,
    pub(super) original_text: String,
    pub(super) state: TextInputState,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PatcherDragState {
    Nodes {
        start_col: f32,
        start_row: f32,
        start_positions: HashMap<String, (f32, f32)>,
    },
    Marquee {
        start_col: f32,
        start_row: f32,
        current_col: f32,
        current_row: f32,
        base_selection: HashSet<String>,
    },
    Cable {
        from: OutputPortRef,
        start_col: f32,
        start_row: f32,
        current_col: f32,
        current_row: f32,
        target: Option<InputPortRef>,
    },
    CableEndpoint {
        cable_id: String,
        endpoint: CableEndpoint,
        original_from: OutputPortRef,
        original_to: InputPortRef,
        start_col: f32,
        start_row: f32,
        end_col: f32,
        end_row: f32,
        current_col: f32,
        current_row: f32,
        target_from: Option<OutputPortRef>,
        target_to: Option<InputPortRef>,
    },
    CableSegment {
        cable_id: String,
        start_col: f32,
        start_row: f32,
        end_col: f32,
        end_row: f32,
    },
}

pub(super) fn get_patcher_interaction_state(key: u64) -> PatcherInteractionState {
    PATCHER_INTERACTION_STATES.with(|states| states.borrow().get(&key).cloned().unwrap_or_default())
}

pub(super) fn set_patcher_interaction_state(key: u64, state: PatcherInteractionState) {
    let changed = PATCHER_INTERACTION_STATES.with(|states| {
        let mut states = states.borrow_mut();
        let old = states.insert(key, state.clone());
        old.as_ref() != Some(&state)
    });
    if changed {
        bump_widget_state_generation();
    }
}

pub(super) fn active_patcher_view_key(interaction_state: &PatcherInteractionState) -> String {
    interaction_state
        .active_macro
        .as_deref()
        .map(|name| format!("macro:{name}"))
        .unwrap_or_else(|| "root".to_string())
}

pub(super) fn scoped_node_key(view_key: &str, node_id: &str) -> String {
    format!("{view_key}::{node_id}")
}

pub(super) fn node_edit_key(view_key: &str, node_id: &str) -> String {
    scoped_node_key(view_key, node_id)
}

pub(super) fn source_connection_id(connection: &PatchConnection) -> String {
    connection_id_from_ports(
        &OutputPortRef {
            node_id: connection.from_node.clone(),
            output_index: connection.from_output,
        },
        &InputPortRef {
            node_id: connection.to_node.clone(),
            input_index: connection.to_input,
        },
    )
}

pub(super) fn connection_id_from_ports(from: &OutputPortRef, to: &InputPortRef) -> String {
    format!(
        "{}:{}->{}:{}",
        from.node_id, from.output_index, to.node_id, to.input_index
    )
}

pub(super) fn connection_edit_key(view_key: &str, connection_id: &str) -> String {
    format!("{view_key}::{connection_id}")
}

pub(super) fn allocate_created_node(
    state: &mut PatcherInteractionState,
    view_key: &str,
    position: (f32, f32),
) -> String {
    let id = format!("created-{}", state.edit_state.next_created_node);
    state.edit_state.next_created_node += 1;
    state.edit_state.nodes.insert(
        node_edit_key(view_key, &id),
        PatcherNodeEdit {
            view_key: view_key.to_string(),
            id: id.clone(),
            origin: PatcherNodeOrigin::Created {
                created_id: id.clone(),
            },
            text: String::new(),
            position,
        },
    );
    id
}

pub(super) fn allocate_created_connection(
    state: &mut PatcherInteractionState,
    view_key: &str,
    from: OutputPortRef,
    to: InputPortRef,
) -> String {
    let id = format!("created-cable-{}", state.edit_state.next_created_connection);
    state.edit_state.next_created_connection += 1;
    state.edit_state.connections.insert(
        format!("{view_key}::{id}"),
        PatcherConnectionEdit {
            view_key: view_key.to_string(),
            id: id.clone(),
            origin: PatcherConnectionOrigin::Created {
                created_id: id.clone(),
            },
            from,
            to,
            kind: ConnectionKind::Forward,
            segment: None,
        },
    );
    id
}

pub(super) fn set_connection_segment_edit(
    state: &mut PatcherInteractionState,
    view_key: &str,
    connection: &PatchConnection,
    segment: Option<CableSegmentInfo>,
) {
    let cable_id = source_connection_id(connection);
    let existing_key = state.edit_state.connections.iter().find_map(|(key, edit)| {
        let edit_cable_id = connection_id_from_ports(&edit.from, &edit.to);
        (edit.view_key == view_key && edit_cable_id == cable_id).then(|| key.clone())
    });
    if let Some(key) = existing_key {
        if let Some(edit) = state.edit_state.connections.get_mut(&key) {
            edit.segment = segment;
        }
        return;
    }
    state.edit_state.connections.insert(
        connection_edit_key(view_key, &cable_id),
        PatcherConnectionEdit {
            view_key: view_key.to_string(),
            id: cable_id,
            origin: PatcherConnectionOrigin::Source {
                source_connection_id: source_connection_id(connection),
            },
            from: OutputPortRef {
                node_id: connection.from_node.clone(),
                output_index: connection.from_output,
            },
            to: InputPortRef {
                node_id: connection.to_node.clone(),
                input_index: connection.to_input,
            },
            kind: connection.kind,
            segment,
        },
    );
}

pub(super) fn delete_connection_edit_or_mark_deleted(
    state: &mut PatcherInteractionState,
    view_key: &str,
    cable_id: &str,
) -> bool {
    let created_key = state.edit_state.connections.iter().find_map(|(key, edit)| {
        let edit_cable_id = connection_id_from_ports(&edit.from, &edit.to);
        (edit.view_key == view_key
            && edit_cable_id == cable_id
            && matches!(edit.origin, PatcherConnectionOrigin::Created { .. }))
        .then(|| key.clone())
    });

    let changed = if let Some(key) = created_key {
        state.edit_state.connections.remove(&key).is_some()
    } else {
        state
            .edit_state
            .connections
            .remove(&connection_edit_key(view_key, cable_id));
        state
            .edit_state
            .deleted_connections
            .insert(connection_edit_key(view_key, cable_id))
    };

    if state.selected_cable.as_deref() == Some(cable_id) {
        state.selected_cable = None;
    }
    changed
}

pub(super) fn delete_selected_nodes(state: &mut PatcherInteractionState, view_key: &str) -> bool {
    if state.selected_nodes.is_empty() {
        return false;
    }

    let selected_nodes = state.selected_nodes.clone();
    for node_id in &selected_nodes {
        let edit_key = node_edit_key(view_key, node_id);
        let removed_created_node = state
            .edit_state
            .nodes
            .get(&edit_key)
            .is_some_and(|edit| matches!(edit.origin, PatcherNodeOrigin::Created { .. }));
        if removed_created_node {
            state.edit_state.nodes.remove(&edit_key);
        } else {
            state.edit_state.nodes.remove(&edit_key);
            state.edit_state.deleted_nodes.insert(edit_key);
        }
    }

    state.edit_state.connections.retain(|_, edit| {
        edit.view_key != view_key
            || !(selected_nodes.contains(&edit.from.node_id)
                || selected_nodes.contains(&edit.to.node_id))
    });

    state.selected_nodes.clear();
    state.selected_cable = None;
    state.drag = None;
    state.text_edit = None;
    true
}

pub(super) fn ensure_source_node_edit(
    state: &mut PatcherInteractionState,
    view_key: &str,
    node: &PatchNode,
    text: String,
) {
    let key = node_edit_key(view_key, &node.id);
    state
        .edit_state
        .nodes
        .entry(key)
        .or_insert(PatcherNodeEdit {
            view_key: view_key.to_string(),
            id: node.id.clone(),
            origin: PatcherNodeOrigin::Source {
                source_node_id: node.id.clone(),
            },
            text,
            position: node.position,
        });
}

pub(super) fn set_node_edit_position(
    state: &mut PatcherInteractionState,
    view_key: &str,
    node: &PatchNode,
    position: (f32, f32),
    text: String,
) {
    ensure_source_node_edit(state, view_key, node, text);
    if let Some(edit) = state
        .edit_state
        .nodes
        .get_mut(&node_edit_key(view_key, &node.id))
    {
        edit.position = position;
    }
}

pub(super) fn active_patcher_patch(
    root_patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> Patch {
    interaction_state
        .active_macro
        .as_deref()
        .and_then(|name| {
            root_patch
                .macros
                .iter()
                .find(|macro_patch| macro_patch.name == name)
                .map(|macro_patch| macro_patch.patch.clone())
        })
        .unwrap_or_else(|| root_patch.clone())
}

pub(super) fn patcher_breadcrumb(
    path: &std::path::Path,
    interaction_state: &PatcherInteractionState,
) -> String {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("dsp.lisp");
    match interaction_state.active_macro.as_deref() {
        Some(name) => format!("root / {file_name} / {name}"),
        None => format!("root / {file_name}"),
    }
}

pub(super) fn patcher_back_label(
    interaction_state: &PatcherInteractionState,
) -> Option<&'static str> {
    interaction_state.active_macro.as_ref().map(|_| "<")
}

pub(super) fn patch_with_interaction_state(
    mut patch: Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
) -> Patch {
    let macro_arities = patch
        .macros
        .iter()
        .map(|macro_patch| (macro_patch.name.clone(), macro_patch.params.len()))
        .collect::<HashMap<_, _>>();
    patch.nodes.retain(|node| {
        !interaction_state
            .edit_state
            .deleted_nodes
            .contains(&node_edit_key(view_key, &node.id))
    });
    patch.connections.retain(|connection| {
        !interaction_state
            .edit_state
            .deleted_connections
            .contains(&connection_edit_key(
                view_key,
                &source_connection_id(connection),
            ))
    });
    let live_nodes = patch
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    patch.connections.retain(|connection| {
        live_nodes.contains(connection.from_node.as_str())
            && live_nodes.contains(connection.to_node.as_str())
    });
    for node in &mut patch.nodes {
        let edit_key = node_edit_key(view_key, &node.id);
        if let Some(edit) = interaction_state.edit_state.nodes.get(&edit_key) {
            node.position = edit.position;
            apply_node_text_override(node, &edit.text, &macro_arities);
        }
        if let Some(edit) = interaction_state
            .text_edit
            .as_ref()
            .filter(|edit| edit.node_id == node.id)
        {
            apply_node_text_override(node, &edit.text, &macro_arities);
        }
    }
    for connection in &mut patch.connections {
        let edit_key = connection_edit_key(view_key, &source_connection_id(connection));
        if let Some(edit) = interaction_state.edit_state.connections.get(&edit_key)
            && matches!(edit.origin, PatcherConnectionOrigin::Source { .. })
        {
            connection.segment = edit.segment;
        }
    }
    for edit in interaction_state
        .edit_state
        .nodes
        .values()
        .filter(|edit| edit.view_key == view_key)
        .filter(|edit| matches!(edit.origin, PatcherNodeOrigin::Created { .. }))
    {
        let mut text = edit.text.clone();
        if let Some(edit) = interaction_state
            .text_edit
            .as_ref()
            .filter(|text_edit| text_edit.node_id == edit.id)
        {
            text = edit.text.clone();
        }
        patch.nodes.push(node_from_editor_text(
            &edit.id,
            &text,
            edit.position,
            &macro_arities,
            interaction_state
                .text_edit
                .as_ref()
                .is_some_and(|text_edit| text_edit.node_id == edit.id),
        ));
    }
    patch.connections.extend(
        interaction_state
            .edit_state
            .connections
            .values()
            .filter(|edit| edit.view_key == view_key)
            .filter(|edit| matches!(edit.origin, PatcherConnectionOrigin::Created { .. }))
            .map(|edit| PatchConnection {
                from_node: edit.from.node_id.clone(),
                from_output: edit.from.output_index,
                to_node: edit.to.node_id.clone(),
                to_input: edit.to.input_index,
                kind: edit.kind,
                segment: edit.segment,
                source: None,
            }),
    );
    patch
}

fn apply_node_text_override(
    node: &mut PatchNode,
    text: &str,
    macro_arities: &HashMap<String, usize>,
) {
    let edited = node_from_editor_text(&node.id, text, node.position, macro_arities, false);
    node.op = edited.op;
    node.kind = edited.kind;
    node.label = edited.label;
    node.args = edited.args;
    node.outputs = edited.outputs;
    node.diagnostic = edited.diagnostic;
}

pub(super) fn node_from_editor_text(
    id: &str,
    text: &str,
    position: (f32, f32),
    macro_arities: &HashMap<String, usize>,
    is_editing: bool,
) -> PatchNode {
    let trimmed = text.trim();
    if trimmed.is_empty() || is_editing {
        return PatchNode {
            id: id.to_string(),
            op: trimmed.to_string(),
            kind: NodeKind::Builtin,
            label: trimmed.to_string(),
            args: Vec::new(),
            outputs: Vec::new(),
            position,
            diagnostic: None,
            source: None,
        };
    }

    let parsed = parse_editor_node_text(trimmed);
    let (op, inline_args, parse_diagnostic) = match parsed {
        Ok((op, inline_args)) => (op, inline_args, None),
        Err(error) => (trimmed.to_string(), Vec::new(), Some(error)),
    };
    let known_macros = macro_arities.keys().cloned().collect::<HashSet<_>>();
    let kind = node_kind_for_op(&op, &known_macros);
    let shape = editor_node_port_shape(&op, kind, macro_arities);
    let args = match kind {
        NodeKind::In => inline_args
            .into_iter()
            .map(ArgValue::Literal)
            .collect::<Vec<_>>(),
        NodeKind::Param | NodeKind::Constant => Vec::new(),
        _ => {
            let input_count = shape.input_count.max(inline_args.len() + 1);
            let mut args = vec![ArgValue::ConnectedExpr; input_count];
            for (idx, value) in inline_args.into_iter().enumerate() {
                let slot = idx + 1;
                if slot >= args.len() {
                    args.resize(slot + 1, ArgValue::ConnectedExpr);
                }
                args[slot] = if value == "?" {
                    ArgValue::ConnectedExpr
                } else {
                    ArgValue::Literal(value)
                };
            }
            args
        }
    };

    PatchNode {
        id: id.to_string(),
        op: op.clone(),
        kind,
        label: trimmed.to_string(),
        args,
        outputs: (0..shape.output_count)
            .map(|idx| {
                if idx == 0 {
                    "out".to_string()
                } else {
                    format!("out{}", idx + 1)
                }
            })
            .collect(),
        position,
        diagnostic: parse_diagnostic.or_else(|| {
            let known = dgenlisp_operator_names().contains(&op) || macro_arities.contains_key(&op);
            (!known
                && !matches!(
                    kind,
                    NodeKind::In
                        | NodeKind::Out
                        | NodeKind::Param
                        | NodeKind::History
                        | NodeKind::Constant
                ))
            .then(|| format!("unknown DGenLisp operator `{op}`"))
        }),
        source: None,
    }
}
