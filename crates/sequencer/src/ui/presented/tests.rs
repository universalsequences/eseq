//! The record's mutators and its legacy mirror.

use super::*;

/// A sink recording the legacy writes.
#[derive(Default)]
struct Writes(Vec<(&'static str, &'static str, Value)>);

impl legacy::Sink for Writes {
    fn write(&mut self, namespace: &'static str, field: &'static str, value: Value) {
        self.0.push((namespace, field, value));
    }
}

impl Writes {
    fn names(&self) -> Vec<&'static str> {
        self.0.iter().map(|(_, field, _)| *field).collect()
    }

    fn get(&self, namespace: &str, field: &str) -> Value {
        (self.0.iter())
            .rev()
            .find(|(ns, name, _)| *ns == namespace && *name == field)
            .map(|(_, _, value)| value.clone())
            .unwrap_or_else(|| panic!("no write of {namespace}.{field}"))
    }
}

/// A runtime with the record's `SEQ` fields registered, as `init_runtime`
/// registers them.
fn runtime() -> Runtime {
    let mut rt = Runtime::new();
    rt.register_reactive("SEQ", seq_registration(), true);
    rt
}

fn s(text: &str) -> Value {
    Value::String(text.to_string())
}

/// A legacy namespace field as the runtime holds it.
fn legacy(rt: &Runtime, namespace: &str, field: &str) -> Value {
    let Some(Value::Map(map)) = rt.global_value(namespace) else {
        panic!("{namespace} should be a map");
    };
    let value = map[field].borrow().clone();
    value
}

fn get(map: &Value, key: &str) -> Value {
    let Value::Map(map) = map else {
        panic!("{map:?} is no map");
    };
    map[key].borrow().clone()
}

fn rows(value: Value) -> Vec<Value> {
    let Value::List(items) = value else {
        panic!("{value:?} is no list");
    };
    items.iter().map(|item| item.borrow().clone()).collect()
}

#[test]
fn typed_mutators_move_the_generation_only_on_change() {
    let mut rt = Runtime::new();
    let generation = || presented(|p| p.editor.generation());
    let start = generation();
    assert!(present_editor(&mut rt, |e| e.error = "boom".to_string()));
    assert_eq!(generation(), start + 1);
    // The same value again: no change, no move, no write.
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
}

#[test]
fn the_mirror_writes_only_the_fields_that_changed() {
    let mut writes = Writes::default();
    present(
        &mut writes,
        |p| &mut p.editor,
        |e| {
            e.mode = "edit-effect".to_string();
            e.open_macro = "lfo".to_string();
        },
        legacy::mirror_editor,
    );
    assert_eq!(writes.names(), vec!["editor-mode"]);
    let mut writes = Writes::default();
    present(
        &mut writes,
        |p| &mut p.editor,
        |e| {
            // No legacy name mirrors the open macro or the error any more
            // (the patch macros sidebar and the browser read the kind).
            e.open_macro = "seq".to_string();
            e.error = "bad".to_string();
        },
        legacy::mirror_editor,
    );
    assert!(writes.0.is_empty(), "unchanged: {:?}", writes.names());
}

#[test]
fn editor_mirror_matches_the_legacy_publishers() {
    let mut rt = runtime();
    present_editor_open(
        &mut rt,
        "new-instrument",
        "*instrument-patcher:new-instrument*",
        Some(CustomInstrumentRunMode::FreePatch),
        EditorSurface::Patch,
    );
    for (field, value) in [
        ("editor-active", Value::Bool(true)),
        ("editor-mode", s("new-instrument")),
    ] {
        assert_eq!(legacy(&rt, "SEQ", field), value, "{field}");
    }
    let editor = presented(|p| p.editor.get().clone());
    assert_eq!(editor.buffer, "*instrument-patcher:new-instrument*");
    assert_eq!(editor.run_mode, "free_patch");
    assert_eq!(editor.surface, "patch");
    present_editor(&mut rt, |e| e.canceling = true);
    present_editor_closed(&mut rt);
    for (field, value) in [
        ("editor-active", Value::Bool(false)),
        ("editor-mode", s("")),
    ] {
        assert_eq!(legacy(&rt, "SEQ", field), value, "{field}");
    }
    let editor = presented(|p| p.editor.get().clone());
    assert!(!editor.canceling);
    assert_eq!(editor.error, "");
    assert_eq!(editor.buffer, "");
    assert_eq!(editor.run_mode, "instrument");
    // The surface is kept for the next open.
    assert_eq!(editor.surface, "patch");
    // The macro sidebar mirrors nothing (the patch macros sidebar reads
    // `editor.*`): the record alone holds it.
    present_editor_sidebar(&mut rt, |sidebar| {
        sidebar.patch_macros = vec![EditorMacro {
            name: "lfo".to_string(),
            ..Default::default()
        }];
    });
    let sidebar = presented(|p| p.editor_sidebar.get().clone());
    assert_eq!(sidebar.patch_macros[0].name, "lfo");
    let Some(Value::Map(seq)) = rt.global_value("SEQ") else {
        panic!("SEQ should be a map");
    };
    assert!(!seq.contains_key("editor-patch-macros"));
}

