//! What the reader hook and the tick share: the handles to live sequencer
//! state, the live-field values ([`live_value`]) and the reader hook.

use super::*;

/// What the reader hook and the tick share.
#[derive(Default)]
pub(crate) struct KindsShared {
    /// Live-field computations per field, for tests and profiling: an
    /// unobserved field never counts.
    pub(crate) computed: HashMap<FieldKey, u64>,
    /// Runs of the model half of the sync (gated by `ModelRevision`).
    pub(crate) model_syncs: u64,
    /// The last meter level of each track position (`t.peak`; the tick
    /// copies the meter cache, pruned to the track count).
    peaks: Vec<f64>,
    /// Bus meter levels by bus position (`b.peak`), the master pair
    /// (`master.peak-l`/`-r`) and the audio load (`engine.cpu-load`), copied
    /// by the tick like `peaks`.
    bus_peaks: Vec<f64>,
    master_peaks: (f64, f64),
    cpu_load: f64,
    /// Who another track's (or a bus's) solo silences, as of the last
    /// sync (`t.audible`; the tick copies it from the `App`).
    solo: Option<app::SoloAudibility>,
    /// Fields a schema mismatch keeps the host from pushing.
    pub(super) skip: HashSet<FieldKey>,
    /// What each device instance's params read, as of the last model sync
    /// (descriptors and where the values live; see [`DeviceSource`]).
    pub(super) devices: HashMap<InstanceId, Rc<DeviceSource>>,
    /// The devices whose params are registered (`d.params` read or
    /// observed once); their `params` is then a model field.
    pub(super) param_devices: HashSet<InstanceId>,
    /// The step p-lock render per track position, with the [`PlockKey`] it
    /// was computed under ([`track_plock_render`]).
    pub(super) plock_renders: Vec<Option<(PlockKey, Rc<[StepPlockRender]>)>>,
    /// Whole-track p-lock render scans, and param instances the tick asked
    /// for their observed fields, for tests.
    pub(crate) plock_scans: u64,
    pub(crate) param_queries: u64,
    /// Tracks whose settings, and tunings whose scale, were pushed (they
    /// changed), for tests.
    pub(crate) settings_pushes: u64,
    pub(crate) tuning_pushes: u64,
    /// The macro engine's override layer, copied by the tick when it
    /// changes (`param.value` shows an engaged macro's value).
    pub(super) macro_overrides: HashMap<sequencer::macro_engine::MacroParamKey, f32>,
    /// Effect chain slots per track position (the length of its
    /// `app.graph.effect_descriptors` row), for `step.plocked`.
    pub(super) effect_slots: Vec<usize>,
    /// The mod port levels (`t.mod-in-1`, `t.mod-out-level`, `b.mod-in-1`,
    /// …), copied by the tick from its meter cache when they change.
    mod_ports: ModPortLevels,
    /// Whether the audio-overload warning shows (`engine.overloaded`).
    overloaded: bool,
    /// The bus ids by bus position, as of the last model sync (the mod
    /// port levels are by bus id).
    pub(super) bus_ids: Vec<u64>,
    /// Each route instance's connection (track positions), as of the last
    /// model sync (`route.selected`).
    pub(super) routes: HashMap<InstanceId, sequencer::sequencer::ModConnection>,
}

impl KindsShared {
    pub(super) fn count(&mut self, key: FieldKey) {
        *self.computed.entry(key).or_default() += 1;
    }

    /// Copy the tick's meter cache (pruned to the track and bus counts) and
    /// the solo state.
    pub(super) fn copy_meters(&mut self, app: &app::App, meters: &KindsMeters<'_>) {
        copy_prefix(&mut self.peaks, meters.tracks, app.tracks.len());
        copy_prefix(&mut self.bus_peaks, meters.buses, app.buses.len());
        self.master_peaks = meters.master;
        self.cpu_load = meters.cpu_load;
        self.solo = Some(app.solo_audibility());
        if &self.mod_ports != meters.mod_ports {
            self.mod_ports.clone_from(meters.mod_ports);
        }
        self.overloaded = meters.overloaded;
        let overrides = app.macro_engine.overrides();
        if &self.macro_overrides != overrides {
            self.macro_overrides.clone_from(overrides);
        }
    }
}

