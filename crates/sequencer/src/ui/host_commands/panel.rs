//! The host kinds' device panel setters (kind-bindings spec §14.2g):
//! `set-device-tensor` (`set-tensor-cell!`), `stamp-variant`
//! (`stamp-variant!`), `stamp-key-variant` (`stamp-key-variant!`),
//! `set-macro` (a project macro's `name` and `value`, a scene macro's
//! `target-scene`, `morph-params`, `steal-patterns`, `quantize` and
//! `tracks`), `set-rack-macro` (a
//! drum rack macro's `name` and `base`), `set-macro-mapping` (a
//! mapping's `min`, `max` and `curve`) and `set-rack-macro-locks` /
//! `clear-rack-macro-locks` (`lock-rack-macro!` / `unlock-rack-macro!`).
//!
//! Everything is named by stable ids resolved when the command lands: a
//! device by its owner's id and `did` (as `set-device`), a track by its
//! `TrackId`, a project macro by its macro id, a rack macro by its rack's
//! device and index, a mapping by its macro and position, a variant by its
//! label. Values follow the value rule (`SetValue`): a number in its range,
//! a label among its options, a name; anything else is an error that
//! changes nothing. Setters are absolute (each acts only where the model
//! differs: steps or keys already playing the variant are left alone) and
//! go through the legacy edits: tensor cells, variant stamps, project macro
//! names, scene configs and mappings through their history commands (one
//! undo entry each; a tensor drag's `set!`s join one, as `ScriptEdit`), a project
//! macro's value as the macro panel's performance control (no undo entry),
//! and a rack macro's name, value and mappings through the rack panel's
//! recorded edit (`App::apply_rack_macro_edit`, eseq-0l17.44: one entry
//! each, a value or range drag's `set!`s join one), with its refreshes.

use super::devices::{addressed, lock_steps, Addressed, StepLocks};
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
    "set-rack-macro-locks",
    "clear-rack-macro-locks",
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
        "set-macro-mapping" => mapping_edit(map, app, editor, ctx),
        "set-rack-macro-locks" | "clear-rack-macro-locks" => {
            rack_macro_lock_edit(name, map, app, editor, ctx)
        }
        other => Err(format!("{other} is no panel command")),
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
    let steps = super::track_steps(map, track_id, "the track")?;
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