/// Patch Learn mirrors no legacy name (the pane reads `learn.*`): an error,
/// then a reset, keep the settings and the target in the record.
#[test]
fn learn_errors_and_resets_keep_the_settings_and_target() {
    let mut writes = Writes::default();
    present(
        &mut writes,
        |p| &mut p.learn,
        |l| {
            l.phase = "training".to_string();
            l.plan_params = vec![LearnPlanParam {
                name: "cutoff".to_string(),
                status: "frozen".to_string(),
                reason: "noise".to_string(),
            }];
            l.checkpoint_wav = "/tmp/c.wav".to_string();
            l.epochs = 700.0;
            l.target_name = "Kick".to_string();
        },
        legacy::unmirrored,
    );
    assert!(writes.0.is_empty(), "{:?}", writes.names());
    let mut rt = Runtime::new();
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
    assert_eq!(learn.checkpoint_wav, "");
    assert_eq!(learn.epochs, 700.0);
    assert_eq!(learn.target_name, "Kick");
}

#[test]
fn export_and_agent_mirrors_match_the_legacy_publishers() {
    let mut writes = Writes::default();
    present(
        &mut writes,
        |p| &mut p.export,
        |x| {
            x.busy = true;
            x.percent = 37.0;
            x.message = "Exporting".to_string();
        },
        legacy::mirror_export,
    );
    assert_eq!(writes.get("EXPORT", "export-busy"), Value::Bool(true));
    assert_eq!(writes.get("EXPORT", "export-percent"), Value::Number(37.0));
    assert_eq!(writes.get("EXPORT", "export-message"), s("Exporting"));
    let mut writes = Writes::default();
    present(
        &mut writes,
        |p| &mut p.agent,
        |a| *a = 9,
        legacy::mirror_agent,
    );
    assert_eq!(writes.get("AGENT", "generation"), Value::Number(9.0));
}

#[test]
fn registrations_derive_from_the_record() {
    let seq: HashMap<&str, Value> = seq_registration().into_iter().collect();
    for (field, value) in [
        ("editor-active", Value::Bool(false)),
        ("editor-mode", s("")),
    ] {
        assert_eq!(seq.get(field), Some(&value), "{field}");
    }
    assert_eq!(seq.len(), 2, "the editor's mode and session flag alone");
    let export: HashMap<&str, Value> = export_registration().into_iter().collect();
    assert_eq!(export.get("export-percent"), Some(&Value::Number(-1.0)));
    // A seeded value is the record's.
    seed_midi_persistent(true);
    assert!(presented(|p| p.settings.get().midi_persistent));
}

#[test]
fn a_fixture_seeds_the_record_and_its_mirror() {
    let mut writes = Writes::default();
    let fields = map_value([
        ("default-name", s("Night Drive (2)")),
        ("busy", Value::Bool(true)),
        ("percent", Value::Number(37.0)),
    ]);
    fixture::present_fixture(&mut writes, "song-export", &fields).unwrap();
    let export = presented(|p| p.export.get().clone());
    assert_eq!(export.default_name, "Night Drive (2)");
    assert!(export.busy);
    assert_eq!(writes.get("EXPORT", "export-percent"), Value::Number(37.0));
    let devices = list_value([map_value([
        ("device-id", s("keys")),
        ("name", s("Keys")),
        ("enabled", Value::Bool(true)),
        ("connected", Value::Bool(true)),
        ("status", s("Connected")),
    ])]);
    let fields = map_value([("midi-devices", devices)]);
    fixture::present_fixture(&mut writes, "settings", &fields).unwrap();
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
    fixture::present_fixture(&mut writes, "learn", &fields).unwrap();
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
    fixture::present_fixture(&mut writes, "editor", &fields).unwrap();
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
    assert!(
        writes.0.iter().all(|(ns, ..)| *ns == "EXPORT"),
        "{:?}",
        writes.names()
    );
    let error = fixture::present_fixture(&mut writes, "song-export", &map_value([("nope", s(""))]));
    assert!(error.unwrap_err().contains("no field nope"));
}
