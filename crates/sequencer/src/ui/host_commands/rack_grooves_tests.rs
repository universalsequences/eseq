//! End to end: the *groove* buffer (the selected drum rack's groove, stacked
//! under the browser; bead eseq-yks3) drives the production host path
//! (docs/rack-groove-spec.md, "UI"). The UI actions are the real Lisp ones —
//! the Extract Groove modal's `open-extract` / `commit-extract`, the picker
//! dropdown's `:on-change`, the amount pickers' `set-groove-amount`, a pad's
//! Amt picker `:on-change` and include dot `:on-click`, the on/off toggle's
//! `:on-change` — evaluated in the UI runtime, drained as host commands and
//! routed through `dispatch_custom_host_command`. "Play" is the scheduler's
//! input: the per-track groove table the lookahead reads, pushed through the
//! scheduler's own timing function.

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

/// One UI action for the test's `drive`: Lisp source, or a widget's own
/// callback with its arguments.
enum UiAction {
    Eval(String),
    Call(Value, Vec<Value>),
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
    // The kick is the current track, so its rack is the selected rack.
    editor
        .runtime_mut()
        .set_reactive("SEQ", "num-tracks", Value::Number(2.0));
    editor
        .runtime_mut()
        .set_reactive("SEQ", "current-track", Value::Number(KICK as f64));
    editor
        .runtime_mut()
        .eval_str(
            "(import eseq.drum-rack-v2) (import eseq.rack-groove-buffer)
             ;; No sidebar: the split observer has no layout to redo here.
             (set! eseq.seq-core-state/samples-sidebar-visible false)",
        )
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

