//! The host kinds' device setters (kind-bindings spec §14.2b, §14.2f):
//! `set-device-param` (`param.base`), `set-device-param-locks` /
//! `clear-device-param-locks` (`lock-param!` / `unlock-param!`) and
//! `set-device` (`device.voices`, `device.delete-target`, `device.base-note`).
//!
//! A device is named by its owner's stable id, `:track-id` (a track's
//! chain, MIDI effects, drum rack slots and their effects) or `:bus-id` (a
//! bus's effects), and its `did` (`:device`), all resolved when the command
//! lands ([`DeviceSlot::resolve`], [`DeviceSlot::resolve_bus`]), so a
//! reorder in between cannot retarget it; a device or param that is gone
//! is an error (nothing changes). A placeholder `did` (a device with no
//! identity bound yet) names the device at its family and position while
//! one is there, even once a first edit has bound it, so several edits of
//! one unbound device in one eval all land. A MIDI effect's descriptor
//! comes from the host kinds' cache (`DeviceState::midi_fx_descriptors`):
//! no command reads a file. Param values are display units (percent
//! params ×100), clamped, rounded for enum and boolean params (`true` /
//! `false` read as 1 / 0); a non-finite value is an error. Setters are
//! absolute: each acts only where the model differs (steps already holding
//! the lock, or holding none to clear, are left alone), through the knob
//! edits' history commands (`Set…Param`, `Set…PlockMulti` /
//! `Clear…PlockMulti`, one undo entry for all steps; a bus effect through
//! its recorded value mutation), and refreshes what the legacy commands
//! refresh (their invalidations, the rack panel's direct fields, the bus
//! value field; shared helpers). Bus effects take no p-locks, and a rack
//! slot's instrument p-locks have no clear command yet: both are errors.
//!
//! Gestures as [`super::ScriptEdit`]: a base edit or a voices edit while
//! the pointer is down stays open and later script edits join it (a drag
//! view's `set!` per frame is one undo entry, which the release ends); any
//! other script edit ends its entry at once. An edit landing while another
//! gesture is active (a user's knob drag) gets an entry of its own beside
//! it.

use super::rack::{rack_param_applied, rack_slot_voices_applied};
use super::routing::bus_effect_param_applied;
use super::track_settings::SetValue;
use super::{apply_device_param_base, clear_plocks_command, step_list, ScriptEdit};
use crate::*;
use std::collections::HashMap;

pub(super) const COMMANDS: &[&str] = &[
    "set-device-param",
    "set-device-param-locks",
    "clear-device-param-locks",
    "set-device",
];

type Payload = HashMap<String, Rc<RefCell<Value>>>;

/// The device a command names, resolved now.
pub(super) struct Addressed {
    /// The track's position, or the bus's for a bus effect.
    pub(super) owner: usize,
    pub(super) device: DeviceSlot,
    /// The owning track's id (`None` for a bus effect).
    pub(super) track_id: Option<sequencer::sequencer::TrackId>,
}

/// The device a `device-target` payload names (`:track-id` or `:bus-id`,
/// `:device`), resolved now.
pub(super) fn addressed(app: &app::App, map: &Payload) -> Result<Addressed, String> {
    let did = map_usize(map, "device").ok_or("needs :device")? as u64;
    if let Some(bus) = map_usize(map, "bus-id") {
        let bus_id = sequencer::sequencer::BusId(bus as u64);
        let (owner, device) =
            DeviceSlot::resolve_bus(app, bus_id, did).ok_or("the device is gone")?;
        return Ok(Addressed {
            owner,
            device,
            track_id: None,
        });
    }
    let track = map_usize(map, "track-id").ok_or("needs :track-id or :bus-id")?;
    let track_id = sequencer::sequencer::TrackId(track as u64);
    let (owner, device) = DeviceSlot::resolve(app, track_id, did).ok_or("the device is gone")?;
    Ok(Addressed {
        owner,
        device,
        track_id: Some(track_id),
    })
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let Value::Map(map) = &payload else {
        return;
    };
    let result = match name {
        "set-device" => device_edit(map, app, editor, ctx),
        _ => param_edit(name, map, app, editor, ctx),
    };
    if let Err(message) = result {
        editor.handle_host_event(HostEvent::Error(format!("{name}: {message}")));
    }
}

/// The rack panel field a rack slot device's param repaints.
fn rack_target(device: DeviceSlot, param_idx: usize) -> Option<RackDirectDisplayTarget> {
    match device {
        DeviceSlot::RackSlot(slot_idx) => Some(RackDirectDisplayTarget::InstrumentParam {
            slot_idx,
            param_idx,
        }),
        DeviceSlot::RackEffect { rack_slot, slot } => Some(RackDirectDisplayTarget::EffectParam {
            rack_slot,
            effect_slot: slot,
            param_idx,
        }),
        _ => None,
    }
}