/// Make `cache` the first `len` of `levels` (fewer when it is shorter),
/// copying only when they differ.
fn copy_prefix(cache: &mut Vec<f64>, levels: &[f64], len: usize) {
    let levels = &levels[..len.min(levels.len())];
    if cache.as_slice() != levels {
        cache.clear();
        cache.extend_from_slice(levels);
    }
}

/// What host kinds read outside the `App`: the shared live state and the UI
/// epochs. The event loop's [`SharedHandles`] holds all of it
/// ([`KindsHandles::of`]); headless capture, which has no `SharedHandles`,
/// builds one from its own handles. The reader hook keeps the first one it
/// is installed with.
#[derive(Clone)]
pub(crate) struct KindsHandles {
    pub(crate) state: Arc<SequencerState>,
    pub(crate) current_track: Arc<AtomicUsize>,
    pub(crate) selected_steps: Arc<Mutex<HashSet<usize>>>,
    pub(crate) active_delete_target: Arc<Mutex<Option<ActiveDeleteTarget>>>,
    pub(crate) active_delete_target_version: Arc<AtomicUsize>,
    pub(crate) record_armed: Arc<Mutex<Vec<bool>>>,
    pub(crate) recording: Arc<AtomicBool>,
    pub(crate) master_recording: Arc<AtomicBool>,
    pub(crate) selected_tracks: Arc<Mutex<HashSet<usize>>>,
    pub(crate) track_collapsed: Arc<Mutex<Vec<bool>>>,
    pub(crate) ui_epoch: Arc<AtomicUsize>,
    pub(crate) fx_epoch: Arc<AtomicUsize>,
    pub(crate) fx_value_epoch: Arc<AtomicUsize>,
    /// Its per-track p-lock revisions say a p-lock may have moved (the step
    /// p-lock render, `has-locks`: [`KindsHandles::plock_key`]).
    pub(crate) ui_invalidations: Arc<UiInvalidationQueue>,
    pub(crate) step_print: Arc<Mutex<StepPrintState>>,
    /// While a deadline in it lies ahead, the view does not follow the
    /// playhead (`selection.auto-follow`).
    pub(crate) auto_follow_override_until: Arc<Mutex<Option<Instant>>>,
}

impl KindsHandles {
    pub(crate) fn of(shared: &SharedHandles) -> Self {
        Self {
            state: shared.state.clone(),
            current_track: shared.current_track.clone(),
            selected_steps: shared.selected_steps.clone(),
            active_delete_target: shared.active_delete_target.clone(),
            active_delete_target_version: shared.active_delete_target_version.clone(),
            record_armed: shared.record_armed.clone(),
            recording: shared.recording.clone(),
            master_recording: shared.master_recording.clone(),
            selected_tracks: shared.selected_tracks.clone(),
            track_collapsed: shared.track_collapsed.clone(),
            ui_epoch: shared.ui_epoch.clone(),
            fx_epoch: shared.fx_epoch.clone(),
            fx_value_epoch: shared.fx_value_epoch.clone(),
            ui_invalidations: shared.ui_invalidations.clone(),
            step_print: shared.step_print.clone(),
            auto_follow_override_until: shared.auto_follow_override_until.clone(),
        }
    }

    pub(super) fn track_exists(&self, track: usize) -> bool {
        track < self.state.active_track_count()
    }

    pub(super) fn num_steps(&self, track: usize) -> usize {
        self.state.pattern.track_params[track]
            .get_num_steps()
            .min(MAX_STEPS)
    }

