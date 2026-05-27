use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::env;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::{Duration, Instant};

use super::super::bump_widget_state_generation;
use super::super::text_input::TextInputState;
use crate::layout::LayoutNode;
use crate::parser::{ASTParser, Expression, Parser};
use crate::vm::Value;

use super::lisp::{
    editor_node_port_shape, node_kind_for_op, parse_editor_node_text, parse_patch_source,
};
use super::metrics::{
    DEFAULT_ZOOM, MAX_ZOOM, MIN_ZOOM, PAN_OVERSCROLL_MIN_CELLS, PAN_OVERSCROLL_VIEWPORT_FACTOR,
};
use super::model::{
    ArgValue, BindingTarget, CableEndpoint, CableSegmentInfo, ConnectionKind, InputPortRef,
    InputPresentation, MacroPatch, MacroSignature, NodeKind, OutputPortRef, ParamNodeInfo, Patch,
    PatchConnection, PatchNode, PatcherIntent, SourceOwner, hidden_inline_node_ids,
    refresh_patch_inline_inputs,
};
use super::project::dgenlisp_operator_names;
use super::prop_str;

const DEBUG_EDITS_ENV: &str = "ESEQ_PATCHER_DEBUG_EDITS";

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
    static PATCHER_PATH_KEYS: RefCell<HashMap<String, HashSet<u64>>> =
        RefCell::new(HashMap::new());
}

pub(super) fn patcher_state_key(node: &LayoutNode) -> u64 {
    let key = patcher_state_key_from_parts(node.stable_widget_id, &node.props);
    if let Some(path) = prop_str(&node.props, "path").or_else(|| prop_str(&node.props, "file")) {
        register_patcher_path_key(path, key);
    }
    key
}