/// `set-device-param` and the p-lock edits (see the module docs).
fn param_edit(
    name: &str,
    map: &Payload,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) -> Result<(), String> {
    let Addressed {
        owner,
        device,
        track_id,
    } = addressed(app, map)?;
    let param_idx = map_usize(map, "param-idx").ok_or("needs :param-idx")?;
    // A MIDI effect's descriptor from the host kinds' cache (no file read
    // per command).
    let midi_fx = match device {
        DeviceSlot::MidiFx(_) => ctx.frame.host_kinds.devices.midi_fx_descriptors(),
        _ => Rc::from([]),
    };
    let pdesc = device
        .descriptor(app, owner, &midi_fx)
        .and_then(|desc| desc.params.get(param_idx).cloned())
        .ok_or("the device has no such param")?;
    let value = match map_number_or_bool(map, "value") {
        Some(value) if !value.is_finite() => return Err("the value is not finite".into()),
        value => value.map(|value| DeviceSlot::from_user_clamped(&pdesc, value as f32)),
    };
    let state = app.state.clone();
    if name == "set-device-param" {
        let value = value.ok_or("needs a :value")?;
        let current = device
            .with_values(&state, &app.buses, owner, |values| {
                values.base(&pdesc, param_idx)
            })
            .ok_or("the device is gone")?;
        if current == value {
            return Ok(());
        }
        let script = ScriptEdit::begin(app, ctx);
        let edit = (owner, device, param_idx);
        let changed = base_edit(app, editor, ctx, &script, edit, &pdesc, value);
        script.end(app, ctx, true, changed);
        return Ok(());
    }
    // Only a bus effect has no track.
    let Some(track_id) = track_id else {
        return Err("bus effects take no p-locks".into());
    };
    let steps = map_usize_list(map, "steps").unwrap_or_default();
    let tracks = map_usize_list(map, "step-tracks").unwrap_or_default();
    if tracks.len() != steps.len() || tracks.iter().any(|tid| *tid as u64 != track_id.0) {
        return Err("steps must be steps of the param's track".into());
    }
    // The step the rack panel shows, and whether it held a lock before
    // (a rack lock's row refresh, as the rack knobs do).
    let shown = rack_target(device, param_idx).and_then(|_| {
        let selected = selected_plock_step(&ctx.shared.selected_steps);
        displayed_plock_step(&state, owner, selected)
    });
    let (locks, shown_locked): (Vec<Option<f32>>, bool) = device
        .with_values(&state, &app.buses, owner, |values| {
            let locks = steps.iter().map(|step| values.lock(*step, param_idx));
            let shown_locked = shown.is_some_and(|step| values.lock(step, param_idx).is_some());
            (locks.collect(), shown_locked)
        })
        .ok_or("the device is gone")?;
    let track = owner;
    let locking = name == "set-device-param-locks";
    let (steps, command) = if locking {
        let value = value.ok_or("needs a :value")?;
        let pending = steps
            .iter()
            .zip(&locks)
            .filter(|(_, lock)| **lock != Some(value));
        let steps = step_list(pending.map(|(step, _)| *step));
        let command = device.lock_command(track, steps.clone(), param_idx, value);
        (steps, command)
    } else {
        let (target, slot_idx, rack_slot) = device
            .plock_target()
            .ok_or("a rack slot instrument's p-locks have no clear command yet")?;
        let locked = steps.iter().zip(&locks).filter(|(_, lock)| lock.is_some());
        let steps = step_list(locked.map(|(step, _)| *step));
        let command = clear_plocks_command(
            app,
            target,
            track,
            steps.clone(),
            param_idx,
            slot_idx,
            rack_slot,
        );
        (steps, command)
    };
    let Some(command) = command.filter(|_| !steps.is_empty()) else {
        return Ok(());
    };
    let script = ScriptEdit::begin(app, ctx);
    let changed = script.apply(app, command);
    if changed {
        let invalidations = &ctx.shared.ui_invalidations;
        if let Some(invalidation) = device.invalidation(track, param_idx, true) {
            invalidations.push(invalidation);
        }
        invalidations.push(UiInvalidation::StepInvalidationBatch {
            track,
            steps: steps.clone(),
            change: StepInvalidation::PlockPresence,
        });
        if let Some(target) = rack_target(device, param_idx) {
            let rebuild = param_change_needs_fx_rebuild(&pdesc);
            // The shown step's row set moves when a lock lands there anew
            // (as the rack knobs' `for_plock_write`) or is cleared there.
            let touched = shown.is_some_and(|step| steps.contains(&step));
            let rows = match (touched, locking) {
                (false, _) => RackPlockRowsSync::Unchanged,
                (true, true) => RackPlockRowsSync::for_plock_write(shown_locked),
                (true, false) => RackPlockRowsSync::RowSetChanged,
            };
            rack_param_applied(editor, app, ctx, track, target, rebuild, Some(rows));
        }
    }
    script.end(app, ctx, false, changed);
    Ok(())
}

