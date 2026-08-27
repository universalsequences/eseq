use crate::*;

pub(super) const COMMANDS: &[&str] = &["scene-slot-history-write"];

pub(super) fn handle(
    _name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    _ctx: &mut LoopCtx<'_>,
) {
    match apply_scene_slot_history_host_command(app, &payload) {
        Ok(app::edit::EditOutcome::Applied(_)) | Ok(app::edit::EditOutcome::NoOp) => {}
        Ok(app::edit::EditOutcome::AppliedUnrecorded) => {
            editor.handle_host_event(HostEvent::Error(
                "Scene-slot edit was applied without history".to_string(),
            ));
        }
        Err(error) => editor.handle_host_event(HostEvent::Error(format!(
            "Scene-slot edit could not be recorded: {error}"
        ))),
    }
}

pub(crate) fn apply_scene_slot_history_host_command(
    app: &mut app::App,
    payload: &Value,
) -> Result<app::edit::EditOutcome, String> {
    let Value::Map(map) = payload else {
        return Err("invalid payload".to_string());
    };
    let field = |name: &str| map.get(name).map(|cell| cell.borrow().clone());
    let scene = match field("scene-id") {
        Some(Value::String(value)) => value
            .parse::<u64>()
            .map(sequencer::sequencer::SceneId)
            .map_err(|_| "invalid pattern identity".to_string())?,
        _ => return Err("missing pattern identity".to_string()),
    };
    let name = match field("slot") {
        Some(Value::String(value)) => value,
        _ => return Err("missing slot name".to_string()),
    };
    let old_present = match field("old-present") {
        Some(Value::Bool(value)) => value,
        _ => return Err("missing previous-value presence".to_string()),
    };
    let before = if old_present {
        Some(sequencer::process::ProcessLiteral::from_value(
            &field("old").ok_or_else(|| "missing previous value".to_string())?,
        )?)
    } else {
        None
    };
    let after = sequencer::process::ProcessLiteral::from_value(
        &field("new").ok_or_else(|| "missing new value".to_string())?,
    )?;
    app.record_applied_scene_slot_write(scene, name, before, after)
        .map_err(|error| format!("{error:?}"))
}
