use super::*;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[test]
fn new_project_default_tracks_are_armable_without_deleting_a_track() {
    const TRACK: usize = 0;
    let eng = engine::init_headless_engine(44_100, 2).expect("headless project engine");
    struct GraphGuard(sequencer::audiograph::LiveGraphPtr);
    impl Drop for GraphGuard {
        fn drop(&mut self) {
            unsafe {
                sequencer::audiograph::clear_os_workgroup();
                sequencer::audiograph::engine_stop_workers();
                sequencer::audiograph::destroy_live_graph(self.0.0);
            }
        }
    }
    let _graph_guard = GraphGuard(eng.lg_ptr);
    let state = eng.state.clone();
    let keyboard_tx = eng.keyboard_tx.clone();
    let mut app = app::App::new(
        state.clone(),
        eng.lg_ptr,
        eng.sample_rate,
        eng.buses,
        eng.master_recorder,
        eng.keyboard_tx,
    );

    let mut runtime = Runtime::new();
    runtime.register_reactive("SEQ", Vec::new(), true);
    let mut editor = Editor::new(runtime, eseqlisp::EditorConfig::default());

    let selected_steps = Arc::new(Mutex::new(HashSet::new()));
    let piano_roll_selection = Arc::new(Mutex::new(HashSet::new()));
    let track_collapsed = Arc::new(Mutex::new(app.track_collapsed.clone()));
    let bus_state = Arc::new(Mutex::new(app.buses.clone()));
    let accumulator_names = Arc::new(Mutex::new(Vec::new()));
    let record_armed = Arc::new(Mutex::new(vec![false]));
    let active_delete_target = Arc::new(Mutex::new(None));
    let active_delete_target_version = Arc::new(AtomicUsize::new(0));
    let expanded_step_projection = Arc::new(ExpandedStepProjectionRegistry::new());
    let ui_epoch = Arc::new(AtomicUsize::new(0));
    let ui_invalidations = Arc::new(UiInvalidationQueue::new());
    let sample_db =
        Rc::new(sequencer::sample_db::SampleDb::open_in_memory().expect("in-memory sample db"));
    let shared = SharedHandles {
        state: state.clone(),
        lg_raw: app.graph.lg.0,
        current_track: Arc::new(AtomicUsize::new(TRACK)),
        selected_tracks: Arc::new(Mutex::new(HashSet::new())),
        selected_steps: selected_steps.clone(),
        selected_neural_neurons: Arc::new(Mutex::new(BTreeSet::new())),
        piano_roll_selection: piano_roll_selection.clone(),
        piano_roll_move_state: Arc::new(Mutex::new(None)),
        piano_roll_focus: super::super::super::new_shared_piano_roll_focus(),
        step_clipboard: Arc::new(Mutex::new(None)),
        ui_epoch: ui_epoch.clone(),
        fx_epoch: Arc::new(AtomicUsize::new(0)),
        fx_value_epoch: Arc::new(AtomicUsize::new(0)),
        ui_invalidations: ui_invalidations.clone(),
        expanded_step_projection: expanded_step_projection.clone(),
        active_delete_target: active_delete_target.clone(),
        active_delete_target_version: active_delete_target_version.clone(),
        auto_follow_override_until: Arc::new(Mutex::new(None)),
        track_pan_ids: Arc::new(Mutex::new(Vec::new())),
        track_collapsed: track_collapsed.clone(),
        bus_state: bus_state.clone(),
        bus_node_ids: Arc::new(Mutex::new(app.graph.bus_node_ids.clone())),
        track_groups: Arc::new(Mutex::new(app.groups.clone())),
        record_armed: record_armed.clone(),
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
        accumulator_names: accumulator_names.clone(),
        piano_roll_clipboard: super::super::super::new_piano_roll_clipboard(),
        arrangement_clipboard: app::song_region::new_arrangement_clipboard(),
    };

    let mut sessions = EditSessionState::default();
    let mut frame = FrameDiffState::default();
    let mut gesture = GestureState::default();
    let mut meters = MeterCache {
        cached_peak_l_level: 0.0,
        cached_peak_r_level: 0.0,
        cached_track_peak_levels: vec![0.0],
        cached_rack_slot_peak_levels: Vec::new(),
        cached_bus_peak_levels: Vec::new(),
        cached_modulator_phases: Vec::new(),
        cached_modulator_levels: Vec::new(),
        cached_mod_port_levels: Default::default(),
        cached_mod_display_values: Default::default(),
        watched_display_modulators: std::collections::HashSet::new(),
        mod_display_poll_fx_epoch: usize::MAX,
        mod_display_poll_track: None,
        cached_cpu_load_bits: 0.0f32.to_bits(),
        last_meter_poll_at: Instant::now(),
        last_cpu_ui_poll_at: Instant::now(),
        last_neural_visualization_poll_at: Instant::now(),
        visualization_liveness: VisualizationLiveness::default(),
        last_voice_count_log_at: Instant::now(),
    };
    let mut track_names = app.tracks.clone();
    for _ in 0..2 {
        let mut ctx = LoopCtx {
            sessions: &mut sessions,
            meters: &mut meters,
            frame: &mut frame,
            gesture: &mut gesture,
            track_names: &mut track_names,
            shared: &shared,
        };
        super::handle("new-project", Value::Nil, &mut app, &mut editor, &mut ctx);
        assert_eq!(app.tracks.len(), 2);
        assert_eq!(*record_armed.lock().unwrap(), vec![false, false]);
        assert_eq!(track_names, app.tracks);
        assert_eq!(
            shared.track_pan_ids.lock().unwrap().len(),
            app.graph.track_node_ids.len()
        );
        assert!(
            matches!(editor.runtime().reactive_field_value("SEQ", "num-tracks"),
    Some(Value::Number(n)) if *n == 2.0)
        );
        for field in [
            "track-ids",
            "track-names",
            "record-armed",
            "track-num-steps",
        ] {
            assert!(
                matches!(editor.runtime().reactive_field_value("SEQ", field),
        Some(Value::List(items)) if items.len() == 2),
                "SEQ.{field}"
            );
        }
        for track in 0..2 {
            assert_eq!(
                natives::toggle_track_record_arm(
                    &mut record_armed.lock().unwrap(),
                    &mut shared.armed_rack.lock().unwrap(),
                    &shared.track_groups.lock().unwrap(),
                    track,
                ),
                Some(true)
            );
        }
    }
}
