//! Track settings (the track panel's fields), the tracks' scales (`tuning`
//! and its `degree`s), the project's option lists and the per-track live
//! extras (bar transposes) (spec §14, stage 7i).
//!
//! Settings are model fields: the tick reads them from the `App` at the
//! model sync and pushes a track's fields only when its [`TrackSettings`]
//! (the raw model values, labelled only when pushed) changed. A track's
//! scale is read once per sync (one tuning lock) and its `tuning` and
//! `degree` fields pushed only when the scale or tuning changed; the push
//! compares each cell, so a morph drag notifies only the readers of `morph`
//! and of the degree pitches it moves.

use super::*;
use sequencer::scale::TrackTuning;
use sequencer::sequencer::{MonoTrigger, SwingResolution, TrackOutput, VoicePriority};

/// One track's settings as last pushed.
#[derive(Clone, PartialEq)]
pub(super) struct TrackSettings {
    poly: bool,
    max_polyphony: usize,
    gate: bool,
    supports_mono_trigger: bool,
    voice_priority: VoicePriority,
    mono_trigger: MonoTrigger,
    mute_group: u8,
    swing: f32,
    swing_resolution: SwingResolution,
    /// The accumulator index and, for a script accumulator, its name.
    accumulator: (usize, Option<String>),
    accum_mode: u32,
    accum_limit: f32,
    mod_output: bool,
}

impl TrackSettings {
    /// `track`'s own settings (a drum rack's voices are per slot, not here).
    fn of(app: &app::App, track: usize) -> Self {
        let tp = &app.state.pattern.track_params[track];
        Self {
            poly: tp.is_polyphonic(),
            max_polyphony: tp.get_max_polyphony(),
            gate: tp.is_gate_on(),
            supports_mono_trigger: track_supports_mono_trigger(app, track),
            voice_priority: tp.get_voice_priority(),
            mono_trigger: tp.get_mono_trigger(),
            mute_group: tp.get_mute_group().min(8),
            swing: tp.get_swing(),
            swing_resolution: tp.get_swing_resolution(),
            accumulator: (tp.get_accumulator_idx(), tp.script_accumulator_name()),
            accum_mode: tp.get_accum_mode(),
            accum_limit: tp.get_accum_limit(),
            mod_output: app.graph.track_exposes_mod_output(track),
        }
    }

    /// Push the fields of track instance `id`, labelled as the track panel
    /// shows them; `accumulators` names the accumulators by index.
    fn push(&self, pusher: &mut Pusher<'_>, id: InstanceId, accumulators: &[String]) {
        pusher.push(id, f::TRACK_POLY, Value::Bool(self.poly));
        let voices = number(self.max_polyphony as f64);
        pusher.push(id, f::TRACK_MAX_POLYPHONY, voices);
        pusher.push(id, f::TRACK_GATE, Value::Bool(self.gate));
        let mono = Value::Bool(self.supports_mono_trigger);
        pusher.push(id, f::TRACK_SUPPORTS_MONO_TRIGGER, mono);
        let priority = text(voice_priority_label(self.voice_priority));
        pusher.push(id, f::TRACK_VOICE_PRIORITY, priority);
        let trigger = text(mono_trigger_label(self.mono_trigger));
        pusher.push(id, f::TRACK_MONO_TRIGGER, trigger);
        pusher.push(id, f::TRACK_MUTE_GROUP, number(self.mute_group));
        pusher.push(id, f::TRACK_SWING, number(self.swing));
        let resolution = text(self.swing_resolution.label());
        pusher.push(id, f::TRACK_SWING_RESOLUTION, resolution);
        let (idx, script) = &self.accumulator;
        let accumulator = accumulator_name(*idx, script.clone(), accumulators);
        pusher.push(id, f::TRACK_ACCUMULATOR, Value::String(accumulator));
        pusher.push(
            id,
            f::TRACK_ACCUM_MODE,
            text(accum_mode_label(self.accum_mode)),
        );
        pusher.push(id, f::TRACK_ACCUM_LIMIT, number(self.accum_limit));
        pusher.push(id, f::TRACK_MOD_OUTPUT, Value::Bool(self.mod_output));
    }
}

