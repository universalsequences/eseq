//! The host kinds' track setters (kind-bindings spec §14.2c): track
//! settings (`set-track-setting`), the scale editor (`set-tuning`), the step
//! cursor (`set-cursor-step`) and bar transposes (`set-track-bar-transpose`).
//!
//! Every command names its track by its stable `TrackId`, resolved when it
//! lands ([`live_track_index`]), so a reorder in between cannot retarget
//! it; a gone track is an error. Setters are absolute: they act only where
//! the model differs, through the legacy edits' history commands (undo
//! restores), and queue the legacy edits' invalidations. Values follow one
//! rule ([`SetValue`]): a string field takes one of its labels
//! (case-insensitive; its current value always works), a number field a
//! finite number in its range, a bool field a bool, an int field an integer
//! in its range; anything else is an error and changes nothing. Gestures as
//! [`super::ScriptEdit`].

use crate::*;
use std::collections::HashMap;

pub(super) const COMMANDS: &[&str] = &[
    "set-track-setting",
    "set-tuning",
    "set-cursor-step",
    "set-track-bar-transpose",
];

type Payload = HashMap<String, Rc<RefCell<Value>>>;

/// After a Slice 3 or scale edit (`slice3-history-action`,
/// `track-tuning-action`, `set-track-setting`, `set-tuning`) applied: pause
/// the playhead follow, queue the mixer strip's targeted invalidation for a
/// mixer op, else the track's whole-track refresh and a UI epoch bump (the
/// legacy `SEQ.tp-*` resync), and show the edit's label when it has one.
pub(super) fn slice3_edit_applied(
    editor: &mut Editor,
    ctx: &LoopCtx<'_>,
    payload: &Value,
    track: Option<usize>,
    label: Option<String>,
) {
    let shared = ctx.shared;
    *shared.auto_follow_override_until.lock().unwrap() =
        Some(Instant::now() + AUTO_FOLLOW_COOLDOWN);
    match (track, slice3_track_mixer_invalidation(payload)) {
        (Some(track), Some(change)) => {
            shared
                .ui_invalidations
                .push(UiInvalidation::TrackMixer { track, change });
        }
        (track, None) => {
            if let Some(track) = track {
                shared.ui_invalidations.push(UiInvalidation::Pattern(
                    PatternInvalidation::WholeTrack { track },
                ));
            }
            shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
        }
        (None, Some(_)) => {
            shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
        }
    }
    if let Some(label) = label {
        editor.show_transient_message(label);
    }
}

/// After a bar transpose landed (`set-bar-transpose`,
/// `set-track-bar-transpose`): refresh the track's expanded step viewports.
pub(super) fn bar_transpose_applied(ctx: &LoopCtx<'_>, track: usize) {
    let shared = ctx.shared;
    for viewport in shared.expanded_step_projection.viewports_for_track(track) {
        shared
            .ui_invalidations
            .push(UiInvalidation::ExpandedStepViewport {
                track,
                track_id: viewport.track_id,
            });
    }
}

/// A setter's value under the value rule (see the module docs), named
/// `what` in errors. Shared with the arrangement setters.
pub(super) struct SetValue<'a> {
    what: &'a str,
    value: Value,
}

/// `value` as a stable model id: a non-negative integer.
pub(super) fn value_id(value: &Value) -> Option<u64> {
    match *value {
        Value::Number(id) if id >= 0.0 && id.fract() == 0.0 => Some(id as u64),
        _ => None,
    }
}

impl<'a> SetValue<'a> {
    pub(super) fn of(map: &Payload, key: &str, what: &'a str) -> Self {
        let value = map
            .get(key)
            .map_or(Value::Nil, |cell| cell.borrow().clone());
        Self { what, value }
    }

    pub(super) fn fail<T>(&self, wants: &str) -> Result<T, String> {
        Err(format!("{} takes {wants}, not {:?}", self.what, self.value))
    }

    /// A stable model id ([`value_id`]); `wants` names it in the error.
    pub(super) fn id(&self, wants: &str) -> Result<u64, String> {
        value_id(&self.value).map_or_else(|| self.fail(wants), Ok)
    }

    /// [`Self::id`], or `None` for nil.
    pub(super) fn id_or_nil(&self, wants: &str) -> Result<Option<u64>, String> {
        match self.value {
            Value::Nil => Ok(None),
            _ => self.id(wants).map(Some),
        }
    }

    pub(super) fn flag(&self) -> Result<bool, String> {
        match self.value {
            Value::Bool(on) => Ok(on),
            _ => self.fail("true or false"),
        }
    }

    /// [`Self::flag`], or `default` for nil.
    pub(super) fn flag_or(&self, default: bool) -> Result<bool, String> {
        match self.value {
            Value::Nil => Ok(default),
            _ => self.flag(),
        }
    }

