//! The capture fixtures' hook: `(present-fixture area fields)` seeds a
//! presented area as a command or job would, by the kind's field names
//! (`song-export`, `settings`, `retro`, `learn`, `editor`,
//! `factory-promote`), so a fixture shows a modal's or a pane's states
//! without running an export, a MIDI service, a capture, a learn job, an
//! edit session or a promotion. The record moves like any typed edit; the
//! host kinds push it after `capture-after-sync`.

use super::*;

/// Register `present-fixture`.
pub(crate) fn register(runtime: &mut Runtime) {
    runtime.register_native("present-fixture", |args, _ctx| {
        let (Some(Value::String(area)), Some(fields)) = (args.first(), args.get(1)) else {
            return Err(
                "present-fixture takes an area name and a dict of its fields"
                    .to_string()
                    .into(),
            );
        };
        present_fixture(area, fields).map_err(|error| format!("present-fixture: {error}"))?;
        Ok(Value::Nil)
    });
}

/// Apply `fields` (a dict by the kind's field names) to `area`.
pub(crate) fn present_fixture(area: &str, fields: &Value) -> Result<(), String> {
    let Value::Map(map) = fields else {
        return Err(format!("{area} takes a dict of fields"));
    };
    let fields: Vec<(&str, Value)> = (map.iter())
        .map(|(name, value)| (name.as_str(), value.borrow().clone()))
        .collect();
    match area {
        "song-export" => {
            let mut view = presented(|p| p.export.get().clone());
            for (name, value) in &fields {
                export_field(&mut view, name, value)?;
            }
            present(|p| &mut p.export, |x| *x = view);
        }
        "settings" => {
            let mut view = presented(|p| p.settings.get().clone());
            for (name, value) in &fields {
                settings_field(&mut view, name, value)?;
            }
            present(|p| &mut p.settings, |s| *s = view);
        }
        "retro" => {
            let mut view = presented(|p| p.retro.get().clone());
            for (name, value) in &fields {
                retro_field(&mut view, name, value)?;
            }
            present(|p| &mut p.retro, |r| *r = view);
        }
        "learn" => {
            let mut view = presented(|p| p.learn.get().clone());
            for (name, value) in &fields {
                learn_field(&mut view, name, value)?;
            }
            present(|p| &mut p.learn, |l| *l = view);
        }
        "editor" => {
            let mut view = presented(|p| p.editor.get().clone());
            let mut sidebar = presented(|p| p.editor_sidebar.get().clone());
            for (name, value) in &fields {
                editor_field(&mut view, &mut sidebar, name, value)?;
            }
            present(|p| &mut p.editor, |e| *e = view);
            present(|p| &mut p.editor_sidebar, |s| *s = sidebar);
        }
        "factory-promote" => {
            let mut view = presented(|p| p.promote.get().clone());
            for (name, value) in &fields {
                promote_field(&mut view, name, value)?;
            }
            present(|p| &mut p.promote, |x| *x = view);
        }
        _ => return Err(format!("no presented area {area}")),
    }
    Ok(())
}

fn export_field(view: &mut ExportView, name: &str, value: &Value) -> Result<(), String> {
    match name {
        "default-name" => view.default_name = text(name, value)?,
        "project" => view.project = text(name, value)?,
        "folder" => view.folder = text(name, value)?,
        "end" => view.end = number(name, value)?,
        "busy" => view.busy = flag(name, value)?,
        "done" => view.done = flag(name, value)?,
        "message" => view.message = text(name, value)?,
        "percent" => view.percent = number(name, value)?,
        "output-name" => view.output_name = text(name, value)?,
        "reveal-label" => view.reveal_label = text(name, value)?,
        _ => return Err(format!("song-export has no field {name}")),
    }
    Ok(())
}

fn promote_field(view: &mut PromoteView, name: &str, value: &Value) -> Result<(), String> {
    match name {
        "target" => view.target = text(name, value)?,
        "destination" => view.destination = text(name, value)?,
        "skipped" => view.skipped = rows(name, value, text)?,
        "blocking" => view.blocking = text(name, value)?,
        "error" => view.error = text(name, value)?,
        "taken" => view.taken = text(name, value)?,
        _ => return Err(format!("factory-promote has no field {name}")),
    }
    Ok(())
}