/// `set-macro` (`:macro-id`, `:field` `name`, `value` or a scene macro's
/// config field, `:value`).
fn macro_edit(map: &Payload, app: &mut app::App, ctx: &mut LoopCtx<'_>) -> Result<(), String> {
    let (id, at) = project_macro(app, map)?;
    let (field, value) = SetValue::field(map)?;
    let current = &app.macro_engine.macros()[at];
    let command = match (SceneField::of(&field), field.as_str()) {
        (Some(scene), _) => {
            use sequencer::macro_engine::MacroKind;
            let MacroKind::Scene(config) = &current.kind else {
                return Err(format!("a mapped macro has no {field}"));
            };
            let Some(config) = scene_config_edit(app, config, scene, &value)? else {
                return Ok(());
            };
            app::AppCommand::MacroSceneConfig { id, config }
        }
        (None, "name") => {
            let name = value.name()?;
            if current.name == name {
                return Ok(());
            }
            app::AppCommand::MacroRename {
                id,
                name: name.to_string(),
            }
        }
        (None, "value") => {
            let value = value.number(0.0, 1.0)? as f32;
            if current.value == value {
                return Ok(());
            }
            app::AppCommand::MacroSetValue { id, value }
        }
        (None, other) => return Err(format!("a macro has no settable field {other}")),
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

/// A scene macro's config field (`set-macro`).
#[derive(Clone, Copy)]
enum SceneField {
    TargetScene,
    MorphParams,
    StealPatterns,
    Quantize,
    Tracks,
}

impl SceneField {
    fn of(field: &str) -> Option<Self> {
        Some(match field {
            "target-scene" => Self::TargetScene,
            "morph-params" => Self::MorphParams,
            "steal-patterns" => Self::StealPatterns,
            "quantize" => Self::Quantize,
            "tracks" => Self::Tracks,
            _ => return None,
        })
    }
}

/// Scene macro `config` with `field` set to `value` (the value rule: a
/// scene's position, a bool, a quantization label, a list of track ids);
/// `None` when it already holds it.
fn scene_config_edit(
    app: &app::App,
    config: &sequencer::macro_engine::SceneMacroConfig,
    field: SceneField,
    value: &SetValue<'_>,
) -> Result<Option<sequencer::macro_engine::SceneMacroConfig>, String> {
    use sequencer::macro_engine::StealQuantize;
    let mut next = config.clone();
    match field {
        SceneField::TargetScene => {
            let scenes = app.state.scene_count();
            next.target_scene = value.integer(0, scenes.saturating_sub(1))?;
        }
        SceneField::MorphParams => next.morph_params = value.flag()?,
        SceneField::StealPatterns => next.steal_patterns = value.flag()?,
        SceneField::Quantize => {
            let index = value.choice(&StealQuantize::LABELS)?;
            next.quantize = StealQuantize::from_index(index).ok_or("no such quantization")?;
        }
        SceneField::Tracks => {
            let mut mask = vec![false; app.tracks.len()];
            for track in value.tracks(app)? {
                mask[track] = true;
            }
            // The tracks it acts on now: the same set is a no-op, so the
            // list a view reads (every track while unmasked) sets back.
            let same = (0..mask.len()).all(|track| config.covers_track(track) == mask[track]);
            if same {
                return Ok(None);
            }
            next.track_mask = Some(mask);
        }
    }
    Ok((next != *config).then_some(next))
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
    use sequencer::sequencer::RackMacroField;
    let (track, id) = rack_macro(app, map)?;
    let (field, value) = SetValue::field(map)?;
    let field = match field.as_str() {
        // A live text field, as the rack panel's: any text (even empty).
        "name" => RackMacroField::Name(value.label()?.to_string()),
        "value" => RackMacroField::Value(value.number(0.0, 1.0)? as f32),
        other => return Err(format!("a rack macro has no settable field {other}")),
    };
    record_rack_macro_edit(app, editor, ctx, track, id, field)
}

/// A host kind setter's rack macro edit: `field` of rack macro `id` of
/// `track` through history as a script edit (a value or range drag joins
/// one entry), then the rack panel's refresh.
fn record_rack_macro_edit(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    track: usize,
    id: sequencer::sequencer::RackMacroId,
    field: sequencer::sequencer::RackMacroField,
) -> Result<(), String> {
    use sequencer::sequencer::RackMacroField;
    let continuous = match field {
        RackMacroField::Value(_) | RackMacroField::Range { .. } => true,
        RackMacroField::Name(_) | RackMacroField::Curve { .. } => false,
    };
    let shared = ctx.shared;
    let fields = (&shared.selected_steps, &*shared.ui_epoch);
    rack_macro_edit_reactive(editor, app, (track, id), field, fields, |app, field| {
        ScriptEdit::run(app, ctx, continuous, |app| {
            app.apply_rack_macro_edit(track, id, field)
        })
    })
    .map(|_| ())
}

/// `set-rack-macro-locks` / `clear-rack-macro-locks` (`device-target`,
/// `:macro`, `:steps` with their `:step-tracks`, and a lock's `:value`, a
/// number in 0–1): the steps whose lock differs (or that hold one, to
/// clear) through `SetRackMacroPlockMulti` / `ClearRackMacroPlockMulti`,
/// one undo entry (a drag's locks of the same steps join one,
/// [`super::devices::lock_steps`]), refreshed as the rack panel's `set-rack-macro-plock`
/// plus the steps' p-lock presence (as `lock-strip!`).
fn rack_macro_lock_edit(
    name: &str,
    map: &Payload,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) -> Result<(), String> {
    let (track, id) = rack_macro(app, map)?;
    let track_id = app.track_registry.id_at(track).ok_or("the track is gone")?;
    let lock = match name == "set-rack-macro-locks" {
        true => Some(SetValue::of(map, "value", "value").number(0.0, 1.0)? as f32),
        false => None,
    };
    let steps = super::track_steps(map, track_id, "the device's track")?;
    let held = with_rack_macro(app, track, id, |_, rack_macro| rack_macro.plocks.clone())
        .ok_or("the macro is gone")?;
    let locks = StepLocks {
        track,
        steps,
        held,
        lock,
    };
    let macro_idx = id.index();
    let command = |steps| match lock {
        Some(value) => app::AppCommand::SetRackMacroPlockMulti {
            track,
            steps,
            macro_idx,
            value,
        },
        None => app::AppCommand::ClearRackMacroPlockMulti {
            track,
            steps,
            macro_idx,
        },
    };
    lock_steps(
        app,
        editor,
        ctx,
        locks,
        command,
        |editor, app, ctx, rows| {
            let shared = ctx.shared;
            let (state, selected, epoch) =
                (&shared.state, &shared.selected_steps, &shared.ui_epoch);
            refresh_rack_macro_plock_reactive(editor, app, state, track, id, selected, epoch, rows);
        },
    );
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
    use sequencer::sequencer::{RackMacroCurve, RackMacroField};
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
    let field = match field.as_str() {
        "min" | "max" => {
            let Some((min, max)) = range.next(&value, &field)? else {
                return Ok(());
            };
            RackMacroField::Range {
                target: mapping.target,
                min,
                max,
            }
        }
        "curve" => {
            let label = value.label()?;
            let Some(curve) = RackMacroCurve::from_label(label) else {
                return value.fail(&format!("one of {:?}", RackMacroCurve::LABELS));
            };
            RackMacroField::Curve {
                target: mapping.target,
                curve,
            }
        }
        other => return Err(format!("a mapping has no settable field {other}")),
    };
    record_rack_macro_edit(app, editor, ctx, track, id, field)
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
