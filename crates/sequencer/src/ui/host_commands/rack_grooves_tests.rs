//! End to end: the drum rack panel's Groove section drives the production
//! host path (docs/rack-groove-spec.md, "UI"; bead eseq-groove.4). The UI
//! actions are the real Lisp ones — the Extract Groove modal's
//! `open-extract` / `commit-extract`, the picker's `set-groove`, the amount
//! knobs' `set-groove-amount` — evaluated in the UI runtime, drained as host
//! commands and routed through `dispatch_custom_host_command`. "Play" is the
//! scheduler's input: the per-track groove table the lookahead reads, pushed
//! through the scheduler's own timing function.

use super::*;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sequencer::groove::{grooved_sample_time, GrooveFloor};
use sequencer::sequencer::{StepParam, DRUM_RACK_FIRST_PAD_NOTE};

const KICK: usize = 0;
const HAT: usize = 1;
/// One 16th step in beats (the default timebase).
const STEP_BEATS: f64 = 0.25;
/// The played take: (step, Delay) per member. The kick's step-7 hit is
/// "very late", which extraction reads as an EARLY hit on step 8.
const KICK_TAKE: [(usize, f32); 3] = [(0, 0.02), (7, 0.85), (10, 0.06)];
const HAT_TAKE: [(usize, f32); 4] = [(2, 0.3), (6, 0.26), (10, 0.34), (14, 0.22)];

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

fn field(editor: &Editor, name: &str) -> Value {
    editor
        .runtime()
        .reactive_field_value("SEQ", name)
        .cloned()
        .unwrap_or(Value::Nil)
}

fn get(value: &Value, key: &str) -> Value {
    match value {
        Value::Map(map) => map
            .get(key)
            .map(|cell| cell.borrow().clone())
            .unwrap_or(Value::Nil),
        _ => Value::Nil,
    }
}

fn list(value: &Value) -> Vec<Value> {
    match value {
        Value::List(items) => items.iter().map(|cell| cell.borrow().clone()).collect(),
        _ => Vec::new(),
    }
}

fn number(value: &Value) -> f64 {
    match value {
        Value::Number(n) => *n,
        other => panic!("expected a number, got {other:?}"),
    }
}

fn string(value: &Value) -> String {
    match value {
        Value::String(s) => s.to_string(),
        other => panic!("expected a string, got {other:?}"),
    }
}

fn find_debug<'a>(
    node: &'a eseqlisp::layout::LayoutNode,
    name: &str,
) -> Option<&'a eseqlisp::layout::LayoutNode> {
    if matches!(node.props.get("debug-name"), Some(Value::String(value)) if value == name) {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_debug(child, name))
}

fn assert_visible(node: &eseqlisp::layout::LayoutNode, label: &str) {
    let rect = &node.rect;
    assert!(
        [rect.col, rect.row, rect.width, rect.height]
            .iter()
            .all(|v| v.is_finite())
            && rect.width > 0.0
            && rect.height > 0.0,
        "{label} must have a finite, nonzero rect: {rect:?}"
    );
}

