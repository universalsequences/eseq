//! Host kinds against the real `eseq.kinds`, the real natives and a headless
//! project (docs/kind-bindings-spec.md §13 stage 4, §14).

use super::*;
use eseqlisp::reactive::read_float_slot;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicI32};

struct GraphGuard(sequencer::audiograph::LiveGraphPtr);

impl Drop for GraphGuard {
    fn drop(&mut self) {
        unsafe {
            sequencer::audiograph::clear_os_workgroup();
            sequencer::audiograph::engine_stop_workers();
            sequencer::audiograph::destroy_live_graph(self.0 .0);
        }
    }
}

/// A headless app with the bare (`-noui`) root, the production natives and
/// a fresh two-track project. Fields drop in order: the graph goes last.
struct Harness {
    app: app::App,
    editor: Editor,
    shared: SharedHandles,
    sessions: EditSessionState,
    frame: FrameDiffState,
    gesture: GestureState,
    meters: MeterCache,
    track_names: Vec<String>,
    _graph: GraphGuard,
}

const REFER: &str =
    "(import eseq.kinds :refer (track tracks scenes banks transport selection launch!))";

fn meter_cache() -> MeterCache {
    MeterCache {
        cached_peak_l_level: 0.0,
        cached_peak_r_level: 0.0,
        cached_track_peak_levels: Vec::new(),
        cached_rack_slot_peak_levels: Vec::new(),
        cached_bus_peak_levels: Vec::new(),
        cached_modulator_phases: Vec::new(),
        cached_modulator_levels: Vec::new(),
        cached_mod_port_levels: Default::default(),
        cached_mod_display_values: Default::default(),
        watched_display_modulators: HashSet::new(),
        mod_display_poll_fx_epoch: usize::MAX,
        mod_display_poll_track: None,
        cached_cpu_load_bits: 0.0f32.to_bits(),
        last_meter_poll_at: Instant::now(),
        last_cpu_ui_poll_at: Instant::now(),
        last_voice_count_log_at: Instant::now(),
    }
}

impl Harness {
    fn new() -> Self {
        Self::with_root(UiRoot::Bare)
    }

