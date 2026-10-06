//! The host kinds' process lane setters (kind-bindings spec §14.2h):
//! `edit-process`, behind `process.enabled`, `inlet.value`, `fanout.lo` /
//! `hi` and the actions `set-process-enabled!`, `set-inlet!`,
//! `set-lane-steps!`, `move-process!`, `add-process!`, `remove-process!`,
//! `bind-port!`, `add-fanout!`, `unbind-port!`, `clear-port!` and
//! `remove-fanout!`.
//!
//! A command names its track by its stable `TrackId` (`:track-id`) and its
//! process by its stable instance id (`:proc-id`, matched as the number
//! Lisp holds, so an id beyond 2^53 still resolves), both resolved when it
//! lands: a reorder in between cannot retarget it, and a gone track or
//! process is an error. A port target names its param, send or process the
//! same way, with its track's id (a param by its device's `device-target`,
//! resolved to the device's slot now): a port targets its own track, as
//! does `move-process!`'s `before`. Setters are absolute: each acts only
//! where the model differs, through the legacy edits
//! ([`super::process_edit::apply_process_edit`], shared with
//! `process-history-action`: one recorded scene-structure entry, undo
//! restores; `app::edit::apply_process_lane_drag_steps` for lane steps)
//! and queues their invalidations. `:all true` edits the shared project
//! slot (every track; a track's own process refuses it), else a project
//! slot forks for this track; moving a project lane moves it on every
//! track. Values follow the value rule (§14.2c): an inlet or lane value is
//! a number of its type (a gate 0 or 1, or a bool; an int, track or enum an
//! integer, an enum's below its option count) within the class's declared
//! range; the current value always round-trips. Gestures as
//! [`super::ScriptEdit`]: lane steps set while the pointer is down join one
//! undo entry (a drag); every other edit is its own entry.

use super::devices::addressed;
use super::process_edit::{apply_process_edit, ProcessEdit};
use super::track_settings::{command_track, SetValue};
use super::ScriptEdit;
use crate::*;
use sequencer::process::{
    ParamTarget, ProcessInstanceId, ProcessPortFanout, PublishedProcessInletDef, TrackProcessChain,
    TrackProcessSlot,
};
use std::collections::HashMap;

pub(super) const COMMANDS: &[&str] = &["edit-process"];

type Payload = HashMap<String, Rc<RefCell<Value>>>;

/// The slot of `chain` whose id is the number at `key` (as Lisp holds it).
fn slot_at<'a>(
    chain: &'a TrackProcessChain,
    map: &Payload,
    key: &str,
) -> Result<&'a TrackProcessSlot, String> {
    let id = map_number(map, key).ok_or_else(|| format!("needs :{key}"))?;
    let mut found = chain
        .slots
        .iter()
        .filter(|slot| slot.instance_id.0 as f64 == id);
    match (found.next(), found.next()) {
        (Some(slot), None) => Ok(slot),
        (Some(_), Some(_)) => Err(format!("process {id} is ambiguous")),
        (None, _) => Err("the process is gone".to_string()),
    }
}

/// Whether `target` names the command's own track (`:track-id` at `key`).
fn on_own_track(map: &Payload, target: &Payload, key: &str) -> bool {
    let own = map_usize(map, "track-id");
    own.is_some() && map_usize(target, key) == own
}

/// A process inlet or lane value under the value rule, or `None` when it
/// is `current` (a no-op). `declared` is the class's own range.
fn inlet_value(
    value: &SetValue<'_>,
    kind: &str,
    options: usize,
    declared: Option<(f32, f32)>,
    current: Option<f32>,
) -> Result<Option<f64>, String> {
    let given = match value.value() {
        Value::Bool(on) if kind == "gate" => f64::from(u8::from(*on)),
        _ => value.finite()?,
    };
    if current.is_some_and(|current| current as f64 == given) {
        return Ok(None);
    }
    let checked = match kind {
        "gate" => value_in(value, given, 0.0, 1.0, true)?,
        "enum" if options == 0 => return value.fail("an option index (it has no options)"),
        "enum" => value_in(value, given, 0.0, (options - 1) as f64, true)?,
        "int" | "track" => {
            let (min, max) = declared.map_or((f64::MIN, f64::MAX), |(min, max)| {
                (f64::from(min), f64::from(max))
            });
            value_in(value, given, min, max, true)?
        }
        _ => match declared {
            Some((min, max)) => value_in(value, given, f64::from(min), f64::from(max), false)?,
            None => given,
        },
    };
    Ok(Some(checked))
}

