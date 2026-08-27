use crate::*;

pub(super) const COMMANDS: &[&str] = &[
    "create-scene-bank",
    "rename-scene-bank",
    "delete-scene-bank",
    "move-scene-to-scene-bank",
];

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let result = match name {
        "create-scene-bank" => app
            .apply_recorded_scene_structure_mutation("Create scene bank", |app| {
                app.state.create_scene_bank()
            })
            .map(|id| format!("Created scene bank {}", id.0)),
        "rename-scene-bank" => {
            let bank = scene_bank_id(&payload);
            let name = optional_name(&payload);
            match (bank, name) {
                (Some(bank), Some(name)) => app
                    .apply_recorded_scene_structure_mutation("Rename scene bank", |app| {
                        app.state.rename_scene_bank(bank, name)
                    })
                    .map(|()| format!("Renamed scene bank {}", bank.0)),
                (None, _) => Err("Scene bank id is missing or invalid".to_string()),
                (_, None) => Err("Scene bank name is missing or invalid".to_string()),
            }
        }
        "delete-scene-bank" => match scene_bank_id(&payload) {
            Some(bank) => app
                .apply_recorded_scene_structure_mutation("Delete scene bank", |app| {
                    app.state.delete_scene_bank(bank)
                })
                .map(|()| format!("Deleted scene bank {}", bank.0)),
            None => Err("Scene bank id is missing or invalid".to_string()),
        },
        "move-scene-to-scene-bank" => {
            let scene = payload_usize(&payload, "scene");
            let bank = scene_bank_id(&payload);
            match (scene, bank) {
                (Some(scene), Some(bank)) => {
                    let moved = app.apply_recorded_scene_structure_mutation(
                        "Move scene to scene bank",
                        |app| {
                            let target = app.state.move_scene_to_scene_bank(scene, bank)?;
                            app.handle_scene_reordered(scene, target);
                            Ok(target)
                        },
                    );
                    moved.map(|target| {
                        format!(
                            "Moved scene {} to scene bank {} at position {}",
                            scene + 1,
                            bank.0,
                            target + 1
                        )
                    })
                }
                (None, _) => Err("Scene index is missing or invalid".to_string()),
                (_, None) => Err("Target scene bank id is missing or invalid".to_string()),
            }
        }
        _ => return,
    };

    match result {
        Ok(status) => {
            let rt = editor.runtime_mut();
            sync_pattern_state(rt, &ctx.shared.state);
            rt.run_reactive_cycle();
            editor.refresh_runtime_side_effects();
            ctx.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
            editor.handle_host_event(HostEvent::Status(status));
        }
        Err(error) => editor.handle_host_event(HostEvent::Status(format!(
            "Scene bank edit failed: {error}"
        ))),
    }
}

fn scene_bank_id(payload: &Value) -> Option<sequencer::sequencer::SceneBankId> {
    ["bank-id", "bank_id", "target-bank-id", "target_bank_id"]
        .iter()
        .find_map(|key| payload_usize(payload, key))
        .and_then(|id| (id != 0).then_some(sequencer::sequencer::SceneBankId(id as u64)))
}

fn payload_usize(payload: &Value, key: &str) -> Option<usize> {
    let Value::Map(map) = payload else {
        return None;
    };
    map.get(key).and_then(|cell| match &*cell.borrow() {
        Value::Number(value) if value.is_finite() && *value >= 0.0 => Some(*value as usize),
        _ => None,
    })
}

/// `Nil` and an empty/whitespace string both clear the optional user name.
/// The outer option distinguishes a deliberate clear from a malformed payload.
fn optional_name(payload: &Value) -> Option<Option<String>> {
    let Value::Map(map) = payload else {
        return None;
    };
    map.get("name").and_then(|cell| match &*cell.borrow() {
        Value::Nil => Some(None),
        Value::String(name) => Some((!name.trim().is_empty()).then(|| name.trim().to_string())),
        _ => None,
    })
}
