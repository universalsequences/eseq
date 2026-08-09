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
    editor_node_port_shape, node_kind_for_op, normalize_editor_node_text, parse_editor_node_text,
    parse_patch_source,
};
use super::metrics::{
    AGENTIC_CLOSE_SECS, DEFAULT_ZOOM, MAX_ZOOM, MIN_ZOOM, PAN_OVERSCROLL_MIN_CELLS,
    PAN_OVERSCROLL_VIEWPORT_FACTOR,
};
use super::model::{
    ArgValue, BindingTarget, CableEndpoint, CableSegmentInfo, ConnectionKind, ExprPath,
    InputPortRef, InputPresentation, MacroPatch, MacroSignature, NodeKind, NodeSource,
    OutputPortRef, ParamNodeInfo, Patch, PatchConnection, PatchNode, PatcherIntent, SourceExprId,
    SourceFormId, SourceOwner, SourceScopeId, hidden_inline_node_ids, orphaned_inline_mod_node_ids,
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
    static PATCHER_PATH_KEYS: RefCell<HashMap<String, Vec<u64>>> =
        RefCell::new(HashMap::new());
    static PATCHER_HISTORIES: RefCell<HashMap<u64, PatcherHistory>> =
        RefCell::new(HashMap::new());
    static PATCHER_CLIPBOARD: RefCell<Option<PatcherClipboard>> =
        RefCell::new(None);
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
    pub(super) agentic_morph_nodes: HashMap<String, AgenticMorph>,
    pub(super) z_order: HashMap<String, Vec<String>>,
    /// Recently interacted node ids for the active view, oldest first. Feeds
    /// the cmd+enter "create below" anchor and the cmd+up "connect last two"
    /// shortcut; cleared on macro navigation.
    pub(super) touched_nodes: Vec<String>,
    pub(super) last_pointer_model_position: Option<(f32, f32)>,
    pub(super) active_macro: Option<String>,
    pub(super) drag: Option<PatcherDragState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PatcherMacroLibraryActionKind {
    SaveToLibrary,
    Fork,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PatcherMacroLibraryAction {
    pub(super) kind: PatcherMacroLibraryActionKind,
    pub(super) macro_name: String,
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
    /// When the bubble was opened, which drives its grow-in animation. Distinct
    /// from `AgenticBubbleState::Pending::started_at`, which resets on submit.
    pub(super) created_at: Instant,
    /// Set by Escape. The bubble stays in the map, playing its shrink-out, and
    /// is dropped by `set_patcher_interaction_state` once that finishes. It
    /// counts as gone the moment this is set — see `is_dismissed`.
    pub(super) closing_at: Option<Instant>,
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
    /// Wire an existing node into the surrounding patch
    /// (docs/patcher-agentic-connect-spec.md §3). The variant is what keeps the
    /// three bubbles isolated: it selects both the prompt template and the
    /// output type the agent is allowed to return.
    ConnectNode {
        instance_node_id: String,
        subject: ConnectSubject,
    },
}

/// What the connect bubble's subject node is, and therefore what the agent is
/// told about its inlets (spec §5.2).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ConnectSubject {
    Macro {
        name: String,
        params: Vec<String>,
        source: String,
    },
    Operator {
        op: String,
    },
}

impl ConnectSubject {
    pub(super) fn name(&self) -> &str {
        match self {
            ConnectSubject::Macro { name, .. } => name.as_str(),
            ConnectSubject::Operator { op } => op.as_str(),
        }
    }
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

/// The pose an agentic bubble last rendered at, in model space (so it stays
/// correct if the view pans or zooms mid-morph).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct AgenticBubblePose {
    /// `(x, y, width, height)` in patch-model cells.
    pub(super) model_rect: (f32, f32, f32, f32),
    pub(super) fill: [f32; 4],
    pub(super) border: [f32; 4],
}

/// A node that has just been materialized from an agentic bubble, and is easing
/// from that bubble's square chrome into its own rounded node chrome.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct AgenticMorph {
    pub(super) started_at: Instant,
    /// The bubble's final pose. `None` when no pose was recorded (headless or
    /// non-macOS render paths), in which case the node just appears.
    pub(super) from: Option<AgenticBubblePose>,
}

