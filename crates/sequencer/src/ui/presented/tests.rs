//! The record's mutators and the capture fixtures' seeding.

use super::*;

fn s(text: &str) -> Value {
    Value::String(text.to_string())
}

#[test]
fn typed_mutators_move_the_generation_only_on_change() {
    let mut rt = Runtime::new();
    let generation = || presented(|p| p.editor.generation());
    let start = generation();
    assert!(present_editor(&mut rt, |e| e.error = "boom".to_string()));
    assert_eq!(generation(), start + 1);
    // The same value again: no change, no move.
    assert!(!present_editor(&mut rt, |e| e.error = "boom".to_string()));
    assert_eq!(generation(), start + 1);
    // Each area moves alone.
    let learn = presented(|p| p.learn.generation());
    let export = presented(|p| p.export.generation());
    assert!(present_learn(&mut rt, |l| l.epochs = 500.0));
    assert_eq!(presented(|p| p.learn.generation()), learn + 1);
    assert_eq!(presented(|p| p.export.generation()), export);
    assert_eq!(generation(), start + 1);
    assert!(!present_export(&mut rt, |x| x.percent = -1.0));
    assert!(present_agent(&mut rt, 3));
    assert!(!present_agent(&mut rt, 3));
    let promote = presented(|p| p.promote.generation());
    assert!(present_promote(|x| x.taken = "Kit".to_string()));
    assert!(!present_promote(|x| x.taken = "Kit".to_string()));
    assert_eq!(presented(|p| p.promote.generation()), promote + 1);
}

#[test]
fn editor_sessions_open_and_close_in_the_record() {
    let mut rt = Runtime::new();
    present_editor_open(
        &mut rt,
        "new-instrument",
        "*instrument-patcher:new-instrument*",
        Some(CustomInstrumentRunMode::FreePatch),
        EditorSurface::Patch,
    );
    let editor = presented(|p| p.editor.get().clone());
    assert_eq!(editor.mode, "new-instrument");
    assert_eq!(editor.buffer, "*instrument-patcher:new-instrument*");
    assert_eq!(editor.run_mode, "free_patch");
    assert_eq!(editor.surface, "patch");
    assert!(instrument_editor_open());
    present_editor(&mut rt, |e| e.canceling = true);
    present_editor_closed(&mut rt);
    let editor = presented(|p| p.editor.get().clone());
    assert_eq!(editor.mode, "");
    assert!(!editor.canceling);
    assert_eq!(editor.error, "");
    assert_eq!(editor.buffer, "");
    assert_eq!(editor.run_mode, "instrument");
    // The surface is kept for the next open.
    assert_eq!(editor.surface, "patch");
    assert!(!instrument_editor_open());
    // An effect session is no instrument editor.
    present_editor(&mut rt, |e| "edit-effect".clone_into(&mut e.mode));
    assert!(!instrument_editor_open());
    present_editor(&mut rt, |e| "edit-instrument".clone_into(&mut e.mode));
    assert!(instrument_editor_open());
    present_editor_closed(&mut rt);
    present_editor_sidebar(&mut rt, |sidebar| {
        sidebar.patch_macros = vec![EditorMacro {
            name: "lfo".to_string(),
            ..Default::default()
        }];
    });
    let sidebar = presented(|p| p.editor_sidebar.get().clone());
    assert_eq!(sidebar.patch_macros[0].name, "lfo");
}