/// Push scale `scale` under `tuning` into tuning instance `id`: its fields,
/// then its degrees (registered and dropped to the degree count).
fn push_tuning(pusher: &mut Pusher<'_>, id: InstanceId, scale: usize, tuning: &TrackTuning) {
    let (period, degrees) = tuning_degrees(scale, tuning);
    let name = sequencer::scale::scale_name(scale, tuning).to_string();
    pusher.push(id, f::TUNING_ON, Value::Bool(!degrees.is_empty()));
    pusher.push(id, f::TUNING_SCALE, Value::String(name));
    pusher.push(id, f::TUNING_CUSTOM, Value::Bool(tuning.custom.is_some()));
    let edited = Value::Bool(tuning.has_degree_edits());
    pusher.push(id, f::TUNING_EDITED, edited);
    let root = Value::String(tuning_root_label(tuning).to_string());
    pusher.push(id, f::TUNING_ROOT, root);
    pusher.push(id, f::TUNING_MORPH, number(tuning.morph));
    let mode = Value::String(tuning.mode.label().to_string());
    pusher.push(id, f::TUNING_MODE, mode);
    pusher.push(id, f::TUNING_PERIOD, number(period));
    let ids = indexed_children(
        &mut *pusher.rt,
        id,
        DEGREE,
        degrees.len(),
        |store, degree, index| {
            store.push(degree, f::DEGREE_TUNING, Value::Instance(id));
            store.push(degree, f::DEGREE_INDEX, number(index as f64));
        },
    );
    for (degree, degree_id) in degrees.into_iter().zip(&ids) {
        let degree_id = *degree_id;
        pusher.push(degree_id, f::DEGREE_BASE, number(degree.base));
        pusher.push(degree_id, f::DEGREE_OFFSET, number(degree.offset));
        pusher.push(degree_id, f::DEGREE_ENABLED, Value::Bool(degree.enabled));
        pusher.push(degree_id, f::DEGREE_PITCH, number(degree.pitch));
        pusher.push(degree_id, f::DEGREE_LABEL, Value::String(degree.label));
        pusher.push(degree_id, f::DEGREE_RATIO, Value::String(degree.ratio));
    }
    pusher.push(id, f::TUNING_DEGREES, instance_list(ids));
}

/// The project's option lists as last pushed (`project.accumulator-options`,
/// `output-options`; the fixed `fts-options`, `sync-options` and
/// `step-param-options` once).
#[derive(Default)]
pub(super) struct ProjectOptions {
    /// Whether the lists were pushed since the last schema change.
    pushed: bool,
    accumulators: Option<Vec<String>>,
    outputs: Option<Vec<InstanceId>>,
}

/// The number of 16-step bars a track of `num_steps` steps shows (at
/// least one).
pub(super) fn bar_count(num_steps: usize) -> usize {
    num_steps
        .div_ceil(sequencer::sequencer::STEPS_PER_PAGE)
        .clamp(1, sequencer::sequencer::BARS_PER_PATTERN)
}

impl HostKinds {
    /// Track `id`'s (position `track`) settings, pushed when they changed
    /// since the last model sync (or, for the accumulator's name, when the
    /// accumulator list did: `accumulators_changed`); its output, and its
    /// scale ([`Self::sync_tuning`]).
    pub(super) fn sync_track_settings(
        &mut self,
        pusher: &mut Pusher<'_>,
        app: &app::App,
        track: usize,
        id: InstanceId,
        accumulators: &[String],
        accumulators_changed: bool,
    ) {
        let settings = TrackSettings::of(app, track);
        let previous = self.settings.get(&id);
        if previous != Some(&settings) || accumulators_changed {
            settings.push(pusher, id, accumulators);
            pusher.shared.borrow_mut().settings_pushes += 1;
            self.settings.insert(id, settings);
        }
        let tp = &app.state.pattern.track_params[track];
        let bus = |bus: sequencer::sequencer::BusId| self.buses.get(&bus.0).copied();
        let mix = || bus(sequencer::sequencer::BusId::MIX);
        // A bus that is gone reads as the main mix, as the output dropdown
        // shows it.
        let output = match tp.output() {
            TrackOutput::Mix => mix(),
            TrackOutput::Bus(id) => bus(id).or_else(mix),
            TrackOutput::None => None,
        };
        pusher.push(id, f::TRACK_OUTPUT, instance_or_nil(output));
        self.sync_tuning(pusher, tp, id);
    }

    /// Track `track_id`'s scale: its `tuning` instance (registered once),
    /// its fields, its degrees and the track's `fts` label, pushed when
    /// the scale or the tuning changed since the last push.
    fn sync_tuning(
        &mut self,
        pusher: &mut Pusher<'_>,
        tp: &sequencer::sequencer::TrackParams,
        track_id: InstanceId,
    ) {
        let key = [track_id, 0];
        let tuning_id = match pusher.rt.keyed_instance(TUNING, &key) {
            Some(id) => id,
            None => {
                let Ok(id) = pusher.rt.register_keyed_instance(TUNING, &key) else {
                    return;
                };
                pusher.changed = true;
                pusher.push(id, f::TUNING_TRACK, Value::Instance(track_id));
                id
            }
        };
        pusher.push(track_id, f::TRACK_TUNING, Value::Instance(tuning_id));
        let scale = tp.get_fts_scale();
        let tuning = tp.tuning();
        let last = self.tunings.get(&tuning_id);
        if last
            .is_some_and(|(last_scale, last_tuning)| *last_scale == scale && *last_tuning == tuning)
        {
            return;
        }
        pusher.push(
            track_id,
            f::TRACK_FTS,
            Value::String(fts_label(scale, &tuning)),
        );
        push_tuning(pusher, tuning_id, scale, &tuning);
        // Degrees may have been registered or dropped.
        pusher.changed = true;
        pusher.shared.borrow_mut().tuning_pushes += 1;
        self.tunings.insert(tuning_id, (scale, tuning));
    }