    fn with_root(root: UiRoot) -> Self {
        let eng = engine::init_headless_engine(44_100, 2).expect("headless engine");
        let graph = GraphGuard(eng.lg_ptr);
        let state = eng.state.clone();
        let keyboard_tx = eng.keyboard_tx.clone();
        let master_recorder = eng.master_recorder.clone();
        let app = app::App::new(
            state.clone(),
            eng.lg_ptr,
            eng.sample_rate,
            eng.buses,
            eng.master_recorder,
            eng.keyboard_tx,
        );
        let sample_db =
            sequencer::sample_db::SampleDb::open_in_memory().expect("in-memory sample db");
        let shared = SharedHandles {
            state: state.clone(),
            lg_raw: app.graph.lg.0,
            current_track: Arc::new(AtomicUsize::new(0)),
            selected_tracks: Arc::new(Mutex::new(HashSet::new())),
            selected_steps: Arc::new(Mutex::new(HashSet::new())),
            selected_neural_neurons: Arc::new(Mutex::new(BTreeSet::new())),
            piano_roll_selection: Arc::new(Mutex::new(HashSet::new())),
            piano_roll_move_state: Arc::new(Mutex::new(None)),
            piano_roll_focus: new_shared_piano_roll_focus(),
            step_clipboard: Arc::new(Mutex::new(None)),
            ui_epoch: Arc::new(AtomicUsize::new(0)),
            fx_epoch: Arc::new(AtomicUsize::new(0)),
            fx_value_epoch: Arc::new(AtomicUsize::new(0)),
            ui_invalidations: Arc::new(UiInvalidationQueue::new()),
            active_delete_target: Arc::new(Mutex::new(None)),
            active_delete_target_version: Arc::new(AtomicUsize::new(0)),
            auto_follow_override_until: Arc::new(Mutex::new(None)),
            track_pan_ids: Arc::new(Mutex::new(Vec::new())),
            track_collapsed: Arc::new(Mutex::new(app.track_collapsed.clone())),
            bus_state: Arc::new(Mutex::new(app.buses.clone())),
            bus_node_ids: Arc::new(Mutex::new(app.graph.bus_node_ids.clone())),
            track_groups: Arc::new(Mutex::new(app.groups.clone())),
            record_armed: Arc::new(Mutex::new(Vec::new())),
            armed_rack: Arc::new(Mutex::new(None)),
            recording: Arc::new(AtomicBool::new(false)),
            master_recording: Arc::new(AtomicBool::new(false)),
            held_notes: Arc::new(Mutex::new(Vec::new())),
            roll_record: Arc::new(Mutex::new(RollRecordBuffer::default())),
            step_print: Arc::new(Mutex::new(StepPrintState::default())),
            keyboard_octave: Arc::new(AtomicI32::new(0)),
            sample_browser: Rc::new(RefCell::new(DebouncedSampleBrowser::new(
                sample_db,
                Duration::from_millis(100),
            ))),
            keyboard_tx,
            accumulator_names: Arc::new(Mutex::new(Vec::new())),
            piano_roll_clipboard: new_piano_roll_clipboard(),
            arrangement_clipboard: app::song_region::new_arrangement_clipboard(),
        };
        let track_names = app.tracks.clone();
        let RuntimeInit { runtime, .. } = init_runtime(
            &app,
            state.clone(),
            &track_names,
            shared.track_pan_ids.clone(),
            shared.track_collapsed.clone(),
            shared.bus_state.clone(),
            shared.bus_node_ids.clone(),
            shared.current_track.clone(),
            shared.selected_tracks.clone(),
            shared.track_groups.clone(),
            shared.selected_steps.clone(),
            shared.piano_roll_selection.clone(),
            shared.piano_roll_move_state.clone(),
            shared.piano_roll_focus.clone(),
            shared.recording.clone(),
            shared.master_recording.clone(),
            master_recorder,
            shared.record_armed.clone(),
            shared.armed_rack.clone(),
            shared.ui_epoch.clone(),
            shared.fx_epoch.clone(),
            shared.ui_invalidations.clone(),
            shared.selected_neural_neurons.clone(),
            shared.active_delete_target.clone(),
            shared.active_delete_target_version.clone(),
            shared.auto_follow_override_until.clone(),
            app.graph.lg.0,
        );
        // Loading a root runs the startup schema check (a panic in debug).
        let editor = create_editor_with_root(runtime, &app, None, root).expect("root loads");
        let mut harness = Self {
            app,
            editor,
            shared,
            sessions: EditSessionState::default(),
            frame: FrameDiffState::default(),
            gesture: GestureState::default(),
            meters: meter_cache(),
            track_names,
            _graph: graph,
        };
        harness.command("new-project", Value::Nil);
        assert_eq!(harness.app.tracks.len(), 2, "a new project has two tracks");
        harness
    }

    /// Apply one host command as the event loop does: the project macro
    /// commands first, then the custom dispatch.
    fn command(&mut self, name: &str, payload: Value) {
        use crate::host_commands::{handle_macro_host_command, MacroHostCommandOutcome};
        let current = self.shared.current_track.load(Ordering::Relaxed);
        let state = self.shared.state.clone();
        match handle_macro_host_command(name, &payload, &mut self.app, &state, current) {
            MacroHostCommandOutcome::Applied => {
                self.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
                return;
            }
            MacroHostCommandOutcome::Ignored => return,
            MacroHostCommandOutcome::NotMacro => {}
        }
        let mut ctx = LoopCtx {
            sessions: &mut self.sessions,
            meters: &mut self.meters,
            frame: &mut self.frame,
            gesture: &mut self.gesture,
            track_names: &mut self.track_names,
            shared: &self.shared,
        };
        dispatch_custom_host_command(name, payload, &mut self.app, &mut self.editor, &mut ctx);
    }

    /// Apply what Lisp queued, as the event loop does (a script note
    /// drag's frame once, after the batch).
    fn drain(&mut self) {
        for command in self.editor.drain_host_commands() {
            if let HostCommand::Custom { name, payload } = command {
                self.command(&name, payload);
            }
        }
        let mut ctx = LoopCtx {
            sessions: &mut self.sessions,
            meters: &mut self.meters,
            frame: &mut self.frame,
            gesture: &mut self.gesture,
            track_names: &mut self.track_names,
            shared: &self.shared,
        };
        crate::host_commands::notes::flush_note_drag(&mut self.app, &mut self.editor, &mut ctx);
    }