fn settings_field(view: &mut SettingsView, name: &str, value: &Value) -> Result<(), String> {
    match name {
        "audio-workers-choice" => view.workers_choice = text(name, value)?,
        "audio-workers-options" => view.workers_options = rows(name, value, text)?,
        "audio-workers-note" => view.workers_note = text(name, value)?,
        "midi-devices" => {
            view.midi_devices = rows(name, value, |name, device| {
                Ok(MidiDevice {
                    id: text(name, &entry(device, "device-id")?)?,
                    name: text(name, &entry(device, "name")?)?,
                    enabled: flag(name, &entry(device, "enabled")?)?,
                    connected: flag(name, &entry(device, "connected")?)?,
                    status: text(name, &entry(device, "status")?)?,
                })
            })?;
        }
        "midi-error" => view.midi_error = text(name, value)?,
        "midi-persistent" => view.midi_persistent = flag(name, value)?,
        _ => return Err(format!("settings has no field {name}")),
    }
    Ok(())
}

fn retro_field(view: &mut RetroView, name: &str, value: &Value) -> Result<(), String> {
    match name {
        "lanes" => view.lanes = rows(name, value, text)?,
        "items" => {
            view.items = rows(name, value, |name, item| {
                Ok(RetroItem {
                    lane: number(name, &entry(item, "lane")?)? as usize,
                    start: number(name, &entry(item, "start")?)?,
                    end: number(name, &entry(item, "end")?)?,
                })
            })?;
        }
        "duration" => view.duration = number(name, value)?,
        "truncated" => view.truncated = flag(name, value)?,
        "error" => view.error = text(name, value)?,
        _ => return Err(format!("retro has no field {name}")),
    }
    Ok(())
}

fn learn_field(view: &mut LearnView, name: &str, value: &Value) -> Result<(), String> {
    match name {
        "target-path" => view.target_path = text(name, value)?,
        "target-name" => view.target_name = text(name, value)?,
        "phase" => view.phase = text(name, value)?,
        "stage" => view.stage = text(name, value)?,
        "current-epoch" => view.current_epoch = number(name, value)?,
        "total-epochs" => view.total_epochs = number(name, value)?,
        "loss" => view.loss = number(name, value)?,
        "losses" => view.losses = rows(name, value, number)?,
        "optimization-losses" => view.optimization_losses = rows(name, value, number)?,
        "plan-params" => {
            view.plan_params = rows(name, value, |name, row| {
                Ok(LearnPlanParam {
                    name: text(name, &entry(row, "name")?)?,
                    status: text(name, &entry(row, "status")?)?,
                    reason: text(name, &entry(row, "reason")?)?,
                })
            })?;
        }
        "epoch-params" => {
            view.epoch_params = rows(name, value, |name, row| {
                Ok(LearnEpochParam {
                    name: text(name, &entry(row, "name")?)?,
                    from: number(name, &entry(row, "from")?)?,
                    value: number(name, &entry(row, "value")?)?,
                    change: number(name, &entry(row, "change")?)?,
                    step: number(name, &entry(row, "step")?)?,
                })
            })?;
        }
        "improvement-pct" => view.improvement_pct = number(name, value)?,
        "abs-distance" => view.abs_distance = number(name, value)?,
        "basin-check" => view.basin_check = text(name, value)?,
        "result-deltas" => {
            view.result_deltas = rows(name, value, |name, row| {
                Ok(LearnDelta {
                    name: text(name, &entry(row, "name")?)?,
                    from: number(name, &entry(row, "from")?)?,
                    to: number(name, &entry(row, "to")?)?,
                    change: number(name, &entry(row, "change")?)?,
                })
            })?;
        }
        "seeded-wav" => view.seeded_wav = text(name, value)?,
        "final-wav" => view.final_wav = text(name, value)?,
        "applied" => view.applied = flag(name, value)?,
        "error" => view.error = text(name, value)?,
        // A training setting, typed like the record's.
        _ => {
            let value = match view.setting(name) {
                Some(Value::String(_)) => Value::String(text(name, value)?),
                Some(_) => Value::Number(number(name, value)?),
                None => return Err(format!("learn has no field {name}")),
            };
            view.set_setting(name, value);
        }
    }
    Ok(())
}

