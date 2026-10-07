//! The host kinds' device setters (kind-bindings spec §14.2b, §14.2f):
//! `set-device-param` (`param.base`), `set-device-param-locks` /
//! `clear-device-param-locks` (`lock-param!` / `unlock-param!`) and
//! `set-device` (`device.delete-target`, a track instrument's
//! `device.base-note`, a rack slot's strip controls `gain`, `pan`, `muted`,
//! `soloed`, `choke`, `enabled`, `base-note` and `voices`), and
//! `set-device-strip-locks` / `clear-device-strip-locks` (`lock-strip!` /
//! `unlock-strip!`: a rack slot's gain, pan, mute, solo, base note and
//! voices p-locks).
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
//! A strip control's value follows the value rule: gain a number in 0–2,
//! pan in −1–1, the base note in −48–48, the choke group an integer in
//! 0–16 (0: none), voices an integer in 1–16, the flags a bool; anything
//! else (or a device that is no rack slot) is an error that changes
//! nothing. They go through the legacy rack strip commands' history edits
//! (`SetRackSlotGain`, …, `SetRackSlotParamPlockMulti`,
//! `ClearRackSlotParamPlockMulti`) with their refreshes
//! (`rack_slot_strip_applied`, `rack_slot_plock_applied`).
//!
//! Gestures as [`super::ScriptEdit`]: a base edit, a gain, pan, base note
//! or voices edit, or a track instrument's base note edit while the
//! pointer is down stays open and later script edits join it (a drag
//! view's `set!` per frame is one undo entry, which the release ends); any
//! other script edit ends its entry at once. A lock (`lock-param!`,
//! `lock-strip!`, `lock-rack-macro!`) while the pointer is down stays open
//! too: the history's device p-lock coalescing joins the frames that lock
//! the same param or macro on the same steps (its merge key names both), so
//! dragging a lane point is one entry per step, and a lock of other steps
//! seals it and starts another (eseq-0l17.58). A clear is an entry of its
//! own (the history records it at once). An edit landing while another
//! gesture is active (a user's knob drag) gets an entry of its own beside
//! it.

use super::rack::{
    rack_param_applied, rack_slot_plock_applied, rack_slot_strip_applied, StripControl,
};
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
    "set-device-strip-locks",
    "clear-device-strip-locks",
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
        "set-device-strip-locks" | "clear-device-strip-locks" => {
            strip_lock_edit(name, map, app, editor, ctx)
        }
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
        let script = ScriptEdit::begin(app, ctx, true);
        let edit = (owner, device, param_idx);
        let changed = base_edit(app, editor, ctx, &script, edit, &pdesc, value);
        script.end(app, ctx, changed);
        return Ok(());
    }
    // Only a bus effect has no track.
    let Some(track_id) = track_id else {
        return Err("bus effects take no p-locks".into());
    };
    let steps = super::track_steps(map, track_id, "the param's track")?;
    let locking = name == "set-device-param-locks";
    let lock = match locking {
        true => Some(value.ok_or("needs a :value")?),
        false => None,
    };
    // The step the rack panel shows, and whether it held a lock before
    // (a rack lock's row refresh, as the rack knobs do).
    let shown = rack_target(device, param_idx).and_then(|_| {
        let selected = selected_plock_step(&ctx.shared.selected_steps);
        displayed_plock_step(&state, owner, selected)
    });
    let (steps, shown_locked) = device
        .with_values(&state, &app.buses, owner, |values| {
            let held = |step| values.lock(step, param_idx);
            let shown_locked = shown.is_some_and(|step| held(step).is_some());
            (pending_steps(&steps, held, lock), shown_locked)
        })
        .ok_or("the device is gone")?;
    let track = owner;
    let command = match lock {
        Some(value) => device.lock_command(track, steps.clone(), param_idx, value),
        None => {
            let (target, slot_idx, rack_slot) = device
                .plock_target()
                .ok_or("a rack slot instrument's p-locks have no clear command yet")?;
            let steps = steps.clone();
            clear_plocks_command(app, target, track, steps, param_idx, slot_idx, rack_slot)
        }
    };
    let Some(command) = command.filter(|_| !steps.is_empty()) else {
        return Ok(());
    };
    let script = ScriptEdit::begin(app, ctx, true);
    let changed = script.apply(app, command);
    if changed {
        let invalidations = &ctx.shared.ui_invalidations;
        if let Some(invalidation) = device.invalidation(track, param_idx, true) {
            invalidations.push(invalidation);
        }
        let rows = plock_rows(shown, shown_locked, &steps, locking);
        invalidations.push(UiInvalidation::StepInvalidationBatch {
            track,
            steps,
            change: StepInvalidation::PlockPresence,
        });
        if let Some(target) = rack_target(device, param_idx) {
            let rebuild = param_change_needs_fx_rebuild(&pdesc);
            rack_param_applied(editor, app, ctx, track, target, rebuild, Some(rows));
        }
    }
    // A drag's locks of the same steps join one entry (eseq-0l17.58).
    script.end(app, ctx, changed);
    Ok(())
}

