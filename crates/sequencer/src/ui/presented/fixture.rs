//! The capture fixtures' hook: `(present-fixture area fields)` seeds a
//! presented area as a command or job would, by the kind's field names
//! (`song-export`, `settings`, `retro`), so a fixture shows a modal's states
//! without running an export, a MIDI service or a capture. The record moves
//! like any typed edit and the legacy mirror follows (applied when the
//! native returns).

use super::*;

/// Register `present-fixture`.
pub(crate) fn register(runtime: &mut Runtime) {
    runtime.register_native("present-fixture", |args, ctx| {
        let (Some(Value::String(area)), Some(fields)) = (args.first(), args.get(1)) else {
            return Err(
                "present-fixture takes an area name and a dict of its fields"
                    .to_string()
                    .into(),
            );
        };
        present_fixture(ctx, area, fields).map_err(|error| format!("present-fixture: {error}"))?;
        Ok(Value::Nil)
    });
}

/// Apply `fields` (a dict by the kind's field names) to `area`.
pub(crate) fn present_fixture(
    sink: &mut dyn legacy::Sink,
    area: &str,
    fields: &Value,
) -> Result<(), String> {
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
            present(
                sink,
                |p| &mut p.export,
                |x| *x = view,
                legacy::mirror_export,
            );
        }
        "settings" => {
            let mut view = presented(|p| p.settings.get().clone());
            for (name, value) in &fields {
                settings_field(&mut view, name, value)?;
            }
            present(
                sink,
                |p| &mut p.settings,
                |s| *s = view,
                legacy::mirror_settings,
            );
        }
        "retro" => {
            let mut view = presented(|p| p.retro.get().clone());
            for (name, value) in &fields {
                retro_field(&mut view, name, value)?;
            }
            present(sink, |p| &mut p.retro, |r| *r = view, legacy::mirror_retro);
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