    fn label(&self) -> Result<&str, String> {
        match &self.value {
            Value::String(label) => Ok(label),
            _ => self.fail("a label"),
        }
    }

    /// The index of the label among `options`, case-insensitively.
    fn choice(&self, options: &[&str]) -> Result<usize, String> {
        let label = self.label()?;
        options
            .iter()
            .position(|option| option.eq_ignore_ascii_case(label))
            .map_or_else(|| self.fail(&format!("one of {options:?}")), Ok)
    }

    /// A finite number of at least `min` (a beat: `from(0.0)`).
    pub(super) fn from(&self, min: f64) -> Result<f64, String> {
        match self.value {
            Value::Number(value) if value.is_finite() && value >= min => Ok(value),
            _ => self.fail(&format!("a finite number of at least {min}")),
        }
    }

    /// A finite number in `min..=max`.
    fn number(&self, min: f64, max: f64) -> Result<f64, String> {
        match self.value {
            Value::Number(value) if value.is_finite() && (min..=max).contains(&value) => Ok(value),
            _ => self.fail(&format!("a number from {min} to {max}")),
        }
    }

    /// An integer in `min..=max`.
    fn integer(&self, min: usize, max: usize) -> Result<usize, String> {
        match self.value {
            Value::Number(value)
                if value.fract() == 0.0 && (min as f64..=max as f64).contains(&value) =>
            {
                Ok(value as usize)
            }
            _ => self.fail(&format!("an integer from {min} to {max}")),
        }
    }
}

/// One edit a setter asks for, resolved against the model when the command
/// lands.
struct TrackSettingEdit {
    command: app::AppCommand,
    /// The Slice 3 or scale-editor payload whose legacy handler this edit
    /// shares ([`slice3_edit_applied`]); `None` for the output.
    payload: Option<Value>,
    /// A value a drag moves (swing, a limit, voices, a scale morph or a
    /// degree offset): joins the script's open entry while the pointer is
    /// down.
    continuous: bool,
}

/// The Slice 3 edit setting `op` to `value` on `track`.
fn slice3_edit(
    op: &str,
    track: usize,
    value: f64,
    continuous: bool,
) -> Result<TrackSettingEdit, String> {
    let payload = slice3_numeric_payload(op, Some(track), value);
    Ok(TrackSettingEdit {
        command: slice3_command(&payload, op, Some(track))?,
        payload: Some(Value::Map(payload)),
        continuous,
    })
}

/// What `set-track-setting` asks for; `Ok(None)` when the track already is
/// so.
fn track_setting_request(
    app: &app::App,
    track: usize,
    setting: &str,
    map: &Payload,
) -> Result<Option<TrackSettingEdit>, String> {
    let tp = &app.state.pattern.track_params[track];
    let value = SetValue::of(map, "value", setting);
    let edit = |changed: bool, op: &str, value: f64, continuous: bool| {
        changed
            .then(|| slice3_edit(op, track, value, continuous))
            .transpose()
    };
    match setting {
        "gate" => edit(value.flag()? != tp.is_gate_on(), "toggle-gate", 0.0, false),
        "poly" => edit(
            value.flag()? != tp.is_polyphonic(),
            "toggle-poly",
            0.0,
            false,
        ),
        "max-polyphony" => {
            let voices = value.integer(1, sequencer::audio::MAX_VOICES)?;
            let changed = voices != tp.get_max_polyphony();
            edit(changed, "max-polyphony", voices as f64, true)
        }
        "voice-priority" => {
            let index = value.choice(&VOICE_PRIORITY_LABELS)?;
            let changed = index != tp.get_voice_priority() as usize;
            edit(changed, "voice-priority", index as f64, false)
        }
        "mono-trigger" => {
            let index = value.choice(&MONO_TRIGGER_LABELS)?;
            let changed = index != tp.get_mono_trigger() as usize;
            edit(changed, "mono-trigger", index as f64, false)
        }
        "mute-group" => {
            let group = value.integer(0, 8)?;
            edit(
                group != usize::from(tp.get_mute_group()),
                "mute-group",
                group as f64,
                false,
            )
        }
        "swing" => {
            let swing = value.number(50.0, 75.0)?;
            edit(swing as f32 != tp.get_swing(), "swing", swing, true)
        }
        "swing-resolution" => {
            let index = value.choice(&sequencer::sequencer::SwingResolution::LABELS)?;
            let changed = index != tp.get_swing_resolution() as usize;
            edit(changed, "swing-resolution", index as f64, false)
        }
        "fts" => {
            // The current label (an edited `*` scale, an imported name)
            // always works.
            let label = value.label()?;
            if label.eq_ignore_ascii_case(&fts_scale_label(tp)) {
                return Ok(None);
            }
            let scale = fts_scale_index(label)
                .map_or_else(|| value.fail("one of project.fts-options"), Ok)?;
            edit(true, "fts", scale as f64, false)
        }
        "accumulator" => {
            let label = value.label()?;
            let names = build_accumulator_names(app);
            if label.eq_ignore_ascii_case(&selected_accumulator_name_in(tp, &names)) {
                return Ok(None);
            }
            let index = accumulator_index(&names, label)
                .map_or_else(|| value.fail("one of project.accumulator-options"), Ok)?;
            let payload = accumulator_edit_payload(track, index, &names);
            Ok(Some(TrackSettingEdit {
                command: slice3_command(&payload, "accumulator", Some(track))?,
                payload: Some(Value::Map(payload)),
                continuous: false,
            }))
        }
        "accum-mode" => {
            let mode = value.choice(ACCUM_MODE_LABELS)?;
            edit(
                mode != tp.get_accum_mode() as usize,
                "accum-mode",
                mode as f64,
                false,
            )
        }
        "accum-limit" => {
            let limit = value.number(0.0, 127.0)?;
            edit(
                limit as f32 != tp.get_accum_limit(),
                "accum-limit",
                limit,
                true,
            )
        }
        "output" => {
            // A bus id (the main mix's for main), or nil for sends only.
            let bus = SetValue::of(map, "bus-id", "output").id_or_nil("a bus or nil")?;
            let bus = bus.map(sequencer::sequencer::BusId);
            let output = track_output_for_bus(app, bus)
                .ok_or_else(|| format!("output: no bus {:?}", bus.map(|bus| bus.0)))?;
            Ok((output != tp.output()).then_some(TrackSettingEdit {
                command: app::AppCommand::SetTrackOutput { track, output },
                payload: None,
                continuous: false,
            }))
        }
        other => Err(format!("unknown setting '{other}'")),
    }
}