/// The `editor` kind's fields a fixture seeds: the open macro view and the
/// macro sidebar (macros as dicts `:name :calls`, a library one's also
/// `:used`; assets `:reference :tier`; the selected asset `:reference` and,
/// each optional, `:tensor-kind :shape :sets :wave-names :waves-per-set
/// :set-count :source`).
fn editor_field(
    view: &mut EditorView,
    sidebar: &mut EditorSidebar,
    name: &str,
    value: &Value,
) -> Result<(), String> {
    let editor_macro = |name: &str, row: &Value| {
        Ok(EditorMacro {
            name: text(name, &entry(row, "name")?)?,
            calls: rows(name, &entry(row, "calls")?, text)?,
            used: optional(row, "used").map_or(Ok(false), |used| flag(name, &used))?,
            ..EditorMacro::default()
        })
    };
    match name {
        "open-macro" => view.open_macro = text(name, value)?,
        "patch-macros" => sidebar.patch_macros = rows(name, value, editor_macro)?,
        "library-macros" => sidebar.library_macros = rows(name, value, editor_macro)?,
        "assets" => {
            sidebar.assets = rows(name, value, |name, row| {
                Ok(EditorAsset {
                    reference: text(name, &entry(row, "reference")?)?,
                    tier: text(name, &entry(row, "tier")?)?,
                    source_path: String::new(),
                })
            })?;
        }
        "selected-asset" => {
            let field = |key: &str| optional(value, key);
            let labels = |key: &str| field(key).map(|v| rows(name, &v, text)).transpose();
            let shape = field("shape").map(|v| rows(name, &v, number)).transpose()?;
            let count = |key: &str| field(key).map(|v| number(name, &v)).transpose();
            let metadata = eseqlisp::editor::AssetMetadata {
                shape: shape
                    .unwrap_or_default()
                    .into_iter()
                    .map(|d| d as u64)
                    .collect(),
                kind: field("tensor-kind").map(|v| text(name, &v)).transpose()?,
                source: field("source").map(|v| text(name, &v)).transpose()?,
                waves_per_set: count("waves-per-set")?.map(|n| n as usize),
                set_count: count("set-count")?.unwrap_or(1.0) as usize,
                sets: labels("sets")?,
                wave_names: labels("wave-names")?,
                ..Default::default()
            };
            sidebar.selected_asset = Some(AssetInfo {
                reference: text(name, &entry(value, "reference")?)?,
                metadata: Some(metadata),
            });
        }
        _ => return Err(format!("editor has no field {name}")),
    }
    Ok(())
}

/// `map`'s `key`, when it has one.
fn optional(map: &Value, key: &str) -> Option<Value> {
    entry(map, key).ok()
}

fn text(name: &str, value: &Value) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        _ => Err(format!("{name} takes a string, not {value:?}")),
    }
}

fn number(name: &str, value: &Value) -> Result<f64, String> {
    match value {
        Value::Number(n) => Ok(*n),
        _ => Err(format!("{name} takes a number, not {value:?}")),
    }
}

fn flag(name: &str, value: &Value) -> Result<bool, String> {
    match value {
        Value::Bool(on) => Ok(*on),
        _ => Err(format!("{name} takes true or false, not {value:?}")),
    }
}

fn rows<T>(
    name: &str,
    value: &Value,
    row: impl Fn(&str, &Value) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    match value {
        Value::List(items) => items.iter().map(|item| row(name, &item.borrow())).collect(),
        _ => Err(format!("{name} takes a list, not {value:?}")),
    }
}

fn entry(map: &Value, key: &str) -> Result<Value, String> {
    match map {
        Value::Map(fields) => (fields.get(key))
            .map(|cell| cell.borrow().clone())
            .ok_or_else(|| format!("missing :{key}")),
        _ => Err(format!("expected a dict with :{key}, not {map:?}")),
    }
}
