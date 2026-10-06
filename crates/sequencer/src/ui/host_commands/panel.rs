//! The host kinds' device panel setters (kind-bindings spec §14.2g):
//! `set-device-tensor` (`set-tensor-cell!`), `stamp-variant`
//! (`stamp-variant!`), `stamp-key-variant` (`stamp-key-variant!`),
//! `set-macro` (a project macro's `name` and `value`), `set-rack-macro` (a
//! drum rack macro's `name` and `base`) and `set-macro-mapping` (a
//! mapping's `min`, `max` and `curve`).
//!
//! Everything is named by stable ids resolved when the command lands: a
//! device by its owner's id and `did` (as `set-device`), a track by its
//! `TrackId`, a project macro by its macro id, a rack macro by its rack's
//! device and index, a mapping by its macro and position, a variant by its
//! label. Values follow the value rule (`SetValue`): a number in its range,
//! a label among its options, a name; anything else is an error that
//! changes nothing. Setters are absolute (each acts only where the model
//! differs: steps or keys already playing the variant are left alone) and
//! go through the legacy edits: tensor cells, variant stamps and project
//! macro names and mappings through their history commands (one undo entry
//! each; a tensor drag's `set!`s join one, as `ScriptEdit`), a project
//! macro's value as the macro panel's performance control (no undo entry),
//! and a rack macro's name, value and mappings through the rack panel's
//! (unrecorded) edits, with their legacy refreshes.

use super::devices::{addressed, Addressed};
use super::instrument_params::{key_variant_command, sync_instrument_tensor_display};
use super::step_history::{stamp_step_variant, step_list, variant_edit_applied};
use super::track_settings::SetValue;
use super::ScriptEdit;
use crate::*;
use std::collections::HashMap;

pub(super) const COMMANDS: &[&str] = &[
    "set-device-tensor",
    "stamp-variant",
    "stamp-key-variant",
    "set-macro",
    "set-rack-macro",
    "set-macro-mapping",
];

type Payload = HashMap<String, Rc<RefCell<Value>>>;

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
        "set-device-tensor" => tensor_edit(map, app, editor, ctx),
        "stamp-variant" => stamp_variant(map, app, ctx),
        "stamp-key-variant" => stamp_key_variant(map, app, ctx),
        "set-macro" => macro_edit(map, app, ctx),
        "set-rack-macro" => rack_macro_edit(map, app, editor, ctx),
        _ => mapping_edit(map, app, editor, ctx),
    };
    if let Err(message) = result {
        editor.handle_host_event(HostEvent::Error(format!("{name}: {message}")));
    }
}

/// A track device named by its `device-target`, which must be on a track.
fn track_device(app: &app::App, map: &Payload) -> Result<(usize, DeviceSlot), String> {
    let Addressed {
        owner,
        device,
        track_id,
    } = addressed(app, map)?;
    match track_id {
        Some(_) => Ok((owner, device)),
        None => Err("a bus effect has none".into()),
    }
}

/// `set-device-tensor` (`:tensor-idx`, `:cell`, `:value`): a track
/// instrument's, effect's or MIDI effect's own cell (never a p-lock).
fn tensor_edit(
    map: &Payload,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) -> Result<(), String> {
    let (track, device) = track_device(app, map)?;
    let tensor_idx = map_usize(map, "tensor-idx").ok_or("needs :tensor-idx")?;
    let midi_fx = match device {
        DeviceSlot::MidiFx(_) => ctx.frame.host_kinds.devices.midi_fx_descriptors(),
        _ => Rc::from([]),
    };
    let tensor = device
        .descriptor(app, track, &midi_fx)
        .and_then(|desc| desc.tensor_params.get(tensor_idx).cloned())
        .ok_or("the device has no such tensor")?;
    let cells = tensor.default.len();
    if cells == 0 {
        return Err("the tensor has no cells".into());
    }
    let cell = SetValue::of(map, "cell", "cell").integer(0, cells - 1)?;
    let (min, max) = (f64::from(tensor.min), f64::from(tensor.max));
    let value = SetValue::of(map, "value", "value").number(min, max)? as f32;
    let command = device
        .tensor_cell_command(track, tensor_idx, cell, value)
        .ok_or("a drum rack's or a bus's tensors take no set! yet")?;
    let slot = device
        .slot_state(&app.state, track)
        .ok_or("the device is gone")?;
    let current = slot.tensor_params.default_values(tensor_idx);
    if current.and_then(|values| values.get(cell).copied()) == Some(value) {
        return Ok(());
    }
    let script = ScriptEdit::begin(app, ctx);
    let changed = script.apply(app, command);
    if changed && device == DeviceSlot::Instrument {
        let current_track = ctx.shared.current_track.load(Ordering::Relaxed);
        let selected = &ctx.shared.selected_steps;
        sync_instrument_tensor_display(editor, app, track, tensor_idx, current_track, selected);
    }
    script.end(app, ctx, true, changed);
    Ok(())
}