pub(super) fn patcher_state_key_from_parts(
    stable_widget_id: Option<u64>,
    props: &HashMap<String, Value>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    "patcher".hash(&mut hasher);
    stable_widget_id.hash(&mut hasher);
    prop_str(props, "path").hash(&mut hasher);
    prop_str(props, "file").hash(&mut hasher);
    prop_str(props, "intent").hash(&mut hasher);
    hasher.finish()
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
    pub(super) hovered_input_port: Option<InputPortRef>,
    pub(super) hovered_output_port: Option<OutputPortRef>,
    pub(super) hover_back_button: bool,
    pub(super) selected_cable: Option<String>,
    pub(super) edit_state: PatchEditState,
    pub(super) text_edit: Option<PatcherTextEdit>,
    pub(super) agentic_bubbles: HashMap<String, AgenticBubble>,
    pub(super) agentic_morph_nodes: HashMap<String, Instant>,
    pub(super) z_order: HashMap<String, Vec<String>>,
    pub(super) last_pointer_model_position: Option<(f32, f32)>,
    pub(super) active_macro: Option<String>,
    pub(super) drag: Option<PatcherDragState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AlignmentAxis {
    X,
    Y,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AlignmentGuideKind {
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct AlignmentGuide {
    pub(super) kind: AlignmentGuideKind,
    pub(super) position: f32,
    pub(super) extent_start: f32,
    pub(super) extent_end: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct AlignmentSnapState {
    pub(super) snapped_x: bool,
    pub(super) snapped_y: bool,
    pub(super) guides: Vec<AlignmentGuide>,
    pub(super) last_haptic_axis: Option<AlignmentAxis>,
    pub(super) last_haptic_at: Option<Instant>,
}

pub(super) const PATCHER_Z_SLOTS_PER_NODE: i32 = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub(super) enum PatcherZSlot {
    NodeChrome = 0,
    EditSelection = 1,
    Ports = 2,
    Text = 3,
    DrillIn = 4,
    EditCursor = 5,
    ResizeHandles = 6,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct AgenticBubble {
    pub(super) id: String,
    pub(super) prompt: String,
    pub(super) text_state: TextInputState,
    pub(super) position: (f32, f32),
    pub(super) target: AgenticBubbleTarget,
    pub(super) state: AgenticBubbleState,
    pub(super) generation: u64,
    pub(super) macro_name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum AgenticBubbleTarget {
    CreateMacro,
    EditMacro {
        instance_node_id: String,
        macro_name: String,
        params: Vec<String>,
        source: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum AgenticBubbleState {
    Editing,
    Pending {
        started_at: Instant,
    },
    Answer {
        text: String,
        answered_at: Instant,
    },
    Error {
        summary: String,
        raw_output: String,
        failed_at: Instant,
    },
}

impl AgenticBubble {
    pub(super) fn elapsed(&self) -> Option<Duration> {
        match self.state {
            AgenticBubbleState::Pending { started_at } => Some(started_at.elapsed()),
            AgenticBubbleState::Answer { .. } => None,
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct PatchEditState {
    pub(super) nodes: HashMap<String, PatcherNodeEdit>,
    pub(super) deleted_nodes: HashSet<String>,
    pub(super) connections: HashMap<String, PatcherConnectionEdit>,
    pub(super) deleted_connections: HashSet<String>,
    pub(super) input_presentations: HashMap<String, PatcherInputPresentationEdit>,
    pub(super) created_macros: HashMap<String, PatcherMacroEdit>,
    pub(super) next_created_node: u64,
    pub(super) next_created_connection: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PatcherMacroEdit {
    pub(super) name: String,
    pub(super) instance_node_id: String,
    pub(super) source: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PatcherNodeEdit {
    pub(super) view_key: String,
    pub(super) id: String,
    pub(super) origin: PatcherNodeOrigin,
    pub(super) text: String,
    pub(super) position: (f32, f32),
    pub(super) width: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PatcherNodeOrigin {
    Source { source_node_id: String },
    Created { created_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NodeResizeCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl NodeResizeCorner {
    pub(super) fn resizes_left_edge(self) -> bool {
        matches!(self, Self::TopLeft | Self::BottomLeft)
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PatcherInputPresentationEdit {
    pub(super) view_key: String,
    pub(super) node_id: String,
    pub(super) input_index: usize,
    pub(super) presentation: InputPresentation,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PatcherTextEdit {
    pub(super) node_id: String,
    pub(super) text: String,
    pub(super) original_text: String,
    pub(super) state: TextInputState,
    pub(super) autocomplete_selected: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PatcherDragState {
    Nodes {
        primary_node_id: String,
        start_col: f32,
        start_row: f32,
        start_positions: HashMap<String, (f32, f32)>,
        alignment: AlignmentSnapState,
    },
    NodeResize {
        node_id: String,
        corner: NodeResizeCorner,
        start_col: f32,
        start_width: f32,
        start_position: (f32, f32),
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

pub(super) fn reset_patcher_widget_state(key: u64) {
    let pan_changed = PATCHER_PAN_STATES.with(|states| states.borrow_mut().remove(&key).is_some());
    let interaction_changed =
        PATCHER_INTERACTION_STATES.with(|states| states.borrow_mut().remove(&key).is_some());
    if pan_changed || interaction_changed {
        bump_widget_state_generation();
    }
}

pub(super) fn reset_patcher_widget_states_for_path(path: impl AsRef<Path>, fallback_key: u64) {
    let path = path.as_ref().to_string_lossy().to_string();
    let mut keys = PATCHER_PATH_KEYS.with(|paths| {
        paths
            .borrow_mut()
            .remove(&path)
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>()
    });
    keys.push(fallback_key);
    keys.sort_unstable();
    keys.dedup();
    for key in keys {
        reset_patcher_widget_state(key);
    }
}

pub(super) fn register_patcher_path_key(path: impl AsRef<Path>, key: u64) {
    let path = path.as_ref().to_string_lossy().to_string();
    PATCHER_PATH_KEYS.with(|paths| {
        paths.borrow_mut().entry(path).or_default().insert(key);
    });
}

pub(super) fn patcher_keys_for_path(path: impl AsRef<Path>) -> Vec<u64> {
    let path = path.as_ref().to_string_lossy().to_string();
    PATCHER_PATH_KEYS.with(|paths| {
        paths
            .borrow()
            .get(&path)
            .map(|keys| keys.iter().copied().collect())
            .unwrap_or_default()
    })
}

pub(super) fn debug_log_edit_event(action: &str, state: &PatcherInteractionState) {
    if !debug_edit_logging_enabled() {
        return;
    }
    eprintln!(
        "[patcher edit] action={action} active_view={} selected_nodes={:?} selected_cable={:?} text_edit={} drag={}",
        active_patcher_view_key(state),
        state.selected_nodes,
        state.selected_cable,
        state
            .text_edit
            .as_ref()
            .map(|edit| format!("{} {:?}->{:?}", edit.node_id, edit.original_text, edit.text))
            .unwrap_or_else(|| "none".to_string()),
        state
            .drag
            .as_ref()
            .map(|drag| format!("{drag:?}"))
            .unwrap_or_else(|| "none".to_string()),
    );
    eprintln!(
        "[patcher edit state] nodes={} deleted_nodes={} connections={} deleted_connections={} created_macros={}",
        state.edit_state.nodes.len(),
        state.edit_state.deleted_nodes.len(),
        state.edit_state.connections.len(),
        state.edit_state.deleted_connections.len(),
        state.edit_state.created_macros.len(),
    );
    for edit in sorted_node_edits(state) {
        eprintln!(
            "[patcher edit node] view={} id={} origin={:?} text={:?} position=({:.2},{:.2})",
            edit.view_key, edit.id, edit.origin, edit.text, edit.position.0, edit.position.1
        );
    }
    for deleted in sorted_strings(&state.edit_state.deleted_nodes) {
        eprintln!("[patcher edit deleted-node] {deleted}");
    }
    for edit in sorted_connection_edits(state) {
        eprintln!(
            "[patcher edit cable] view={} id={} origin={:?} from={}:{} to={}:{} kind={:?} segment={:?}",
            edit.view_key,
            edit.id,
            edit.origin,
            edit.from.node_id,
            edit.from.output_index,
            edit.to.node_id,
            edit.to.input_index,
            edit.kind,
            edit.segment
        );
    }
    for deleted in sorted_strings(&state.edit_state.deleted_connections) {
        eprintln!("[patcher edit deleted-cable] {deleted}");
    }
    for macro_edit in sorted_macro_edits(state) {
        eprintln!(
            "[patcher edit macro] name={} instance={}",
            macro_edit.name, macro_edit.instance_node_id
        );
    }
    eprintln!("[/patcher edit]");
}

pub(super) fn debug_log_writeback_event(action: &str, details: impl AsRef<str>) {
    if debug_edit_logging_enabled() {
        eprintln!(
            "[patcher writeback] action={action}\n{}\n[/patcher writeback]",
            details.as_ref()
        );
    }
}

fn debug_edit_logging_enabled() -> bool {
    env::var(DEBUG_EDITS_ENV)
        .ok()
        .is_some_and(|value| env_flag_enabled(&value))
}

fn env_flag_enabled(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
}

fn sorted_node_edits(state: &PatcherInteractionState) -> Vec<&PatcherNodeEdit> {
    let mut edits = state.edit_state.nodes.values().collect::<Vec<_>>();
    edits.sort_by_key(|edit| (edit.view_key.as_str(), edit.id.as_str()));
    edits
}

fn sorted_connection_edits(state: &PatcherInteractionState) -> Vec<&PatcherConnectionEdit> {
    let mut edits = state.edit_state.connections.values().collect::<Vec<_>>();
    edits.sort_by_key(|edit| (edit.view_key.as_str(), edit.id.as_str()));
    edits
}

fn sorted_macro_edits(state: &PatcherInteractionState) -> Vec<&PatcherMacroEdit> {
    let mut edits = state.edit_state.created_macros.values().collect::<Vec<_>>();
    edits.sort_by_key(|edit| edit.name.as_str());
    edits
}

fn sorted_strings(values: &HashSet<String>) -> Vec<&str> {
    let mut values = values.iter().map(String::as_str).collect::<Vec<_>>();
    values.sort_unstable();
    values
}

pub(super) fn active_patcher_view_key(interaction_state: &PatcherInteractionState) -> String {
    interaction_state
        .active_macro
        .as_deref()
        .map(|name| format!("macro:{name}"))
        .unwrap_or_else(|| "root".to_string())
}

pub(super) fn sync_patcher_z_order(
    state: &mut PatcherInteractionState,
    view_key: &str,
    patch: &Patch,
) {
    let live_ids = patch
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    let stack = state.z_order.entry(view_key.to_string()).or_default();
    stack.retain(|node_id| live_ids.contains(node_id.as_str()));
    let present = stack.iter().map(String::as_str).collect::<HashSet<_>>();
    let missing = patch
        .nodes
        .iter()
        .filter(|node| !present.contains(node.id.as_str()))
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    stack.extend(missing);
}

pub(super) fn bring_nodes_to_front(
    state: &mut PatcherInteractionState,
    view_key: &str,
    node_ids: &[String],
) {
    if node_ids.is_empty() {
        return;
    }
    let requested = node_ids.iter().map(String::as_str).collect::<HashSet<_>>();
    let stack = state.z_order.entry(view_key.to_string()).or_default();
    let mut moved = Vec::new();
    stack.retain(|node_id| {
        if requested.contains(node_id.as_str()) {
            moved.push(node_id.clone());
            false
        } else {
            true
        }
    });
    for node_id in node_ids {
        if !moved.iter().any(|moved_id| moved_id == node_id) {
            moved.push(node_id.clone());
        }
    }
    stack.extend(moved);
}

pub(super) fn ordered_patch_nodes<'a>(
    patch: &'a Patch,
    state: &PatcherInteractionState,
    view_key: &str,
) -> Vec<&'a PatchNode> {
    let hidden_node_ids = hidden_inline_node_ids(patch);
    let by_id = patch
        .nodes
        .iter()
        .filter(|node| !hidden_node_ids.contains(&node.id))
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    if let Some(stack) = state.z_order.get(view_key) {
        for node_id in stack {
            if let Some(node) = by_id.get(node_id.as_str()) {
                seen.insert(node.id.as_str());
                ordered.push(*node);
            }
        }
    }
    ordered.extend(
        patch
            .nodes
            .iter()
            .filter(|node| !hidden_node_ids.contains(&node.id))
            .filter(|node| !seen.contains(node.id.as_str())),
    );
    ordered
}

pub(super) fn node_z_index(
    state: &PatcherInteractionState,
    view_key: &str,
    node_id: &str,
    slot: PatcherZSlot,
) -> i32 {
    let stack_index = state
        .z_order
        .get(view_key)
        .and_then(|stack| stack.iter().position(|id| id == node_id))
        .unwrap_or(0);
    stack_index as i32 * PATCHER_Z_SLOTS_PER_NODE + slot as i32
}

pub(super) fn max_node_z_index(patch: &Patch) -> i32 {
    patch.nodes.len().saturating_sub(1) as i32 * PATCHER_Z_SLOTS_PER_NODE
        + PatcherZSlot::ResizeHandles as i32
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

pub(super) fn input_presentation_key(view_key: &str, node_id: &str, input_index: usize) -> String {
    format!("{view_key}::{node_id}:{input_index}")
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
            width: None,
        },
    );
    debug_log_edit_event(
        &format!("allocate-created-node view={view_key} id={id}"),
        state,
    );
    bring_nodes_to_front(state, view_key, std::slice::from_ref(&id));
    id
}

pub(super) fn allocate_agentic_bubble(
    state: &mut PatcherInteractionState,
    position: (f32, f32),
) -> String {
    allocate_agentic_bubble_with_target(state, position, AgenticBubbleTarget::CreateMacro)
}

pub(super) fn allocate_agentic_bubble_with_target(
    state: &mut PatcherInteractionState,
    position: (f32, f32),
    target: AgenticBubbleTarget,
) -> String {
    let id = format!("bubble-{}", state.edit_state.next_created_node);
    state.edit_state.next_created_node += 1;
    let macro_name = match &target {
        AgenticBubbleTarget::CreateMacro => format!("agentic-{}", id.replace('_', "-")),
        AgenticBubbleTarget::EditMacro { macro_name, .. } => macro_name.clone(),
    };
    state.agentic_bubbles.insert(
        id.clone(),
        AgenticBubble {
            id: id.clone(),
            prompt: String::new(),
            text_state: TextInputState::default(),
            position,
            target,
            state: AgenticBubbleState::Editing,
            generation: 0,
            macro_name,
        },
    );
    state.selected_nodes.clear();
    state.selected_cable = None;
    state.text_edit = None;
    state.drag = None;
    id
}

pub(super) fn editing_agentic_bubble_id(state: &PatcherInteractionState) -> Option<String> {
    state
        .agentic_bubbles
        .values()
        .find(|bubble| matches!(bubble.state, AgenticBubbleState::Editing))
        .map(|bubble| bubble.id.clone())
}

pub(super) fn allocate_created_connection(
    state: &mut PatcherInteractionState,
    view_key: &str,
    from: OutputPortRef,
    to: InputPortRef,
) -> String {
    let id = format!("created-cable-{}", state.edit_state.next_created_connection);
    state.edit_state.next_created_connection += 1;
    let action = format!(
        "allocate-created-connection view={view_key} id={id} from={}:{} to={}:{}",
        from.node_id, from.output_index, to.node_id, to.input_index
    );
    state.edit_state.connections.insert(
        format!("{view_key}::{id}"),
        PatcherConnectionEdit {
            view_key: view_key.to_string(),
            id: id.clone(),
            origin: PatcherConnectionOrigin::Created {
                created_id: id.clone(),
            },
            from,
            to: to.clone(),
            kind: ConnectionKind::Forward,
            segment: None,
        },
    );
    set_input_presentation_override(
        state,
        view_key,
        &to.node_id,
        to.input_index,
        InputPresentation::Cable,
    );
    debug_log_edit_event(&action, state);
    id
}

pub(super) fn set_input_presentation_override(
    state: &mut PatcherInteractionState,
    view_key: &str,
    node_id: &str,
    input_index: usize,
    presentation: InputPresentation,
) {
    state.edit_state.input_presentations.insert(
        input_presentation_key(view_key, node_id, input_index),
        PatcherInputPresentationEdit {
            view_key: view_key.to_string(),
            node_id: node_id.to_string(),
            input_index,
            presentation,
        },
    );
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
        debug_log_edit_event(
            &format!("set-source-connection-segment view={view_key} cable={cable_id}"),
            state,
        );
        return;
    }
    state.edit_state.connections.insert(
        connection_edit_key(view_key, &cable_id),
        PatcherConnectionEdit {
            view_key: view_key.to_string(),
            id: cable_id.clone(),
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
    debug_log_edit_event(
        &format!("delete-connection view={view_key} cable={cable_id} changed={changed}"),
        state,
    );
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
    debug_log_edit_event(
        &format!("delete-selected-nodes view={view_key} nodes={selected_nodes:?}"),
        state,
    );
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
            width: node.width,
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

pub(super) fn set_node_edit_width(
    state: &mut PatcherInteractionState,
    view_key: &str,
    node: &PatchNode,
    position: (f32, f32),
    width: Option<f32>,
    text: String,
) {
    ensure_source_node_edit(state, view_key, node, text);
    if let Some(edit) = state
        .edit_state
        .nodes
        .get_mut(&node_edit_key(view_key, &node.id))
    {
        edit.position = position;
        edit.width = width.filter(|width| width.is_finite());
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
            patch_with_created_macros(root_patch.clone(), interaction_state)
                .macros
                .into_iter()
                .find(|macro_patch| macro_patch.name == name)
                .map(|macro_patch| {
                    let mut patch = macro_patch.patch;
                    patch.macros =
                        patch_with_created_macros(root_patch.clone(), interaction_state).macros;
                    patch
                })
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

pub(super) fn patch_with_interaction_state(
    mut patch: Patch,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
) -> Patch {
    patch = patch_with_created_macros(patch, interaction_state);
    let macro_signatures = macro_signatures_with_visual_edits(&patch, interaction_state);
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
        refresh_macro_instance_outputs(node, &macro_signatures);
        let edit_key = node_edit_key(view_key, &node.id);
        if let Some(edit) = interaction_state.edit_state.nodes.get(&edit_key) {
            node.position = edit.position;
            node.width = edit.width;
            apply_node_text_override(node, &edit.text, &macro_signatures);
        }
        if let Some(edit) = interaction_state
            .text_edit
            .as_ref()
            .filter(|edit| edit.node_id == node.id)
        {
            apply_node_text_override(node, &edit.text, &macro_signatures);
        }
    }
    for connection in &mut patch.connections {
        let edit_key = connection_edit_key(view_key, &source_connection_id(connection));
        if let Some(edit) = interaction_state.edit_state.connections.get(&edit_key)
            && matches!(edit.origin, PatcherConnectionOrigin::Source { .. })
        {
            connection.segment = edit.segment;
        }
        let presentation_key =
            input_presentation_key(view_key, &connection.to_node, connection.to_input);
        if let Some(edit) = interaction_state
            .edit_state
            .input_presentations
            .get(&presentation_key)
        {
            connection.presentation = edit.presentation;
            connection.presentation_override = Some(edit.presentation);
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
            &macro_signatures,
            interaction_state
                .text_edit
                .as_ref()
                .is_some_and(|text_edit| text_edit.node_id == edit.id),
        ));
        if let Some(node) = patch.nodes.last_mut() {
            node.width = edit.width;
        }
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
                presentation: InputPresentation::Cable,
                presentation_override: Some(InputPresentation::Cable),
                source: None,
            }),
    );
    refresh_patch_inline_inputs(&mut patch);
    patch
}

fn refresh_macro_instance_outputs(
    node: &mut PatchNode,
    macro_signatures: &HashMap<String, MacroSignature>,
) {
    if node.kind != NodeKind::MacroInstance {
        return;
    }
    let Some(signature) = macro_signatures.get(&node.op) else {
        return;
    };
    node.outputs = match destructuring_output_names(node) {
        Some(names) if names.len() == signature.outputs.len() => names,
        _ => signature.outputs.clone(),
    };
}

fn destructuring_output_names(node: &PatchNode) -> Option<Vec<String>> {
    let source = node.source.as_ref()?;
    match &source.owner {
        SourceOwner::BindingValue {
            binding: BindingTarget::Destructuring(names),
            ..
        } => Some(names.clone()),
        SourceOwner::Compound { parts } => parts.iter().find_map(|owner| match owner {
            SourceOwner::BindingValue {
                binding: BindingTarget::Destructuring(names),
                ..
            } => Some(names.clone()),
            _ => None,
        }),
        _ => None,
    }
}

pub(super) fn patch_with_created_macros(
    mut patch: Patch,
    interaction_state: &PatcherInteractionState,
) -> Patch {
    for macro_edit in interaction_state.edit_state.created_macros.values() {
        if patch
            .macros
            .iter()
            .any(|macro_patch| macro_patch.name == macro_edit.name)
        {
            continue;
        }
        let source = macro_edit
            .source
            .as_deref()
            .map(str::to_string)
            .unwrap_or_else(|| default_created_macro_source(&macro_edit.name));
        if let Some(macro_patch) = created_macro_patch_from_source(&macro_edit.name, &source) {
            patch.macros.push(macro_patch);
        }
    }
    patch
}

fn macro_signatures_with_visual_edits(
    patch: &Patch,
    interaction_state: &PatcherInteractionState,
) -> HashMap<String, MacroSignature> {
    let mut macro_signatures = patch
        .macros
        .iter()
        .map(|macro_patch| {
            (
                macro_patch.name.clone(),
                MacroSignature {
                    params: macro_patch.params.clone(),
                    outputs: macro_patch.outputs.clone(),
                },
            )
        })
        .collect::<HashMap<_, _>>();

    for macro_patch in &patch.macros {
        if let Some(output_count) = visual_macro_output_count(macro_patch, interaction_state)
            && let Some(signature) = macro_signatures.get_mut(&macro_patch.name)
        {
            signature.outputs = default_output_names(output_count);
        }
    }

    for edit in interaction_state
        .edit_state
        .nodes
        .values()
        .filter(|edit| matches!(edit.origin, PatcherNodeOrigin::Created { .. }))
    {
        let Some(macro_name) = edit.view_key.strip_prefix("macro:") else {
            continue;
        };
        let Ok((op, inline_args)) = parse_editor_node_text(edit.text.trim()) else {
            continue;
        };
        if op != "in" {
            continue;
        }
        let Some(channel) = inline_args
            .first()
            .and_then(|arg| arg.parse::<usize>().ok())
            .filter(|channel| *channel > 0)
        else {
            continue;
        };
        macro_signatures
            .entry(macro_name.to_string())
            .and_modify(|signature| {
                while signature.params.len() < channel {
                    signature
                        .params
                        .push(format!("input{}", signature.params.len() + 1));
                }
            })
            .or_insert_with(|| MacroSignature {
                params: (0..channel)
                    .map(|idx| format!("input{}", idx + 1))
                    .collect(),
                outputs: default_output_names(1),
            });
    }
    macro_signatures
}

fn visual_macro_output_count(
    macro_patch: &MacroPatch,
    interaction_state: &PatcherInteractionState,
) -> Option<usize> {
    let view_key = format!("macro:{}", macro_patch.name);
    let mut edited = false;
    let mut max_channel = 0usize;

    for node in &macro_patch.patch.nodes {
        if node.kind != NodeKind::Out {
            continue;
        }
        let edit_key = node_edit_key(&view_key, &node.id);
        if interaction_state
            .edit_state
            .deleted_nodes
            .contains(&edit_key)
        {
            edited = true;
            continue;
        }
        max_channel = max_channel.max(output_channel_from_node(node).unwrap_or(1));
    }

    for edit in interaction_state
        .edit_state
        .nodes
        .values()
        .filter(|edit| edit.view_key == view_key)
    {
        match &edit.origin {
            PatcherNodeOrigin::Created { .. } => {
                if let Some(channel) = output_channel_from_text(&edit.text) {
                    edited = true;
                    max_channel = max_channel.max(channel);
                }
            }
            PatcherNodeOrigin::Source { source_node_id } => {
                let is_source_out = macro_patch
                    .patch
                    .nodes
                    .iter()
                    .any(|node| node.id == *source_node_id && node.kind == NodeKind::Out);
                if !is_source_out {
                    continue;
                }
                edited = true;
                if let Some(channel) = output_channel_from_text(&edit.text) {
                    max_channel = max_channel.max(channel);
                }
            }
        }
    }

    edited.then_some(max_channel)
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

fn output_channel_from_text(text: &str) -> Option<usize> {
    let Ok((op, inline_args)) = parse_editor_node_text(text.trim()) else {
        return None;
    };
    if op != "out" {
        return None;
    }
    inline_args
        .first()
        .and_then(|arg| arg.parse::<usize>().ok())
        .filter(|channel| *channel > 0)
        .or(Some(1))
}

fn default_output_names(count: usize) -> Vec<String> {
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

fn created_macro_patch_from_source(name: &str, source: &str) -> Option<MacroPatch> {
    parse_patch_source(source, PatcherIntent::Instrument)
        .ok()?
        .macros
        .into_iter()
        .find(|macro_patch| macro_patch.name == name)
}

pub(super) fn default_created_macro_source(name: &str) -> String {
    format!("(defmacro {name} (input) (* input 1))")
}

fn apply_node_text_override(
    node: &mut PatchNode,
    text: &str,
    macro_signatures: &HashMap<String, MacroSignature>,
) {
    let edited = node_from_editor_text(&node.id, text, node.position, macro_signatures, false);
    node.op = edited.op;
    node.kind = edited.kind;
    node.label = edited.label;
    node.args = edited.args;
    node.outputs = edited.outputs;
    node.param = edited.param;
    node.inline_inputs = edited.inline_inputs;
    node.diagnostic = edited.diagnostic;
}

pub(super) fn node_from_editor_text(
    id: &str,
    text: &str,
    position: (f32, f32),
    macro_signatures: &HashMap<String, MacroSignature>,
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
            width: None,
            param: None,
            inline_inputs: Vec::new(),
            diagnostic: None,
            source: None,
        };
    }

    let parsed = parse_editor_node_text(trimmed);
    let (op, inline_args, parse_diagnostic) = match parsed {
        Ok((op, inline_args)) => (op, inline_args, None),
        Err(error) => (trimmed.to_string(), Vec::new(), Some(error)),
    };
    let known_macros = macro_signatures.keys().cloned().collect::<HashSet<_>>();
    let kind = node_kind_for_op(&op, &known_macros);
    let shape = editor_node_port_shape(&op, kind, macro_signatures);
    let param = editor_param_node_info(&op, trimmed, &inline_args);
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
        outputs: macro_signatures
            .get(&op)
            .filter(|_| kind == NodeKind::MacroInstance)
            .map(|signature| signature.outputs.clone())
            .unwrap_or_else(|| {
                (0..shape.output_count)
                    .map(|idx| {
                        if idx == 0 {
                            "out".to_string()
                        } else {
                            format!("out{}", idx + 1)
                        }
                    })
                    .collect()
            }),
        position,
        width: None,
        param,
        inline_inputs: Vec::new(),
        diagnostic: parse_diagnostic.or_else(|| {
            let known =
                dgenlisp_operator_names().contains(&op) || macro_signatures.contains_key(&op);
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

fn editor_param_node_info(
    op: &str,
    text: &str,
    positional_args: &[String],
) -> Option<ParamNodeInfo> {
    if op != "param" {
        return None;
    }
    let items = parse_editor_node_items(text).unwrap_or_default();
    let name = items
        .get(1)
        .and_then(|expr| match expr {
            Expression::Symbol(name) => Some(name.clone()),
            _ => None,
        })
        .or_else(|| positional_args.first().cloned())?;
    let modulatable = items.windows(2).any(|pair| {
        matches!(
            (&pair[0], &pair[1]),
            (Expression::Symbol(key), Expression::Symbol(value))
                if key == "@mod" && value == "true"
        )
    });
    Some(ParamNodeInfo { name, modulatable })
}

fn parse_editor_node_items(text: &str) -> Result<Vec<Expression>, String> {
    let source = format!("({text})");
    let tokens = Parser::new(source)
        .parse()
        .map_err(|error| format!("failed to tokenize node text: {error:?}"))?;
    let exprs = ASTParser::new(tokens)
        .parse()
        .map_err(|error| format!("failed to parse node text: {error:?}"))?;
    match exprs.first() {
        Some(Expression::List(items)) => Ok(items.clone()),
        _ => Err("node text must parse as a list".to_string()),
    }
}
