//! History for graph node process-chain edits (eseq-waa9.23). The
//! `graph-node-process-*` natives the expr edit buffer calls (add, inlet,
//! expr-set, rebind, edit-as-expr) apply their edit to the node's chain at
//! once and enqueue the chain before/after it; this records that as one undo
//! entry (a picker drag as one coalesced gesture). Replay goes through
//! `EditPatch::GraphNodeProcessChain`.

use crate::*;

pub(super) const COMMANDS: &[&str] = &[sequencer::lisp_host::GRAPH_NODE_PROCESS_HISTORY_COMMAND];

pub(super) fn handle(
    _name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    _ctx: &mut LoopCtx<'_>,
) {
    match apply_graph_node_process_history_host_command(app, &payload) {
        Ok(app::edit::EditOutcome::Applied(_)) | Ok(app::edit::EditOutcome::NoOp) => {}
        Ok(app::edit::EditOutcome::AppliedUnrecorded) => {
            editor.handle_host_event(HostEvent::Error(
                "Node process edit was applied without history".to_string(),
            ));
        }
        Err(error) => editor.handle_host_event(HostEvent::Error(format!(
            "Node process edit could not be recorded: {error}"
        ))),
    }
}

pub(crate) fn apply_graph_node_process_history_host_command(
    app: &mut app::App,
    payload: &Value,
) -> Result<app::edit::EditOutcome, String> {
    let Value::Map(map) = payload else {
        return Err("invalid payload".to_string());
    };
    let field = |name: &str| map.get(name).map(|cell| cell.borrow().clone());
    let id = |name: &str| match field(name) {
        Some(Value::String(value)) => value.parse::<u64>().map_err(|_| format!("invalid {name}")),
        _ => Err(format!("missing {name}")),
    };
    let chain = |name: &str| -> Result<Option<sequencer::process::TrackProcessChain>, String> {
        match field(name) {
            Some(Value::String(json)) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|error| format!("invalid {name} chain: {error}")),
            Some(Value::Nil) | None => Ok(None),
            Some(_) => Err(format!("invalid {name} chain")),
        }
    };
    let node = match field("node") {
        Some(Value::Number(node)) if node >= 0.0 => node as usize,
        _ => return Err("missing node".to_string()),
    };
    let merge = match field("merge") {
        Some(Value::String(key)) => Some(key),
        _ => None,
    };
    let patch = app::history::GraphNodeProcessChainPatch {
        scene: sequencer::sequencer::SceneId(id("scene-id")?),
        sequencer_id: id("sequencer-id")?,
        node,
        before: chain("before")?,
        after: chain("after")?,
    };
    app.record_applied_graph_node_process_edit(patch, merge)
        .map_err(|error| format!("{error:?}"))
}