/// The label a variant command names: `None` for `"def"` (clear).
fn variant_label(map: &Payload) -> Result<Option<String>, String> {
    let label = SetValue::of(map, "label", "label");
    let label = label.label()?;
    Ok((label != "def").then(|| label.to_string()))
}

/// The key of the variant labelled `label` in `registry` (`None` for
/// `None`: clear); an unknown label is the error `missing` followed by the
/// label.
fn variant_key(
    registry: &sequencer::plock_variants::PlockVariantRegistry,
    label: Option<&str>,
    missing: &str,
) -> Result<Option<sequencer::plock_variants::PlockVariantKey>, String> {
    let Some(label) = label else {
        return Ok(None);
    };
    let assignment = registry.assignment_for_label(label);
    let assignment = assignment.ok_or_else(|| format!("{missing} {label}"))?;
    Ok(Some(assignment.key))
}

/// `stamp-variant` (`:track-id`, `:label`, `:steps`, `:step-tracks`).
fn stamp_variant(map: &Payload, app: &mut app::App, ctx: &mut LoopCtx<'_>) -> Result<(), String> {
    let track_id = map_usize(map, "track-id").ok_or("needs :track-id")?;
    let track_id = sequencer::sequencer::TrackId(track_id as u64);
    let track = live_track_index(app, track_id).ok_or("the track is gone")?;
    let steps = map_usize_list(map, "steps").unwrap_or_default();
    let tracks = map_usize_list(map, "step-tracks").unwrap_or_default();
    if tracks.len() != steps.len() || tracks.iter().any(|tid| *tid as u64 != track_id.0) {
        return Err("steps must be steps of the track".into());
    }
    let label = variant_label(map)?;
    let key = match &label {
        None => None,
        Some(_) => {
            let registry = app.state.plock_variant_registry_snapshot(track);
            variant_key(&registry, label.as_deref(), "the track has no variant")?
        }
    };
    // Only the steps that differ: stamped with another variant (or none),
    // or, to clear, playing one.
    let pending = steps.iter().copied().filter(|step| {
        let now = sequencer::plock_variants::live_track_variant_key(&app.state, track, *step);
        match &key {
            Some(key) => now.as_ref() != Some(key),
            None => now.is_some(),
        }
    });
    let steps = step_list(pending);
    if steps.is_empty() {
        return Ok(());
    }
    let script = ScriptEdit::begin(app, ctx);
    let changed = script.apply_with(app, |app| {
        stamp_step_variant(app, track, &steps, key.as_ref())
    })?;
    if changed {
        variant_edit_applied(ctx.shared);
    }
    script.end(app, ctx, false, changed);
    Ok(())
}

/// `stamp-key-variant` (`device-target`, `:label`, `:notes`): a track
/// instrument's keys.
fn stamp_key_variant(
    map: &Payload,
    app: &mut app::App,
    ctx: &mut LoopCtx<'_>,
) -> Result<(), String> {
    let (track, device) = track_device(app, map)?;
    if device != DeviceSlot::Instrument {
        return Err(format!("a {} has no key-lock variants", device.role()));
    }
    let notes = map_usize_list(map, "notes").unwrap_or_default();
    if let Some(note) = notes
        .iter()
        .find(|note| **note >= sequencer::effects::MAX_MIDI_NOTES)
    {
        return Err(format!("{note} is no MIDI note"));
    }
    let label = variant_label(map)?;
    let (registry, assignments) = app.state.key_lock_variant_registry_with_assignments(track);
    let key = variant_key(
        &registry,
        label.as_deref(),
        "the instrument has no key variant",
    )?;
    let mut notes: Vec<u8> = notes
        .into_iter()
        .filter(|note| {
            let now = assignments.get(*note).and_then(Option::as_ref);
            match &label {
                Some(label) => now.is_none_or(|now| &now.label != label),
                None => now.is_some(),
            }
        })
        .map(|note| note as u8)
        .collect();
    notes.sort_unstable();
    notes.dedup();
    if notes.is_empty() {
        return Ok(());
    }
    let script = ScriptEdit::begin(app, ctx);
    let changed = script.apply(app, key_variant_command(track, notes, key));
    if changed {
        variant_edit_applied(ctx.shared);
    }
    script.end(app, ctx, false, changed);
    Ok(())
}