fn value_in(
    value: &SetValue<'_>,
    given: f64,
    min: f64,
    max: f64,
    integer: bool,
) -> Result<f64, String> {
    if (min..=max).contains(&given) && (!integer || given.fract() == 0.0) {
        return Ok(given);
    }
    let what = if integer { "an integer" } else { "a number" };
    let range = match (min > f64::MIN, max < f64::MAX) {
        (true, true) => format!(" from {min} to {max}"),
        _ => String::new(),
    };
    value.fail(&format!("{what}{range}"))
}

/// What an inlet's class declares: its type name, option count and range.
fn inlet_rule(
    inlet: Option<&PublishedProcessInletDef>,
) -> (&'static str, usize, Option<(f32, f32)>) {
    let kind = inlet.map_or("float", |inlet| process_inlet_kind_name(&inlet.kind));
    let options = match inlet.map(|inlet| &inlet.kind) {
        Some(sequencer::process::ProcessInletKind::Enum(options)) => options.len(),
        _ => 0,
    };
    let declared = inlet.and_then(|inlet| Some((inlet.min?, inlet.max?)));
    (kind, options, declared)
}

/// The port `:target` names, resolved now on `track` (whose chain is
/// `chain`; `writer` is the slot binding it).
fn port_target(
    app: &app::App,
    track: usize,
    chain: &TrackProcessChain,
    writer: &TrackProcessSlot,
    map: &Payload,
) -> Result<ParamTarget, String> {
    let Some(Value::Map(target)) = map.get("target").map(|cell| cell.borrow().clone()) else {
        return Err("needs a :target".to_string());
    };
    let kind = map_string(&target, "kind").ok_or("the target needs a :kind")?;
    let own_track = || {
        on_own_track(map, &target, "track-id")
            .then_some(())
            .ok_or_else(|| "a port targets its own track".to_string())
    };
    match kind.as_str() {
        "step-param" => {
            let name = SetValue::of(&target, "param", "step param");
            let param = crate::param_words::canonical_step_param_name(name.name()?)
                .map_or_else(|| name.fail("one of project.step-param-options"), Ok)?;
            Ok(ParamTarget::StepParam {
                param: param.to_string(),
            })
        }
        "inlet" => {
            own_track()?;
            let reader = slot_at(chain, &target, "proc-id")?;
            let inlet = SetValue::of(&target, "inlet", "inlet").name()?.to_string();
            if reader.project_layer != writer.project_layer {
                return Err("a wire stays within its layer (project or track lanes)".to_string());
            }
            if reader.instance_id == writer.instance_id {
                return Err("a process cannot wire into itself".to_string());
            }
            Ok(ParamTarget::ProcessInlet {
                process: reader.class_name.clone(),
                inlet,
                instance_id: Some(reader.instance_id),
            })
        }
        "param" => {
            let device = addressed(app, &target)?;
            let track_id = app.track_registry.ids().get(track).copied();
            if device.track_id.is_none() || device.track_id != track_id {
                return Err("a process writes its own track's params only".to_string());
            }
            let param = SetValue::of(&target, "param-idx", "param").integer(0, usize::MAX >> 1)?;
            let state = &app.state;
            match device.device {
                DeviceSlot::Instrument => {
                    natives::instrument_process_target(state, track, param, None)
                }
                DeviceSlot::Effect(slot) => {
                    natives::effect_process_target(state, track, slot, param, (None, None))
                }
                DeviceSlot::MidiFx(slot) => {
                    natives::midi_fx_process_target(state, track, slot, param, (None, None))
                }
                _ => {
                    Err("a drum rack slot's or bus effect's param is no process target".to_string())
                }
            }
        }
        "bus-send" => {
            own_track()?;
            let bus = SetValue::of(&target, "bus-id", "send").id("a bus id")?;
            if !app.buses.iter().any(|known| known.id.0 == bus) {
                return Err(format!("no bus {bus}"));
            }
            natives::bus_send_process_target(bus)
        }
        other => Err(format!("unknown target kind '{other}'")),
    }
}

/// The fan-out entry `index` of `slot`'s port `port`.
fn fanout_entry<'a>(
    slot: &'a TrackProcessSlot,
    port: &str,
    index: usize,
) -> Result<&'a ProcessPortFanout, String> {
    (slot.fanout.get(port))
        .and_then(|list| list.get(index))
        .ok_or_else(|| "the fan-out entry is gone".to_string())
}