    /// A step's parameter (`held` has none: see [`track_held_steps`]).
    pub(super) fn step_param(&self, track: usize, step: usize, param: StepParam) -> f64 {
        self.state.pattern.step_data[track].get(step, param) as f64
    }

    /// The step whose p-locks `track`'s controls show: on the current track
    /// the selected step, else the playing one; none on other tracks (like
    /// the legacy `track-N-bus-M-send`).
    pub(super) fn plock_display_step(&self, track: usize) -> Option<usize> {
        (self.current_track.load(Ordering::Relaxed) == track)
            .then(|| {
                displayed_plock_step(
                    &self.state,
                    track,
                    selected_plock_step(&self.selected_steps),
                )
            })
            .flatten()
    }

    pub(super) fn playing_step(&self, track: usize) -> Option<usize> {
        self.state
            .transport
            .playing
            .load(Ordering::Relaxed)
            .then(|| track_active_playhead_step(&self.state, track))
    }

    /// Whether the step selection applies to `track`: the current track,
    /// or one of a rack-wide selection's tracks while its delete target is
    /// armed (like the legacy step-selection publish).
    fn selection_covers(&self, track: usize) -> bool {
        self.current_track.load(Ordering::Relaxed) == track
            || matches!(
                &*self.active_delete_target.lock().unwrap(),
                Some(ActiveDeleteTarget::TrackSteps { tracks }) if tracks.contains(&track)
            )
    }
}

