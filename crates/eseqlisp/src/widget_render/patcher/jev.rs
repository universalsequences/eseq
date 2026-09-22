//! Ghost-cable suggestions from typesafe.ai's Jev model (bead eseq-c049).
//!
//! When exactly one node is selected and it has free inlets or unconnected
//! outlets, the render pass fans out one `choice` question per open port
//! ("which existing outlet should feed this inlet?", "which free inlet should
//! this outlet feed?") against the same §5 context string the connect bubble
//! sends (docs/patcher-agentic-connect-spec.md). Answers come back as ranked
//! cables drawn translucently over the canvas. Clicking a ghost focuses it and
//! shows Connect / × chips at its midpoint; Connect applies it through
//! `allocate_created_connection`, the same primitive a cable drag uses.
//!
//! Nothing here talks to the network. The render pass enqueues request bodies
//! into a thread-local queue the host drains once per frame
//! (`take_jev_suggestion_requests`), and the host hands the raw JSON answer
//! back through `resolve_jev_suggestions`. Every request carries a
//! fingerprint of (view, subject, context) so an answer that arrives after the
//! selection or the patch moved on is dropped rather than drawn.
//!
//! Enabled only when `JEV_API_KEY` is set in the environment.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use serde_json::{Map as JsonMap, Value as Json, json};

use super::connect::{ArgOccupancy, arg_occupancies, connect_context, inlet_name};
use super::display::node_display_label;
use super::geometry::patch_input_indices;
use super::model::{
    InputPortRef, InputPresentation, NodeKind, OutputPortRef, Patch, hidden_inline_node_ids,
};
use super::state::{
    ConnectSubject, PatcherInteractionState, get_patcher_interaction_state,
    set_patcher_interaction_state_without_history,
};

pub const JEV_API_KEY_ENV: &str = "JEV_API_KEY";

/// Model id sent with every request.
const JEV_MODEL: &str = "jev-latest";

/// Answers below this probability are not worth a ghost: at three or four
/// candidates a flat distribution still puts ~0.3 on the top pick.
const MIN_GHOST_PROBABILITY: f32 = 0.3;

/// Choice questions take at most 255 options; one slot is `none`.
const MAX_CANDIDATES: usize = 254;

const NONE_KEY: &str = "none";

/// How far from a ghost's endpoints a press still belongs to the port under
/// it (unscaled cells). Generous on purpose: a missed ghost click costs one
/// more click, a stolen cable drag costs a whole gesture.
const JEV_GHOST_END_GUARD_CELLS: f32 = 1.2;