/// What one `edit-process` asks for on `track`: the slot it edits and the
/// edit, or `None` when the model already is so.
fn process_request(
    app: &app::App,
    track: usize,
    all: bool,
    map: &Payload,
) -> Result<Option<(ProcessInstanceId, ProcessEdit)>, String> {
    let op = map_string(map, "op").ok_or("needs an :op")?;
    let state = &app.state;
    if op == "add" {
        let class = SetValue::of(map, "class", "class").name()?.to_string();
        if !natives::process_class_is_known(state, &class) {
            return Err(format!("no process class '{class}'"));
        }
        let id = state.next_track_roster_slot_id();
        let edit = ProcessEdit::AddRosterSlot { class_name: class };
        return Ok(Some((id, edit)));
    }
    let chain = state
        .composed_track_process_chain(track)
        .ok_or("the track is gone")?;
    let slot = slot_at(&chain, map, "proc-id")?;
    let id = slot.instance_id;
    if all && !slot.project_layer {
        return Err(":all edits a project lane; this is the track's own".to_string());
    }
    let published = || state.published_process_authoring();
    let port = || -> Result<(String, sequencer::process::ProcessPortDef), String> {
        let name = SetValue::of(map, "port", "port").name()?.to_string();
        let def = process_slot_port_defs(slot, process_slot_def(&published(), slot))
            .into_iter()
            .find(|port| port.name == name)
            .ok_or_else(|| format!("no port '{name}'"))?;
        Ok((name, def))
    };
    // A shared (`:all`) edit addresses the project layer's own slot (this
    // track's fork may differ from it).
    let shared_slot = || {
        (state.project_process_chain().slots.into_iter())
            .find(|shared| shared.instance_id == id)
            .ok_or_else(|| "the process is gone".to_string())
    };
    // Per-track edits compare with this track's slot; a shared edit is a
    // no-op only when the model says so when it lands.
    let unchanged = |same: bool| !all && same;
    let edit = match op.as_str() {
        "enabled" => {
            let enabled = SetValue::of(map, "value", "enabled").flag()?;
            if unchanged(enabled == slot.enabled) {
                return Ok(None);
            }
            ProcessEdit::SetEnabled(enabled)
        }
        "inlet" => {
            let name = SetValue::of(map, "inlet", "inlet").name()?.to_string();
            let published = published();
            let def = process_slot_def(&published, slot);
            let inlet = process_inlet_def(def, &name);
            let view = process_scalar_inlet_view(slot, def, &name, inlet)
                .filter(|_| !slot.lanes.contains_key(&name) && !inlet.is_some_and(|i| i.lane))
                .ok_or_else(|| format!("no numeric inlet '{name}'"))?;
            let (kind, options, declared) = inlet_rule(inlet);
            let value = SetValue::of(map, "value", &name);
            let current = (!all).then_some(view.value);
            let Some(value) = inlet_value(&value, kind, options, declared, current)? else {
                return Ok(None);
            };
            let literal = sequencer::process::ProcessLiteral::Number(value);
            ProcessEdit::SetInlet {
                inlet: name,
                literal,
            }
        }
        "fanout-lo" | "fanout-hi" => {
            let (name, _) = port()?;
            let index = SetValue::of(map, "index", "fan-out").integer(0, usize::MAX >> 1)?;
            let lo = op == "fanout-lo";
            let value = SetValue::of(map, "value", if lo { "lo" } else { "hi" });
            let value = value.finite()? as f32;
            if all {
                fanout_entry(&shared_slot()?, &name, index)?;
            } else {
                let entry = fanout_entry(slot, &name, index)?;
                if value == if lo { entry.lo } else { entry.hi } {
                    return Ok(None);
                }
            }
            let (lo, hi) = if lo {
                (Some(value), None)
            } else {
                (None, Some(value))
            };
            ProcessEdit::SetFanoutRange {
                port: name,
                index,
                lo,
                hi,
            }
        }
        "move" => {
            let at = chain.slots.iter().position(|other| other.instance_id == id);
            let before = match map.get("before").map(|cell| cell.borrow().clone()) {
                None | Some(Value::Nil) => None,
                Some(_) => {
                    if !on_own_track(map, map, "before-track-id") {
                        return Err("a process moves within its own track".to_string());
                    }
                    let before = slot_at(&chain, map, "before")?;
                    if before.project_layer != slot.project_layer {
                        return Err(
                            "a process moves within its layer (project or track lanes)".to_string()
                        );
                    }
                    Some(before.instance_id)
                }
            };
            let next = at
                .and_then(|at| chain.slots.get(at + 1))
                .map(|next| next.instance_id);
            if before == Some(id) || next == before {
                return Ok(None);
            }
            ProcessEdit::MoveSlot { before }
        }
        "remove" => ProcessEdit::RemoveSlot,
        "bind" | "add-fanout" => {
            let (name, def) = port()?;
            let target = port_target(app, track, &chain, slot, map)?;
            if !def.allows_binding_target(&target) {
                let wants = def
                    .effective_target_kind()
                    .map_or("any", |kind| kind.as_str());
                return Err(format!("port '{name}' takes {wants} targets"));
            }
            if op == "add-fanout" {
                ProcessEdit::AddFanout {
                    port: name,
                    target,
                    lo: None,
                    hi: None,
                }
            } else {
                let view = process_port_view(slot, &def);
                if unchanged(view.binding == Some(&target) && !view.disconnected) {
                    return Ok(None);
                }
                ProcessEdit::BindPort { port: name, target }
            }
        }
        "unbind" => {
            let (name, def) = port()?;
            if unchanged(process_port_view(slot, &def).disconnected) {
                return Ok(None);
            }
            ProcessEdit::UnbindPort { port: name }
        }
        "clear" => {
            let (name, def) = port()?;
            if unchanged(!process_port_view(slot, &def).clearable()) {
                return Ok(None);
            }
            ProcessEdit::ClearPortBinding { port: name }
        }
        "remove-fanout" => {
            let (name, _) = port()?;
            let index = SetValue::of(map, "index", "fan-out").integer(0, usize::MAX >> 1)?;
            match all {
                true => fanout_entry(&shared_slot()?, &name, index)?,
                false => fanout_entry(slot, &name, index)?,
            };
            ProcessEdit::RemoveFanout { port: name, index }
        }
        other => return Err(format!("unknown process edit '{other}'")),
    };
    Ok(Some((id, edit)))
}