/// A live field's current value (see [`Feed::Live`]); `None` for anything
/// else, and for a field a schema mismatch skips. Counts the computation.
pub(super) fn live_value<S: KindStore>(
    store: &mut S,
    sources: &KindsHandles,
    shared: &RefCell<KindsShared>,
    id: InstanceId,
    field: &str,
) -> Option<Value> {
    let kind = store.kind_of(id)?;
    let key = *LIVE_KEYS
        .iter()
        .find(|(live_kind, live_field)| *live_kind == kind && *live_field == field)?;
    if shared.borrow().skip.contains(&key) {
        return None;
    }
    let value = match key.0 {
        TRACK => {
            let track = *store.key_of(id)?.first()? as usize;
            if !sources.track_exists(track) {
                return None;
            }
            let params = &sources.state.pattern.track_params[track];
            match key {
                f::TRACK_VOLUME => number(params.get_volume()),
                f::TRACK_MUTED => Value::Bool(params.is_muted()),
                // Effective mute, like the legacy `track-muted-effective`.
                f::TRACK_AUDIBLE => Value::Bool(
                    !params.is_muted()
                        && !shared
                            .borrow()
                            .solo
                            .as_ref()
                            .is_some_and(|solo| solo.track_is_muted(params)),
                ),
                f::TRACK_ARMED => Value::Bool(
                    sources
                        .record_armed
                        .lock()
                        .unwrap()
                        .get(track)
                        .copied()
                        .unwrap_or(false),
                ),
                f::TRACK_SELECTED => {
                    Value::Bool(sources.current_track.load(Ordering::Relaxed) == track)
                }
                f::TRACK_NUM_STEPS => number(sources.num_steps(track) as f64),
                f::TRACK_STEPS => {
                    let num_steps = sources.num_steps(track);
                    track_steps(store, id, num_steps)
                }
                f::TRACK_PEAK => number(shared.borrow().peaks.get(track).copied()?),
                f::TRACK_PAN => number(params.get_pan()),
                f::TRACK_SOLOED => Value::Bool(params.is_solo()),
                f::TRACK_COLLAPSED => Value::Bool(
                    sources
                        .track_collapsed
                        .lock()
                        .unwrap()
                        .get(track)
                        .copied()
                        .unwrap_or(false),
                ),
                f::TRACK_PLAYHEAD => {
                    number(sources.playing_step(track).map_or(-1.0, |step| step as f64))
                }
                f::TRACK_TIMEBASE => Value::String(params.get_timebase().label().to_string()),
                f::TRACK_MOD_OUT_LEVEL => {
                    let shared = shared.borrow();
                    number(
                        shared
                            .mod_ports
                            .track_outputs
                            .get(track)
                            .copied()
                            .unwrap_or(0.0),
                    )
                }
                f::TRACK_BAR_TRANSPOSES => {
                    let bars =
                        track_bar_transposes(&sources.state, track, sources.num_steps(track));
                    list_value(bars.iter().map(|semitones| number(*semitones)))
                }
                f::TRACK_DELETE_TARGET => {
                    let target = sources.active_delete_target.lock().unwrap();
                    Value::Bool(mixer_track_delete_target_selected(target.as_ref(), track))
                }
                key => {
                    let input = mod_input(&f::TRACK_MOD_IN, key)?;
                    let shared = shared.borrow();
                    let inputs = shared.mod_ports.track_inputs.get(track);
                    number(inputs.map_or(0.0, |inputs| inputs[input]))
                }
            }
        }
        STEP => {
            let &[parent, step] = store.key_of(id)? else {
                return None;
            };
            let track = *store.key_of(parent)?.first()? as usize;
            let step = step as usize;
            if !sources.track_exists(track) {
                return None;
            }
            match key {
                f::STEP_ACTIVE => {
                    Value::Bool(sources.state.pattern.patterns[track].is_active(step))
                }
                f::STEP_PLAYING => Value::Bool(sources.playing_step(track) == Some(step)),
                f::STEP_SELECTED => Value::Bool(
                    step < sources.num_steps(track)
                        && sources.selection_covers(track)
                        && sources.selected_steps.lock().unwrap().contains(&step),
                ),
                f::STEP_HELD => {
                    Value::Bool(track_step_duration_covered(&sources.state, track, step))
                }
                f::STEP_PLOCKED | f::STEP_LOCK_KIND | f::STEP_VARIANT_COLOR => {
                    track_plock_render(sources, shared, track)
                        .get(step)?
                        .field(key)?
                }
                _ => number(sources.step_param(track, step, step_param_named(key.1)?)),
            }
        }
        SEND => {
            let &[parent, bus] = store.key_of(id)? else {
                return None;
            };
            let track = *store.key_of(parent)?.first()? as usize;
            if !sources.track_exists(track) {
                return None;
            }
            let (state, bus) = (&sources.state, sequencer::sequencer::BusId(bus));
            match key {
                f::SEND_AMOUNT => number(track_send_base(state, track, bus)),
                f::SEND_DISPLAY => number(displayed_track_send_amount(
                    state,
                    track,
                    bus,
                    sources.plock_display_step(track),
                )),
                f::SEND_LOCKED => Value::Bool(
                    track_send_lock(state, track, bus, sources.plock_display_step(track)).is_some(),
                ),
                f::SEND_HAS_LOCKS => Value::Bool(
                    state.pattern.track_send_plocks[track]
                        .has_lock_for(bus, sources.num_steps(track)),
                ),
                _ => return None,
            }
        }
        DEVICE => {
            let device = shared.borrow().devices.get(&id)?.clone();
            match key {
                f::DEVICE_PLAYHEAD => number(device.sampler.as_ref().map_or(0.0, |s| s.seconds())),
                _ => return None,
            }
        }
        PARAM => {
            let &[device_id, index] = store.key_of(id)? else {
                return None;
            };
            let device = shared.borrow().devices.get(&device_id)?.clone();
            param_live_value(sources, shared, &device, index as usize, key)?
        }
        BUS => {
            let bus = *store.key_of(id)?.first()? as usize;
            let shared = shared.borrow();
            match key {
                f::BUS_PEAK => number(shared.bus_peaks.get(bus).copied()?),
                key => {
                    let input = mod_input(&f::BUS_MOD_IN, key)?;
                    let bus_id = *shared.bus_ids.get(bus)?;
                    let ports = &shared.mod_ports.bus_inputs;
                    let inputs = ports.iter().find(|(id, _)| *id == bus_id);
                    number(inputs.map_or(0.0, |(_, inputs)| inputs[input]))
                }
            }
        }
        ROUTE => {
            let connection = *shared.borrow().routes.get(&id)?;
            let target = sources.active_delete_target.lock().unwrap();
            Value::Bool(target.as_ref() == Some(&mod_route_delete_target(&connection)))
        }
        _ => match key {
            f::TRANSPORT_PLAYING => {
                Value::Bool(sources.state.transport.playing.load(Ordering::Relaxed))
            }
            f::TRANSPORT_RECORDING => Value::Bool(sources.recording.load(Ordering::Relaxed)),
            f::TRANSPORT_BPM => number(sources.state.transport.bpm.load(Ordering::Relaxed)),
            f::TRANSPORT_POSITION => {
                number(sources.state.transport.playhead.load(Ordering::Relaxed))
            }
            f::TRANSPORT_METRONOME => Value::Bool(
                sources
                    .state
                    .transport
                    .metronome_enabled
                    .load(Ordering::Relaxed),
            ),
            f::TRANSPORT_ROLL_MODE => {
                Value::Bool(sources.state.transport.roll_mode.load(Ordering::Relaxed))
            }
            f::TRANSPORT_RECORD_QUANTIZE => {
                let raw = sources
                    .state
                    .transport
                    .record_quantize
                    .load(Ordering::Relaxed);
                let quantize = sequencer::record_quantize::RecordQuantize::from_atomic(raw as u8);
                Value::String(quantize.transport_label().to_string())
            }
            f::MASTER_PEAK_L => number(shared.borrow().master_peaks.0),
            f::MASTER_PEAK_R => number(shared.borrow().master_peaks.1),
            f::MASTER_RECORDING => Value::Bool(sources.master_recording.load(Ordering::Relaxed)),
            f::ENGINE_CPU_LOAD => number(shared.borrow().cpu_load),
            f::ENGINE_LATENCY_MS => number(sources.state.pdc_latency_seconds() as f64 * 1000.0),
            f::ENGINE_OVERLOADED => Value::Bool(shared.borrow().overloaded),
            f::TRANSPORT_ROLL_RATE => {
                let raw = sources.state.transport.roll_rate.load(Ordering::Relaxed);
                Value::String(roll_rate_label(raw).to_string())
            }
            f::TRANSPORT_SEQUENCE_ROLLING => Value::Bool(
                sources
                    .state
                    .transport
                    .sequence_rolling
                    .load(Ordering::Relaxed),
            ),
            f::SELECTION_AUTO_FOLLOW => {
                Value::Bool(auto_follow_enabled(&sources.auto_follow_override_until))
            }
            f::SELECTION_STEPS => {
                let steps = selection_steps(store, sources);
                instance_list(steps.map_or_else(Vec::new, |(_, steps)| steps))
            }
            f::SELECTION_CURSOR_STEP | f::SELECTION_EDIT_STEP => {
                let (cursor, edit) = selection_cursor(store, sources);
                let step = if key == f::SELECTION_CURSOR_STEP {
                    cursor
                } else {
                    edit
                };
                instance_or_nil(step)
            }
            f::SELECTION_TRACK => {
                let track = sources.current_track.load(Ordering::Relaxed) as u64;
                instance_or_nil(store.keyed(TRACK, &[track]))
            }
            f::SELECTION_TRACKS => {
                let tracks = sorted_selected_tracks(&sources.selected_tracks.lock().unwrap());
                instance_list(
                    tracks
                        .into_iter()
                        .filter_map(|track| store.keyed(TRACK, &[track as u64])),
                )
            }
            _ => return None,
        },
    };
    shared.borrow_mut().count(key);
    Some(value)
}

