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
//! selection moved on is dropped rather than drawn. Edits that leave the
//! selection alone (accepting a ghost, wiring other nodes) keep the ghosts
//! that still apply rather than re-asking; only a subject port opening that the
//! last request did not cover asks again. The host debounces the queue, so
//! clicking through nodes sends one request for where the selection settles.
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

/// The trimmed `JEV_API_KEY`, or `None` when it is unset or blank. The one
/// rule both the feature gate and the host's HTTP client read, so a stray
/// whitespace-only value never turns the feature on only to fail every call.
pub fn jev_api_key() -> Option<String> {
    std::env::var(JEV_API_KEY_ENV)
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

pub fn jev_suggestions_enabled() -> bool {
    jev_api_key().is_some()
}

/// Whether the widget at `key` is still waiting on the request with
/// `fingerprint`. The host checks this before sending a debounced request and
/// while one is in flight, so a superseded request is neither sent nor keeps
/// the loop at the active cadence.
pub fn jev_suggestion_wanted(key: u64, fingerprint: u64) -> bool {
    super::state::pending_jev_fingerprint_for_key(key) == Some(fingerprint)
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
    /// The subject's open ports (`in0`, `out1`, ...) when the request went
    /// out. Patch edits that only close ports (a ghost accepted, a cable
    /// dragged in) or touch other nodes keep the ghosts; only a port opening
    /// that was not asked about re-queries.
    pub(super) open_ports: Vec<String>,
    /// The subject's label when the request went out. Retyping the node's
    /// operator keeps its id but makes the old answer meaningless.
    pub(super) subject_label: String,
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

/// The subject's free inlets and uncabled outlets, keyed like the request's
/// questions. Cheap enough for every render: no candidate strings, no context.
fn open_subject_ports(
    patch: &Patch,
    node: &super::model::PatchNode,
    drawn_ports: &HashMap<String, Vec<usize>>,
) -> Vec<String> {
    let mut open = arg_occupancies(patch, node, drawn_ports)
        .into_iter()
        .enumerate()
        .filter(|(_, occupancy)| *occupancy == ArgOccupancy::Free)
        .map(|(index, _)| format!("in{index}"))
        .collect::<Vec<_>>();
    for index in 0..node.outputs.len() {
        let cabled = patch.connections.iter().any(|connection| {
            connection.from_node == node.id
                && connection.from_output == index
                && connection.presentation == InputPresentation::Cable
        });
        if !cabled {
            open.push(format!("out{index}"));
        }
    }
    open
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
                    format!(
                        " (already fed by {source} outlet {from_output}; another cable would sum with it)"
                    )
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
/// single node with open ports is newly selected (or a port on it opened that
/// the last request did not ask about), prunes ghosts that edits made moot,
/// clears it when the selection goes away. Per render with the selection
/// unchanged this costs one port scan: the request body is only built when a
/// request will actually be queued. Returns whether `state`
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
    sync_selection(key, state, patch, view_key, subject_for)
}

/// `sync_for_render` past the API-key gate.
fn sync_selection(
    key: u64,
    state: &mut PatcherInteractionState,
    patch: &Patch,
    view_key: &str,
    subject_for: impl FnOnce(&str) -> Option<ConnectSubject>,
) -> bool {
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
    let drawn_ports = patch_input_indices(patch);
    let open_ports = open_subject_ports(patch, node, &drawn_ports);
    if let Some(jev) = state.jev.as_mut()
        && jev.subject_node_id == node.id
        && jev.view_key == view_key
        && jev.subject_label == node_display_label(node)
        && open_ports.iter().all(|port| jev.open_ports.contains(port))
    {
        // Same subject, and nothing opened on it that the pending or landed
        // answer did not cover: the patch moved around it (a ghost accepted,
        // an unrelated edit). Keep the ghosts, dropping the ones the edit
        // made moot, and do not pay for another request.
        let before = jev.cables.len();
        jev.cables
            .retain(|cable| ghost_applicable(patch, &drawn_ports, cable));
        if jev.cables.len() == before {
            return false;
        }
        jev.focused = None;
        return true;
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
        open_ports,
        subject_label: node_display_label(node),
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
    /// Every drawn node's `(col, row, width, height)`. A ghost passing over a
    /// node must not steal the press that selects or drags that node.
    pub(super) node_rects: Vec<(f32, f32, f32, f32)>,
    /// Every drawn real cable's `(start, end)`. A press on a real cable
    /// selects that cable even where a ghost runs alongside it.
    pub(super) real_cables: Vec<((f32, f32), (f32, f32))>,
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
    fn accepting_one_ghost_keeps_the_others_without_requerying() {
        use super::super::state::patch_with_interaction_state;
        let mut base = parse_patch_source(
            "(def trigger (in 1 @name trigger))\n\
             (def pitch (in 2 @name pitch))\n\
             (def env (adsrexp trigger 0.05 3 0 3 1 6))\n\
             (def osc (saw pitch))\n\
             (out (* osc 0.5) 1)",
            PatcherIntent::Instrument,
        )
        .unwrap();
        base.nodes.push(super::super::state::node_from_editor_text(
            "typed-svf",
            "svf",
            (10.0, 10.0),
            &HashMap::new(),
            false,
        ));
        let subject = || {
            Some(ConnectSubject::Operator {
                op: "svf".to_string(),
            })
        };
        let key = 0x6a65_7654;
        let view_key = "root";
        let _ = take_jev_suggestion_requests();
        let mut state = PatcherInteractionState::default();
        state.selected_nodes.insert("typed-svf".to_string());

        let effective = patch_with_interaction_state(base.clone(), &state, view_key);
        assert!(sync_selection(
            key,
            &mut state,
            &effective,
            view_key,
            |_| subject()
        ));
        let requests = take_jev_suggestion_requests();
        assert_eq!(requests.len(), 1);
        // Answer every question with its first real candidate, each on a
        // distinct source so the ghosts are independent.
        let mut answers = JsonMap::new();
        let mut used = Vec::new();
        for (question_id, question) in requests[0].body["questions"].as_object().unwrap() {
            let Some(choice) = question["criteria"]
                .as_object()
                .unwrap()
                .keys()
                .find(|candidate| *candidate != NONE_KEY && !used.contains(*candidate))
                .cloned()
            else {
                continue;
            };
            used.push(choice.clone());
            answers.insert(
                question_id.clone(),
                json!({"type": "choice", "choice": choice, "confidence": 0.9,
                       "probabilities": {choice.as_str(): 0.9}}),
            );
        }
        let jev = state.jev.as_mut().unwrap();
        jev.cables = decode_answers("typed-svf", &json!({ "answers": answers }));
        jev.status = JevStatus::Ready;
        let ghosts = jev.cables.clone();
        assert!(ghosts.len() >= 2, "{ghosts:?}");

        // Connect the first ghost, then let the next render see the patch
        // with the new cable in it.
        assert!(apply_ghost(&mut state, &effective, view_key, &ghosts[0]));
        let effective = patch_with_interaction_state(base.clone(), &state, view_key);
        sync_selection(key, &mut state, &effective, view_key, |_| subject());
        let jev = state.jev.as_ref().unwrap();
        assert_eq!(jev.status, JevStatus::Ready);
        assert_eq!(jev.cables, ghosts[1..].to_vec());
        assert!(
            take_jev_suggestion_requests().is_empty(),
            "accepting a ghost must not send another request"
        );

        // An unrelated edit (another node typed elsewhere on the canvas, which
        // changes the context every request carries) leaves the ghosts alone.
        let mut edited = effective.clone();
        edited
            .nodes
            .push(super::super::state::node_from_editor_text(
                "typed-phasor",
                "phasor",
                (30.0, 30.0),
                &HashMap::new(),
                false,
            ));
        assert_ne!(
            connect_context(&effective, "typed-svf", &subject().unwrap()),
            connect_context(&edited, "typed-svf", &subject().unwrap()),
        );
        assert!(!sync_selection(key, &mut state, &edited, view_key, |_| {
            subject()
        }));
        assert_eq!(state.jev.as_ref().unwrap().cables, ghosts[1..].to_vec());
        assert!(take_jev_suggestion_requests().is_empty());

        // Retyping the subject keeps its id but not its meaning: the old
        // ghosts go and a fresh request is queued.
        let mut retyped = edited.clone();
        let subject_node = retyped
            .nodes
            .iter_mut()
            .find(|node| node.id == "typed-svf")
            .unwrap();
        *subject_node = super::super::state::node_from_editor_text(
            "typed-svf",
            "lores",
            (10.0, 10.0),
            &HashMap::new(),
            false,
        );
        let lores = || {
            Some(ConnectSubject::Operator {
                op: "lores".to_string(),
            })
        };
        assert!(sync_selection(key, &mut state, &retyped, view_key, |_| lores()));
        let jev = state.jev.as_ref().unwrap();
        assert_eq!(jev.status, JevStatus::Pending);
        assert!(jev.cables.is_empty());
        assert_eq!(take_jev_suggestion_requests().len(), 1);
    }

    #[test]
    fn blank_api_key_does_not_enable_suggestions() {
        // Nextest runs each test in its own process, so the variable does
        // not leak into other tests.
        unsafe { std::env::set_var(JEV_API_KEY_ENV, " \n") };
        assert_eq!(jev_api_key(), None);
        assert!(!jev_suggestions_enabled());
        unsafe { std::env::set_var(JEV_API_KEY_ENV, " key \n") };
        assert_eq!(jev_api_key().as_deref(), Some("key"));
        assert!(jev_suggestions_enabled());
        unsafe { std::env::remove_var(JEV_API_KEY_ENV) };
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

/// Whether `ghost` can still be wired into `patch`: both ports exist and this
/// exact cable is not already there.
fn ghost_applicable(
    patch: &Patch,
    drawn: &HashMap<String, Vec<usize>>,
    ghost: &JevGhostCable,
) -> bool {
    // The inlet must still be a port (free or cabled: cables into one inlet
    // sum), and this exact cable must not already exist.
    let target_is_port = patch
        .nodes
        .iter()
        .find(|candidate| candidate.id == ghost.to.node_id)
        .map(|candidate| arg_occupancies(patch, candidate, drawn))
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
    target_is_port && !duplicate && source_exists
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
    let applied = ghost_applicable(patch, &patch_input_indices(patch), ghost);
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
        // Ghosts are drawn over the canvas, not over the nodes' purpose: a
        // press on a node body belongs to that node even where a ghost runs
        // across it.
        let on_node = hits.node_rects.iter().any(|(col, row, width, height)| {
            local_col >= *col
                && local_col <= col + width
                && local_row >= *row
                && local_row <= row + height
        });
        if on_node {
            return None;
        }
        let zoom = hits.zoom.max(0.01);
        let hit_radius = CABLE_HIT_RADIUS_CELLS * zoom;
        let on_real_cable = hits.real_cables.iter().any(|(start, end)| {
            distance_to_cable_px(*start, *end, (local_col, local_row)) <= hit_radius
        });
        if on_real_cable {
            return None;
        }
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
                (distance <= hit_radius).then_some((distance, *index))
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