#[test]
fn extract_pick_and_play_a_rack_groove_through_the_ui() {
    let eng = engine::init_headless_engine(44_100, 2).expect("headless engine");
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

    // Setup (not under test): two sampler members on a drum rack, with a
    // played, off-grid take on their live patterns.
    let kick = app
        .graph_controller()
        .add_blank_sampler_track()
        .expect("kick");
    let hat = app
        .graph_controller()
        .add_blank_sampler_track()
        .expect("hat");
    assert_eq!((kick, hat), (KICK, HAT));
    let (group_id, _) = app.create_drum_rack_recorded(None).expect("rack");
    app.assign_rack_pad_track_recorded(group_id, DRUM_RACK_FIRST_PAD_NOTE, KICK)
        .expect("kick pad");
    app.assign_rack_pad_track_recorded(group_id, DRUM_RACK_FIRST_PAD_NOTE + 6, HAT)
        .expect("hat pad");
    for (track, take) in [(KICK, &KICK_TAKE[..]), (HAT, &HAT_TAKE[..])] {
        for &(step, delay) in take {
            app.state.pattern.patterns[track].set_step_active(step, true);
            app::try_apply_command(
                &mut app,
                app::AppCommand::SetStepParam {
                    track,
                    step,
                    param: StepParam::Delay,
                    value: delay,
                },
            )
            .expect("take delay");
        }
    }
    // Where each hit was heard, in beats: step start + Delay of a step.
    let heard = |take: &[(usize, f32)]| -> Vec<f64> {
        take.iter()
            .map(|&(step, delay)| (step as f64 + delay as f64) * STEP_BEATS)
            .collect()
    };
    let (kick_heard, hat_heard) = (heard(&KICK_TAKE), heard(&HAT_TAKE));

    // The production UI runtime with the real rack modules loaded.
    let mut runtime = Runtime::new();
    runtime.register_reactive("SEQ", Vec::new(), true);
    let mut editor = Editor::new(runtime, eseqlisp::EditorConfig::default());
    let paths = sequencer::app_paths::app_paths();
    let (roots, errors) = paths.module_load_roots();
    assert!(errors.is_empty(), "{errors:?}");
    editor.runtime_mut().set_load_root(paths.factory_root());
    editor.runtime_mut().set_scoped_module_load_path(roots);
    sync_groups_bindings(editor.runtime_mut(), &app.groups, &app.grooves);
    editor
        .runtime_mut()
        .eval_str("(import eseq.drum-rack-v2) (import eseq.effects.rack-groove)")
        .expect("load the rack groove UI modules");

    let sample_db = sequencer::sample_db::SampleDb::open_in_memory().expect("sample db");
    let shared = SharedHandles {
        state: state.clone(),
        lg_raw: app.graph.lg.0,
        current_track: Arc::new(AtomicUsize::new(KICK)),
        selected_tracks: Arc::new(Mutex::new(HashSet::new())),
        selected_steps: Arc::new(Mutex::new(HashSet::new())),
        selected_neural_neurons: Arc::new(Mutex::new(BTreeSet::new())),
        piano_roll_selection: Arc::new(Mutex::new(HashSet::new())),
        piano_roll_move_state: Arc::new(Mutex::new(None)),
        piano_roll_focus: super::super::super::new_shared_piano_roll_focus(),
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
        record_armed: Arc::new(Mutex::new(vec![false; 2])),
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
        piano_roll_clipboard: super::super::super::new_piano_roll_clipboard(),
        arrangement_clipboard: app::song_region::new_arrangement_clipboard(),
    };
    let mut sessions = EditSessionState::default();
    let mut frame = FrameDiffState::default();
    let mut gesture = GestureState::default();
    let mut meters = MeterCache {
        cached_peak_l_level: 0.0,
        cached_peak_r_level: 0.0,
        cached_track_peak_levels: vec![0.0; 2],
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
    };
    let mut track_names = app.tracks.clone();

    // Evaluates UI Lisp and routes every host command it emits through the
    // production dispatcher. Returns the command names, in order.
    let mut ui = |source: &str, app: &mut app::App, editor: &mut Editor| -> Vec<String> {
        editor
            .runtime_mut()
            .eval_str(source)
            .unwrap_or_else(|e| panic!("{source}: {e:?}"));
        let mut names = Vec::new();
        for command in editor.drain_host_commands() {
            let eseqlisp::HostCommand::Custom { name, payload } = command else {
                continue;
            };
            let mut ctx = LoopCtx {
                sessions: &mut sessions,
                meters: &mut meters,
                frame: &mut frame,
                gesture: &mut gesture,
                track_names: &mut track_names,
                shared: &shared,
            };
            dispatch_custom_host_command(&name, payload, app, editor, &mut ctx);
            names.push(name);
        }
        names
    };
    let rack_state = |editor: &Editor| {
        list(&field(editor, "rack-grooves"))
            .into_iter()
            .find(|entry| number(&get(entry, "group-id")) as u64 == group_id)
            .expect("the rack's SEQ.rack-grooves entry")
    };
    let spq = 44_100.0 * 60.0 / state.transport.bpm.load(Ordering::Relaxed) as f64;
    // What the scheduler plays for a straight trig at `beat` on `track`.
    let played = |track: usize, beat: f64| -> Option<f64> {
        let table = state.track_grooves();
        let groove = table.get(track).and_then(Option::as_ref)?;
        let straight = (beat * spq).round() as u64;
        let sample = grooved_sample_time(groove, beat, straight, spq, GrooveFloor::default())
            .expect("an on-time trig always plays");
        Some(sample as f64 / spq)
    };

    // Nothing active yet: the section says Off and the swing control is live.
    assert_eq!(string(&get(&rack_state(&editor), "active-key")), "off");
    assert_eq!(get(&rack_state(&editor), "heatmap"), Value::Nil);
    assert!(played(KICK, 0.0).is_none());

    // 1. Extract Groove… : open the modal, name it, commit (1 bar, 1/16,
    //    quantize source on — the modal's defaults).
    let sent = ui(
        &format!(
            "(eseq.effects.rack-groove/open-extract {group_id})
             (set! eseq.effects.rack-groove/extract-name \"Take\")
             (eseq.effects.rack-groove/commit-extract)"
        ),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, vec!["extract-rack-groove".to_string()]);
    assert_eq!(
        editor
            .runtime_mut()
            .eval_str("eseq.effects.rack-groove/extract-open?")
            .unwrap(),
        Some(Value::Bool(false)),
        "the modal closes on commit"
    );
    let entry = rack_state(&editor);
    assert_eq!(string(&get(&entry, "active-label")), "Take");
    assert!(string(&get(&entry, "active-key")).starts_with("pool:"));
    assert_eq!(
        app.grooves.len(),
        1,
        "the extracted groove is in the project pool"
    );
    assert_eq!(
        number(&get(&entry, "active-groove-id")),
        app.grooves[0].id as f64
    );
    // The picker: the pool, then the library (the factory MPC swings), then
    // Off; library entries carry `factory:` / `user:` keys.
    let keys = list(&get(&entry, "picker-keys"))
        .iter()
        .map(string)
        .collect::<Vec<_>>();
    assert_eq!(keys[0], format!("pool:{}", app.grooves[0].id));
    assert!(
        keys.contains(&"factory:mpc-swing-66-16th".to_string()),
        "{keys:?}"
    );
    assert_eq!(keys.last().map(String::as_str), Some("off"));
    let pool_field = list(&field(&editor, "groove-pool"));
    assert_eq!(pool_field.len(), 1);
    let instances = list(&get(&pool_field[0], "instances"));
    assert_eq!(instances.len(), 1, "the source rack plays it");
    assert_eq!(number(&get(&instances[0], "group-id")) as u64, group_id);
    assert!(list(&field(&editor, "groove-library"))
        .iter()
        .any(|entry| string(&get(entry, "key")) == "factory:mpc-swing-58-16th"));
    assert_eq!(string(&get(&entry, "active-grid")), "1 bar · 1/16");
    let heat = get(&entry, "heatmap");
    assert_eq!(number(&get(&heat, "slots")), 16.0);
    let rows = list(&get(&heat, "rows"));
    assert_eq!(
        rows.iter()
            .map(|row| string(&get(row, "label")))
            .collect::<Vec<_>>(),
        vec!["All", "C1", "F#1"],
        "an All row, then one row per pad in pad-note order"
    );
    let kick_row = &rows[1];
    let kick_cells = list(&get(kick_row, "cells"));
    let kick_measured = list(&get(kick_row, "measured"));
    assert!(
        (number(&kick_cells[8]) + 0.15).abs() < 1e-4,
        "the early kick"
    );
    assert_eq!(kick_measured[8], Value::Bool(true));
    assert_eq!(
        kick_measured[4],
        Value::Bool(false),
        "an unplayed slot is filled, dimmed"
    );
    // Quantize source: the take is now on the grid (kick step 7 -> 8), no
    // Delay left.
    let patterns = &app.state.pattern;
    assert!(patterns.patterns[KICK].is_active(8) && !patterns.patterns[KICK].is_active(7));
    for step in [0usize, 8, 10] {
        assert_eq!(patterns.step_data[KICK].get(step, StepParam::Delay), 0.0);
    }
    // The member's swing control shows the groove hint instead.
    assert_eq!(
        editor
            .runtime_mut()
            .eval_str(&format!(
                "(eseq.drum-rack-v2/groove-active-for-track? {KICK})"
            ))
            .unwrap(),
        Some(Value::Bool(true))
    );

    // The section itself lays out: panel, picker, knobs and a heatmap with
    // one cell per slot per row.
    editor
        .runtime_mut()
        .eval_str("(effect-buffer \"*groove-test*\" (eseq.effects.rack-groove/panel 0))")
        .expect("mount the groove panel");
    editor.set_layout_viewport(160, 30);
    editor.refresh_runtime_side_effects();
    let buffer = editor
        .buffers
        .iter()
        .find(|b| b.name == "*groove-test*")
        .unwrap()
        .id;
    editor.set_active_buffer(buffer);
    editor.refresh_runtime_side_effects();
    let layout = editor.widget_layout().expect("groove panel layout");
    let panel = find_debug(&layout, "rack-groove-panel").expect("groove panel");
    assert_visible(panel, "groove panel");
    for name in [
        "rack-groove-picker",
        "rack-groove-heatmap",
        "rack-groove-extract",
        "rack-groove-timing",
        "rack-groove-velocity",
        "rack-groove-random",
    ] {
        assert_visible(
            find_debug(panel, name).unwrap_or_else(|| panic!("{name}")),
            name,
        );
    }
    let heatmap = find_debug(panel, "rack-groove-heatmap").unwrap();
    // 3 rows + the beat ruler.
    assert_eq!(heatmap.children.len(), 4);

    // 2. Play: the quantized source through the groove lands where the take
    //    was heard (one repeat, so the slot median IS the hit).
    let check_take = |steps: &[usize], heard: &[f64], track: usize| {
        for (step, heard) in steps.iter().zip(heard) {
            let beat = *step as f64 * STEP_BEATS;
            let at = played(track, beat).expect("the member plays through the groove");
            assert!(
                (at - heard).abs() < 2.0 / spq,
                "track {track} step {step}: played at {at} beats, heard at {heard}"
            );
        }
    };
    check_take(&[0, 8, 10], &kick_heard, KICK);
    check_take(&[2, 6, 10, 14], &hat_heard, HAT);
    assert!(played(KICK, 2.0).unwrap() < 2.0, "the kick is pushed EARLY");

    // 3. Pick through the picker: Off plays straight, a factory library
    //    swing is copied into the pool (copy-on-apply) and plays its shared
    //    row on every pad, and the extracted groove comes back.
    ui(
        &format!("(eseq.drum-rack-v2/set-groove {group_id} \"Off\")"),
        &mut app,
        &mut editor,
    );
    assert_eq!(string(&get(&rack_state(&editor), "active-key")), "off");
    assert!(played(KICK, 2.0).is_none() && played(HAT, 0.5).is_none());
    assert_eq!(
        editor
            .runtime_mut()
            .eval_str(&format!(
                "(eseq.drum-rack-v2/groove-active-for-track? {KICK})"
            ))
            .unwrap(),
        Some(Value::Bool(false))
    );
    ui(
        &format!("(eseq.drum-rack-v2/set-groove {group_id} \"MPC 16 Swing 66%\")"),
        &mut app,
        &mut editor,
    );
    assert_eq!(
        app.grooves.len(),
        2,
        "the library swing was copied into the pool"
    );
    assert_eq!(app.grooves[1].name, "MPC 16 Swing 66%");
    assert_eq!(
        string(&get(&rack_state(&editor), "active-key")),
        format!("pool:{}", app.grooves[1].id)
    );
    let swung = played(HAT, 0.25).unwrap();
    assert!(
        (swung - (0.25 + 0.32 * 0.25)).abs() < 2.0 / spq,
        "66% swing: {swung}"
    );
    assert!((played(KICK, 0.0).unwrap()).abs() < 1.0 / spq);
    ui(
        &format!("(eseq.drum-rack-v2/set-groove {group_id} \"Take\")"),
        &mut app,
        &mut editor,
    );
    check_take(&[0, 8, 10], &kick_heard, KICK);

    // 4. Amount knobs: a drag writes through immediately and lands as ONE
    //    undo step when the gesture ends.
    let undo_len = app.history.undo_len();
    for value in [0.8, 0.6, 0.5] {
        let sent = ui(
            &format!("(eseq.drum-rack-v2/set-groove-amount {group_id} \"timing\" {value})"),
            &mut app,
            &mut editor,
        );
        assert_eq!(sent, vec!["set-rack-groove-amount".to_string()]);
    }
    assert_eq!(
        number(&field(&editor, &format!("rack-groove-timing-{group_id}"))),
        0.5
    );
    let half = played(KICK, 2.0).unwrap();
    assert!(
        (half - (2.0 - 0.5 * 0.15 * STEP_BEATS)).abs() < 2.0 / spq,
        "half timing: {half}"
    );
    app::edit::finish_active_gesture(&mut app);
    assert_eq!(
        app.history.undo_len(),
        undo_len + 1,
        "one drag, one undo step"
    );
    assert!(matches!(
        app::edit::undo(&mut app),
        app::history::HistoryReplay::Applied(_)
    ));
    let timing = app
        .groups
        .iter()
        .find(|g| g.id == group_id)
        .unwrap()
        .rack
        .as_ref()
        .unwrap()
        .groove
        .timing_amount;
    assert_eq!(timing, 1.0, "undo restores the pre-drag amount");
    check_take(&[0, 8, 10], &kick_heard, KICK);
}

#[test]
fn extract_modal_payload_maps_to_the_extract_request() {
    // Pure payload -> request mapping of the Extract Groove modal.
    let payload = eseqlisp::vm::Value::Map(
        [
            ("group-id", Value::Number(1.0)),
            ("name", Value::String("  Pocket ".into())),
            ("bars", Value::Number(2.0)),
            ("resolution", Value::String("1/32".into())),
            ("quantize", Value::Bool(false)),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
        .collect(),
    );
    let request = super::extract_request_from_payload(&payload).expect("request");
    assert_eq!(request.options.name, "Pocket");
    assert_eq!(request.options.period_beats, 8.0);
    assert_eq!(request.options.resolution_beats, 0.125);
    assert!(!request.quantize_source);
}