    /// One host-kinds sync; returns whether anything changed.
    fn sync(&mut self) -> bool {
        let meters = KindsMeters {
            tracks: &self.meters.cached_track_peak_levels,
            buses: &self.meters.cached_bus_peak_levels,
            master: (
                self.meters.cached_peak_l_level,
                self.meters.cached_peak_r_level,
            ),
            cpu_load: f32::from_bits(self.meters.cached_cpu_load_bits) as f64,
            mod_ports: &self.meters.cached_mod_port_levels,
            overloaded: self.frame.cpu_overload.displayed(),
            pad_triggers: &self.frame.rack_pad_triggers,
            mod_display: &self.meters.cached_mod_display_values,
            modulator_phases: &self.meters.cached_modulator_phases,
            modulator_levels: &self.meters.cached_modulator_levels,
        };
        self.frame
            .host_kinds
            .sync(&self.app, self.editor.runtime_mut(), &self.shared, &meters)
    }

    /// Evaluate `code` as a view would: with eseq.kinds referred (an
    /// import's `:refer` covers the source it heads).
    fn eval(&mut self, code: &str) -> Value {
        self.eval_with(REFER, code)
    }

    /// Evaluate `code` after the `refer` prelude; panics on an error.
    pub(super) fn eval_with(&mut self, refer: &str, code: &str) -> Value {
        let source = format!("{refer}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    fn rt(&self) -> &Runtime {
        self.editor.runtime()
    }

    /// The instance of singleton kind `kind`.
    fn singleton(&self, kind: &str) -> InstanceId {
        self.rt().singleton_instance(kind).expect(kind)
    }

    /// The instances of a list value.
    fn instances(&self, value: Value) -> Vec<InstanceId> {
        match value {
            Value::List(items) => items
                .iter()
                .map(|item| match &*item.borrow() {
                    Value::Instance(id) => *id,
                    other => panic!("not an instance: {other:?}"),
                })
                .collect(),
            other => panic!("not a list: {other:?}"),
        }
    }

    fn track_id(&self, index: u64) -> InstanceId {
        self.rt()
            .keyed_instance(TRACK, &[index])
            .expect("track instance")
    }

    fn computed(&self, key: FieldKey) -> u64 {
        self.frame
            .host_kinds
            .shared
            .borrow()
            .computed
            .get(&key)
            .copied()
            .unwrap_or(0)
    }

    fn model_syncs(&self) -> u64 {
        self.frame.host_kinds.shared.borrow().model_syncs
    }

    fn steps_of(&self, track: InstanceId) -> Vec<InstanceId> {
        self.rt()
            .keyed_children(track)
            .into_iter()
            .filter(|id| self.rt().instance_kind(*id) == Some(STEP))
            .collect()
    }

    /// Treat every effect buffer as on screen (none sits in a tile here),
    /// so dirty views re-render instead of waiting to be shown.
    fn show_all(&mut self) {
        self.editor.refresh_runtime_side_effects();
        self.editor
            .runtime_mut()
            .set_hidden_effect_buffer_names(HashSet::new());
        self.editor.runtime_mut().run_reactive_cycle();
    }

    /// Fire widget `widget`'s `handler` with `arg`, then apply what it
    /// queued, sync and render, as one event-loop turn (`fire` in
    /// patching_view returns the queued commands instead).
    fn fire_and_show(&mut self, widget: &HashMap<String, Value>, handler: &str, arg: Value) {
        let callback = widget
            .get(handler)
            .unwrap_or_else(|| panic!("no {handler} on {widget:?}"))
            .clone();
        self.editor
            .runtime_mut()
            .invoke(callback, vec![arg])
            .unwrap_or_else(|error| panic!("{handler}: {error:?}"));
        self.drain();
        self.sync();
        self.show_all();
    }

    /// The custom host commands Lisp queued, in order, taken off the queue.
    fn custom_commands(&mut self) -> Vec<(String, Value)> {
        self.editor
            .drain_host_commands()
            .into_iter()
            .filter_map(|command| match command {
                HostCommand::Custom { name, payload } => Some((name, payload)),
                _ => None,
            })
            .collect()
    }

    /// The payload of the custom host command `name` Lisp queued last.
    fn last_custom(&mut self, name: &str) -> Value {
        self.custom_commands()
            .into_iter()
            .filter(|(n, _)| n == name)
            .map(|(_, payload)| payload)
            .last()
            .unwrap_or_else(|| panic!("no {name} queued"))
    }

    /// Start or stop the transport (the host kinds read it on the next
    /// sync).
    fn set_playing(&self, playing: bool) {
        self.shared
            .state
            .transport
            .playing
            .store(playing, Ordering::Relaxed);
    }

    /// Field `name` of the track at position `index`.
    fn track_field(&mut self, index: usize, name: &str) -> Value {
        self.eval(&format!("(let ((t (track {index}))) t.{name})"))
    }

    /// A Lisp-held `#'` slot's current value.
    fn slot(&mut self, global: &str) -> f64 {
        match self.eval(&format!("(do {global})")) {
            Value::ReactiveRef { slot, .. } => read_float_slot(&slot),
            other => panic!("{global} is not a binding: {other:?}"),
        }
    }
}

/// A process chain of one `rand` slot (`enabled` or not) whose `out` port
/// writes `target` (shared with the state-value tests).
pub(crate) fn one_slot_chain(
    target: sequencer::process::ParamTarget,
    enabled: bool,
) -> sequencer::process::TrackProcessChain {
    sequencer::process::TrackProcessChain {
        slots: vec![sequencer::process::TrackProcessSlot {
            instance_id: sequencer::process::ProcessInstanceId(1),
            instance_name: None,
            class_name: "rand".to_string(),
            enabled,
            project_layer: false,
            inlets: Default::default(),
            lanes: Default::default(),
            fanout: Default::default(),
            unbound_ports: Default::default(),
            expr_source: None,
            bindings: std::collections::BTreeMap::from([("out".to_string(), Some(target))]),
        }],
    }
}

fn s(text: &str) -> Value {
    Value::String(text.to_string())
}

/// A map's field (nil when absent or not a map).
pub(crate) fn get(value: &Value, key: &str) -> Value {
    match value {
        Value::Map(map) => map
            .get(key)
            .map_or(Value::Nil, |cell| cell.borrow().clone()),
        _ => Value::Nil,
    }
}

/// A list's items (none when not a list).
pub(crate) fn items(value: &Value) -> Vec<Value> {
    match value {
        Value::List(items) => items.iter().map(|item| item.borrow().clone()).collect(),
        _ => Vec::new(),
    }
}

impl Harness {
    /// Undo the last entry, with the resync the undo command's epoch bump
    /// brings.
    pub(super) fn undo(&mut self) {
        app::edit::undo(&mut self.app);
        self.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
        self.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
    }

