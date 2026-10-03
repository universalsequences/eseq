use crate::*;

pub(super) const COMMANDS: &[&str] = &["scene-slot-history-write", "apply-scene-transpose"];

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    _ctx: &mut LoopCtx<'_>,
) {
    if name == "apply-scene-transpose" {
        match apply_scene_transpose_host_command(app, &payload) {
            Ok(_) => {
                let slot = sequencer::sequencer::SCENE_TRANSPOSE_SLOT;
                let epoch = app.state.current_scene_slots().epoch(slot);
                let result = editor.runtime_mut().invalidate_reactive_source(
                    sequencer::lisp_host::SCENE_SLOT_REACTIVE_NAMESPACE,
                    slot, Value::String(epoch.to_string()));
                if let Err(error) = result {
                    editor.handle_host_event(HostEvent::Error(format!("Scene transpose repaint failed: {error:?}")));
                }
                editor.refresh_runtime_side_effects();
            }
            Err(error) => editor.handle_host_event(HostEvent::Error(error)),
        }
        return;
    }
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
    if let Some(slots) = field("slots") {
        let Value::List(slots) = slots else {
            return Err("invalid slot list".to_string());
        };
        let mut writes = Vec::with_capacity(slots.len());
        for slot in slots {
            writes.push(scene_slot_history_write(&slot.borrow())?);
        }
        return app
            .record_applied_scene_slot_writes(scene, writes)
            .map_err(|error| format!("{error:?}"));
    }
    let (name, before, after) = scene_slot_history_write(payload)?;
    app.record_applied_scene_slot_write(scene, name, before, after)
        .map_err(|error| format!("{error:?}"))
}

/// One `{slot old-present old new}` write of a scene-slot history payload.
#[allow(clippy::type_complexity)]
fn scene_slot_history_write(
    payload: &Value,
) -> Result<
    (
        String,
        Option<sequencer::process::ProcessLiteral>,
        sequencer::process::ProcessLiteral,
    ),
    String,
> {
    let Value::Map(map) = payload else {
        return Err("invalid slot write".to_string());
    };
    let field = |name: &str| map.get(name).map(|cell| cell.borrow().clone());
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
    Ok((name, before, after))
}

fn apply_scene_transpose_host_command(
    app: &mut app::App,
    payload: &Value,
) -> Result<app::edit::EditOutcome, String> {
    let Value::Map(map) = payload else {
        return Err("invalid scene transpose payload".into());
    };
    let field = |name: &str| map.get(name).map(|cell| cell.borrow().clone());
    let value = match field("value") {
        Some(Value::Number(value)) => value,
        _ => return Err("scene transpose value is missing".into()),
    };
    let bank = match field("scope") {
        Some(Value::String(scope)) if scope == "all-banks" => None,
        Some(Value::String(scope)) if scope == "bank" => {
            match field("bank-id") {
                Some(Value::Number(id)) if id.is_finite() && id > 0.0
                    && id.fract() == 0.0 && id < u64::MAX as f64 => {
                    Some(sequencer::sequencer::SceneBankId(id as u64))
                }
                _ => return Err("scene bank identity is missing or invalid".into()),
            }
        }
        _ => return Err("scene transpose scope is missing or invalid".into()),
    };
    app.apply_scene_transpose_to_bank(bank, value)
}