/// What `set-tuning` asks for (`{:op :value :degree}`: `root`, `morph`,
/// `mode`, `offset`, `enabled`, `reset`, `just`, `rand`, `stretch`), as the
/// scale editor's edit ([`track_tuning_command`]); `Ok(None)` when the
/// scale already is so.
fn tuning_request(
    app: &app::App,
    track: usize,
    map: &Payload,
) -> Result<Option<TrackSettingEdit>, String> {
    let op = map_string(map, "op").ok_or("needs an :op")?;
    let tp = &app.state.pattern.track_params[track];
    let what = format!("tuning {op}");
    let value = SetValue::of(map, "value", &what);
    let degree = || {
        let count = tuning_degrees(tp.get_fts_scale(), &tp.tuning()).1.len();
        let degree = SetValue::of(map, "degree", "degree");
        match count {
            0 => Err("the scale is off".to_string()),
            count => degree.integer(0, count - 1),
        }
    };
    let cell = |value| Rc::new(RefCell::new(value));
    let mut payload: Payload = HashMap::new();
    payload.insert("track".to_string(), cell(Value::Number(track as f64)));
    let mut set = |key: &str, value: Value| {
        payload.insert(key.to_string(), cell(value));
    };
    let mut op = op.as_str();
    match op {
        "root" => set(
            "value",
            Value::Number(value.choice(&TUNING_ROOT_NAMES)? as f64),
        ),
        "morph" => set("value", Value::Number(value.number(0.0, 1.0)?)),
        "mode" => {
            let label = TUNING_MODE_LABELS[value.choice(&TUNING_MODE_LABELS)?];
            set("label", Value::String(label.to_string()));
        }
        "offset" => {
            set("degree", Value::Number(degree()? as f64));
            set("value", Value::Number(value.number(-1200.0, 1200.0)?));
        }
        "enabled" => {
            let index = degree()?;
            if value.flag()? == tp.tuning().degree_enabled(index) {
                return Ok(None);
            }
            set("degree", Value::Number(index as f64));
            op = "toggle";
        }
        "reset" | "just" => {}
        "rand" => set("value", Value::Number(value.number(0.0, 600.0)?)),
        "stretch" => set("value", Value::Number(value.number(-600.0, 600.0)?)),
        other => return Err(format!("unknown tuning edit '{other}'")),
    }
    let continuous = matches!(op, "morph" | "offset");
    payload.insert("op".to_string(), cell(Value::Keyword(op.to_string())));
    let command = track_tuning_command(app, track, &payload)?;
    Ok(command.map(|command| TrackSettingEdit {
        command,
        payload: Some(Value::Map(payload)),
        continuous,
    }))
}

/// The track a command's `:track-id` names now.
pub(super) fn command_track(app: &app::App, map: &Payload) -> Result<usize, String> {
    map_usize(map, "track-id")
        .and_then(|id| live_track_index(app, sequencer::sequencer::TrackId(id as u64)))
        .ok_or_else(|| "the track is gone".to_string())
}

