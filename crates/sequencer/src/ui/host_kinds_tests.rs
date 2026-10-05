//! Host kinds against the real `eseq.kinds`, the real natives and a headless
//! project (docs/kind-bindings-spec.md §13 stage 4).

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
        last_neural_visualization_poll_at: Instant::now(),
        visualization_liveness: VisualizationLiveness::default(),
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
            expanded_step_projection: Arc::new(ExpandedStepProjectionRegistry::new()),
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
            shared.expanded_step_projection.clone(),
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

    fn command(&mut self, name: &str, payload: Value) {
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

    /// Apply what Lisp queued, as the event loop does.
    fn drain(&mut self) {
        for command in self.editor.drain_host_commands() {
            if let HostCommand::Custom { name, payload } = command {
                self.command(&name, payload);
            }
        }
    }

    /// One host-kinds sync; returns whether anything changed.
    fn sync(&mut self) -> bool {
        self.frame.host_kinds.sync(
            &self.app,
            self.editor.runtime_mut(),
            &self.shared,
            &self.meters.cached_track_peak_levels,
        )
    }

    /// Evaluate `code` as a view would: with eseq.kinds referred (an
    /// import's `:refer` covers the source it heads).
    fn eval(&mut self, code: &str) -> Value {
        let source = format!("{REFER}\n{code}");
        self.editor
            .runtime_mut()
            .eval_str(&source)
            .unwrap_or_else(|error| panic!("{code}: {error:?}"))
            .unwrap_or(Value::Nil)
    }

    fn rt(&self) -> &Runtime {
        self.editor.runtime()
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

    /// A Lisp-held `#'` slot's current value.
    fn slot(&mut self, global: &str) -> f64 {
        match self.eval(&format!("(do {global})")) {
            Value::ReactiveRef { slot, .. } => read_float_slot(&slot),
            other => panic!("{global} is not a binding: {other:?}"),
        }
    }
}

fn s(text: &str) -> Value {
    Value::String(text.to_string())
}

#[test]
fn host_kinds_schema_matches_eseq_kinds() {
    let mut h = Harness::new();
    assert_eq!(check_schema(h.rt()), Ok(()));
    for name in host_kind_names() {
        let error = h
            .editor
            .runtime_mut()
            .eval_str(&format!("(def-kind {name} :key () :state ((open false)))"))
            .expect_err("reserved");
        assert!(
            format!("{error:?}").contains("reserved"),
            "{name}: {error:?}"
        );
    }
    // A drifted declaration is caught, naming both directions.
    h.eval(
        "(module eseq.kinds)
         (def-kind scene :key (index)
           :host ((index :int) (number :int) (name :string) (active :bool)
                  (queued :bool) (bank bank) (color :rgb)))
         (def-kind bank :key (index)
           :host ((index :int) (label :string) (scenes (list-of scene))))",
    );
    let errors = check_schema(h.rt()).expect_err("drift").join("\n");
    assert!(
        errors.contains("'color' is declared but the host never publishes it"),
        "{errors}"
    );
    assert!(
        errors.contains("publishes 'playing' (:bool), which is not a :host field"),
        "{errors}"
    );
}

#[test]
fn the_full_daw_root_loads_eseq_kinds_and_publishes_it() {
    let mut h = Harness::with_root(UiRoot::Distro);
    assert_eq!(check_schema(h.rt()), Ok(()));
    assert!(h.sync());
    assert_eq!(h.eval("(len (tracks))"), Value::Number(2.0));
    // The root imports the module without referring its names.
    let bare = h.editor.runtime_mut().eval_str("(tracks)");
    assert!(bare.is_err(), "kind names are not bare globals: {bare:?}");
}

#[test]
fn track_scene_bank_transport_selection_and_device_fields_read_through_kinds() {
    let mut h = Harness::new();
    assert!(h.sync(), "the first sync registers and pushes");
    assert_eq!(h.eval("(len (tracks))"), Value::Number(2.0));
    let t0 = h.track_id(0);
    h.eval("(def t0 (track 0)) (def t1 (track 1))");
    assert_eq!(h.eval("(first (tracks))"), Value::Instance(t0));
    assert_eq!(h.eval("t0"), Value::Instance(t0));
    assert_eq!(h.eval("t0.index"), Value::Number(0.0));
    assert_eq!(h.eval("t1.index"), Value::Number(1.0));
    assert_eq!(h.eval("t1.name"), s(&h.app.tracks[1]));
    assert_eq!(h.eval("(first t0.color)"), Value::Symbol("rgb".into()));
    let volume = h.shared.state.pattern.track_params[0].get_volume() as f64;
    assert_eq!(h.eval("t0.volume"), Value::Number(volume));
    assert_eq!(h.eval("t0.muted"), Value::Bool(false));
    assert_eq!(h.eval("t0.armed"), Value::Bool(false));
    assert_eq!(h.eval("t0.selected"), Value::Bool(true));
    assert_eq!(h.eval("t1.selected"), Value::Bool(false));
    assert_eq!(h.eval("t0.preset"), s(""));
    let steps = h.shared.state.pattern.track_params[0].get_num_steps();
    assert_eq!(h.eval("t0.num-steps"), Value::Number(steps as f64));
    assert_eq!(h.eval("(len t0.steps)"), Value::Number(steps as f64));
    h.eval("(def s3 (nth t0.steps 3))");
    assert_eq!(h.eval("s3.index"), Value::Number(3.0));
    assert_eq!(h.eval("s3.track"), Value::Instance(t0));
    assert_eq!(h.eval("s3.active"), Value::Bool(false));
    assert_eq!(h.eval("s3.playing"), Value::Bool(false));
    assert_eq!(h.eval("s3.selected"), Value::Bool(false));
    assert_eq!(h.eval("(= s3 (nth t0.steps 3))"), Value::Bool(true));
    // Devices: the instrument at slot -1, then the effects.
    let chain = track_device_chain(&h.app, &h.shared.state, 0);
    assert_eq!(
        h.eval("(len t0.devices)"),
        Value::Number(chain.len() as f64)
    );
    if let Some(first) = chain.first() {
        h.eval("(def d0 (first t0.devices))");
        assert_eq!(h.eval("d0.name"), s(&first.name));
        assert_eq!(h.eval("d0.slot"), Value::Number(first.slot as f64));
        assert_eq!(h.eval("d0.enabled"), Value::Bool(first.enabled));
        assert_eq!(h.eval("d0.track"), Value::Instance(t0));
    }
    // Transport, scenes, banks, selection.
    assert_eq!(h.eval("transport.playing"), Value::Bool(false));
    assert_eq!(h.eval("transport.recording"), Value::Bool(false));
    assert_eq!(
        h.eval("transport.launch-quantize"),
        h.eval("SEQ.scene-launch-quantize")
    );
    let scene_count = h.shared.state.scene_count();
    assert!(scene_count >= 1);
    assert_eq!(h.eval("(len (scenes))"), Value::Number(scene_count as f64));
    assert_eq!(h.eval("transport.scene"), h.eval("(first (scenes))"));
    assert_eq!(h.eval("transport.queued"), Value::Nil);
    assert_eq!(h.eval("transport.scene.index"), Value::Number(0.0));
    assert_eq!(h.eval("transport.scene.number"), Value::Number(1.0));
    assert_eq!(h.eval("transport.scene.active"), Value::Bool(true));
    assert_eq!(h.eval("transport.scene.queued"), Value::Bool(false));
    assert_eq!(h.eval("transport.scene.name"), {
        let name = h
            .shared
            .state
            .with_project_scenes(|scenes| scenes.scenes[0].name.clone());
        s(&name)
    });
    if h.eval("(len (banks))") != Value::Number(0.0) {
        h.eval("(def b0 (first (banks)))");
        assert_eq!(h.eval("transport.scene.bank"), h.eval("b0"));
        assert_eq!(h.eval("transport.scene.bank.label"), s("A"));
        assert_eq!(h.eval("b0.index"), Value::Number(0.0));
        assert_eq!(h.eval("b0.playing"), Value::Bool(true));
        assert_eq!(h.eval("(first b0.scenes)"), h.eval("(first (scenes))"));
    }
    assert_eq!(h.eval("selection.track"), Value::Instance(t0));
    // Nothing changed: a second sync pushes nothing.
    assert!(!h.sync(), "pushes only changed values");
}

#[test]
fn bound_volume_and_step_playing_follow_changes_without_re_rendering() {
    let mut h = Harness::new();
    h.sync();
    let renders = Rc::new(std::cell::Cell::new(0u32));
    let counter = renders.clone();
    h.editor
        .runtime_mut()
        .register_native("count-render", move |_args, _ctx| {
            counter.set(counter.get() + 1);
            Ok(Value::Nil)
        });
    h.eval(
        r#"(def t0 (track 0))
           (def s3 (nth t0.steps 3))
           (effect-buffer "*kinds*"
             (do (count-render)
                 (label "x" :active #'t0.volume)))
           (def vol #'t0.volume)
           (def playing3 #'s3.playing)"#,
    );
    h.show_all();
    let rendered = renders.get();
    assert!(rendered >= 1);
    let t0 = h.track_id(0);
    assert!(h.rt().host_field_observed(t0, "volume"));
    // Volume: the model changes, the slot follows on the next sync.
    h.shared.state.pattern.track_params[0].set_volume(0.3);
    assert!(h.sync());
    assert!((h.slot("vol") - 0.3).abs() < 1e-6);
    // Step playing: the playhead lands on step 3, then moves on.
    h.shared.state.transport.track_playheads[0].store(3, Ordering::Relaxed);
    h.shared
        .state
        .transport
        .playing
        .store(true, Ordering::Relaxed);
    assert!(h.sync());
    assert_eq!(h.slot("playing3"), 1.0);
    h.shared.state.transport.track_playheads[0].store(4, Ordering::Relaxed);
    assert!(h.sync());
    assert_eq!(h.slot("playing3"), 0.0);
    h.editor.runtime_mut().run_reactive_cycle();
    assert_eq!(
        renders.get(),
        rendered,
        "bindings repaint; the view never re-renders"
    );
    // A by-value reader does re-render.
    h.eval(r#"(effect-buffer "*name*" (do (count-render) (label t0.name)))"#);
    h.show_all();
    let rendered = renders.get();
    let payload = h.eval("(dict :track 0 :name \"Kicks\")");
    h.command("rename-track", payload);
    assert!(h.sync());
    h.editor.runtime_mut().run_reactive_cycle();
    assert_eq!(h.eval("t0.name"), s("Kicks"));
    assert!(
        renders.get() > rendered,
        "a by-value read re-renders on change"
    );
}

#[test]
fn set_paths_change_the_model() {
    let mut h = Harness::new();
    h.sync();
    h.eval("(def t0 (track 0)) (def s3 (nth t0.steps 3))");
    // Volume.
    h.eval("(set! t0.volume 0.25)");
    h.drain();
    assert!((h.shared.state.pattern.track_params[0].get_volume() - 0.25).abs() < 1e-6);
    h.sync();
    assert_eq!(h.eval("t0.volume"), Value::Number(0.25));
    // Mute.
    h.eval("(toggle! t0.muted)");
    h.drain();
    assert!(h.shared.state.pattern.track_params[0].is_muted());
    assert_eq!(h.eval("t0.muted"), Value::Bool(true));
    h.eval("(set! t0.muted true)");
    h.drain();
    assert!(
        h.shared.state.pattern.track_params[0].is_muted(),
        "already muted: no toggle"
    );
    // Record arm.
    h.eval("(toggle! t0.armed)");
    h.drain();
    assert_eq!(h.shared.record_armed.lock().unwrap().first(), Some(&true));
    assert_eq!(h.eval("t0.armed"), Value::Bool(true));
    // Step active.
    h.eval("(toggle! s3.active)");
    h.drain();
    assert!(h.shared.state.pattern.patterns[0].is_active(3));
    assert_eq!(h.eval("s3.active"), Value::Bool(true));
    // Selection.
    h.eval("(set! selection.track (track 1))");
    assert_eq!(h.shared.current_track.load(Ordering::Relaxed), 1);
    assert_eq!(
        h.eval("(let ((t (track 1))) t.selected)"),
        Value::Bool(true)
    );
    assert_eq!(h.eval("selection.track"), h.eval("(track 1)"));
    // Transport.
    h.eval("(set! transport.playing true)");
    h.drain();
    assert!(h.shared.state.transport.playing.load(Ordering::Relaxed));
    assert_eq!(h.eval("transport.playing"), Value::Bool(true));
    h.eval("(toggle! transport.playing)");
    h.drain();
    assert!(!h.shared.state.transport.playing.load(Ordering::Relaxed));
    // Read-only fields stay read-only.
    let error = h
        .editor
        .runtime_mut()
        .eval_str(&format!("{REFER} (set! t0.peak 1)"))
        .expect_err("read-only");
    assert!(
        format!("{error:?}").contains("track.peak is read-only"),
        "{error:?}"
    );
}

#[test]
fn track_add_remove_and_reorder_keep_instance_identity() {
    let mut h = Harness::new();
    h.sync();
    let (a, b) = (h.track_id(0), h.track_id(1));
    h.eval("(def held-b (track 1))");
    h.eval("(len (let ((t (track 1))) t.steps))");
    let b_steps = h.steps_of(b);
    assert!(!b_steps.is_empty());
    // Add a track and move it to the front: the old ones re-key.
    h.app.graph_controller().add_empty_track().expect("add");
    h.app
        .graph_controller()
        .move_appended_track_to(0)
        .expect("move");
    assert_eq!(h.app.tracks.len(), 3);
    h.sync();
    let c = h.track_id(0);
    assert!(c != a && c != b);
    assert_eq!(h.track_id(1), a);
    assert_eq!(h.track_id(2), b);
    assert_eq!(h.eval("held-b"), Value::Instance(b));
    assert_eq!(h.eval("held-b.index"), Value::Number(2.0));
    assert_eq!(h.eval("held-b.key"), h.eval("(list 2)"));
    assert_eq!(h.steps_of(b), b_steps, "steps stay with their track");
    assert_eq!(h.eval("(len (tracks))"), Value::Number(3.0));
    // Remove the first original track: its instance goes stale with its
    // steps; the others keep their ids.
    h.app.graph_controller().delete_track(1).expect("delete");
    h.sync();
    assert!(!h.rt().instance_is_live(a));
    assert_eq!(h.track_id(0), c);
    assert_eq!(h.track_id(1), b);
    assert_eq!(h.eval("held-b.index"), Value::Number(1.0));
    assert_eq!(h.eval("(len (tracks))"), Value::Number(2.0));
    // A rebuilt track shell (new graph nodes, same registry id) keeps the
    // instance: identity is the registry's TrackId, not a graph node.
    let rebuilt = h.app.track_registry.id_at(1).expect("track id");
    h.app.graph.track_node_ids[1].pan_id += 1000;
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.app.track_registry.id_at(1), Some(rebuilt));
    assert_eq!(h.track_id(1), b);
    // A project load replaces every track, although the new project's
    // registry hands out the same TrackIds again.
    let old_ids = h.app.track_registry.ids().to_vec();
    h.command("new-project", Value::Nil);
    assert!(
        h.app
            .track_registry
            .ids()
            .iter()
            .any(|id| old_ids.contains(id)),
        "ids restart per project: {old_ids:?} then {:?}",
        h.app.track_registry.ids()
    );
    h.sync();
    assert!(!h.rt().instance_is_live(b) && !h.rt().instance_is_live(c));
    assert!(h.steps_of(b).is_empty());
    assert_eq!(h.eval("held-b.name"), s(""), "a stale track reads defaults");
}

#[test]
fn steps_register_only_when_read_and_drop_when_the_track_shrinks() {
    let mut h = Harness::new();
    h.sync();
    let t0 = h.track_id(0);
    assert!(
        h.steps_of(t0).is_empty(),
        "no step instances until t.steps is read"
    );
    h.sync();
    assert!(h.steps_of(t0).is_empty());
    h.shared.state.pattern.track_params[0].set_num_steps(16);
    h.eval("(def t0 (track 0)) (len t0.steps)");
    assert_eq!(h.steps_of(t0).len(), 16);
    assert!(
        h.steps_of(h.track_id(1)).is_empty(),
        "only the track that was read"
    );
    let step12 = h.rt().keyed_instance(STEP, &[t0, 12]).expect("step 12");
    // Shrinking drops the steps past the end, even with no reader.
    h.shared.state.pattern.track_params[0].set_num_steps(8);
    h.sync();
    assert_eq!(h.steps_of(t0).len(), 8);
    assert!(!h.rt().instance_is_live(step12));
    // A reader of t.steps sees the list follow the length.
    h.eval(r#"(effect-buffer "*steps*" (label (str (len t0.steps))))"#);
    h.editor.runtime_mut().run_reactive_cycle();
    assert!(h.rt().host_field_observed(t0, "steps"));
    h.shared.state.pattern.track_params[0].set_num_steps(12);
    assert!(h.sync());
    assert_eq!(h.steps_of(t0).len(), 12);
    assert_eq!(h.eval("(len t0.steps)"), Value::Number(12.0));
}

#[test]
fn unobserved_live_fields_are_never_computed() {
    let mut h = Harness::new();
    h.sync();
    h.eval("(def t0 (track 0)) (len t0.steps)");
    // Playback with a moving playhead and live meters, nothing observing.
    h.shared
        .state
        .transport
        .playing
        .store(true, Ordering::Relaxed);
    for step in 0..8 {
        h.shared.state.transport.track_playheads[0].store(step, Ordering::Relaxed);
        h.sync();
    }
    h.meters.cached_track_peak_levels = vec![0.5, 0.25];
    h.sync();
    for key in [
        f::STEP_PLAYING,
        f::STEP_ACTIVE,
        f::STEP_SELECTED,
        f::TRACK_PEAK,
        f::TRACK_VOLUME,
    ] {
        assert_eq!(h.computed(key), 0, "{key:?} computed while unobserved");
    }
    // Observing one step's playhead computes that step only, on change.
    h.eval("(def s5 (nth t0.steps 5)) (def p5 #'s5.playing)");
    let cold = h.computed(f::STEP_PLAYING);
    assert_eq!(cold, 1, "the binding's seed asks the reader once");
    h.shared.state.transport.track_playheads[0].store(5, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.slot("p5"), 1.0);
    assert_eq!(h.computed(f::STEP_PLAYING), cold + 1);
    h.sync();
    assert_eq!(h.computed(f::STEP_PLAYING), cold + 1, "no change, no work");
    // A meter is read only while observed, from the meter cache.
    h.eval("(def peak0 #'t0.peak)");
    assert_eq!(h.slot("peak0"), 0.5);
    assert!(
        h.frame.host_kinds.wants_peaks() || {
            h.sync();
            h.frame.host_kinds.wants_peaks()
        }
    );
    let before = h.computed(f::TRACK_PEAK);
    h.meters.cached_track_peak_levels = vec![0.75, 0.25];
    h.sync();
    h.sync();
    assert_eq!(h.computed(f::TRACK_PEAK), before + 2);
    assert_eq!(h.slot("peak0"), 0.75);
    h.eval("(set! peak0 nil)");
    let before = h.computed(f::TRACK_PEAK);
    h.sync();
    assert_eq!(h.computed(f::TRACK_PEAK), before);
    assert!(!h.frame.host_kinds.wants_peaks());
}

#[test]
fn reconcile_keeps_results_aligned_when_a_registration_fails() {
    let mut h = Harness::new();
    h.sync();
    let t0 = h.track_id(0);
    let sources = LiveSources::from_shared(&h.shared);
    let shared = RefCell::new(KindsShared::default());
    let mut pusher = Pusher {
        rt: h.editor.runtime_mut(),
        sources: &sources,
        shared: &shared,
        changed: false,
    };
    // 77 is a live instance already at index 0; 88 and 99 cannot register
    // (no such kind): their slots stay None instead of shifting.
    let mut known = HashMap::from([(77, t0)]);
    let ids = reconcile(&mut pusher, "eseq.kinds:missing", &mut known, &[77, 88, 99]);
    assert_eq!(ids, vec![Some(t0), None, None]);
    assert_eq!(known.len(), 1);
}

#[test]
fn repeated_sets_in_one_frame_never_double_toggle() {
    let mut h = Harness::new();
    h.sync();
    h.eval("(def t0 (track 0)) (def s3 (nth t0.steps 3))");
    // Bound fields are observed: their cells only move on the next sync,
    // so a value-comparing toggle wrapper would flip twice.
    h.eval("(def m #'t0.muted) (def a #'s3.active) (def p #'transport.playing)");
    h.eval(
        "(do (set! t0.muted true) (set! t0.muted true)
             (toggle! s3.active) (toggle! s3.active)
             (set! transport.playing true) (set! transport.playing true)
             (set! t0.armed true) (toggle! t0.armed)
             (set! transport.recording true) (set! transport.recording true))",
    );
    h.drain();
    assert!(h.shared.state.pattern.track_params[0].is_muted());
    assert!(h.shared.state.pattern.patterns[0].is_active(3));
    assert!(h.shared.state.transport.playing.load(Ordering::Relaxed));
    assert!(h.shared.recording.load(Ordering::Relaxed));
    // `toggle!` reads the (stale or fresh) arm and sets its negation; the
    // arm is applied at once, so the second call sees it.
    assert_eq!(h.shared.record_armed.lock().unwrap().first(), Some(&false));
    h.sync();
    assert_eq!(h.slot("m"), 1.0);
    assert_eq!(h.slot("a"), 1.0);
    // Setting what already holds is a no-op.
    h.eval("(set! t0.muted true) (set! s3.active true) (set! transport.playing true)");
    h.drain();
    assert!(h.shared.state.pattern.track_params[0].is_muted());
    assert!(h.shared.state.pattern.patterns[0].is_active(3));
    assert!(h.shared.state.transport.playing.load(Ordering::Relaxed));
    h.eval("(set! t0.muted false) (set! transport.playing false)");
    h.drain();
    assert!(!h.shared.state.pattern.track_params[0].is_muted());
    assert!(!h.shared.state.transport.playing.load(Ordering::Relaxed));
}

#[test]
fn a_hot_reloaded_schema_drift_is_skipped_without_panicking() {
    let mut h = Harness::new();
    h.sync();
    h.eval("(def b0 (first (banks)))");
    // Hot reload a drifted eseq.kinds: bank loses `playing`, scene gains a
    // field the host never publishes and retypes `name`.
    h.eval(
        "(module eseq.kinds)
         (def-kind scene :key (index)
           :host ((index :int) (number :int) (name :int) (active :bool)
                  (queued :bool) (bank bank) (color :rgb)))
         (def-kind bank :key (index)
           :host ((index :int) (label :string) (scenes (list-of scene))))",
    );
    assert!(check_schema(h.rt()).is_err());
    // Pushes of the mismatched fields are skipped; the rest still flow.
    h.shared.state.pattern.track_params[0].set_volume(0.4);
    h.eval("(def t0 (track 0)) (def vol #'t0.volume)");
    h.sync();
    h.sync();
    assert!((h.slot("vol") - 0.4).abs() < 1e-6);
    let skip = h.frame.host_kinds.shared.borrow().skip.clone();
    assert!(
        skip.contains(&f::BANK_PLAYING) && skip.contains(&f::SCENE_NAME),
        "{skip:?}"
    );
    assert!(!skip.contains(&f::SCENE_INDEX));
    assert_eq!(
        h.eval("(let ((s (first (scenes)))) s.index)"),
        Value::Number(0.0)
    );
}

#[test]
fn step_selected_follows_a_rack_wide_selection() {
    let mut h = Harness::new();
    h.sync();
    h.shared.state.pattern.track_params[1].set_num_steps(4);
    h.eval("(def t1 (track 1)) (def s2 (nth t1.steps 2)) (def sel2 #'s2.selected)");
    h.shared.selected_steps.lock().unwrap().extend([2, 6]);
    h.sync();
    assert_eq!(h.slot("sel2"), 0.0, "track 1 is not the current track");
    // Rack-wide Cmd+A: the selection covers every listed track.
    *h.shared.active_delete_target.lock().unwrap() =
        Some(ActiveDeleteTarget::TrackSteps { tracks: vec![0, 1] });
    h.shared
        .active_delete_target_version
        .fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.slot("sel2"), 1.0);
    assert_eq!(
        h.eval("(let ((t (track 0)) (s (nth t.steps 6))) s.selected)"),
        Value::Bool(true)
    );
    // Clipped to the track's length: track 1 has no step 6.
    assert_eq!(h.eval("(len t1.steps)"), Value::Number(4.0));
    // Disarming the target drops the other tracks again.
    *h.shared.active_delete_target.lock().unwrap() = None;
    h.shared
        .active_delete_target_version
        .fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.slot("sel2"), 0.0);
}

#[test]
fn model_fields_are_recomputed_only_when_the_model_revision_moves() {
    let mut h = Harness::new();
    h.sync();
    let base = h.model_syncs();
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(
        h.model_syncs(),
        base,
        "an unchanged model is not re-derived"
    );
    // Any counter the model fields derive from brings it back.
    h.shared.ui_epoch.fetch_add(1, Ordering::Relaxed);
    h.sync();
    assert_eq!(h.model_syncs(), base + 1);
    h.sync();
    assert_eq!(h.model_syncs(), base + 1);
    // A recorded edit moves the history revision.
    let payload = h.eval("(dict :track 1 :name \"Bass\")");
    h.command("rename-track", payload);
    h.sync();
    assert_eq!(h.model_syncs(), base + 2);
    assert_eq!(h.eval("(let ((t (track 1))) t.name)"), s("Bass"));
    // Live fields still follow every tick.
    h.eval("(def t0 (track 0)) (def vol #'t0.volume)");
    h.shared.state.pattern.track_params[0].set_volume(0.6);
    h.sync();
    assert!((h.slot("vol") - 0.6).abs() < 1e-6);
    assert_eq!(h.model_syncs(), base + 2);
}

#[test]
fn startup_scripts_beside_the_core_modules_do_not_resolve_as_modules() {
    let mut h = Harness::new();
    for name in ["eseq.init", "eseq.sdf-stdlib"] {
        let result = h.editor.runtime_mut().eval_str(&format!("(import {name})"));
        let resolved = match &result {
            Err(_) => false,
            Ok(Some(Value::String(message))) => !message.contains("no module file found"),
            Ok(Some(_)) => true,
            Ok(None) => true,
        };
        assert!(!resolved, "{name} resolved: {result:?}");
    }
    // The core module itself does.
    h.eval("(import eseq.kinds)");
}