/// The steps of `steps` a lock edit acts on (sorted, deduplicated): those
/// whose lock (`held`) differs from `lock`, or, to clear (`None`), that
/// hold one.
fn pending_steps(
    steps: &[usize],
    held: impl Fn(usize) -> Option<f32>,
    lock: Option<f32>,
) -> Vec<usize> {
    step_list(steps.iter().copied().filter(|step| match lock {
        Some(lock) => held(*step) != Some(lock),
        None => held(*step).is_some(),
    }))
}

/// How a lock edit on `steps` moves the rack panel's p-lock rows at the
/// shown step: a lock landing there anew (as the rack knobs'
/// `for_plock_write`, `shown_locked`: it held one before) or a clear there
/// changes the row set.
fn plock_rows(
    shown: Option<usize>,
    shown_locked: bool,
    steps: &[usize],
    locking: bool,
) -> RackPlockRowsSync {
    let touched = shown.is_some_and(|step| steps.contains(&step));
    match (touched, locking) {
        (false, _) => RackPlockRowsSync::Unchanged,
        (true, true) => RackPlockRowsSync::for_plock_write(shown_locked),
        (true, false) => RackPlockRowsSync::RowSetChanged,
    }
}

/// A lock edit (`lock-strip!`, `lock-rack-macro!` and their clears) of
/// `steps` of `track`: `held` the locks the target holds now, by step;
/// `lock` the lock to set, or `None` to clear.
pub(super) struct StepLocks {
    pub(super) track: usize,
    pub(super) steps: Vec<usize>,
    pub(super) held: Vec<Option<f32>>,
    pub(super) lock: Option<f32>,
}