/// `set-lane-steps!`: `{:inlet :steps :step-tracks :value}`, steps of the
/// process's track; only the steps holding another value change.
fn set_lane_steps(
    app: &mut app::App,
    ctx: &mut LoopCtx<'_>,
    track: usize,
    map: &Payload,
) -> Result<(), String> {
    let state = &app.state;
    let chain = state
        .composed_track_process_chain(track)
        .ok_or("the track is gone")?;
    let slot = slot_at(&chain, map, "proc-id")?;
    let id = slot.instance_id;
    let inlet = SetValue::of(map, "inlet", "inlet").name()?.to_string();
    let published = state.published_process_authoring();
    let entries = process_lane_entries_for_chain(state, track, &chain, &published);
    let entry = entries
        .iter()
        .find(|entry| entry.instance_id == id && entry.inlet_name == inlet)
        .ok_or_else(|| format!("no lane '{inlet}'"))?;
    let track_id = app.track_registry.id_at(track).ok_or("the track is gone")?;
    let steps = super::track_steps(map, track_id, "the lane's track")?;
    let num_steps = state.pattern.track_params[track]
        .get_num_steps()
        .min(MAX_STEPS);
    if let Some(step) = steps.iter().find(|step| **step >= num_steps) {
        return Err(format!("step {step} is past the track's end"));
    }
    let def = process_inlet_def(process_slot_def(&published, slot), &inlet);
    let (kind, options, declared) = inlet_rule(def);
    let value = SetValue::of(map, "value", &inlet);
    let current = |step: usize| entry.values.get(step).copied();
    // The value every step holds now round-trips (a no-op).
    let first = steps.first().and_then(|step| current(*step));
    let shared_current =
        first.filter(|first| steps.iter().all(|step| current(*step) == Some(*first)));
    let Some(value) = inlet_value(&value, kind, options, declared, shared_current)? else {
        return Ok(());
    };
    let value = value as f32;
    let changed: Vec<usize> = steps
        .into_iter()
        .filter(|step| current(*step) != Some(value))
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    let changed = super::step_list(changed);
    let script = ScriptEdit::begin(app, ctx);
    let drags = script.drags(ctx, true);
    let result = script.apply_with(app, |app| {
        let result =
            app::edit::apply_process_lane_drag_steps(app, track, id, &inlet, &changed, value);
        if !drags {
            app::edit::finish_active_gesture(app);
        }
        result
    });
    let landed = result.is_ok();
    if landed {
        ctx.shared
            .ui_invalidations
            .push(UiInvalidation::ProcessLaneValues { track });
    }
    script.end(app, ctx, true, landed);
    result
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let Value::Map(map) = &payload else {
        editor.handle_host_event(HostEvent::Error(format!(
            "{name}: the payload is not a dict"
        )));
        return;
    };
    let result = command_track(app, map).and_then(|track| {
        if map_string(map, "op").as_deref() == Some("lane-steps") {
            return set_lane_steps(app, ctx, track, map);
        }
        let all = SetValue::of(map, "all", "all").flag_or(false)?;
        let Some((id, edit)) = process_request(app, track, all, map)? else {
            return Ok(());
        };
        let script = ScriptEdit::begin(app, ctx);
        let result = script.apply_with(app, |app| apply_process_edit(app, track, id, all, edit));
        let changed = matches!(result, Ok(true));
        if changed {
            ctx.shared
                .ui_invalidations
                .push(UiInvalidation::ProcessChain { track });
        }
        script.end(app, ctx, false, changed);
        result.map(|_| ())
    });
    if let Err(message) = result {
        editor.handle_host_event(HostEvent::Error(format!("{name}: {message}")));
    }
}