/// Which of the four mod inputs `key` is among `keys`.
fn mod_input(keys: &[FieldKey; 4], key: FieldKey) -> Option<usize> {
    keys.iter().position(|input| *input == key)
}

/// The bar transposes of `track`'s pattern, one per 16-step bar of its
/// `num_steps` (at least one).
fn track_bar_transposes(state: &SequencerState, track: usize, num_steps: usize) -> Vec<f64> {
    (0..bar_count(num_steps))
        .map(|bar| state.bar_transpose(track, bar) as f64)
        .collect()
}

/// The current track's instance and its length, while it exists.
fn current_track<S: KindStore>(store: &S, sources: &KindsHandles) -> Option<(InstanceId, usize)> {
    let track = sources.current_track.load(Ordering::Relaxed);
    if !sources.track_exists(track) {
        return None;
    }
    let id = store.keyed(TRACK, &[track as u64])?;
    Some((id, sources.num_steps(track)))
}

/// Step `step` of a track (instance `track`, `num_steps` long), registering
/// the track's steps when it has none yet (spec D2).
fn step_of<S: KindStore>(
    store: &mut S,
    track: InstanceId,
    num_steps: usize,
    step: usize,
) -> Option<InstanceId> {
    if step >= num_steps {
        return None;
    }
    if let Some(id) = store.keyed(STEP, &[track, step as u64]) {
        return Some(id);
    }
    track_steps(store, track, num_steps);
    store.keyed(STEP, &[track, step as u64])
}