thread_local! {
    /// Poses of the bubbles drawn in the most recent frame, keyed by bubble id.
    /// The renderer rebuilds this every frame, which also prunes bubbles that
    /// have gone away.
    ///
    /// This is a side channel rather than a field on `AgenticBubble` because a
    /// bubble is removed in the same state write that inserts its node, so
    /// there is no frame where both exist to read the start pose from — and
    /// writing the pose back into the interaction state each frame would bump
    /// the widget-state generation on every tick.
    static AGENTIC_BUBBLE_POSES: RefCell<HashMap<String, AgenticBubblePose>> =
        RefCell::new(HashMap::new());
}

pub(super) fn set_agentic_bubble_poses(poses: HashMap<String, AgenticBubblePose>) {
    AGENTIC_BUBBLE_POSES.with(|cell| *cell.borrow_mut() = poses);
}

pub(super) fn agentic_bubble_pose(bubble_id: &str) -> Option<AgenticBubblePose> {
    AGENTIC_BUBBLE_POSES.with(|cell| cell.borrow().get(bubble_id).copied())
}

impl AgenticBubble {
    /// The macro this bubble is bound to. A bound bubble's prompt is scoped to
    /// that macro, so the name is shown in the bubble's header.
    pub(super) fn bound_macro_name(&self) -> Option<&str> {
        match &self.target {
            AgenticBubbleTarget::EditMacro { macro_name, .. } => Some(macro_name.as_str()),
            AgenticBubbleTarget::ConnectNode { subject, .. } => Some(subject.name()),
            AgenticBubbleTarget::CreateMacro => None,
        }
    }

    /// What the body shows before an answer arrives: the typed prompt, or a
    /// placeholder that says what this bubble is for.
    ///
    /// Rendering can only wrap text whose glyph widths a measure pass has
    /// already cached under the *exact* same string, so this and `body_text`
    /// are the single source both passes go through — if they drift, the
    /// renderer silently falls back or drops the bubble.
    pub(super) fn prompt_text(&self) -> String {
        if self.prompt.trim().is_empty() {
            match self.target {
                AgenticBubbleTarget::EditMacro { .. } => "ask about or edit this macro".to_string(),
                AgenticBubbleTarget::ConnectNode { .. } => {
                    "connect this node into the patch".to_string()
                }
                AgenticBubbleTarget::CreateMacro => "cmd+k prompt".to_string(),
            }
        } else {
            self.prompt.clone()
        }
    }

    /// The text the body settles on for the bubble's current state.
    pub(super) fn body_text(&self) -> String {
        match &self.state {
            AgenticBubbleState::Answer { text, .. } => text.clone(),
            _ => self.prompt_text(),
        }
    }

    /// Dismissed bubbles are still rendered while they shrink out, but are
    /// invisible to every query that asks what the patcher is doing.
    pub(super) fn is_dismissed(&self) -> bool {
        self.closing_at.is_some()
    }