/// A project macro named by `:macro-id`: its id and position.
fn project_macro(app: &app::App, map: &Payload) -> Result<(u32, usize), String> {
    let id = SetValue::of(map, "macro-id", "macro-id").id("a macro id")?;
    let id = u32::try_from(id).map_err(|_| "no such macro")?;
    let at = (app.macro_engine.macros().iter())
        .position(|definition| definition.id == id)
        .ok_or("the macro is gone")?;
    Ok((id, at))
}

/// `set-macro` (`:macro-id`, `:field` `name` or `value`, `:value`).
fn macro_edit(map: &Payload, app: &mut app::App, ctx: &mut LoopCtx<'_>) -> Result<(), String> {
    let (id, at) = project_macro(app, map)?;
    let (field, value) = SetValue::field(map)?;
    let current = &app.macro_engine.macros()[at];
    let command = match field.as_str() {
        "name" => {
            let name = value.name()?;
            if current.name == name {
                return Ok(());
            }
            app::AppCommand::MacroRename {
                id,
                name: name.to_string(),
            }
        }
        "value" => {
            let value = value.number(0.0, 1.0)? as f32;
            if current.value == value {
                return Ok(());
            }
            app::AppCommand::MacroSetValue { id, value }
        }
        other => return Err(format!("a macro has no settable field {other}")),
    };
    let script = ScriptEdit::begin(app, ctx);
    let changed = script.apply(app, command);
    if changed {
        // As the macro commands: the legacy `SEQ.macros` resync.
        ctx.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    }
    script.end(app, ctx, false, changed);
    Ok(())
}

/// A drum rack macro named by its rack's `device-target` and `:macro`.
fn rack_macro(
    app: &app::App,
    map: &Payload,
) -> Result<(usize, sequencer::sequencer::RackMacroId), String> {
    let (track, device) = track_device(app, map)?;
    let index = SetValue::of(map, "macro", "macro").integer(0, usize::MAX)?;
    let id = sequencer::sequencer::RackMacroId::from_index(index).ok_or("no such macro")?;
    let racks = app.state.pattern.rack_tracks.lock().unwrap();
    let rack = racks.get(track).and_then(Option::as_ref);
    match (device, rack) {
        (DeviceSlot::Instrument, Some(rack)) if index < rack.macros.len() => Ok((track, id)),
        (DeviceSlot::Instrument, Some(_)) => Err("no such macro".into()),
        _ => Err(format!("a {} has no macros", device.role())),
    }
}

/// Rack macro `id` of `track`, read under the rack lock.
fn with_rack_macro<R>(
    app: &app::App,
    track: usize,
    id: sequencer::sequencer::RackMacroId,
    read: impl FnOnce(&sequencer::sequencer::RackTrackSnapshot, &sequencer::sequencer::RackMacro) -> R,
) -> Option<R> {
    let racks = app.state.pattern.rack_tracks.lock().unwrap();
    let rack = racks.get(track)?.as_ref()?;
    Some(read(rack, rack.macros.get(id.index())?))
}

/// `set-rack-macro` (`device-target`, `:macro`, `:field` `name` or
/// `value` (the macro's own), `:value`).
fn rack_macro_edit(
    map: &Payload,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) -> Result<(), String> {
    let (track, id) = rack_macro(app, map)?;
    let (field, value) = SetValue::field(map)?;
    let (name, base) = with_rack_macro(app, track, id, |_, rack_macro| {
        (rack_macro.name.clone(), rack_macro.value)
    })
    .ok_or("the macro is gone")?;
    match field.as_str() {
        "name" => {
            // A live text field, as the rack panel's: any text (even empty).
            let next = value.label()?;
            if name != next {
                rename_rack_macro_reactive(editor, app, track, id, next.to_string());
            }
        }
        "value" => {
            let next = value.number(0.0, 1.0)? as f32;
            if base != next && app.set_rack_macro_value(track, id, next) {
                let selected = &ctx.shared.selected_steps;
                refresh_rack_macro_value_reactive(editor, app, track, id, selected);
            }
        }
        other => return Err(format!("a rack macro has no settable field {other}")),
    }
    Ok(())
}