/// An error, then a reset, keep Patch Learn's settings and target.
#[test]
fn learn_errors_and_resets_keep_the_settings_and_target() {
    let mut rt = Runtime::new();
    present_learn(&mut rt, |l| {
        l.phase = "training".to_string();
        l.plan_params = vec![LearnPlanParam {
            name: "cutoff".to_string(),
            status: "frozen".to_string(),
            reason: "noise".to_string(),
        }];
        l.epochs = 700.0;
        l.target_name = "Kick".to_string();
    });
    present_learn_error(&mut rt, "boom");
    let learn = presented(|p| p.learn.get().clone());
    assert_eq!(
        (learn.phase.as_str(), learn.error.as_str()),
        ("error", "boom")
    );
    present_learn(&mut rt, LearnView::reset);
    let learn = presented(|p| p.learn.get().clone());
    assert_eq!((learn.phase.as_str(), learn.error.as_str()), ("pick", ""));
    assert!(learn.plan_params.is_empty());
    assert_eq!(learn.epochs, 700.0);
    assert_eq!(learn.target_name, "Kick");
}

#[test]
fn a_seeded_value_moves_no_generation() {
    let generation = presented(|p| p.settings.generation());
    seed_midi_persistent(true);
    assert!(presented(|p| p.settings.get().midi_persistent));
    assert_eq!(presented(|p| p.settings.generation()), generation);
}

#[test]
fn a_fixture_seeds_the_record() {
    let fields = map_value([
        ("default-name", s("Night Drive (2)")),
        ("busy", Value::Bool(true)),
        ("percent", Value::Number(37.0)),
    ]);
    fixture::present_fixture("song-export", &fields).unwrap();
    let export = presented(|p| p.export.get().clone());
    assert_eq!(export.default_name, "Night Drive (2)");
    assert!(export.busy);
    assert_eq!(export.percent, 37.0);
    let devices = list_value([map_value([
        ("device-id", s("keys")),
        ("name", s("Keys")),
        ("enabled", Value::Bool(true)),
        ("connected", Value::Bool(true)),
        ("status", s("Connected")),
    ])]);
    let fields = map_value([("midi-devices", devices)]);
    fixture::present_fixture("settings", &fields).unwrap();
    assert_eq!(
        presented(|p| p.settings.get().midi_devices[0].id.clone()),
        "keys"
    );
    let plan = list_value([map_value([
        ("name", s("cutoff")),
        ("status", s("learnable")),
        ("reason", s("")),
    ])]);
    let fields = map_value([("phase", s("configure")), ("plan-params", plan)]);
    fixture::present_fixture("learn", &fields).unwrap();
    let learn = presented(|p| p.learn.get().clone());
    assert_eq!(learn.phase, "configure");
    assert_eq!(learn.plan_params[0].status, "learnable");
    let macros = list_value([map_value([("name", s("osc")), ("calls", list_value([]))])]);
    let asset = map_value([
        ("reference", s("waves/basic")),
        (
            "shape",
            list_value([Value::Number(4.0), Value::Number(2048.0)]),
        ),
        ("waves-per-set", Value::Number(2.0)),
    ]);
    let fields = map_value([
        ("open-macro", s("osc")),
        ("patch-macros", macros),
        ("selected-asset", asset),
    ]);
    fixture::present_fixture("editor", &fields).unwrap();
    assert_eq!(presented(|p| p.editor.get().open_macro.clone()), "osc");
    let sidebar = presented(|p| p.editor_sidebar.get().clone());
    assert_eq!(sidebar.patch_macros[0].name, "osc");
    let metadata = sidebar
        .selected_asset
        .and_then(|asset| asset.metadata)
        .unwrap();
    assert_eq!(
        (metadata.shape, metadata.waves_per_set),
        (vec![4, 2048], Some(2))
    );
    let fields = map_value([
        ("target", s("kit")),
        ("skipped", list_value([s("pad 'Kick': skipped")])),
        ("taken", s("house kit")),
    ]);
    fixture::present_fixture("factory-promote", &fields).unwrap();
    let promote = presented(|p| p.promote.get().clone());
    assert_eq!(promote.target, "kit");
    assert_eq!(promote.skipped, vec!["pad 'Kick': skipped".to_string()]);
    assert_eq!(promote.taken, "house kit");
    let error = fixture::present_fixture("song-export", &map_value([("nope", s(""))]));
    assert!(error.unwrap_err().contains("no field nope"));
}