    pub(super) fn close_finished(&self) -> bool {
        self.closing_at
            .is_some_and(|at| at.elapsed().as_secs_f32() >= AGENTIC_CLOSE_SECS)
    }

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

pub(super) fn set_patcher_interaction_state(key: u64, mut state: PatcherInteractionState) {
    prune_closed_agentic_bubbles(&mut state);
    let changed = PATCHER_INTERACTION_STATES.with(|states| {
        let mut states = states.borrow_mut();
        record_patcher_history_transition(key, states.get(&key), &state);
        let old = states.insert(key, state.clone());
        old.as_ref() != Some(&state)
    });
    if changed {
        bump_widget_state_generation();
    }
}

/// Drop bubbles whose shrink-out has played to the end. Rendering already skips
/// them, so this is bookkeeping rather than a visual change; doing it on write
/// gives a single choke point instead of mutating state from the render path.
fn prune_closed_agentic_bubbles(state: &mut PatcherInteractionState) {
    if state
        .agentic_bubbles
        .values()
        .any(|bubble| bubble.close_finished())
    {
        state
            .agentic_bubbles
            .retain(|_, bubble| !bubble.close_finished());
    }
}

/// Store interaction state while an undo/redo is being applied: the edit-state
/// transition must not be recorded as a fresh gesture (it would clobber the
/// redo stack the application just built).
pub(super) fn set_patcher_interaction_state_without_history(
    key: u64,
    state: PatcherInteractionState,
) {
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
    PATCHER_HISTORIES.with(|histories| histories.borrow_mut().remove(&key));
    if pan_changed || interaction_changed {
        bump_widget_state_generation();
    }
}

const PATCHER_HISTORY_LIMIT: usize = 64;

/// Graph-level undo history for one patcher widget. Each undo step is a full
/// `PatchEditState` snapshot taken at the start of a committed gesture;
/// restoring one regenerates + recompiles through the normal semantic-change
/// payload, so no source reconciliation is needed (spec
/// docs/patch-vs-code-editor-spec.md §4.4). Snapshots are only valid within
/// one base-source epoch: `reset_patcher_widget_state` (which runs after every
/// save rewrites the source) drops the whole history.
#[derive(Clone, Debug, Default)]
pub(super) struct PatcherHistory {
    pub(super) undo: Vec<PatchEditState>,
    pub(super) redo: Vec<PatchEditState>,
    /// `edit_state` as of the start of the in-flight gesture (an open drag or
    /// text edit); committed as one undo step when the gesture closes.
    pub(super) pending_gesture_base: Option<PatchEditState>,
}

/// Undo-relevant equality: the created-id counters only ever grow (a
/// cancelled gesture keeps its bump) and must not make an otherwise reverted
/// edit state look like a change worth an undo step.
fn edit_states_equivalent(left: &PatchEditState, right: &PatchEditState) -> bool {
    left.nodes == right.nodes
        && left.deleted_nodes == right.deleted_nodes
        && left.connections == right.connections
        && left.deleted_connections == right.deleted_connections
        && left.input_presentations == right.input_presentations
        && left.created_macros == right.created_macros
}

fn push_patcher_undo_step(history: &mut PatcherHistory, base: PatchEditState) {
    history.undo.push(base);
    if history.undo.len() > PATCHER_HISTORY_LIMIT {
        let excess = history.undo.len() - PATCHER_HISTORY_LIMIT;
        history.undo.drain(..excess);
    }
    history.redo.clear();
}

/// Observe an interaction-state transition and fold it into undo history.
/// A gesture is "open" while a drag or text edit is active; edit-state changes
/// made during an open gesture coalesce into one undo step, committed on the
/// first store with the gesture closed. A gesture that ends back at its base
/// (e.g. Esc canceling a fresh created node) records nothing.
fn record_patcher_history_transition(
    key: u64,
    previous: Option<&PatcherInteractionState>,
    next: &PatcherInteractionState,
) {
    let previous_edit = previous.map(|state| &state.edit_state);
    PATCHER_HISTORIES.with(|histories| {
        let mut histories = histories.borrow_mut();
        let history = histories.entry(key).or_default();
        let default_edit = PatchEditState::default();
        let previous_edit = previous_edit.unwrap_or(&default_edit);
        let edit_changed = !edit_states_equivalent(previous_edit, &next.edit_state);
        if edit_changed && history.pending_gesture_base.is_none() {
            history.pending_gesture_base = Some(previous_edit.clone());
        }
        let gesture_open = next.drag.is_some() || next.text_edit.is_some();
        if !gesture_open
            && let Some(base) = history.pending_gesture_base.take()
            && !edit_states_equivalent(&base, &next.edit_state)
        {
            push_patcher_undo_step(history, base);
        }
    });
}

/// Apply one undo (or redo) step to `state`, moving the replaced edit state to
/// the opposite stack. Returns false when the stack is empty. The caller must
/// store the state via `set_patcher_interaction_state_without_history` and emit
/// a semantic change so the patch regenerates and recompiles.
pub(super) fn apply_patcher_history_step(
    key: u64,
    state: &mut PatcherInteractionState,
    redo: bool,
) -> bool {
    PATCHER_HISTORIES.with(|histories| {
        let mut histories = histories.borrow_mut();
        let history = histories.entry(key).or_default();
        // A leftover gesture base should have been committed when the gesture
        // closed; commit it now rather than losing the step.
        if let Some(base) = history.pending_gesture_base.take()
            && !edit_states_equivalent(&base, &state.edit_state)
        {
            push_patcher_undo_step(history, base);
        }
        let (from, to) = if redo {
            (&mut history.redo, &mut history.undo)
        } else {
            (&mut history.undo, &mut history.redo)
        };
        let Some(mut snapshot) = from.pop() else {
            return false;
        };
        // Created-id counters only ever grow: keep the newer counter so ids
        // allocated after an undo can never collide with ids still referenced
        // by snapshots on either stack.
        snapshot.next_created_node = snapshot
            .next_created_node
            .max(state.edit_state.next_created_node);
        snapshot.next_created_connection = snapshot
            .next_created_connection
            .max(state.edit_state.next_created_connection);
        to.push(state.edit_state.clone());
        state.edit_state = snapshot;
        state.selected_nodes.clear();
        state.selected_cable = None;
        state.text_edit = None;
        state.drag = None;
        debug_log_edit_event(if redo { "redo" } else { "undo" }, state);
        true
    })
}

#[cfg(test)]
pub(super) fn patcher_history_for_key(key: u64) -> PatcherHistory {
    PATCHER_HISTORIES.with(|histories| histories.borrow().get(&key).cloned().unwrap_or_default())
}

/// One copied node: its editable header text plus geometry. Paste re-creates
/// it as a created node, so fidelity matches retyping the header by hand.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PatcherClipboardNode {
    pub(super) text: String,
    pub(super) position: (f32, f32),
    pub(super) width: Option<f32>,
}