/// The current track and its selected steps (`selection.steps`), in order.
fn selection_steps<S: KindStore>(
    store: &mut S,
    sources: &KindsHandles,
) -> Option<(InstanceId, Vec<InstanceId>)> {
    let (track, num_steps) = current_track(store, sources)?;
    let mut selected: Vec<usize> = sources
        .selected_steps
        .lock()
        .unwrap()
        .iter()
        .copied()
        .collect();
    selected.sort_unstable();
    let steps = selected
        .into_iter()
        .filter_map(|step| step_of(store, track, num_steps, step))
        .collect();
    Some((track, steps))
}

/// `selection.cursor-step` and `selection.edit-step`: the step under the
/// step cursor (the Lisp global the step panel moves) and the step the
/// panel edits, on the current track ([`fx_step_cursor`]).
fn selection_cursor<S: KindStore>(
    store: &mut S,
    sources: &KindsHandles,
) -> (Option<InstanceId>, Option<InstanceId>) {
    let Some((track, num_steps)) = current_track(store, sources) else {
        return (None, None);
    };
    let cursor = fx_step_cursor_value(store.global(FX_STEP_CURSOR_GLOBAL));
    let selected = selected_plock_step(&sources.selected_steps);
    let (cursor, edit) = fx_step_cursor(num_steps, cursor, selected);
    (
        step_of(store, track, num_steps, cursor),
        step_of(store, track, num_steps, edit),
    )
}

/// Install the reader hook that answers by-value reads of unobserved live
/// fields (and registers steps on a cold `t.steps`, params on the first
/// `d.params`). Anything but a live
/// field name returns `None` before any lookup.
pub(super) fn install_reader(
    rt: &mut Runtime,
    sources: Rc<KindsHandles>,
    shared: Rc<RefCell<KindsShared>>,
) {
    let reader: HostFieldReader = Rc::new(move |vm: &mut VM, id: InstanceId, field: &str| {
        if field == f::DEVICE_PARAMS.1 && vm.instance_kind(id) == Some(DEVICE) {
            // A model field, but registered on the first read.
            return cold_device_params(vm, &shared, id);
        }
        if !LIVE_KEYS.iter().any(|(_, live)| *live == field) {
            return None;
        }
        live_value(vm, &sources, &shared, id, field)
    });
    rt.set_host_field_reader(Some(reader));
}