/// Apply a lock edit to the steps whose lock differs (or that hold one, to
/// clear): the command `command` builds for them, as one script edit (one
/// undo entry, which a drag's later locks of the same steps join; nothing
/// at all when no step differs), then the steps' p-lock presence and
/// `refresh`, given how the shown step's p-lock rows moved.
pub(super) fn lock_steps(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    locks: StepLocks,
    command: impl FnOnce(Vec<usize>) -> app::AppCommand,
    refresh: impl FnOnce(&mut Editor, &app::App, &mut LoopCtx<'_>, RackPlockRowsSync),
) {
    let StepLocks {
        track,
        steps,
        held,
        lock,
    } = locks;
    let held = |step: usize| held.get(step).copied().flatten();
    // The step the rack panel shows, and whether it held a lock before
    // (its p-lock rows move when a lock lands there anew or is cleared).
    let shared = ctx.shared;
    let selected = selected_plock_step(&shared.selected_steps);
    let shown = displayed_plock_step(&shared.state, track, selected);
    let shown_locked = shown.is_some_and(|step| held(step).is_some());
    let steps = pending_steps(&steps, held, lock);
    if steps.is_empty() {
        return;
    }
    let script = ScriptEdit::begin(app, ctx, true);
    let changed = script.apply(app, command(steps.clone()));
    if changed {
        let rows = plock_rows(shown, shown_locked, &steps, lock.is_some());
        shared
            .ui_invalidations
            .push(UiInvalidation::StepInvalidationBatch {
                track,
                steps,
                change: StepInvalidation::PlockPresence,
            });
        refresh(editor, app, ctx, rows);
    }
    // A drag's locks of the same steps join one entry (eseq-0l17.58).
    script.end(app, ctx, changed);
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

/// `set-device` (`:field` `delete-target`, a track instrument's
/// `base-note` or a strip control, `:value`).
fn device_edit(
    map: &Payload,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) -> Result<(), String> {
    let Addressed { owner, device, .. } = addressed(app, map)?;
    let (field, value) = SetValue::field(map)?;
    match field.as_str() {
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
        // A rack slot's base note is a strip control.
        "base-note" if device == DeviceSlot::Instrument => {
            let note = value.number(-48.0, 48.0)? as f32;
            let offsets = &app.state.pattern.instrument_base_note_offsets;
            let current = offsets.get(owner).ok_or("the device is gone")?;
            if f32::from_bits(current.load(Ordering::Relaxed)) == note {
                return Ok(());
            }
            let script = ScriptEdit::begin(app, ctx, true);
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
            script.end(app, ctx, changed);
            Ok(())
        }
        other => match StripControl::from_field(other) {
            Some(control) => strip_edit(owner, device, control, &value, app, editor, ctx),
            None => Err(format!("a device has no settable field {other}")),
        },
    }
}

/// A strip control's base edit (`set-device` `gain`, `pan`, `muted`,
/// `soloed`, `choke`, `enabled`, `base-note`, `voices`): the value the slot
/// holds now is a no-op (even one stored out of range), else the value
/// rule, then the legacy command through history (a drag of a continuous
/// control joins one entry, [`StripControl::drags`]) and its refresh.
fn strip_edit(
    owner: usize,
    device: DeviceSlot,
    control: StripControl,
    value: &SetValue<'_>,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) -> Result<(), String> {
    let DeviceSlot::RackSlot(slot_idx) = device else {
        return Err(match control {
            StripControl::Enabled => format!("a {}'s enabled is not settable", device.role()),
            StripControl::BaseNote => format!("a {} has no base note", device.role()),
            _ => format!("a {} has no {}", device.role(), control.field()),
        });
    };
    let current = with_rack_slot(&app.state, owner, slot_idx, |_, slot| control.read(slot))
        .ok_or("the device is gone")?;
    if current.is(value.value()) {
        return Ok(());
    }
    let wanted = control.parse(value)?;
    let script = ScriptEdit::begin(app, ctx, control.drags());
    let changed = script.apply(app, control.command(owner, slot_idx, wanted));
    if changed {
        rack_slot_strip_applied(editor, app, ctx, owner, slot_idx, control);
    }
    script.end(app, ctx, changed);
    Ok(())
}

/// `set-device-strip-locks` / `clear-device-strip-locks` (`:field` `gain`,
/// `pan`, `muted`, `soloed`, `base-note` or `voices`, `:steps` with their
/// `:step-tracks`, and a lock's `:value` under the value rule): the steps
/// whose lock differs (or that hold one, to clear) through
/// `SetRackSlotParamPlockMulti` / `ClearRackSlotParamPlockMulti`, one undo
/// entry.
fn strip_lock_edit(
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
    let (field, value) = SetValue::field(map)?;
    let control = StripControl::from_field(&field);
    let Some((control, param)) = control.and_then(|control| Some((control, control.param()?)))
    else {
        return Err(format!(
            "{field} takes no p-locks (gain, pan, muted, soloed, base-note or voices)"
        ));
    };
    let (DeviceSlot::RackSlot(slot_idx), Some(track_id)) = (device, track_id) else {
        return Err(format!("a {} has no strip controls", device.role()));
    };
    let lock = match name == "set-device-strip-locks" {
        true => Some(control.parse(&value)?.number()),
        false => None,
    };
    let steps = super::track_steps(map, track_id, "the device's track")?;
    let track = owner;
    let held = with_rack_slot(&app.state, track, slot_idx, |_, slot| {
        (0..MAX_STEPS)
            .map(|step| slot.param_plocks.get(step, param))
            .collect()
    })
    .ok_or("the device is gone")?;
    let locks = StepLocks {
        track,
        steps,
        held,
        lock,
    };
    let command = |steps| match lock {
        Some(value) => app::AppCommand::SetRackSlotParamPlockMulti {
            track,
            slot_idx,
            steps,
            param,
            value,
        },
        None => app::AppCommand::ClearRackSlotParamPlockMulti {
            track,
            slot_idx,
            steps,
            param,
        },
    };
    lock_steps(
        app,
        editor,
        ctx,
        locks,
        command,
        |editor, app, ctx, rows| {
            rack_slot_plock_applied(editor, app, ctx, track, slot_idx, param, rows)
        },
    );
    Ok(())
}