/// A wire internal to the copied selection, endpoints as indices into
/// `PatcherClipboard::nodes` so paste can remap them onto the new node ids.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PatcherClipboardConnection {
    pub(super) from_index: usize,
    pub(super) from_output: usize,
    pub(super) to_index: usize,
    pub(super) to_input: usize,
}

/// Process-local patcher clipboard, shared across patcher widgets so a
/// selection copied in one patch can be pasted into another.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct PatcherClipboard {
    pub(super) nodes: Vec<PatcherClipboardNode>,
    pub(super) connections: Vec<PatcherClipboardConnection>,
    /// Number of pastes since this clipboard was captured; staggers repeated
    /// pastes so they don't stack on the same spot.
    pub(super) paste_serial: u32,
}

pub(super) fn set_patcher_clipboard(clipboard: PatcherClipboard) {
    PATCHER_CLIPBOARD.with(|slot| *slot.borrow_mut() = Some(clipboard));
}

/// Take the clipboard for a paste, bumping its serial in place.
pub(super) fn next_patcher_clipboard_paste() -> Option<PatcherClipboard> {
    PATCHER_CLIPBOARD.with(|slot| {
        let mut slot = slot.borrow_mut();
        let clipboard = slot.as_mut()?;
        clipboard.paste_serial += 1;
        Some(clipboard.clone())
    })
}

