//! The focus step setter (kind-bindings spec §14, stage 7e-2):
//! `set-focus-step` (`focus-step.duration`, `velocity`, `delay`, `aux-a`,
//! `transpose`, `pan`, `sync`, `retrig`, `retrig-rate`; `set-focus-step!`).
//!
//! A focus step is named by its track's stable `TrackId` and its index on
//! the piano roll's source axis, resolved when the command lands: step
//! `index` of the track's edit focus then (positional, as `step`'s setters).
//! Values follow the value rule (spec §14.2c): a finite number in the
//! param's range (`StepParam::min` to `max`); its current value always works
//! and changes nothing; anything else, an index past the source's length or
//! a field a focus step does not set, is an error that changes nothing. The
//! setter goes through the piano roll's focus-aware history
//! (`app::edit::apply_recorded_focus_step_mutation`: one undo entry, undo
//! restores; a transpose or duration moves the step's chord notes with it)
//! and lands like the piano roll's edits ([`piano_roll_edit_landed`]). A
//! transpose or delay moves the step's notes (their keys: step, transpose,
//! offset); their note ids move with them ([`move_step_notes`]), so note
//! handles follow, as through the note setters.
//!
//! `set-step` (`set-step-param!`) is its live pattern twin: step `index` of
//! the track's live pattern (the `step` instances, what `t.steps` shows),
//! whatever its edit focus is pinned to, under the same value rule, as one
//! undo entry (`set-step-param-history`'s recorded step mutation); its
//! `:activate` turns the step on in that same entry (a note typed into an
//! empty step).
//!
//! Script drags ([`super::ScriptEdit`]): while the pointer is down every
//! focus step `set!` of one focus (any field, any step) joins ONE undo entry
//! (`app::edit::focus_step_param_drag`, the shape of the piano roll's note
//! drag gestures); Esc rolls it back (the notes it
//! moved then get fresh handles, as after an undo). With the pointer up
//! each `set!` is its own entry.

use super::step_history::{piano_roll_edit_landed, PianoRollLanding};
use super::track_settings::{command_track, SetValue};
use crate::host_kinds::{focus_step_param, NoteKey, NoteSource};
use crate::*;
use std::collections::HashMap;

pub(super) const COMMANDS: &[&str] = &["set-focus-step", "set-step"];

type Payload = HashMap<String, Rc<RefCell<Value>>>;

/// The label of a focus step edit's undo entry (the legacy lane's).
const LABEL: &str = "Edit piano-roll automation";

/// `set-focus-step`: `{:track-id :index :field :value}`.
fn set_focus_step(app: &mut app::App, ctx: &mut LoopCtx<'_>, map: &Payload) -> Result<(), String> {
    let track = command_track(app, map)?;
    let (field, value) = SetValue::field(map)?;
    let param = focus_step_param(&field)
        .ok_or_else(|| format!("a focus step has no settable field {field}"))?;
    let focus = app.track_edit_focus(track);
    let lanes = |app: &app::App| {
        PianoRollLanes::new(&app.state, track, PianoRollFocusSpec::from_focus(focus))
    };
    let last = lanes(app).num_steps() - 1;
    let step = SetValue::of(map, "index", "index").integer(0, last)?;
    let current = lanes(app).step_param(step, param);
    if matches!(value.value(), Value::Number(v) if *v == f64::from(current)) {
        return Ok(());
    }
    let next = value.number(param.min().into(), param.max().into())? as f32;
    let write = |app: &mut app::App| lanes(app).set_step_param(step, param, next);
    let keys = |app: &app::App| -> Vec<NoteKey> {
        let notes = lanes(app).note_entries(step);
        notes.iter().map(|note| NoteKey::of(step, note)).collect()
    };
    let before = keys(app);
    let script = super::ScriptEdit::begin(app, ctx, true);
    let drags = script.drags(ctx);
    let changed = if drags {
        app::edit::focus_step_param_drag(app, focus, &[step], LABEL, write)?;
        true
    } else {
        let outcome = script.apply_with(app, |app| {
            app::edit::apply_recorded_focus_step_mutation(app, focus, &[step], LABEL, |app| {
                write(app);
                Ok(())
            })
        });
        let outcome = outcome.map_err(|error| format!("{error:?}"))?;
        matches!(outcome, app::edit::EditOutcome::Applied(_))
    };
    script.end(app, ctx, changed);
    if changed {
        move_step_notes(app, ctx, track, &before, &keys(app));
        let landing = match drags {
            true => PianoRollLanding::Frame,
            false => PianoRollLanding::Recorded,
        };
        piano_roll_edit_landed(ctx, track, landing);
    }
    Ok(())
}