    /// `code` (evaluated with `refer` heading it) reports an error
    /// containing `expected`; with `records_nothing`, it also adds no
    /// history entry.
    pub(super) fn rejects_in(
        &mut self,
        refer: &str,
        code: &str,
        expected: &str,
        records_nothing: bool,
    ) {
        self.editor.minibuffer = None;
        let before = self.app.history.undo_len();
        let source = format!("{refer}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"));
        self.drain();
        assert!(self.error().contains(expected), "{code}: {}", self.error());
        if records_nothing {
            assert_eq!(
                self.app.history.undo_len(),
                before,
                "{code} changed nothing"
            );
        }
    }
}

/// Every kind the stage-7 tests read.
const REFER_ALL: &str = "(import eseq.kinds :refer (track tracks buses groups transport \
                         selection master engine device-param lock-param! unlock-param! \
                         lock-strip! unlock-strip! \
                         lock-none lock-seq lock-variant))";

fn num(value: Value) -> f64 {
    match value {
        Value::Number(n) => n,
        other => panic!("not a number: {other:?}"),
    }
}

impl Harness {
    fn eval_all(&mut self, code: &str) -> Value {
        let source = format!("{REFER_ALL}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    /// Mirror the `App`'s buses and groups into the shared handles the
    /// natives read, as the event loop does after an edit.
    fn share_buses_and_groups(&mut self) {
        *self.shared.bus_state.lock().unwrap() = self.app.buses.clone();
        *self.shared.track_groups.lock().unwrap() = self.app.groups.clone();
    }

    fn add_bus(&mut self, name: &str) -> sequencer::sequencer::BusId {
        let id = self
            .app
            .add_bus_recorded(name.to_string())
            .expect("add bus");
        self.share_buses_and_groups();
        id
    }

    fn instance_of(&self, kind: &str, key: &[u64]) -> InstanceId {
        self.rt()
            .keyed_instance(kind, key)
            .unwrap_or_else(|| panic!("{kind} {key:?}"))
    }
}

mod arrangement;
mod arrangement_view;
mod browser;
mod browser_view;
mod devices;
mod focus_steps;
mod generator;
mod graph;
mod graph_demos_view;
mod lanes;
mod mixer;
mod mixer_view;
mod neural;
mod neural_panel;
mod packages_view;
mod panel;
mod panel_extras;
mod panels_view;
mod patching_view;
mod params;
mod pending;
mod piano_roll;
mod piano_roll_view;
mod racks;
mod scenes;
mod schema;
mod sequencer_editor;
mod sequencer_view;
mod settings;
mod steps;
mod table_editor;
mod tracks;
mod transport;
mod views;