/// Set a device param's base (stored units) through its family's history
/// edit and refresh what shows it; returns whether the model changed.
fn base_edit(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    script: &ScriptEdit,
    (owner, device, param_idx): (usize, DeviceSlot, usize),
    pdesc: &sequencer::effects::ParamDescriptor,
    value: f32,
) -> bool {
    if let DeviceSlot::BusEffect(slot) = device {
        let result = script.apply_with(app, |app| {
            app.apply_recorded_bus_effect_value_mutation(
                owner,
                slot,
                "Set bus effect parameter",
                format!("param:{param_idx}"),
                |app| app.set_bus_effect_param(owner, slot, param_idx, value),
            )
        });
        return match result {
            Ok(()) => {
                bus_effect_param_applied(
                    app,
                    editor,
                    ctx.shared,
                    (owner, slot, param_idx),
                    Some(pdesc),
                );
                true
            }
            Err(error) => {
                editor.handle_host_event(HostEvent::Error(format!("set-device-param: {error}")));
                false
            }
        };
    }
    let edit = (owner, device, param_idx);
    let apply = |app: &mut app::App, command| script.apply(app, command);
    let rack = rack_target(device, param_idx);
    // A rack slot's refresh (the panel rebuild included) is
    // `rack_param_applied`'s alone.
    let rebuild = rack.is_none().then_some(pdesc);
    let changed = apply_device_param_base(app, ctx.shared, edit, rebuild, value, apply);
    if let Some(target) = rack.filter(|_| changed) {
        let rebuild = param_change_needs_fx_rebuild(pdesc);
        rack_param_applied(editor, app, ctx, owner, target, rebuild, None);
    }
    changed
}

/// `set-device` (`:field` `voices` or `delete-target`, `:value`).
fn device_edit(
    map: &Payload,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) -> Result<(), String> {
    let Addressed { owner, device, .. } = addressed(app, map)?;
    let (field, value) = SetValue::field(map)?;
    match field.as_str() {
        "voices" => {
            let DeviceSlot::RackSlot(slot_idx) = device else {
                return Err(format!("a {} has no voices", device.role()));
            };
            let voices = value.integer(1, sequencer::audio::MAX_VOICES)?;
            let current =
                rack_slot_voices(&app.state, owner, slot_idx).ok_or("the device is gone")?;
            if current == voices {
                return Ok(());
            }
            let script = ScriptEdit::begin(app, ctx);
            let command = app::AppCommand::SetRackSlotMaxPolyphony {
                track: owner,
                slot_idx,
                value: voices,
            };
            let changed = script.apply(app, command);
            if changed {
                rack_slot_voices_applied(editor, app, ctx, owner, slot_idx);
            }
            script.end(app, ctx, true, changed);
            Ok(())
        }
        "delete-target" => {
            let on = value.flag()?;
            let shared = ctx.shared;
            let current = shared.current_track.load(Ordering::Relaxed);
            let target = device
                .delete_target(owner, current)
                .ok_or_else(|| match device {
                    DeviceSlot::Instrument => "an instrument is no delete target".to_string(),
                    _ => "an effect of a track is a delete target only on the current track"
                        .to_string(),
                })?;
            let mut held = shared.active_delete_target.lock().unwrap();
            if on != (held.as_ref() == Some(&target)) {
                *held = on.then_some(target);
                bump_delete_target_version(&shared.active_delete_target_version);
            }
            Ok(())
        }
        "base-note" => {
            let DeviceSlot::Instrument = device else {
                return Err(format!("a {} has no base note", device.role()));
            };
            let note = value.number(-48.0, 48.0)? as f32;
            let offsets = &app.state.pattern.instrument_base_note_offsets;
            let current = offsets.get(owner).ok_or("the device is gone")?;
            if f32::from_bits(current.load(Ordering::Relaxed)) == note {
                return Ok(());
            }
            let script = ScriptEdit::begin(app, ctx);
            let command = app::AppCommand::SetInstrumentBaseNoteOffset {
                track: owner,
                value: note,
            };
            let changed = script.apply(app, command);
            if changed {
                ctx.shared
                    .ui_invalidations
                    .push(UiInvalidation::Instrument {
                        track: owner,
                        change: InstrumentInvalidation::BaseNote,
                    });
            }
            script.end(app, ctx, true, changed);
            Ok(())
        }
        other => Err(format!("a device has no settable field {other}")),
    }
}

/// Rack slot `slot_idx` of `track`'s voices (`device.voices`).
fn rack_slot_voices(state: &SequencerState, track: usize, slot_idx: usize) -> Option<usize> {
    with_rack_slot(state, track, slot_idx, |_, slot| slot.max_polyphony)
}