/// `set-macro-mapping` (a project macro's `:macro-id`, or a rack macro's
/// `device-target` and `:macro`; `:mapping`, `:field` `min`, `max` or
/// `curve`, `:value`). `min` and `max` are display units, within the
/// target's range when it is a device param.
fn mapping_edit(
    map: &Payload,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) -> Result<(), String> {
    let mapping_idx = map_usize(map, "mapping").ok_or("needs :mapping")?;
    let (field, value) = SetValue::field(map)?;
    if map.contains_key("macro-id") {
        let (id, at) = project_macro(app, map)?;
        let definition = &app.macro_engine.macros()[at];
        let mapping = definition
            .mappings
            .get(mapping_idx)
            .ok_or("no such mapping")?;
        let (_, _, shown_min, shown_max, lo, hi, scale, ..) =
            macro_mapping_display_metadata(app, mapping);
        let range = MappingRange {
            stored: (mapping.range_min, mapping.range_max),
            shown: (shown_min, shown_max),
            bounded: macro_mapping_location(app, mapping).is_some(),
            lo,
            hi,
            scale,
        };
        let command = match field.as_str() {
            "min" | "max" => {
                let Some((min, max)) = range.next(&value, &field)? else {
                    return Ok(());
                };
                app::AppCommand::MacroSetRange {
                    id,
                    mapping_idx,
                    min,
                    max,
                }
            }
            "curve" => {
                use sequencer::macro_engine::MacroCurve;
                let label = value.label()?;
                let Some(curve) = MacroCurve::from_label(label) else {
                    return value.fail(&format!("one of {:?}", MacroCurve::LABELS));
                };
                if mapping.curve == curve {
                    return Ok(());
                }
                app::AppCommand::MacroSetCurve {
                    id,
                    mapping_idx,
                    curve,
                }
            }
            other => return Err(format!("a mapping has no settable field {other}")),
        };
        let script = ScriptEdit::begin(app, ctx);
        let changed = script.apply(app, command);
        if changed {
            ctx.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
        }
        script.end(app, ctx, field != "curve", changed);
        return Ok(());
    }
    let (track, id) = rack_macro(app, map)?;
    let mapping = with_rack_macro(app, track, id, |rack, rack_macro| {
        let mapping = rack_macro.mappings.get(mapping_idx)?.clone();
        let metadata = rack_macro_mapping_display_metadata(app, rack, &mapping);
        Some((mapping, metadata))
    })
    .flatten();
    let (mapping, (_, _, shown_min, shown_max, lo, hi, scale, ..)) =
        mapping.ok_or("no such mapping")?;
    let range = MappingRange {
        stored: (mapping.range_min, mapping.range_max),
        shown: (shown_min, shown_max),
        bounded: !matches!(
            mapping.target,
            sequencer::sequencer::RackMacroTarget::SlotParam { .. }
        ),
        lo,
        hi,
        scale,
    };
    let changed = match field.as_str() {
        "min" | "max" => {
            let Some((min, max)) = range.next(&value, &field)? else {
                return Ok(());
            };
            app.set_rack_macro_mapping_range(track, id, mapping_idx, min, max)
        }
        "curve" => {
            use sequencer::sequencer::RackMacroCurve;
            let label = value.label()?;
            let Some(curve) = RackMacroCurve::from_label(label) else {
                return value.fail(&format!("one of {:?}", RackMacroCurve::LABELS));
            };
            if mapping.curve == curve {
                return Ok(());
            }
            app.set_rack_macro_mapping_curve(track, id, mapping_idx, curve)
        }
        other => return Err(format!("a mapping has no settable field {other}")),
    };
    if changed {
        let shared = ctx.shared;
        refresh_instrument_panel_reactive(
            editor,
            app,
            track,
            &shared.selected_steps,
            &shared.ui_epoch,
        );
    }
    Ok(())
}

/// A mapping's range as its `min` / `max` setters see it (shared by the
/// project and rack mapping branches).
struct MappingRange {
    /// The stored bounds, and as the kind shows them (display units).
    stored: (f32, f32),
    shown: (f32, f32),
    /// Whether the target is a device param, whose display range `lo..=hi`
    /// bounds the value.
    bounded: bool,
    lo: f32,
    hi: f32,
    /// Display units per stored unit.
    scale: f32,
}

impl MappingRange {
    /// The stored (min, max) with `field` (`min` or `max`) set to `value`
    /// (display units: a finite number, within the target's range when
    /// bounded); `None` when `value` is the bound's current value, checked
    /// before the range so the value a view reads always sets back.
    fn next(&self, value: &SetValue<'_>, field: &str) -> Result<Option<(f32, f32)>, String> {
        let is_min = field == "min";
        let current = if is_min { self.shown.0 } else { self.shown.1 };
        let user = value.number(f64::MIN, f64::MAX)?;
        if user as f32 == current {
            return Ok(None);
        }
        if self.bounded {
            let (lo, hi) = (self.lo.min(self.hi), self.lo.max(self.hi));
            value.number(f64::from(lo), f64::from(hi))?;
        }
        let stored = (user / f64::from(self.scale)) as f32;
        let (mut min, mut max) = self.stored;
        let bound = if is_min { &mut min } else { &mut max };
        if *bound == stored {
            return Ok(None);
        }
        *bound = stored;
        Ok(Some((min, max)))
    }
}
