//! The Grooves browser tab (docs/rack-groove-spec.md, "Rev 2 UI"; bead
//! eseq-groove.11). The tree is checked as the pure function of
//! `SEQ.groove-pool` / `SEQ.groove-library` it is; every context-menu action
//! is then driven through the production path: the real
//! `eseq.grooves-tab` Lisp (`open-menu` + `select-menu-action` on a row of
//! the real `seq-groove-tree`), its host commands drained and routed through
//! `dispatch_custom_host_command`, and the app / library files checked.
//! That the menu is mounted and reachable by a right-click on a rendered
//! tree row is checked in `state_values::tests::
//! metal_seq_browser_grooves_tab_right_click_opens_mounted_menu`.

use super::*;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sequencer::groove::library::override_groove_library_dirs_for_tests;
use sequencer::sequencer::StepParam;

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

/// Library redirect undone even when an assertion fails.
struct LibraryGuard;
impl Drop for LibraryGuard {
    fn drop(&mut self) {
        override_groove_library_dirs_for_tests(None);
    }
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

fn string(value: &Value) -> String {
    match value {
        Value::String(s) => s.to_string(),
        other => panic!("expected a string, got {other:?}"),
    }
}

fn labels(rows: &[Value]) -> Vec<String> {
    rows.iter().map(|row| string(&get(row, "label"))).collect()
}

fn instance(group: f64, name: &str, timing: f64, velocity: f64, random: f64) -> Value {
    map_value([
        ("group-id", Value::Number(group)),
        ("name", Value::String(name.to_string().into())),
        ("timing", Value::Number(timing)),
        ("velocity", Value::Number(velocity)),
        ("random", Value::Number(random)),
    ])
}

fn pool_entry(id: f64, name: &str, instances: Vec<Value>) -> Value {
    map_value([
        ("id", Value::Number(id)),
        ("key", Value::String(format!("pool:{id}").into())),
        ("name", Value::String(name.to_string().into())),
        ("grid", Value::String("1 bar · 1/16".into())),
        ("instances", list_value(instances)),
    ])
}

fn library_entry(tier: &str, stem: &str, name: &str) -> Value {
    map_value([
        ("key", Value::String(format!("{tier}:{stem}").into())),
        ("name", Value::String(name.to_string().into())),
        ("tier", Value::String(tier.to_string().into())),
        ("stem", Value::String(stem.to_string().into())),
    ])
}

#[test]
fn groove_tree_sections_instances_and_search() {
    let pool = list_value(vec![
        pool_entry(
            1.0,
            "Dilla take",
            vec![
                instance(4.0, "Kit A", 1.0, 0.4, 0.0),
                instance(7.0, "Kit B", 0.5, 0.0, 0.25),
            ],
        ),
        pool_entry(2.0, "Spare", vec![]),
    ]);
    let library = list_value(vec![
        library_entry("factory", "mpc-swing-58-16th", "MPC 16 Swing 58%"),
        library_entry("user", "lazy", "Lazy"),
    ]);
    let rows = list(&groove_tree_value("", &pool, &library));
    assert_eq!(
        labels(&rows),
        [
            "In use",
            "Dilla take",
            "Project",
            "Dilla take",
            "Spare",
            "Library",
            "Lazy",
            "Factory",
            "MPC 16 Swing 58%"
        ],
        "In use lists only grooves some rack plays; Library is the user tier"
    );
    for header in [0, 2, 5, 7] {
        assert_eq!(string(&get(&rows[header], "kind")), "header");
    }
    // A played groove: check + rack-count badge, one child per rack with its
    // amounts; the In use and Project copies have distinct paths.
    let in_use = &rows[1];
    assert_eq!(get(in_use, "status-icon"), Value::Keyword("check".into()));
    assert_eq!(get(in_use, "badge"), Value::Number(2.0));
    assert_eq!(string(&get(in_use, "path")), "in-use/pool:1");
    assert_eq!(string(&get(&rows[3], "path")), "project/pool:1");
    let children = list(&get(in_use, "children"));
    assert_eq!(
        labels(&children),
        ["Kit A · T100 V40 R0", "Kit B · T50 V0 R25"]
    );
    assert_eq!(get(&children[1], "group-id"), Value::Number(7.0));
    assert_eq!(get(&children[1], "groove-id"), Value::Number(1.0));
    assert_eq!(string(&get(&children[1], "kind")), "instance");
    // An unplayed pool groove has no badge and no children.
    assert_eq!(get(&rows[4], "badge"), Value::Nil);
    assert_eq!(get(&rows[4], "children"), Value::Nil);
    // Library rows: user files are editable, factory ones read-only.
    assert_eq!(get(&rows[6], "read-only?"), Value::Bool(false));
    assert_eq!(string(&get(&rows[6], "key")), "user:lazy");
    assert_eq!(get(&rows[8], "read-only?"), Value::Bool(true));

    // Empty sections say so.
    let empty = list(&groove_tree_value(
        "",
        &list_value(vec![]),
        &list_value(vec![]),
    ));
    assert_eq!(
        empty
            .iter()
            .filter(|row| string(&get(row, "kind")) == "empty")
            .count(),
        4
    );

    // Search: groove names, or a rack name (only that instance), and no
    // empty sections.
    let rows = list(&groove_tree_value("kit b", &pool, &library));
    assert_eq!(
        labels(&rows),
        ["In use", "Dilla take", "Project", "Dilla take"]
    );
    assert_eq!(
        labels(&list(&get(&rows[1], "children"))),
        ["Kit B · T50 V0 R25"]
    );
    let rows = list(&groove_tree_value("SWING", &pool, &library));
    assert_eq!(labels(&rows), ["Factory", "MPC 16 Swing 58%"]);
}

#[test]
fn groove_confirm_messages_list_racks() {
    assert_eq!(
        super::super::rack_grooves::delete_pool_groove_confirm_message(
            "Take",
            &["Kit A".to_string()]
        ),
        "Delete groove 'Take'? Kit A plays it; it will play straight (undo restores it)."
    );
    assert_eq!(
        super::super::rack_grooves::delete_pool_groove_confirm_message(
            "Take",
            &["Kit A".into(), "Kit B".into(), "Kit C".into()]
        ),
        "Delete groove 'Take'? Kit A, Kit B and Kit C play it; they will play straight (undo restores it)."
    );
}

/// Every Grooves-tab context-menu action through the real handler.
#[test]
fn grooves_tab_menu_actions_through_the_real_handler() {
    let user_dir = tempfile::tempdir().expect("user groove dir");
    let factory_dir = sequencer::app_paths::app_paths().grooves_dir();
    override_groove_library_dirs_for_tests(Some((
        factory_dir.clone(),
        user_dir.path().to_path_buf(),
    )));
    let _library_guard = LibraryGuard;

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

    // Setup (not under test): two drum racks, A (kick on C1, a closed hat on
    // F#1 — standard-layout roles) and B; a played take on A extracted into
    // the pool and played by BOTH racks.
    let (kick_c1, hat_fs1) = (
        sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE,
        sequencer::sequencer::DRUM_RACK_FIRST_PAD_NOTE + 6,
    );
    for _ in 0..4 {
        app.graph_controller()
            .add_blank_sampler_track()
            .expect("sampler track");
    }
    let (rack_a, _) = app.create_drum_rack_recorded(None).expect("rack A");
    app.assign_rack_pad_track_recorded(rack_a, kick_c1, 0)
        .expect("kick");
    app.assign_rack_pad_track_recorded(rack_a, hat_fs1, 1)
        .expect("hat");
    let (rack_b, _) = app.create_drum_rack_recorded(None).expect("rack B");
    app.assign_rack_pad_track_recorded(rack_b, kick_c1, 2)
        .expect("B kick");
    app.assign_rack_pad_track_recorded(rack_b, hat_fs1, 3)
        .expect("B hat");
    for (track, step, delay) in [
        (0usize, 0usize, 0.02f32),
        (0, 8, 0.1),
        (1, 2, 0.3),
        (1, 6, 0.26),
    ] {
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
    let take = app
        .extract_rack_groove_recorded(
            rack_a,
            &sequencer::app::rack_grooves::RackGrooveExtractRequest {
                options: sequencer::groove::GrooveExtractOptions {
                    name: "Take".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("extract");
    app.set_rack_active_groove_recorded(rack_b, Some(take))
        .expect("B plays the take");
    let rack_name = |app: &app::App, id: u64| {
        app.groups
            .iter()
            .find(|group| group.id == id)
            .unwrap()
            .name
            .clone()
    };
    let (name_a, name_b) = (rack_name(&app, rack_a), rack_name(&app, rack_b));
    let active = |app: &app::App, id: u64| {
        app.groups
            .iter()
            .find(|group| group.id == id)
            .and_then(|group| group.rack.as_ref())
            .unwrap()
            .groove
            .active
    };

    // The production UI runtime with the real modules.
    let mut runtime = Runtime::new();
    runtime.register_reactive("SEQ", Vec::new(), true);
    let mut editor = Editor::new(runtime, eseqlisp::EditorConfig::default());
    let paths = sequencer::app_paths::app_paths();
    let (roots, errors) = paths.module_load_roots();
    assert!(errors.is_empty(), "{errors:?}");
    editor.runtime_mut().set_load_root(paths.factory_root());
    editor.runtime_mut().set_scoped_module_load_path(roots);
    register_groove_tab_natives(editor.runtime_mut());
    for native in ["seq-clear-selection", "seq-clear-delete-target"] {
        editor
            .runtime_mut()
            .register_native(native, |_args, _ctx| Ok(Value::Nil));
    }
    let publish = |app: &app::App, editor: &mut Editor| {
        let rt = editor.runtime_mut();
        sync_groups_bindings(rt, &app.groups, &app.grooves);
        sync_bus_mixer_control_state(rt, app);
        rt.set_reactive("SEQ", "num-tracks", Value::Number(app.tracks.len() as f64));
        rt.set_reactive("SEQ", "current-track", Value::Number(0.0));
        rt.run_reactive_cycle();
    };
    publish(&app, &mut editor);
    editor
        .runtime_mut()
        .eval_str(
            "(import eseq.drum-rack-v2) (import eseq.file-dialogs) (import eseq.grooves-tab)
             (def test-flat (rows)
               (reduce |acc row|
                 (append (append acc (list row)) (test-flat (or (get row :children) (list))))
                 (list) rows))
             (def test-row (path)
               (nth (filter (lambda (row) (= (get row :path) path))
                      (test-flat (seq-groove-tree \"\" SEQ.groove-pool SEQ.groove-library)))
                    0))
             (def test-menu (path)
               (do (eseq.grooves-tab/open-menu (dict :item (test-row path) :col 1 :row 1))
                   (map (lambda (chosen) (get chosen :label)) (eseq.grooves-tab/menu-actions))))
             (def test-pick (path label)
               (do (eseq.grooves-tab/open-menu (dict :item (test-row path) :col 1 :row 1))
                   (eseq.grooves-tab/select-menu-action
                     (nth (filter (lambda (chosen) (= (get chosen :label) label))
                            (eseq.grooves-tab/menu-actions)) 0))))",
        )
        .expect("load the Grooves tab");

    let sample_db = sequencer::sample_db::SampleDb::open_in_memory().expect("sample db");
    let shared = SharedHandles {
        state: state.clone(),
        lg_raw: app.graph.lg.0,
        current_track: Arc::new(AtomicUsize::new(0)),
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
        record_armed: Arc::new(Mutex::new(vec![false; 4])),
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
        cached_track_peak_levels: vec![0.0; 4],
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

    // Evaluates UI Lisp, routes every host command it emits through the
    // production dispatcher (then republishes the mixer fields the harness,
    // not the dispatcher, owns). Returns the command names.
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
        publish(app, editor);
        names
    };
    let eval = |editor: &mut Editor, source: &str| -> Value {
        editor
            .runtime_mut()
            .eval_str(source)
            .unwrap_or_else(|e| panic!("{source}: {e:?}"))
            .unwrap_or(Value::Nil)
    };
    let strings = |value: Value| -> Vec<String> { list(&value).iter().map(string).collect() };
    let bus_of = |app: &app::App, id: u64| -> f64 {
        let bus = app.groups.iter().find(|g| g.id == id).unwrap().bus_id;
        app.buses.iter().position(|b| b.id.0 == bus).unwrap() as f64
    };
    let select_rack = |app: &app::App, editor: &mut Editor, id: u64| {
        let bus = bus_of(app, id);
        eval(
            editor,
            &format!("(set! eseq.seq-core-state/selected-bus {bus})"),
        );
    };
    let take_path = format!("project/pool:{take}");

    // ── The tree and its menus ──
    let tree = list(&eval(
        &mut editor,
        "(seq-groove-tree \"\" SEQ.groove-pool SEQ.groove-library)",
    ));
    assert_eq!(string(&get(&tree[0], "label")), "In use");
    assert_eq!(string(&get(&tree[1], "label")), "Take");
    let instances = list(&get(&tree[1], "children"));
    assert_eq!(
        labels(&instances),
        [
            format!("{name_a} · T100 V0 R0"),
            format!("{name_b} · T100 V0 R0")
        ]
    );
    assert_eq!(
        strings(eval(&mut editor, &format!("(test-menu \"{take_path}\")"))),
        [
            "Apply to Selected Rack",
            "Rename",
            "Duplicate",
            "Save to Library",
            "Delete"
        ]
    );
    assert_eq!(
        strings(eval(
            &mut editor,
            "(test-menu \"library/factory:mpc-swing-58-16th\")"
        )),
        ["Apply to Selected Rack"],
        "factory files are read-only"
    );
    assert_eq!(
        strings(eval(
            &mut editor,
            &format!("(test-menu \"{take_path}/rack:{rack_b}\")")
        )),
        ["Show Rack".to_string(), format!("Turn Off on {name_b}")]
    );

    // Selecting a pool groove previews its heatmap: All, then one row per
    // recorded pad, labelled by role.
    eval(
        &mut editor,
        &format!("(eseq.grooves-tab/select-item (test-row \"{take_path}\"))"),
    );
    assert_eq!(
        string(&eval(&mut editor, "eseq.grooves-tab/selected-key")),
        format!("pool:{take}")
    );
    let pool_field = list(
        &editor
            .runtime()
            .reactive_field_value("SEQ", "groove-pool")
            .cloned()
            .unwrap(),
    );
    let heat = get(&pool_field[0], "heatmap");
    assert_eq!(
        labels(&list(&get(&heat, "rows"))),
        ["All", "Kick", "Closed Hat"]
    );
    assert_eq!(string(&get(&heat, "grid")), "1 bar · 1/16");

    // ── Duplicate ──
    let undo = app.history.undo_len();
    let sent = ui(
        &format!("(test-pick \"{take_path}\" \"Duplicate\")"),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, ["duplicate-pool-groove"]);
    assert_eq!(app.grooves.len(), 2);
    assert_eq!(app.grooves[1].name, "Take copy");
    assert_eq!(
        (&app.grooves[1].pad_rows, &app.grooves[1].shared_row),
        (&app.grooves[0].pad_rows, &app.grooves[0].shared_row),
        "the copy has the original's rows"
    );
    assert_eq!(app.history.undo_len(), undo + 1, "one undo step");
    let copy = app.grooves[1].id;
    let copy_path = format!("project/pool:{copy}");

    // ── Apply to Selected Rack (pool) ── rack B selected by its bus.
    select_rack(&app, &mut editor, rack_b);
    let sent = ui(
        &format!("(test-pick \"{copy_path}\" \"Apply to Selected Rack\")"),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, ["set-rack-groove"]);
    assert_eq!(active(&app, rack_b), Some(copy));
    assert_eq!(active(&app, rack_a), Some(take), "only the selected rack");

    // ── Rename (pool) ── inline field, committed as one undo step.
    let undo = app.history.undo_len();
    assert!(ui(
        &format!("(test-pick \"{copy_path}\" \"Rename\")"),
        &mut app,
        &mut editor
    )
    .is_empty());
    assert_eq!(
        string(&eval(&mut editor, "eseq.grooves-tab/rename-draft")),
        "Take copy"
    );
    let sent = ui(
        "(set! eseq.grooves-tab/rename-draft \"Swingy\") (eseq.grooves-tab/commit-rename)",
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, ["rename-rack-groove"]);
    assert_eq!(app.grooves[1].name, "Swingy");
    assert_eq!(app.history.undo_len(), undo + 1);

    // ── Save to Library ── a user file, the project unchanged.
    let undo = app.history.undo_len();
    let sent = ui(
        &format!("(test-pick \"{copy_path}\" \"Save to Library\")"),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, ["save-groove-to-library"]);
    assert_eq!(
        app.history.undo_len(),
        undo,
        "saving a file is not a project edit"
    );
    let saved = user_dir.path().join("Swingy.groove");
    assert!(saved.is_file(), "saved into the user library");
    let user_path = "library/user:Swingy";
    assert_eq!(
        strings(eval(&mut editor, &format!("(test-menu \"{user_path}\")"))),
        ["Apply to Selected Rack", "Rename", "Delete"]
    );

    // ── Rename (user library file) ──
    assert!(ui(
        &format!("(test-pick \"{user_path}\" \"Rename\")"),
        &mut app,
        &mut editor
    )
    .is_empty());
    let sent = ui(
        "(set! eseq.grooves-tab/rename-draft \"Lazy\") (eseq.grooves-tab/commit-rename)",
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, ["rename-library-groove"]);
    assert!(!saved.exists() && user_dir.path().join("Lazy.groove").is_file());
    let user_path = "library/user:Lazy";

    // ── Apply to Selected Rack (library, copy-on-apply) ── rack A, selected
    // through its member track this time (no bus selected). The file (now
    // "Lazy") is copied into the pool and the rack points at the copy.
    eval(&mut editor, "(set! eseq.seq-core-state/selected-bus -1)");
    let sent = ui(
        &format!("(test-pick \"{user_path}\" \"Apply to Selected Rack\")"),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, ["set-rack-groove"]);
    assert_eq!(app.grooves.len(), 3);
    assert_eq!(app.grooves[2].name, "Lazy");
    assert_eq!(active(&app, rack_a), Some(app.grooves[2].id));
    assert_eq!(active(&app, rack_b), Some(copy), "only the selected rack");
    // Applying the same file again reuses that pool copy (same feel).
    ui(
        &format!("(test-pick \"{user_path}\" \"Apply to Selected Rack\")"),
        &mut app,
        &mut editor,
    );
    assert_eq!(app.grooves.len(), 3, "same feel: no second copy");
    // A factory swing is new to the project: it is copied into the pool.
    let sent = ui(
        "(test-pick \"library/factory:mpc-swing-58-16th\" \"Apply to Selected Rack\")",
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, ["set-rack-groove"]);
    assert_eq!(app.grooves.len(), 4);
    assert_eq!(active(&app, rack_a), Some(app.grooves[3].id));
    // Its preview loads from the file.
    eval(
        &mut editor,
        "(eseq.grooves-tab/select-item (test-row \"library/factory:mpc-swing-58-16th\"))",
    );
    assert_eq!(
        labels(&list(&get(
            &eval(&mut editor, "eseq.grooves-tab/library-heat"),
            "rows"
        ))),
        ["All"]
    );

    // ── Delete (user library file) ── always confirms; not undoable.
    let sent = ui(
        &format!("(test-pick \"{user_path}\" \"Delete\")"),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, ["delete-library-groove"]);
    assert_eq!(
        eval(&mut editor, "eseq.file-dialogs/confirm-open?"),
        Value::Bool(true)
    );
    let message = string(&eval(&mut editor, "eseq.file-dialogs/confirm-message"));
    assert!(
        message.contains("'Lazy'") && message.contains("cannot be undone"),
        "{message}"
    );
    assert!(
        user_dir.path().join("Lazy.groove").is_file(),
        "nothing deleted before the confirm"
    );
    let sent = ui("(eseq.file-dialogs/accept-confirm)", &mut app, &mut editor);
    assert_eq!(sent, ["delete-library-groove"]);
    assert!(!user_dir.path().join("Lazy.groove").exists());

    // ── Delete (pool groove nobody plays) ── at once, one undo step.
    assert_eq!(app.racks_using_groove(take), Vec::<u64>::new());
    let undo = app.history.undo_len();
    let sent = ui(
        &format!("(test-pick \"{take_path}\" \"Delete\")"),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, ["delete-rack-groove"]);
    assert_eq!(
        eval(&mut editor, "eseq.file-dialogs/confirm-open?"),
        Value::Bool(false)
    );
    assert!(app.grooves.iter().all(|groove| groove.id != take));
    assert_eq!(app.history.undo_len(), undo + 1);

    // ── Delete (pool groove racks play) ── confirms, listing both racks;
    // accepting turns it off on both, as ONE undo step.
    let sent = ui(
        &format!("(test-pick \"{copy_path}\" \"Apply to Selected Rack\")"),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, ["set-rack-groove"]);
    assert_eq!(app.racks_using_groove(copy), vec![rack_a, rack_b]);
    let undo = app.history.undo_len();
    let sent = ui(
        &format!("(test-pick \"{copy_path}\" \"Delete\")"),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, ["delete-rack-groove"]);
    assert!(
        app.grooves.iter().any(|groove| groove.id == copy),
        "asks first"
    );
    let message = string(&eval(&mut editor, "eseq.file-dialogs/confirm-message"));
    assert_eq!(
        message,
        format!(
            "Delete groove 'Swingy'? {name_a} and {name_b} play it; they will play straight (undo restores it)."
        )
    );
    let sent = ui("(eseq.file-dialogs/accept-confirm)", &mut app, &mut editor);
    assert_eq!(sent, ["delete-rack-groove"]);
    assert!(app.grooves.iter().all(|groove| groove.id != copy));
    assert_eq!((active(&app, rack_a), active(&app, rack_b)), (None, None));
    assert_eq!(app.history.undo_len(), undo + 1, "one undo step");
    assert!(matches!(
        app::edit::undo(&mut app),
        app::history::HistoryReplay::Applied(_)
    ));
    assert_eq!(
        (active(&app, rack_a), active(&app, rack_b)),
        (Some(copy), Some(copy)),
        "undo brings the groove back on both racks"
    );
    publish(&app, &mut editor);

    // ── Instance rows ── a click focuses the rack; Show Rack too; Turn Off
    // stops that rack only.
    let instance_b = format!("project/pool:{copy}/rack:{rack_b}");
    eval(&mut editor, "(set! eseq.seq-core-state/selected-bus -1)");
    eval(
        &mut editor,
        &format!("(eseq.grooves-tab/select-item (test-row \"{instance_b}\"))"),
    );
    assert_eq!(
        eval(&mut editor, "eseq.seq-core-state/selected-bus"),
        Value::Number(bus_of(&app, rack_b)),
        "clicking an instance focuses its rack"
    );
    let instance_a = format!("project/pool:{copy}/rack:{rack_a}");
    assert!(ui(
        &format!("(test-pick \"{instance_a}\" \"Show Rack\")"),
        &mut app,
        &mut editor
    )
    .is_empty());
    assert_eq!(
        eval(&mut editor, "eseq.seq-core-state/selected-bus"),
        Value::Number(bus_of(&app, rack_a))
    );
    let sent = ui(
        &format!("(test-pick \"{instance_a}\" \"Turn Off on {name_a}\")"),
        &mut app,
        &mut editor,
    );
    assert_eq!(sent, ["set-rack-groove"]);
    assert_eq!(
        (active(&app, rack_a), active(&app, rack_b)),
        (None, Some(copy))
    );

    // ── The panel lays out: toolbar, tree (instances expanded) and the
    // selected groove's heatmap with one row per heat row.
    eval(
        &mut editor,
        &format!(
            "(set! eseq.grooves-tab/expand-instances true)
             (eseq.grooves-tab/select-item (test-row \"{copy_path}\"))
             (effect-buffer \"*grooves-test*\" (eseq.grooves-tab/panel \"\"))"
        ),
    );
    editor.set_layout_viewport(60, 60);
    editor.refresh_runtime_side_effects();
    let buffer = editor
        .buffers
        .iter()
        .find(|b| b.name == "*grooves-test*")
        .unwrap()
        .id;
    editor.set_active_buffer(buffer);
    editor.refresh_runtime_side_effects();
    let layout = editor.widget_layout().expect("grooves tab layout");
    for name in [
        "grooves-tab-tree",
        "groove-tab-preview",
        "groove-tab-heatmap",
    ] {
        let node = find_debug(&layout, name).unwrap_or_else(|| panic!("{name}"));
        let rect = &node.rect;
        assert!(
            [rect.col, rect.row, rect.width, rect.height]
                .iter()
                .all(|v| v.is_finite())
                && rect.width > 0.0
                && rect.height > 0.0,
            "{name}: {rect:?}"
        );
    }
    let heatmap = find_debug(&layout, "groove-tab-heatmap").unwrap();
    // All + Kick + Closed Hat rows, then the beat ruler.
    assert_eq!(heatmap.children.len(), 4);
    let tree = find_debug(&layout, "grooves-tab-tree").unwrap();
    assert_eq!(tree.props.get("expand-all"), Some(&Value::Bool(true)));
    for handler in ["on-select", "on-activate", "on-right-click"] {
        assert!(tree.props.contains_key(handler), "tree needs {handler}");
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