/// The label of a live step edit's undo entry (`set-step-param-history`'s).
const STEP_LABEL: &str = "Set step parameter";

/// `set-step`: `{:track-id :index :field :value}`, `:activate` optional.
fn set_step(app: &mut app::App, ctx: &mut LoopCtx<'_>, map: &Payload) -> Result<(), String> {
    let track = command_track(app, map)?;
    let (field, value) = SetValue::field(map)?;
    let param =
        focus_step_param(&field).ok_or_else(|| format!("a step has no settable field {field}"))?;
    let num_steps = app.state.pattern.track_params[track].get_num_steps();
    let last = num_steps.clamp(1, MAX_STEPS) - 1;
    let step = SetValue::of(map, "index", "index").integer(0, last)?;
    let activate = SetValue::of(map, "activate", "activate").flag_or(false)?;
    let pattern = &app.state.pattern;
    let turn_on = activate && !pattern.patterns[track].is_active(step);
    let current = pattern.step_data[track].get(step, param);
    if !turn_on && matches!(value.value(), Value::Number(v) if *v == f64::from(current)) {
        return Ok(());
    }
    let next = value.number(param.min().into(), param.max().into())? as f32;
    let outcome = app::edit::apply_recorded_step_mutation(app, track, &[step], STEP_LABEL, |app| {
        if turn_on {
            app.state.pattern.patterns[track].set_step_active(step, true);
        }
        app.state
            .set_step_param_no_publish(track, step, param, next);
        Ok(())
    });
    let outcome = outcome.map_err(|error| format!("{error:?}"))?;
    if !matches!(outcome, app::edit::EditOutcome::Applied(_)) {
        return Ok(());
    }
    let shared = ctx.shared;
    *shared.auto_follow_override_until.lock().unwrap() =
        Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
    let invalidations = &shared.ui_invalidations;
    if turn_on {
        invalidations.push(UiInvalidation::StepBatch {
            track,
            steps: vec![step],
        });
    }
    invalidations.push(UiInvalidation::Step {
        track,
        step,
        change: StepInvalidation::Param(param.into()),
    });
    if param == StepParam::Duration {
        invalidations.push(UiInvalidation::Step {
            track,
            step,
            change: StepInvalidation::DurationSpan,
        });
    }
    Ok(())
}

/// The notes of a step a focus step edit moved (`before` → `after`, by
/// voice: a step param moves every voice alike, keeping their order) keep
/// their note ids, while the host kinds know them under this source.
fn move_step_notes(
    app: &app::App,
    ctx: &LoopCtx<'_>,
    track: usize,
    before: &[NoteKey],
    after: &[NoteKey],
) {
    if before == after {
        return;
    }
    let Some(tid) = app.track_registry.id_at(track) else {
        return;
    };
    let Some(instance) = ctx.frame.host_kinds.track_instance(tid.0) else {
        return;
    };
    let source = NoteSource::resolve(app, track, instance);
    let notes = &mut ctx.frame.host_kinds.shared.borrow_mut().notes;
    if !(notes.source).is_some_and(|known| known.same_notes(&source)) {
        return;
    }
    let moves: Vec<(u64, NoteKey)> = (before.iter().zip(after))
        .filter_map(|(from, to)| Some((notes.nid_at(from)?, *to)))
        .collect();
    notes.move_notes(&moves);
}

pub(super) fn handle(
    name: &str,
    payload: Value,
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
) {
    let result = match (name, &payload) {
        ("set-focus-step", Value::Map(map)) => set_focus_step(app, ctx, map),
        ("set-step", Value::Map(map)) => set_step(app, ctx, map),
        _ => Err("the payload is not a dict".to_string()),
    };
    if let Err(message) = result {
        editor.handle_host_event(HostEvent::Error(format!("{name}: {message}")));
    }
}