pub(super) fn reset_patcher_widget_states_for_path(path: impl AsRef<Path>, fallback_key: u64) {
    let path = path.as_ref().to_string_lossy().to_string();
    let mut keys =
        PATCHER_PATH_KEYS.with(|paths| paths.borrow_mut().remove(&path).unwrap_or_default());
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
        let mut paths = paths.borrow_mut();
        let keys = paths.entry(path).or_default();
        keys.retain(|existing| *existing != key);
        keys.push(key);
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

const MAX_TOUCHED_NODES: usize = 8;

pub(super) fn note_touched_node(state: &mut PatcherInteractionState, node_id: &str) {
    state.touched_nodes.retain(|id| id != node_id);
    state.touched_nodes.push(node_id.to_string());
    if state.touched_nodes.len() > MAX_TOUCHED_NODES {
        let excess = state.touched_nodes.len() - MAX_TOUCHED_NODES;
        state.touched_nodes.drain(..excess);
    }
}

pub(super) fn allocate_created_node(
    state: &mut PatcherInteractionState,
    view_key: &str,
    position: (f32, f32),
) -> String {
    allocate_created_node_avoiding(state, view_key, position, &HashSet::new())
}

/// Allocate a created-node id that collides neither with other interaction
/// edits (via the counter) nor with any id in `taken_node_ids` — the current
/// patch model's node ids. Sources written by older builds of the generator
/// can legitimately contain `created-N` bindings; reusing such an id would
/// visually attach that node's cables to the new node and corrupt the next
/// regeneration (existing edges splice through the new node).
pub(super) fn allocate_created_node_avoiding(
    state: &mut PatcherInteractionState,
    view_key: &str,
    position: (f32, f32),
    taken_node_ids: &HashSet<String>,
) -> String {
    let id = loop {
        let candidate = format!("created-{}", state.edit_state.next_created_node);
        state.edit_state.next_created_node += 1;
        if !taken_node_ids.contains(&candidate) {
            break candidate;
        }
    };
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
        AgenticBubbleTarget::ConnectNode { subject, .. } => subject.name().to_string(),
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
            created_at: Instant::now(),
            closing_at: None,
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
        .find(|bubble| {
            !bubble.is_dismissed() && matches!(bubble.state, AgenticBubbleState::Editing)
        })
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

/// Drop session-created macros that nothing instantiates any more, along with
/// the staged edits for their bodies. A created macro is only ever reachable
/// from an editor node whose text is its name, so the node edits are the whole
/// reference set — once the last instance node is gone the definition is dead
/// and must not survive into the emitted source.
///
/// Runs to a fixpoint: collecting an outer macro removes its body edits, which
/// can orphan a macro that only the outer one called.
pub(super) fn prune_unreferenced_created_macros(state: &mut PatcherInteractionState) -> bool {
    if state.edit_state.created_macros.is_empty() {
        return false;
    }
    let mut pruned = false;
    loop {
        let referenced = state
            .edit_state
            .nodes
            .values()
            .filter_map(|edit| {
                parse_editor_node_text(edit.text.trim())
                    .ok()
                    .map(|(op, _)| op)
            })
            .collect::<HashSet<_>>();
        let dead = state
            .edit_state
            .created_macros
            .keys()
            .filter(|name| !referenced.contains(name.as_str()))
            // The macro whose view is open stays: its instance may not exist
            // yet (encapsulation stages the definition first), and collecting
            // the view the user is standing in would be jarring.
            .filter(|name| state.active_macro.as_deref() != Some(name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if dead.is_empty() {
            return pruned;
        }
        for name in &dead {
            state.edit_state.created_macros.remove(name);
            discard_macro_view_edits(state, name);
        }
        pruned = true;
    }
}

fn discard_macro_view_edits(state: &mut PatcherInteractionState, macro_name: &str) {
    let view = format!("macro:{macro_name}");
    let prefix = format!("{view}::");
    state
        .edit_state
        .nodes
        .retain(|_, edit| edit.view_key != view);
    state
        .edit_state
        .connections
        .retain(|_, edit| edit.view_key != view);
    state
        .edit_state
        .input_presentations
        .retain(|_, edit| edit.view_key != view);
    state
        .edit_state
        .deleted_nodes
        .retain(|key| !key.starts_with(&prefix));
    state
        .edit_state
        .deleted_connections
        .retain(|key| !key.starts_with(&prefix));
    state.z_order.remove(&view);
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
    state
        .touched_nodes
        .retain(|id| !selected_nodes.contains(id));
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
    infer_modulatable_params(&mut patch);
    desugar_editor_mod_suffix_args(&mut patch);
    drop_orphaned_inline_mod_nodes(&mut patch);
    refresh_patch_inline_inputs(&mut patch);
    patch
}

/// Modulation demand is inferred from the graph rather than declared twice.
///
/// Reaching a param through a `mod` accessor — the user dropping a `mod` node
/// in front of it, or typing the `gain~` shorthand — *is* the statement "this
/// is a host modulation target", so `@mod true @mod-mode additive` is derived
/// from that use instead of hand-written on the param. The generator writes the
/// inferred attributes into the emitted `param` form
/// (`augment_param_label_with_mod` in generate.rs); nothing else reads the
/// inference, so the emitted dsp.lisp stays self-contained.
///
/// Explicit attributes always win: an authored `@mod-mode` is preserved, and
/// `@mod false` opts out entirely — a `gain~` against such a param keeps the
/// diagnostic below.
///
/// Runs before `desugar_editor_mod_suffix_args`, which refuses to expand
/// `gain~` against a param that is not modulatable.
fn infer_modulatable_params(patch: &mut Patch) {
    let demanded = modulation_demanded_param_names(patch);
    if demanded.is_empty() {
        return;
    }
    for node in &mut patch.nodes {
        let label = node.label.clone();
        let Some(param) = node.param.as_mut() else {
            continue;
        };
        if param.modulatable || !demanded.contains(&param.name) || param_opts_out_of_mod(&label) {
            continue;
        }
        param.modulatable = true;
    }
}

/// An authored `@mod false` on the param form: the one way to say "reaching
/// this through `mod` is a mistake, don't infer it".
fn param_opts_out_of_mod(label: &str) -> bool {
    let Ok(items) = parse_editor_node_items(label) else {
        return false;
    };
    items.windows(2).any(|pair| {
        matches!(
            (&pair[0], &pair[1]),
            (Expression::Symbol(key), Expression::Symbol(value))
                if key == "@mod" && value == "false"
        )
    })
}

/// Params read through a `mod` accessor: a `gain~` literal awaiting desugaring,
/// or a `mod` node fed by the param — by cable, or by a name typed straight
/// into the accessor's own text.
fn modulation_demanded_param_names(patch: &Patch) -> HashSet<String> {
    let param_name_by_node = patch
        .nodes
        .iter()
        .filter_map(|node| Some((node.id.as_str(), node.param.as_ref()?.name.as_str())))
        .collect::<HashMap<_, _>>();
    if param_name_by_node.is_empty() {
        return HashSet::new();
    }
    let param_names = param_name_by_node.values().copied().collect::<HashSet<_>>();
    let mod_accessor_ids = patch
        .nodes
        .iter()
        .filter(|node| node.op == "mod" && node.param.is_none())
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();

    let mut demanded = HashSet::new();
    for node in &patch.nodes {
        if matches!(node.kind, NodeKind::Param | NodeKind::Constant) {
            continue;
        }
        let is_accessor = mod_accessor_ids.contains(node.id.as_str());
        for arg in &node.args {
            let (ArgValue::Literal(value) | ArgValue::SymbolRef(value)) = arg else {
                continue;
            };
            match mod_suffix_base(value) {
                Some(base) if param_names.contains(base.as_str()) => {
                    demanded.insert(base);
                }
                // `mod gain` typed into the accessor's text, before any cable.
                _ if is_accessor && param_names.contains(value.as_str()) => {
                    demanded.insert(value.clone());
                }
                _ => {}
            }
        }
    }
    for connection in &patch.connections {
        if mod_accessor_ids.contains(connection.to_node.as_str())
            && let Some(name) = param_name_by_node.get(connection.from_node.as_str())
        {
            demanded.insert((*name).to_string());
        }
    }
    demanded
}

/// `gain~` typed into a node's text is UI-level sugar, not DGenLisp: the real
/// form is `(mod gain)`, and the compiler rejects the bare `gain~` symbol with
/// "unknown symbol". Desugar it here, in the model, into exactly the structure
/// parsing `(* x (mod gain))` from source produces — a `NestedExpr`-owned `mod`
/// accessor node fed by the param, wired into the consumer slot with the
/// `InlineModParam` presentation. Doing it at model level (rather than at
/// emission) keeps the in-memory patch free of bogus `name~` symbols, so
/// display, generation, orphan GC (§4.2b) and reparse round-trip all treat a
/// typed `gain~` identically to a source-authored `(mod gain)`.
///
/// A `name~` whose base is not a modulatable param is left alone and flagged
/// with a node diagnostic — silently emitting it would produce source the
/// DGenLisp compiler rejects. After `infer_modulatable_params` that means one
/// of two things: `name` is not a param at all, or it opts out with an authored
/// `@mod false`.
///
/// The common flow is a *retype*: `(* x gain)` already projects a `gain -> node`
/// edge, so typing `* gain~` must replace that plain reference with the
/// accessor. Only a reference to that same param is replaced — an unrelated
/// cable into the slot still wins, exactly as it does over a typed literal.
fn desugar_editor_mod_suffix_args(patch: &mut Patch) {
    if !patch.nodes.iter().any(|node| {
        node.args
            .iter()
            .any(|arg| matches!(arg, ArgValue::Literal(value) if mod_suffix_base(value).is_some()))
    }) {
        return;
    }

    let param_nodes = patch
        .nodes
        .iter()
        .filter_map(|node| {
            let param = node.param.as_ref()?;
            Some((param.name.clone(), (node.id.clone(), param.modulatable)))
        })
        .collect::<HashMap<_, _>>();
    let mut inbound: HashMap<(String, usize), String> = HashMap::new();
    for connection in &patch.connections {
        inbound
            .entry((connection.to_node.clone(), connection.to_input))
            .or_insert_with(|| connection.from_node.clone());
    }
    let mut taken_ids = patch
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<HashSet<_>>();

    let mut accessors: Vec<PatchNode> = Vec::new();
    let mut cables: Vec<PatchConnection> = Vec::new();
    let mut replaced_slots: HashSet<(String, usize)> = HashSet::new();

    for node in &mut patch.nodes {
        if matches!(node.kind, NodeKind::Param | NodeKind::Constant) {
            continue;
        }
        for idx in 0..node.args.len() {
            let ArgValue::Literal(value) = &node.args[idx] else {
                continue;
            };
            let Some(name) = mod_suffix_base(value) else {
                continue;
            };
            let param_node = param_nodes.get(&name);
            let existing = inbound.get(&(node.id.clone(), idx));
            // An unrelated cable owns the slot; leave the text alone.
            if let Some(from) = existing
                && param_node.is_none_or(|(param_node_id, _)| param_node_id != from)
            {
                continue;
            }
            let Some((param_node_id, true)) = param_node else {
                if node.diagnostic.is_none() {
                    node.diagnostic = Some(format!(
                        "`{name}~` requires `{name}` to be a param that allows modulation (it is not a param, or declares @mod false)"
                    ));
                }
                continue;
            };
            if existing.is_some() {
                replaced_slots.insert((node.id.clone(), idx));
            }
            let mut accessor_id = format!("{}-mod{idx}", node.id);
            while taken_ids.contains(&accessor_id) {
                accessor_id.push('_');
            }
            taken_ids.insert(accessor_id.clone());

            node.args[idx] = ArgValue::ConnectedExpr;
            accessors.push(inline_mod_accessor_node(&accessor_id, node.position));
            cables.push(PatchConnection {
                from_node: param_node_id.clone(),
                from_output: 0,
                to_node: accessor_id.clone(),
                to_input: 0,
                kind: ConnectionKind::Forward,
                segment: None,
                presentation: InputPresentation::Cable,
                presentation_override: None,
                source: None,
            });
            cables.push(PatchConnection {
                from_node: accessor_id,
                from_output: 0,
                to_node: node.id.clone(),
                to_input: idx,
                kind: ConnectionKind::Forward,
                segment: None,
                presentation: InputPresentation::InlineModParam,
                presentation_override: None,
                source: None,
            });
        }
    }

    if !replaced_slots.is_empty() {
        patch.connections.retain(|connection| {
            !replaced_slots.contains(&(connection.to_node.clone(), connection.to_input))
        });
    }
    patch.nodes.extend(accessors);
    patch.connections.extend(cables);
}

/// The projector-synthesized `(mod param)` accessor behind `param~`. `NestedExpr`
/// ownership is what marks it as synthesized rather than user-authored — see
/// `inline_mod_accessor_param` in model.rs.
fn inline_mod_accessor_node(id: &str, position: (f32, f32)) -> PatchNode {
    PatchNode {
        id: id.to_string(),
        op: "mod".to_string(),
        kind: NodeKind::Builtin,
        label: "mod".to_string(),
        args: vec![ArgValue::ConnectedExpr],
        outputs: vec!["out".to_string()],
        position,
        width: None,
        param: None,
        inline_inputs: vec![None],
        diagnostic: None,
        source: Some(NodeSource {
            owner: SourceOwner::NestedExpr {
                expr: SourceExprId {
                    form_id: SourceFormId {
                        scope: SourceScopeId::Root,
                        index: 0,
                    },
                    path: ExprPath::default(),
                },
            },
            expr: None,
            call_shape: None,
        }),
    }
}

/// `gain~` -> `gain`. Rejects a bare `~`, and anything whose base is not a
/// plausible symbol (numbers, punctuation), so literals never get mangled.
fn mod_suffix_base(value: &str) -> Option<String> {
    let base = value.strip_suffix('~')?;
    if base.is_empty() || base.parse::<f64>().is_ok() {
        return None;
    }
    let mut chars = base.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    chars
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        .then(|| base.to_string())
}

/// The projector desugars `gain~` into a hidden `(mod gain)` accessor node
/// nested inside its consumer's expression. Deleting the consumer must take
/// the accessor with it: otherwise the helper the user never authored pops
/// into view as a bare `gain -> mod` node, and regeneration persists it as a
/// standalone `(def mod0 (mod gain))`.
/// See docs/patch-vs-code-editor-spec.md §4.2b.
fn drop_orphaned_inline_mod_nodes(patch: &mut Patch) {
    let orphaned = orphaned_inline_mod_node_ids(patch);
    if orphaned.is_empty() {
        return;
    }
    patch.nodes.retain(|node| !orphaned.contains(&node.id));
    patch.connections.retain(|connection| {
        !orphaned.contains(&connection.from_node) && !orphaned.contains(&connection.to_node)
    });
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

pub(super) fn macro_signatures_with_visual_edits(
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

/// Seed source for a macro whose whole body arrives as created-node edits
/// (Cmd+E encapsulation). `project_defmacro` takes the body as `items[3..]`,
/// so an empty body projects to a `MacroPatch` with no nodes and no params —
/// a clean shell. The default seed's `(* input 1)` would show up inside the
/// macro as junk the user has to delete.
///
/// The seed never reaches disk: `generate_patch_source` re-emits every local
/// macro from its `Patch` model, which by then carries the created nodes.
pub(super) fn empty_created_macro_source(name: &str) -> String {
    format!("(defmacro {name} ())")
}

/// Rename a macro that exists only in the interaction state. Every edit-state
/// key is `"{view_key}::{id}"`, so the macro's whole body has to be re-keyed
/// from `macro:{old}` to `macro:{new}` alongside the registration itself.
///
/// Only created macros can be renamed — a source-backed macro's name is its
/// identity on disk and may be referenced from elsewhere.
pub(super) fn rename_created_macro(
    state: &mut PatcherInteractionState,
    old: &str,
    new: &str,
    taken_names: &HashSet<String>,
) -> bool {
    if old == new || taken_names.contains(new) {
        return false;
    }
    let Some(mut macro_edit) = state.edit_state.created_macros.remove(old) else {
        return false;
    };
    let instance_node_id = macro_edit.instance_node_id.clone();
    macro_edit.name = new.to_string();
    macro_edit.source = macro_edit.source.as_deref().map(|source| {
        if source == empty_created_macro_source(old) {
            empty_created_macro_source(new)
        } else if source == default_created_macro_source(old) {
            default_created_macro_source(new)
        } else {
            source.to_string()
        }
    });
    state
        .edit_state
        .created_macros
        .insert(new.to_string(), macro_edit);

    let old_view = format!("macro:{old}");
    let new_view = format!("macro:{new}");
    let rekey = |key: &str| -> Option<String> {
        key.strip_prefix(&format!("{old_view}::"))
            .map(|rest| format!("{new_view}::{rest}"))
    };

    state.edit_state.nodes = state
        .edit_state
        .nodes
        .drain()
        .map(|(key, mut edit)| {
            let key = rekey(&key).unwrap_or(key);
            if edit.view_key == old_view {
                edit.view_key = new_view.clone();
            }
            (key, edit)
        })
        .collect();
    state.edit_state.connections = state
        .edit_state
        .connections
        .drain()
        .map(|(key, mut edit)| {
            let key = rekey(&key).unwrap_or(key);
            if edit.view_key == old_view {
                edit.view_key = new_view.clone();
            }
            (key, edit)
        })
        .collect();
    state.edit_state.input_presentations = state
        .edit_state
        .input_presentations
        .drain()
        .map(|(key, mut edit)| {
            let key = rekey(&key).unwrap_or(key);
            if edit.view_key == old_view {
                edit.view_key = new_view.clone();
            }
            (key, edit)
        })
        .collect();
    state.edit_state.deleted_nodes = state
        .edit_state
        .deleted_nodes
        .drain()
        .map(|key| rekey(&key).unwrap_or(key))
        .collect();
    state.edit_state.deleted_connections = state
        .edit_state
        .deleted_connections
        .drain()
        .map(|key| rekey(&key).unwrap_or(key))
        .collect();

    if let Some(stack) = state.z_order.remove(&old_view) {
        state.z_order.insert(new_view.clone(), stack);
    }
    if state.active_macro.as_deref() == Some(old) {
        state.active_macro = Some(new.to_string());
    }
    if let Some(edit) = state
        .edit_state
        .nodes
        .values_mut()
        .find(|edit| edit.id == instance_node_id && edit.view_key != new_view)
    {
        edit.text = new.to_string();
    }
    debug_log_edit_event(&format!("rename-created-macro {old} -> {new}"), state);
    true
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
    let source = format!("({})", normalize_editor_node_text(text));
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