    /// `project.accumulator-options`, pushed when the list changed (and the
    /// fixed `fts-options`, `sync-options`, `step-param-options` and
    /// `focus-step-params` once).
    /// Returns whether the accumulator list changed.
    pub(super) fn sync_accumulator_options(
        &mut self,
        pusher: &mut Pusher<'_>,
        accumulators: &[String],
    ) -> bool {
        let Some(project) = pusher.singleton(PROJECT) else {
            return false;
        };
        let options = &mut self.project_options;
        let changed = options.accumulators.as_deref() != Some(accumulators);
        if options.pushed && !changed {
            return false;
        }
        if !options.pushed {
            let fts = list_value(fts_scale_names().map(|name| Value::String(name.to_string())));
            pusher.push(project, f::PROJECT_FTS_OPTIONS, fts);
            let sync = list_value(sync_labels().map(Value::String));
            pusher.push(project, f::PROJECT_SYNC_OPTIONS, sync);
            let step_params = crate::param_words::step_param_target_names();
            let step_params = list_value(step_params.map(|name| Value::String(name.to_string())));
            pusher.push(project, f::PROJECT_STEP_PARAM_OPTIONS, step_params);
            pusher.push(
                project,
                f::PROJECT_FOCUS_STEP_PARAMS,
                focus_step_params_value(),
            );
            options.pushed = true;
        }
        let list = list_value(accumulators.iter().cloned().map(Value::String));
        pusher.push(project, f::PROJECT_ACCUMULATOR_OPTIONS, list);
        if changed {
            options.accumulators = Some(accumulators.to_vec());
        }
        changed
    }

    /// `project.output-options` (the bus instances a track's output may
    /// name, in bus order), pushed when they changed.
    pub(super) fn sync_output_options(
        &mut self,
        pusher: &mut Pusher<'_>,
        buses: &[Option<InstanceId>],
    ) {
        let Some(project) = pusher.singleton(PROJECT) else {
            return;
        };
        let outputs = &mut self.project_options.outputs;
        if outputs
            .as_ref()
            .is_some_and(|last| last.iter().copied().eq(buses.iter().flatten().copied()))
        {
            return;
        }
        let list: Vec<InstanceId> = buses.iter().flatten().copied().collect();
        pusher.push(
            project,
            f::PROJECT_OUTPUT_OPTIONS,
            instance_list(list.iter().copied()),
        );
        *outputs = Some(list);
    }

    /// Push the option lists again at the next model sync (a schema change
    /// may have reset the fields); the tracks' accumulator names stand.
    pub(super) fn reset_project_options(&mut self) {
        self.project_options.pushed = false;
        self.project_options.outputs = None;
    }

    /// `t.bar-transposes` of an observing track, pushed when a bar moved
    /// since the last push: compared in place, allocating only on a change.
    pub(super) fn sync_bar_transposes(
        &mut self,
        pusher: &mut Pusher<'_>,
        track: usize,
        id: InstanceId,
    ) {
        let sources = pusher.sources;
        let state = &sources.state;
        let bars = bar_count(sources.num_steps(track));
        let bar = |bar: usize| state.bar_transpose(track, bar) as f64;
        let last = self.bar_transposes.entry(id).or_default();
        let changed = last.len() != bars || (0..bars).any(|index| last[index] != bar(index));
        if changed {
            last.clear();
            last.extend((0..bars).map(bar));
        }
        let value = || list_value(last.iter().map(|semitones| number(*semitones)));
        pusher.push_computed_if(id, f::TRACK_BAR_TRANSPOSES, changed, value);
    }

    /// `t.step-params-in-use` of an observing track, pushed when the set
    /// moved since the last push: a scan of its active steps, allocating
    /// only on a change (a step edit that keeps the set pushes nothing).
    pub(super) fn sync_step_params_in_use(
        &mut self,
        pusher: &mut Pusher<'_>,
        track: usize,
        id: InstanceId,
    ) {
        let sources = pusher.sources;
        let mask = live::step_params_in_use(&sources.state, track, sources.num_steps(track));
        let changed = self.step_params_in_use.insert(id, mask) != Some(mask);
        let value = || live::step_param_names(mask);
        pusher.push_computed_if(id, f::TRACK_STEP_PARAMS_IN_USE, changed, value);
    }
}