/// Apply `edit` on `track` as a script edit and queue its invalidations.
/// An edit applied without history refreshes like a recorded one.
fn apply_edit(
    app: &mut app::App,
    editor: &mut Editor,
    ctx: &mut LoopCtx<'_>,
    track: usize,
    edit: TrackSettingEdit,
) -> Result<(), String> {
    let script = super::ScriptEdit::begin(app, ctx);
    let outcome = script.apply_with(app, |app| app::try_apply_command(app, edit.command));
    let label = match outcome {
        Ok(app::edit::EditOutcome::Applied(result)) => Some(Some(result.label)),
        Ok(app::edit::EditOutcome::AppliedUnrecorded) => Some(None),
        Ok(app::edit::EditOutcome::NoOp) => None,
        Err(error) => {
            script.end(app, ctx, edit.continuous, false);
            return Err(format!("{error:?}"));
        }
    };
    if let Some(label) = label.clone() {
        match &edit.payload {
            Some(payload) => slice3_edit_applied(editor, ctx, payload, Some(track), label),
            None => super::routing::track_output_applied(app, editor, ctx, track),
        }
    }
    script.end(app, ctx, edit.continuous, label.is_some());
    Ok(())
}

/// The step cursor (`selection.cursor-step`'s setter): makes the step's
/// track the current one ([`natives::CurrentTrackSwitch`], as
/// `seq-set-track`), moves the cursor (the Lisp global the step panel and
/// grid read), refreshes the step panel's fields from that track, and runs
/// the grid's cursor hook (`sequencer-cursor-step-changed`), as a click on
/// the step does.
fn set_cursor_step(
    app: &app::App,
    editor: &mut Editor,
    ctx: &LoopCtx<'_>,
    map: &Payload,
) -> Result<(), String> {
    let track = command_track(app, map)?;
    let num_steps = app.state.pattern.track_params[track]
        .get_num_steps()
        .clamp(1, MAX_STEPS);
    let step = SetValue::of(map, "step", "cursor-step").integer(0, num_steps - 1)?;
    let shared = ctx.shared;
    super::natives::CurrentTrackSwitch::of(shared).select(track);
    let rt = editor.runtime_mut();
    rt.set_global_value(FX_STEP_CURSOR_GLOBAL, Value::Number(step as f64));
    let (selected, count) = {
        let selected = shared.selected_steps.lock().unwrap();
        (selected.iter().copied().min(), selected.len())
    };
    sync_fx_step_cursor_binding_fields(rt, &shared.state, track, step, selected, count);
    const HOOK: &str = "sequencer-cursor-step-changed";
    if rt.global_value(HOOK).is_some() {
        let args = vec![Value::Number(track as f64), Value::Number(step as f64)];
        rt.invoke_global(HOOK, args)
            .map_err(|error| format!("{HOOK}: {error:?}"))?;
    }
    rt.run_reactive_cycle();
    shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// `set-bar-transpose!`: `{:track-id :bar :value}`, the bar of the track's
/// pattern and its semitones (in range: errors otherwise).
fn set_bar_transpose(
    app: &mut app::App,
    ctx: &mut LoopCtx<'_>,
    map: &Payload,
) -> Result<(), String> {
    let track = command_track(app, map)?;
    let bar = SetValue::of(map, "bar", "bar");
    let bar = bar.integer(0, sequencer::sequencer::BARS_PER_PATTERN - 1)?;
    let limit = f64::from(sequencer::sequencer::BAR_TRANSPOSE_LIMIT);
    let value = SetValue::of(map, "value", "bar transpose").number(-limit, limit)?;
    let script = super::ScriptEdit::begin(app, ctx);
    let outcome = script.apply_with(app, |app| {
        app::edit::apply_bar_transpose_edit(app, track, bar, value as f32)
    });
    let changed = matches!(
        outcome,
        Ok(app::edit::EditOutcome::Applied(_) | app::edit::EditOutcome::AppliedUnrecorded)
    );
    if changed {
        bar_transpose_applied(ctx, track);
    }
    script.end(app, ctx, true, changed);
    outcome.map(|_| ()).map_err(|error| format!("{error:?}"))
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
    let result = match name {
        "set-track-setting" | "set-tuning" => command_track(app, map).and_then(|track| {
            let request = if name == "set-tuning" {
                tuning_request(app, track, map)
            } else {
                let setting = map_string(map, "setting").ok_or("needs a :setting")?;
                track_setting_request(app, track, &setting, map)
            };
            match request? {
                Some(edit) => apply_edit(app, editor, ctx, track, edit),
                None => Ok(()),
            }
        }),
        "set-cursor-step" => set_cursor_step(app, editor, ctx, map),
        "set-track-bar-transpose" => set_bar_transpose(app, ctx, map),
        _ => Ok(()),
    };
    if let Err(message) = result {
        editor.handle_host_event(HostEvent::Error(format!("{name}: {message}")));
    }
}