pub fn jev_suggestions_enabled() -> bool {
    std::env::var_os(JEV_API_KEY_ENV).is_some_and(|value| !value.is_empty())
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct JevGhostCable {
    pub(super) from: OutputPortRef,
    pub(super) to: InputPortRef,
    pub(super) probability: f32,
    pub(super) confidence: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum JevStatus {
    Pending,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct JevSuggestionState {
    pub(super) fingerprint: u64,
    pub(super) view_key: String,
    pub(super) subject_node_id: String,
    pub(super) status: JevStatus,
    pub(super) cables: Vec<JevGhostCable>,
    /// Index into `cables` of the ghost showing its Connect / × chips.
    pub(super) focused: Option<usize>,
}

/// One request the host should send. `body` is the full `/v1/systemone`
/// payload; the host adds the model key and authorization.
pub struct JevSuggestionRequest {
    pub key: u64,
    pub fingerprint: u64,
    pub body: Json,
}

thread_local! {
    static JEV_REQUESTS: RefCell<Vec<JevSuggestionRequest>> = const { RefCell::new(Vec::new()) };
}

/// Drain the requests the last frames queued. Called by the host once per
/// frame; each request is sent on its own worker thread.
pub fn take_jev_suggestion_requests() -> Vec<JevSuggestionRequest> {
    JEV_REQUESTS.with(|cell| std::mem::take(&mut *cell.borrow_mut()))
}

fn fingerprint_for(view_key: &str, subject_node_id: &str, context: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    view_key.hash(&mut hasher);
    subject_node_id.hash(&mut hasher);
    context.hash(&mut hasher);
    hasher.finish()
}

fn port_key(node_id: &str, index: usize) -> String {
    format!("{node_id}:{index}")
}

fn parse_port_key(key: &str) -> Option<(String, usize)> {
    let (node_id, index) = key.rsplit_once(':')?;
    Some((node_id.to_string(), index.parse().ok()?))
}

fn choice_question(instructions: String, criteria: JsonMap<String, Json>) -> Json {
    json!({
        "type": "choice",
        "instructions": instructions,
        "criteria": Json::Object(criteria),
    })
}

/// Build the fan-out request for `subject_node_id`, or `None` when the node has
/// no open port worth asking about. Returns the fingerprint alongside so the
/// caller can dedupe against what is already pending.
pub(super) fn build_request(
    patch: &Patch,
    view_key: &str,
    subject_node_id: &str,
    subject: &ConnectSubject,
) -> Option<(u64, Json)> {
    let node = patch.nodes.iter().find(|node| node.id == subject_node_id)?;
    let drawn_ports = patch_input_indices(patch);
    let hidden = hidden_inline_node_ids(patch);
    let subject_label = node_display_label(node);
    let others = patch
        .nodes
        .iter()
        .filter(|other| other.id != node.id && !hidden.contains(&other.id))
        .collect::<Vec<_>>();

    // Every outlet on the level but the subject's own, as inlet candidates.
    let mut outlet_criteria = JsonMap::new();
    for other in &others {
        let label = node_display_label(other);
        for (index, name) in other.outputs.iter().enumerate() {
            if outlet_criteria.len() >= MAX_CANDIDATES {
                break;
            }
            outlet_criteria.insert(
                port_key(&other.id, index),
                Json::String(format!("outlet {index} '{name}' of node {label}")),
            );
        }
    }
    // Every drawn inlet on the level but the subject's own, as outlet
    // candidates. Cabled inlets count: several cables into one inlet sum, and
    // a feedback loop through a history node has to land on an inlet that is
    // already fed from the other direction.
    let mut inlet_criteria = JsonMap::new();
    for other in &others {
        if matches!(other.kind, NodeKind::MacroDefinition | NodeKind::CodeIsland) {
            continue;
        }
        let label = node_display_label(other);
        for (index, occupancy) in arg_occupancies(patch, other, &drawn_ports)
            .into_iter()
            .enumerate()
        {
            if inlet_criteria.len() >= MAX_CANDIDATES {
                break;
            }
            let fed_by = match &occupancy {
                ArgOccupancy::Free => String::new(),
                ArgOccupancy::Cabled {
                    from_node,
                    from_output,
                } => {
                    if from_node == &node.id {
                        // Already wired from the subject; not a proposal.
                        continue;
                    }
                    let source = patch
                        .nodes
                        .iter()
                        .find(|candidate| &candidate.id == from_node)
                        .map(node_display_label)
                        .unwrap_or_else(|| from_node.clone());
                    format!(" (already fed by {source} outlet {from_output}; another cable would sum with it)")
                }
                _ => continue,
            };
            inlet_criteria.insert(
                port_key(&other.id, index),
                Json::String(format!(
                    "inlet {index} '{}' of node {label}{fed_by}",
                    inlet_name(patch, other, index)
                )),
            );
        }
    }

    let mut questions = JsonMap::new();
    if !outlet_criteria.is_empty() {
        for (index, occupancy) in arg_occupancies(patch, node, &drawn_ports)
            .into_iter()
            .enumerate()
        {
            if occupancy != ArgOccupancy::Free {
                continue;
            }
            let mut criteria = outlet_criteria.clone();
            criteria.insert(
                NONE_KEY.to_string(),
                Json::String("leave this inlet unconnected (a literal value or nothing)".into()),
            );
            questions.insert(
                format!("in{index}"),
                choice_question(
                    format!(
                        "Which existing outlet should feed inlet {index} '{}' of the selected \
                         node {subject_label}? Choose none if nothing on the canvas belongs \
                         there.",
                        inlet_name(patch, node, index)
                    ),
                    criteria,
                ),
            );
        }
    }
    if !inlet_criteria.is_empty() {
        for (index, name) in node.outputs.iter().enumerate() {
            let cabled = patch.connections.iter().any(|connection| {
                connection.from_node == node.id
                    && connection.from_output == index
                    && connection.presentation == InputPresentation::Cable
            });
            if cabled {
                continue;
            }
            let mut criteria = inlet_criteria.clone();
            criteria.insert(
                NONE_KEY.to_string(),
                Json::String("leave this outlet unconnected".into()),
            );
            questions.insert(
                format!("out{index}"),
                choice_question(
                    format!(
                        "Which free inlet should outlet {index} '{name}' of the selected node \
                         {subject_label} feed? Choose none if nothing on the canvas should \
                         take it."
                    ),
                    criteria,
                ),
            );
        }
    }
    if questions.is_empty() {
        return None;
    }

    let context = connect_context(patch, subject_node_id, subject);
    let fingerprint = fingerprint_for(view_key, subject_node_id, &context);
    let state = format!(
        "A modular DSP patch (dgenlisp) in a patch editor. Cables carry signals from a node's \
         outlet (out) to another node's inlet (in). Inlets are argument slots: 'free' means a \
         port is drawn and nothing is cabled to it. An outlet may feed any number of inlets, \
         and an inlet may take several cables, which sum. A 'history' node is a one-sample \
         delay whose outlet is read before its inlet is written, so feeding its inlet from \
         something downstream of its outlet is the normal way to build a feedback loop. The \
         user just selected node {subject_node_id} ({subject_label}). Propose which existing \
         ports it should be wired to, one decision per open port.\n\n{context}"
    );
    let body = json!({
        "state": state,
        "model": JEV_MODEL,
        "questions": Json::Object(questions),
    });
    Some((fingerprint, body))
}

/// Turn a `/v1/systemone` response into ranked ghost cables for the subject.
pub(super) fn decode_answers(subject_node_id: &str, response: &Json) -> Vec<JevGhostCable> {
    let Some(answers) = response.get("answers").and_then(Json::as_object) else {
        return Vec::new();
    };
    let mut cables: Vec<JevGhostCable> = Vec::new();
    for (question_id, answer) in answers {
        let Some(choice) = answer.get("choice").and_then(Json::as_str) else {
            continue;
        };
        if choice == NONE_KEY {
            continue;
        }
        let probability = answer
            .get("probabilities")
            .and_then(|probabilities| probabilities.get(choice))
            .and_then(Json::as_f64)
            .unwrap_or(0.0) as f32;
        let confidence = answer
            .get("confidence")
            .and_then(Json::as_f64)
            .unwrap_or(0.0) as f32;
        if probability < MIN_GHOST_PROBABILITY {
            continue;
        }
        let Some((other_id, other_index)) = parse_port_key(choice) else {
            continue;
        };
        let cable = if let Some(index) = question_id.strip_prefix("in") {
            let Ok(input_index) = index.parse::<usize>() else {
                continue;
            };
            JevGhostCable {
                from: OutputPortRef {
                    node_id: other_id,
                    output_index: other_index,
                },
                to: InputPortRef {
                    node_id: subject_node_id.to_string(),
                    input_index,
                },
                probability,
                confidence,
            }
        } else if let Some(index) = question_id.strip_prefix("out") {
            let Ok(output_index) = index.parse::<usize>() else {
                continue;
            };
            JevGhostCable {
                from: OutputPortRef {
                    node_id: subject_node_id.to_string(),
                    output_index,
                },
                to: InputPortRef {
                    node_id: other_id,
                    input_index: other_index,
                },
                probability,
                confidence,
            }
        } else {
            continue;
        };
        // One ghost per inlet: an inlet takes one cable, so keep the stronger
        // proposal when the inlet and outlet questions both land on it.
        if let Some(existing) = cables.iter_mut().find(|existing| existing.to == cable.to) {
            if cable.probability > existing.probability {
                *existing = cable;
            }
            continue;
        }
        cables.push(cable);
    }
    cables.sort_by(|a, b| b.probability.total_cmp(&a.probability));
    cables
}

/// Called from the render pass with the effective patch for the active view.
/// Keeps `state.jev` in step with the selection: queues a request when a
/// single node with open ports is newly selected (or the patch around it
/// changed), clears it when the selection goes away. Returns whether `state`
/// changed so the caller can persist it.
pub(super) fn sync_for_render(
    key: u64,
    state: &mut PatcherInteractionState,
    patch: &Patch,
    view_key: &str,
    subject_for: impl FnOnce(&str) -> Option<ConnectSubject>,
) -> bool {
    if !jev_suggestions_enabled() {
        return false;
    }
    // Mid-gesture the selection is not settled; leave whatever is there alone.
    if state.drag.is_some() || state.text_edit.is_some() {
        return false;
    }
    let selected = patch
        .nodes
        .iter()
        .filter(|node| state.selected_nodes.contains(&node.id))
        .collect::<Vec<_>>();
    let [node] = selected.as_slice() else {
        return clear(state);
    };
    if matches!(
        node.kind,
        NodeKind::MacroDefinition | NodeKind::CodeIsland | NodeKind::Constant
    ) {
        return clear(state);
    }
    let Some(subject) = subject_for(&node.id) else {
        return clear(state);
    };
    let Some((fingerprint, body)) = build_request(patch, view_key, &node.id, &subject) else {
        return clear(state);
    };
    if state
        .jev
        .as_ref()
        .is_some_and(|jev| jev.fingerprint == fingerprint)
    {
        return false;
    }
    state.jev = Some(JevSuggestionState {
        fingerprint,
        view_key: view_key.to_string(),
        subject_node_id: node.id.clone(),
        status: JevStatus::Pending,
        cables: Vec::new(),
        focused: None,
    });
    JEV_REQUESTS.with(|cell| {
        cell.borrow_mut().push(JevSuggestionRequest {
            key,
            fingerprint,
            body,
        })
    });
    true
}

fn clear(state: &mut PatcherInteractionState) -> bool {
    if state.jev.is_some() {
        state.jev = None;
        true
    } else {
        false
    }
}

/// Host callback with the answer for one request. Ignored unless the request
/// is still the one the widget is waiting on. Returns whether anything visible
/// changed.
pub fn resolve_jev_suggestions(key: u64, fingerprint: u64, result: Result<Json, String>) -> bool {
    let mut state = get_patcher_interaction_state(key);
    let Some(jev) = state.jev.as_mut() else {
        return false;
    };
    if jev.fingerprint != fingerprint {
        return false;
    }
    match result {
        Ok(response) => {
            jev.cables = decode_answers(&jev.subject_node_id, &response);
            jev.status = JevStatus::Ready;
        }
        Err(error) => {
            jev.cables.clear();
            jev.status = JevStatus::Failed(error);
        }
    }
    jev.focused = None;
    set_patcher_interaction_state_without_history(key, state);
    true
}

/// The chips the last frame drew at the focused ghost's midpoint, and every
/// ghost's endpoints, in widget-local cell space, for pointer hit testing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum JevButtonKind {
    Connect,
    Dismiss,
}

#[derive(Clone, Debug, Default)]
pub(super) struct JevOverlayHits {
    /// `(ghost index, kind, (col, row, width, height))`
    pub(super) buttons: Vec<(usize, JevButtonKind, (f32, f32, f32, f32))>,
    /// `(ghost index, start, end)`
    pub(super) cables: Vec<(usize, (f32, f32), (f32, f32))>,
    pub(super) zoom: f32,
}

thread_local! {
    static JEV_OVERLAY_HITS: RefCell<HashMap<u64, JevOverlayHits>> = RefCell::new(HashMap::new());
}

pub(super) fn set_overlay_hits(key: u64, hits: JevOverlayHits) {
    JEV_OVERLAY_HITS.with(|cell| {
        cell.borrow_mut().insert(key, hits);
    });
}

pub(super) fn overlay_hits(key: u64) -> JevOverlayHits {
    JEV_OVERLAY_HITS.with(|cell| cell.borrow().get(&key).cloned().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::super::lisp::parse_patch_source;
    use super::super::model::PatcherIntent;
    use super::*;

    #[test]
    fn request_asks_one_choice_per_open_port_with_none() {
        let mut patch = parse_patch_source(
            "(def trigger (in 1 @name trigger))\n\
             (def pitch (in 2 @name pitch))\n\
             (def env (adsrexp trigger 0.05 3 0 3 1 6))\n\
             (def osc (saw pitch))\n\
             (out (* osc 0.5) 1)",
            PatcherIntent::Instrument,
        )
        .unwrap();
        // A freshly typed `svf` with nothing wired: placeholder args draw
        // free inlets, and its outlet is unconnected. That is the case the
        // feature exists for.
        let typed = super::super::state::node_from_editor_text(
            "typed-svf",
            "svf",
            (10.0, 10.0),
            &HashMap::new(),
            false,
        );
        patch.nodes.push(typed);
        let subject = ConnectSubject::Operator {
            op: "svf".to_string(),
        };
        let (fingerprint, body) = build_request(&patch, "root", "typed-svf", &subject)
            .expect("an unwired svf has open ports");
        assert_ne!(fingerprint, 0);
        assert_eq!(body["model"], JEV_MODEL);
        assert!(body["state"].as_str().unwrap().contains("patch:"));
        let questions = body["questions"].as_object().unwrap();
        // The signal inlet is free and asks over every other outlet.
        let in0 = questions.get("in0").expect("inlet question");
        assert_eq!(in0["type"], "choice");
        let criteria = in0["criteria"].as_object().unwrap();
        assert!(criteria.contains_key(NONE_KEY));
        assert!(
            criteria
                .keys()
                .any(|key| key.ends_with(":0") && key != NONE_KEY)
        );
        // The outlet asks over every drawn inlet, cabled ones included, since
        // cables into one inlet sum; a cabled candidate says what feeds it.
        let out0 = questions.get("out0").expect("outlet question");
        let targets = out0["criteria"].as_object().unwrap();
        let cabled = targets
            .iter()
            .find(|(key, _)| key.as_str() != NONE_KEY)
            .expect("a cabled inlet is offered");
        assert!(
            cabled.1.as_str().unwrap().contains("already fed by"),
            "{cabled:?}"
        );
        // Every candidate must round-trip to a port the decoder can address.
        for key in criteria.keys().filter(|key| key.as_str() != NONE_KEY) {
            let (node_id, index) = parse_port_key(key).expect("port key");
            assert!(patch.nodes.iter().any(|node| node.id == node_id), "{key}");
            let _ = index;
        }
        if let Ok(dump) = std::env::var("ESEQ_JEV_DUMP_REQUEST") {
            std::fs::write(dump, serde_json::to_string_pretty(&body).unwrap()).unwrap();
        }
    }

    #[test]
    fn decode_keeps_one_ghost_per_inlet_and_drops_none() {
        let response = json!({
            "answers": {
                "in0": {"type": "choice", "choice": "trig:0", "confidence": 0.9,
                        "probabilities": {"trig:0": 0.9, "none": 0.1}},
                "in1": {"type": "choice", "choice": "none", "confidence": 0.6,
                        "probabilities": {"trig:0": 0.4, "none": 0.6}},
                "out0": {"type": "choice", "choice": "vca:1", "confidence": 0.8,
                         "probabilities": {"vca:1": 0.8, "none": 0.2}},
                "in2": {"type": "choice", "choice": "lfo:0", "confidence": 0.2,
                        "probabilities": {"lfo:0": 0.2, "none": 0.8}},
            }
        });
        let cables = decode_answers("subject", &response);
        assert_eq!(cables.len(), 2);
        assert_eq!(cables[0].from.node_id, "trig");
        assert_eq!(cables[0].to.node_id, "subject");
        assert_eq!(cables[0].to.input_index, 0);
        assert_eq!(cables[1].from.node_id, "subject");
        assert_eq!(cables[1].to.node_id, "vca");
        assert_eq!(cables[1].to.input_index, 1);
    }
}

/// Wire one ghost into `state` if its ports still exist and the inlet is still
/// free; otherwise drop it. Returns whether a connection was allocated.
fn apply_ghost(
    state: &mut PatcherInteractionState,
    patch: &Patch,
    view_key: &str,
    ghost: &JevGhostCable,
) -> bool {
    use super::state::allocate_created_connection;
    let drawn = patch_input_indices(patch);
    // The inlet must still be a port (free or cabled: cables into one inlet
    // sum), and this exact cable must not already exist.
    let target_is_port = patch
        .nodes
        .iter()
        .find(|candidate| candidate.id == ghost.to.node_id)
        .map(|candidate| arg_occupancies(patch, candidate, &drawn))
        .and_then(|occupancies| occupancies.get(ghost.to.input_index).cloned())
        .is_some_and(|occupancy| {
            matches!(occupancy, ArgOccupancy::Free | ArgOccupancy::Cabled { .. })
        });
    let duplicate = super::connect::connection_exists(
        patch,
        &ghost.from.node_id,
        ghost.from.output_index,
        &ghost.to.node_id,
        ghost.to.input_index,
    );
    let source_exists = patch.nodes.iter().any(|candidate| {
        candidate.id == ghost.from.node_id && ghost.from.output_index < candidate.outputs.len()
    });
    let applied = target_is_port && !duplicate && source_exists;
    if applied {
        allocate_created_connection(state, view_key, ghost.from.clone(), ghost.to.clone());
    }
    if let Some(jev) = state.jev.as_mut() {
        jev.cables.retain(|cable| cable.to != ghost.to);
        jev.focused = None;
    }
    applied
}

/// Tab with ghosts on screen: accept every proposal in one gesture. Ghosts
/// are ranked, so when two target the same inlet the stronger one lands
/// first and the other is dropped as no longer free. Returns `None` when
/// there was nothing to accept so the key falls through.
pub(super) fn accept_all_ghosts(
    node: &crate::layout::LayoutNode,
) -> Option<crate::widget_render::WidgetEvent> {
    use super::interaction::{PatcherChangeKind, load_interactive_patch_for_node};
    use super::state::patcher_state_key;
    let key = patcher_state_key(node);
    let ghosts = {
        let state = get_patcher_interaction_state(key);
        let jev = state.jev.as_ref()?;
        if jev.status != JevStatus::Ready || jev.cables.is_empty() {
            return None;
        }
        jev.cables.clone()
    };
    let (mut patch, _, view_key) = load_interactive_patch_for_node(node)?;
    let mut state = get_patcher_interaction_state(key);
    if view_key != state.jev.as_ref()?.view_key {
        return None;
    }
    let mut applied_any = false;
    for ghost in &ghosts {
        if apply_ghost(&mut state, &patch, &view_key, ghost) {
            applied_any = true;
            // Later ghosts must see this inlet as taken.
            patch.connections.push(super::model::PatchConnection {
                from_node: ghost.from.node_id.clone(),
                from_output: ghost.from.output_index,
                to_node: ghost.to.node_id.clone(),
                to_input: ghost.to.input_index,
                kind: super::model::ConnectionKind::Forward,
                segment: None,
                presentation: InputPresentation::Cable,
                presentation_override: None,
                source: None,
                authored_reference: None,
            });
        }
    }
    super::state::set_patcher_interaction_state(key, state);
    Some(super::patcher_widget_event(if applied_any {
        PatcherChangeKind::Semantic
    } else {
        PatcherChangeKind::None
    }))
}

/// Pointer-down over the ghost overlay. Claims the click when it lands on a
/// Connect / × chip or on a ghost cable; otherwise `None` so the canvas takes
/// it (which, on empty space, clears the selection and with it the ghosts).
pub(super) fn handle_jev_click(
    node: &crate::layout::LayoutNode,
    local_col: f32,
    local_row: f32,
) -> Option<crate::widget_render::WidgetEvent> {
    use super::interaction::{PatcherChangeKind, load_interactive_patch_for_node};
    use super::metrics::CABLE_HIT_RADIUS_CELLS;
    use super::state::patcher_state_key;
    use crate::vm::Value;
    use crate::widget_render::WidgetEvent;
    use crate::widget_render::cable::distance_to_cable_px;

    let key = patcher_state_key(node);
    let mut state = get_patcher_interaction_state(key);
    let jev = state.jev.as_ref()?;
    if jev.status != JevStatus::Ready || jev.cables.is_empty() {
        return None;
    }
    let hits = overlay_hits(key);
    const HIT_PAD_CELLS: f32 = 0.22;
    let button = hits
        .buttons
        .iter()
        .find(|(_, _, (col, row, width, height))| {
            local_col >= col - HIT_PAD_CELLS
                && local_col <= col + width + HIT_PAD_CELLS
                && local_row >= row - HIT_PAD_CELLS
                && local_row <= row + height + HIT_PAD_CELLS
        })
        .copied();
    if let Some((index, kind, _)) = button {
        let jev = state.jev.as_mut()?;
        let Some(ghost) = jev.cables.get(index).cloned() else {
            return None;
        };
        match kind {
            JevButtonKind::Dismiss => {
                jev.cables.remove(index);
                jev.focused = None;
                set_patcher_interaction_state_without_history(key, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            JevButtonKind::Connect => {
                let (patch, _, view_key) = load_interactive_patch_for_node(node)?;
                // Re-read after the loader wrote back z-order.
                let mut state = get_patcher_interaction_state(key);
                if view_key != state.jev.as_ref()?.view_key {
                    return None;
                }
                if !apply_ghost(&mut state, &patch, &view_key, &ghost) {
                    set_patcher_interaction_state_without_history(key, state);
                    return Some(WidgetEvent::Custom(Value::Nil));
                }
                // Through the history-recording write: this is a real edit,
                // and one Cmd+Z should take it back like a dragged cable.
                super::state::set_patcher_interaction_state(key, state);
                Some(super::patcher_widget_event(PatcherChangeKind::Semantic))
            }
        }
    } else {
        let zoom = hits.zoom.max(0.01);
        // A ghost runs into the very ports it proposes, and a press there is
        // the start of a cable drag, not a click on the ghost. Leave the
        // ends alone so the port underneath keeps the pointer.
        let port_guard = JEV_GHOST_END_GUARD_CELLS * zoom;
        let near = |point: (f32, f32)| {
            (point.0 - local_col).abs() <= port_guard && (point.1 - local_row).abs() <= port_guard
        };
        let hit = hits
            .cables
            .iter()
            .filter(|(_, start, end)| !near(*start) && !near(*end))
            .filter_map(|(index, start, end)| {
                let distance = distance_to_cable_px(*start, *end, (local_col, local_row));
                (distance <= CABLE_HIT_RADIUS_CELLS * zoom).then_some((distance, *index))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, index)| index)?;
        let jev = state.jev.as_mut()?;
        jev.focused = if jev.focused == Some(hit) {
            None
        } else {
            Some(hit)
        };
        set_patcher_interaction_state_without_history(key, state);
        Some(WidgetEvent::Custom(Value::Nil))
    }
}