    // Evaluates UI Lisp (or runs a widget's own callback) and routes every
    // host command it emits through the production dispatcher. Returns the
    // command names, in order.
    let mut drive = |action: UiAction, app: &mut app::App, editor: &mut Editor| -> Vec<String> {
        match action {
            UiAction::Eval(source) => {
                editor
                    .runtime_mut()
                    .eval_str(&source)
                    .unwrap_or_else(|e| panic!("{source}: {e:?}"));
            }
            UiAction::Call(callback, args) => {
                editor
                    .runtime_mut()
                    .invoke(callback, args)
                    .expect("widget callback");
            }
        }
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

    // Nothing active yet: no groove, and the swing control is live.
    assert_eq!(string(&get(&rack_state(&editor), "active-key")), "off");
    assert!(played(KICK, 0.0).is_none());
    assert_eq!(
        editor
            .runtime_mut()
            .eval_str("(eseq.rack-groove-buffer/selected-rack-id)")
            .unwrap(),
        Some(Value::Number(group_id as f64)),
        "the current track's rack is the buffer's rack"
    );
    // Straight lanes: one per pad in pad-note order, a bar of 16ths with no
    // cells, so the buffer draws the pads' hits instead.
    let lanes = get(&rack_state(&editor), "lanes");
    assert_eq!(number(&get(&lanes, "slots")), 16.0);
    assert!(list(&get(&lanes, "all-cells")).is_empty());
    let pads = list(&get(&lanes, "pads"));
    assert_eq!(
        pads.iter().map(|pad| number(&get(pad, "track")) as usize).collect::<Vec<_>>(),
        vec![KICK, HAT]
    );
    assert!(list(&get(&pads[0], "cells")).is_empty());

    // 1. Extract Groove… : open the modal, name it, commit (1 bar, 1/16,
    //    quantize source on — the modal's defaults).
    let sent = drive(
        UiAction::Eval(format!(
            "(eseq.rack-groove-buffer/open-extract {group_id})
             (set! eseq.rack-groove-buffer/extract-name \"Take\")
             (eseq.rack-groove-buffer/commit-extract)"
        )),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, vec!["extract-rack-groove".to_string()]);
    assert_eq!(
        editor
            .runtime_mut()
            .eval_str("eseq.rack-groove-buffer/extract-open?")
            .unwrap(),
        Some(Value::Bool(false)),
        "the modal closes on commit"
    );
    let entry = rack_state(&editor);
    assert_eq!(string(&get(&entry, "active-label")), "Take");
    assert!(string(&get(&entry, "active-key")).starts_with("pool:"));
    assert_eq!(get(&entry, "enabled"), Value::Bool(true));
    assert_eq!(
        app.grooves.len(),
        1,
        "the extracted groove is in the project pool"
    );
    assert_eq!(
        number(&get(&entry, "active-groove-id")),
        app.grooves[0].id as f64
    );
    // The picker: No groove, then a "This project" header over the pool and
    // a "Factory" header over the factory MPC swings (`:headers` rows with
    // no key); library entries carry `factory:` / `user:` keys.
    let keys = list(&get(&entry, "picker-keys"))
        .iter()
        .map(string)
        .collect::<Vec<_>>();
    let labels = list(&get(&entry, "picker-labels"))
        .iter()
        .map(string)
        .collect::<Vec<_>>();
    assert_eq!((labels[0].as_str(), keys[0].as_str()), ("No groove", "off"));
    assert_eq!((labels[1].as_str(), keys[1].as_str()), ("This project", ""));
    assert_eq!(keys[2], format!("pool:{}", app.grooves[0].id));
    assert_eq!((labels[3].as_str(), keys[3].as_str()), ("Factory", ""));
    assert_eq!(
        list(&get(&entry, "picker-headers"))[..2],
        [Value::Number(1.0), Value::Number(3.0)]
    );
    assert!(keys[4].starts_with("factory:"), "{keys:?}");

    assert!(
        keys.contains(&"factory:mpc-swing-66-16th".to_string()),
        "{keys:?}"
    );
    let pool_field = list(&field(&editor, "groove-pool"));
    assert_eq!(pool_field.len(), 1);
    let instances = list(&get(&pool_field[0], "instances"));
    assert_eq!(instances.len(), 1, "the source rack plays it");
    assert_eq!(number(&get(&instances[0], "group-id")) as u64, group_id);
    assert_eq!(string(&get(&entry, "active-grid")), "1 bar · 1/16");
    // The lanes: the All row and each pad's own row (the one it plays).
    let lanes = get(&entry, "lanes");
    assert_eq!(number(&get(&lanes, "slots")), 16.0);
    assert_eq!(list(&get(&lanes, "all-cells")).len(), 16);
    let pads = list(&get(&lanes, "pads"));
    assert_eq!(
        pads.iter().map(|pad| get(pad, "pad-note")).collect::<Vec<_>>(),
        vec![
            Value::Number(DRUM_RACK_FIRST_PAD_NOTE as f64),
            Value::Number((DRUM_RACK_FIRST_PAD_NOTE + 6) as f64),
        ],
        "one lane per pad in pad-note order"
    );
    let kick_cells = list(&get(&pads[0], "cells"));
    let kick_measured = list(&get(&pads[0], "measured"));
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
    assert_eq!(get(&pads[0], "enabled"), Value::Bool(true));
    let kick_share_field = string(&get(&pads[0], "amount-field"));
    assert_eq!(number(&field(&editor, &kick_share_field)), 1.0);
    // Quantize source: the take is now on the grid (kick step 7 -> 8), no
    // Delay left.
    let patterns = &app.state.pattern;
    assert!(patterns.patterns[KICK].is_active(8) && !patterns.patterns[KICK].is_active(7));
    for step in [0usize, 8, 10] {
        assert_eq!(patterns.step_data[KICK].get(step, StepParam::Delay), 0.0);
    }
    // The member's swing control shows the groove hint instead.
    let groove_active_for_kick = |editor: &mut Editor| {
        editor
            .runtime_mut()
            .eval_str(&format!(
                "(eseq.drum-rack-v2/groove-active-for-track? {KICK})"
            ))
            .unwrap()
    };
    assert_eq!(groove_active_for_kick(&mut editor), Some(Value::Bool(true)));

    // The buffer lays out: header, amounts and a lane per pad.
    editor.set_layout_viewport(40, 24);
    editor.refresh_runtime_side_effects();
    let buffer = editor
        .buffers
        .iter()
        .find(|b| b.name == "*groove*")
        .expect("the *groove* buffer")
        .id;
    editor.set_active_buffer(buffer);
    editor.refresh_runtime_side_effects();
    let layout = editor.widget_layout().expect("groove buffer layout");
    let panel = find_debug(&layout, "rack-groove-buffer").expect("groove buffer");
    assert_visible(panel, "groove buffer");
    for name in [
        "rack-groove-picker",
        "rack-groove-actions",
        "rack-groove-enabled",
        "rack-groove-timing",
        "rack-groove-velocity",
        "rack-groove-random",
        "rack-groove-all",
        "rack-groove-lanes-scroll",
        "rack-groove-pad",
        "rack-groove-pad-amount",
        "rack-groove-pad-enabled",
        "rack-groove-scale",
    ] {
        assert_visible(
            find_debug(panel, name).unwrap_or_else(|| panic!("{name}")),
            name,
        );
    }
    let picker = find_debug(panel, "rack-groove-picker").unwrap();
    assert_eq!(picker.props.get("filterable"), Some(&Value::Bool(true)));
    assert_eq!(
        picker.props.get("detail"),
        Some(&Value::String("1 bar · 1/16".into())),
        "the trigger shows the groove's grid before its chevron"
    );
    let details = list(picker.props.get("details").expect("picker details"));
    assert_eq!(details.len(), labels.len(), "a detail per option");
    assert_eq!(details[2], Value::String("1 bar · 1/16".into()), "the take's grid");
    let footer = string(picker.props.get("footer").expect("the Extract footer"));
    // The ≡ menu beside the picker: Extract, then the playing groove's
    // Save / Rename / Duplicate / Delete.
    let actions = find_debug(panel, "rack-groove-actions").unwrap();
    assert_eq!(
        list(actions.props.get("options").expect("actions")).iter().map(string).collect::<Vec<_>>(),
        [
            "Extract from this rack’s clip…",
            "Save to Library",
            "Rename…",
            "Duplicate",
            "Delete from Project"
        ]
    );
    let actions_on_change = actions.props.get("on-change").expect("actions on-change").clone();
    let picker_on_change = picker.props.get("on-change").expect("picker on-change").clone();
    let on = |node: &str, prop: &str| {
        find_debug(panel, node)
            .unwrap_or_else(|| panic!("{node}"))
            .props
            .get(prop)
            .unwrap_or_else(|| panic!("{node} {prop}"))
            .clone()
    };
    // The first pad row is the kick (pad-note order).
    let kick_share_on_change = on("rack-groove-pad-amount", "on-change");
    let kick_include_on_click = on("rack-groove-pad-enabled", "on-click");
    let enabled_on_change = on("rack-groove-enabled", "on-change");
    let scale_on_change = on("rack-groove-scale", "on-change");
    // No role was set, so no lane carries a role badge (never a role
    // guessed from a pad's note, like "Kick" for C1).
    assert!(find_debug(panel, "rack-groove-role").is_none());
    assert_eq!(get(&pads[0], "role-tag"), Value::String("".into()));

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

    // 3. Pick through the picker dropdown's own `:on-change`: the footer
    //    opens the Extract Groove modal, No groove plays straight, a factory
    //    library swing is copied into the pool (copy-on-apply) and
    //    activated, playing its shared row on every pad, and the extracted
    //    groove comes back.
    let pick = |label: &str| vec![Value::String(label.into())];
    let sent = drive(
        UiAction::Call(picker_on_change.clone(), pick(&footer)),
        &mut app,
        &mut editor,
    );
    assert!(sent.is_empty(), "the footer is UI only: {sent:?}");
    assert_eq!(
        editor.runtime_mut().eval_str("eseq.rack-groove-buffer/extract-open?").unwrap(),
        Some(Value::Bool(true)),
        "the footer opens the Extract Groove modal"
    );
    editor
        .runtime_mut()
        .eval_str("(set! eseq.rack-groove-buffer/extract-open? false)")
        .unwrap();
    let sent = drive(
        UiAction::Call(picker_on_change.clone(), pick("No groove")),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, vec!["set-rack-groove".to_string()]);
    assert_eq!(string(&get(&rack_state(&editor), "active-key")), "off");
    assert!(played(KICK, 2.0).is_none() && played(HAT, 0.5).is_none());
    assert_eq!(groove_active_for_kick(&mut editor), Some(Value::Bool(false)));
    let undo_before_library = app.history.undo_len();
    let sent = drive(
        UiAction::Call(picker_on_change.clone(), pick("MPC 16 Swing 66%")),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, vec!["set-rack-groove".to_string()]);
    assert_eq!(
        app.grooves.len(),
        2,
        "the library swing was copied into the pool"
    );
    assert_eq!(app.grooves[1].name, "MPC 16 Swing 66%");
    assert_eq!(
        app.history.undo_len(),
        undo_before_library + 1,
        "import + activate is one undo step"
    );
    let entry = rack_state(&editor);
    assert_eq!(
        string(&get(&entry, "active-key")),
        format!("pool:{}", app.grooves[1].id)
    );
    // The imported groove is now a pool groove under This project.
    let labels = list(&get(&entry, "picker-labels"))
        .iter()
        .map(string)
        .collect::<Vec<_>>();
    assert_eq!(
        &labels[..5],
        ["No groove", "This project", "Take", "MPC 16 Swing 66%", "Factory"]
    );
    assert_eq!(string(&get(&entry, "active-label")), "MPC 16 Swing 66%");
    // A two-slot swing tiles out to a bar of lanes.
    assert_eq!(number(&get(&get(&entry, "lanes"), "slots")), 16.0);
    let swung = played(HAT, 0.25).unwrap();
    assert!(
        (swung - (0.25 + 0.32 * 0.25)).abs() < 2.0 / spq,
        "66% swing: {swung}"
    );
    assert!((played(KICK, 0.0).unwrap()).abs() < 1.0 / spq);
    drive(
        UiAction::Call(picker_on_change.clone(), pick("Take")),
        &mut app,
        &mut editor,
    );
    assert_eq!(
        string(&get(&rack_state(&editor), "active-key")),
        format!("pool:{}", app.grooves[0].id)
    );
    check_take(&[0, 8, 10], &kick_heard, KICK);

    // 4. Amount pickers: a drag writes through immediately and lands as ONE
    //    undo step when the gesture ends.
    let rack_settings = |app: &app::App| {
        app.groups
            .iter()
            .find(|g| g.id == group_id)
            .unwrap()
            .rack
            .as_ref()
            .unwrap()
            .groove
            .clone()
    };
    let undo_len = app.history.undo_len();
    for value in [0.8, 0.6, 0.5] {
        let sent = drive(
            UiAction::Eval(format!(
                "(eseq.drum-rack-v2/set-groove-amount {group_id} \"timing\" {value})"
            )),
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
    assert_eq!(
        rack_settings(&app).timing_amount,
        1.0,
        "undo restores the pre-drag amount"
    );
    check_take(&[0, 8, 10], &kick_heard, KICK);

    // 5. A pad's share: the kick's Amt picker halves only the kick; its
    //    include dot leaves it straight (still grooved, so no track swing)
    //    while the hat keeps the pocket. Both undo.
    let undo_len = app.history.undo_len();
    let sent = drive(
        UiAction::Call(kick_share_on_change.clone(), vec![Value::Number(0.5)]),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, vec!["set-rack-groove-pad-amount".to_string()]);
    assert_eq!(number(&field(&editor, &kick_share_field)), 0.5);
    let half = played(KICK, 2.0).unwrap();
    assert!(
        (half - (2.0 - 0.5 * 0.15 * STEP_BEATS)).abs() < 2.0 / spq,
        "half kick share: {half}"
    );
    check_take(&[2, 6, 10, 14], &hat_heard, HAT);
    app::edit::finish_active_gesture(&mut app);
    assert_eq!(app.history.undo_len(), undo_len + 1);
    let sent = drive(
        UiAction::Call(
            kick_include_on_click.clone(),
            vec![Value::Number(0.0), Value::Number(0.0), Value::Nil],
        ),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, vec!["set-rack-groove-pad-enabled".to_string()]);
    assert_eq!(
        rack_settings(&app).pad(DRUM_RACK_FIRST_PAD_NOTE),
        sequencer::groove::RackGroovePad {
            pad_note: DRUM_RACK_FIRST_PAD_NOTE,
            amount: 0.5,
            enabled: false,
        },
        "excluding keeps the pad's amount"
    );
    let pads = list(&get(&get(&rack_state(&editor), "lanes"), "pads"));
    assert_eq!(get(&pads[0], "enabled"), Value::Bool(false));
    assert_eq!(played(KICK, 2.0), Some(2.0), "an excluded pad plays straight");
    check_take(&[2, 6, 10, 14], &hat_heard, HAT);
    for _ in 0..2 {
        assert!(matches!(
            app::edit::undo(&mut app),
            app::history::HistoryReplay::Applied(_)
        ));
    }
    assert!(rack_settings(&app).pads.is_empty(), "undo restores every pad");
    check_take(&[0, 8, 10], &kick_heard, KICK);

    // 6. The on/off switch bypasses the groove and keeps the selection.
    let sent = drive(
        UiAction::Call(enabled_on_change.clone(), vec![Value::Bool(false)]),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, vec!["set-rack-groove-enabled".to_string()]);
    let entry = rack_state(&editor);
    assert_eq!(get(&entry, "enabled"), Value::Bool(false));
    assert_eq!(string(&get(&entry, "active-label")), "Take");
    assert!(played(KICK, 2.0).is_none() && played(HAT, 0.5).is_none());
    assert_eq!(groove_active_for_kick(&mut editor), Some(Value::Bool(false)));
    drive(
        UiAction::Call(enabled_on_change.clone(), vec![Value::Bool(true)]),
        &mut app,
        &mut editor,
    );
    check_take(&[0, 8, 10], &kick_heard, KICK);

    // 7. The ≡ menu acts on the playing groove.
    let pool_len = app.grooves.len();
    let sent = drive(
        UiAction::Call(actions_on_change.clone(), vec![Value::String("Duplicate".into())]),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, vec!["duplicate-pool-groove".to_string()]);
    assert_eq!(app.grooves.len(), pool_len + 1);
    assert_eq!(app.grooves.last().unwrap().name, "Take copy");

    // 8. Scale 2×: the take's grid doubles (the picker reads 2 bars · 1/8)
    //    and the early kick of step 8 lands at beat 4, half a slot-width
    //    scaled: the same pocket for the pattern on 1/8 steps.
    let sent = drive(
        UiAction::Call(scale_on_change.clone(), vec![Value::String("2×".into())]),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, vec!["set-rack-groove-scale".to_string()]);
    let entry = rack_state(&editor);
    assert_eq!(string(&get(&entry, "active-grid")), "2 bars · 1/8");
    assert_eq!(string(&get(&entry, "scale-label")), "2×");
    let doubled = played(KICK, 4.0).unwrap();
    assert!(
        (doubled - (4.0 - 0.15 * 2.0 * STEP_BEATS)).abs() < 2.0 / spq,
        "2× kick: {doubled}"
    );
    drive(
        UiAction::Call(scale_on_change.clone(), vec![Value::String("1×".into())]),
        &mut app,
        &mut editor,
    );
    check_take(&[0, 8, 10], &kick_heard, KICK);

    // 9. A role set on the pad (the pad grid's Role ▸ menu) reaches the
    //    kick's lane as its tag; clearing it back to Standard removes it.
    let set_role = |role: &str| {
        format!(
            "(host-command \"set-rack-pad-role\" (dict :group-id {group_id} :pad-note {DRUM_RACK_FIRST_PAD_NOTE} :role \"{role}\"))"
        )
    };
    let sent = drive(UiAction::Eval(set_role("closed-hat")), &mut app, &mut editor);
    assert_eq!(sent, vec!["set-rack-pad-role".to_string()]);
    let pads = list(&get(&get(&rack_state(&editor), "lanes"), "pads"));
    assert_eq!(get(&pads[0], "role-tag"), Value::String("CH".into()));
    drive(UiAction::Eval(set_role("standard")), &mut app, &mut editor);
    let pads = list(&get(&get(&rack_state(&editor), "lanes"), "pads"));
    assert_eq!(get(&pads[0], "role-tag"), Value::String("".into()));

    // 10. Clips: grooves are per clip. Converted, a clip shows the rack's
    //     groove until its first edit gives it its own: Timing 50% on one
    //     clip leaves the other at the rack's; 75% there leaves the first at
    //     50%. "Use Rack Groove" hands a clip back; "Apply to All" makes a
    //     clip's groove every clip's.
    let eval = |editor: &mut Editor, source: &str| editor.runtime_mut().eval_str(source).unwrap();
    let sent = drive(
        UiAction::Eval(format!("(eseq.drum-rack-v2/convert-to-clips {group_id})")),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, vec!["convert-rack-to-clips".to_string()]);
    let verse = app.current_rack_clip(group_id).expect("the playing clip");
    let chorus = app.save_rack_clip_as_recorded(group_id, "Chorus").expect("second clip");
    let view_clip = |editor: &mut Editor| {
        eval(editor, &format!("(eseq.drum-rack-v2/groove-clip-id {group_id})"))
    };
    let launch = |clip: u64| {
        UiAction::Eval(format!(
            "(eseq.drum-rack-v2/launch-clip {group_id} {clip})"
        ))
    };
    let set_timing = |value: f64| {
        UiAction::Eval(format!(
            "(eseq.drum-rack-v2/set-groove-amount {group_id} \"timing\" {value})"
        ))
    };
    let timing_of = |app: &app::App, clip: u64| {
        app.groups.iter().find(|g| g.id == group_id).unwrap().rack.as_ref().unwrap()
            .groove_for_clip(Some(clip)).timing_amount
    };
    drive(launch(verse), &mut app, &mut editor);
    assert_eq!(view_clip(&mut editor), Some(Value::Number(-1.0)), "the verse shows the rack's");
    let menu = list(&eval(&mut editor, &format!("(eseq.rack-groove-buffer/menu-actions {group_id})")).unwrap());
    assert!(menu.contains(&Value::String("Apply to All Clips in This Rack".into())), "{menu:?}");
    assert!(!menu.contains(&Value::String("Use Rack Groove for This Clip".into())));

    drive(set_timing(0.5), &mut app, &mut editor);
    app::edit::finish_active_gesture(&mut app);
    assert_eq!(view_clip(&mut editor), Some(Value::Number(verse as f64)), "the edit forked it");
    assert_eq!(
        number(&field(&editor, &format!("rack-groove-timing-{group_id}-c{verse}"))),
        0.5
    );
    drive(launch(chorus), &mut app, &mut editor);
    assert_eq!(app.current_rack_clip(group_id), Some(chorus));
    assert_eq!(timing_of(&app, chorus), 1.0, "the chorus is untouched");
    drive(set_timing(0.75), &mut app, &mut editor);
    app::edit::finish_active_gesture(&mut app);
    drive(launch(verse), &mut app, &mut editor);
    assert_eq!(timing_of(&app, verse), 0.5, "the verse kept its own");
    assert_eq!(timing_of(&app, chorus), 0.75);
    let rack = app.groups.iter().find(|g| g.id == group_id).unwrap().rack.clone().unwrap();
    assert_eq!(rack.groove.timing_amount, 1.0, "the rack's own groove is untouched");

    let menu = list(&eval(&mut editor, &format!("(eseq.rack-groove-buffer/menu-actions {group_id})")).unwrap());
    assert!(menu.contains(&Value::String("Use Rack Groove for This Clip".into())), "{menu:?}");
    let sent = drive(
        UiAction::Call(
            actions_on_change.clone(),
            vec![Value::String("Apply to All Clips in This Rack".into())],
        ),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, vec!["apply-rack-groove-to-all-clips".to_string()]);
    assert_eq!((timing_of(&app, verse), timing_of(&app, chorus)), (0.5, 0.5));
    assert_eq!(view_clip(&mut editor), Some(Value::Number(-1.0)), "every clip follows again");
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

#[test]
fn groove_confirm_messages_list_racks() {
    assert_eq!(
        super::delete_pool_groove_confirm_message("Take", &["Kit A".to_string()]),
        "Delete groove 'Take'? Kit A plays it; it will play straight (undo restores it)."
    );
    assert_eq!(
        super::delete_pool_groove_confirm_message(
            "Take",
            &["Kit A".into(), "Kit B".into(), "Kit C".into()]
        ),
        "Delete groove 'Take'? Kit A, Kit B and Kit C play it; they will play straight (undo restores it)."
    );
}
