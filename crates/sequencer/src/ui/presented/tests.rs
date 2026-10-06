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
            e.error = "bad".to_string();
            e.canceling = true;
        },
        legacy::mirror_editor,
    );
    assert_eq!(writes.names(), vec!["editor-error", "editor-canceling"]);
    let mut writes = Writes::default();
    present(
        &mut writes,
        |p| &mut p.editor,
        |e| e.canceling = true,
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
        (
            "editor-buffer-name",
            s("*instrument-patcher:new-instrument*"),
        ),
        ("editor-instrument-run-mode", s("free_patch")),
        ("editor-surface", s("patch")),
    ] {
        assert_eq!(legacy(&rt, "SEQ", field), value, "{field}");
    }
    present_editor(&mut rt, |e| e.canceling = true);
    present_editor_closed(&mut rt);
    for (field, value) in [
        ("editor-active", Value::Bool(false)),
        ("editor-canceling", Value::Bool(false)),
        ("editor-mode", s("")),
        ("editor-error", s("")),
        ("editor-buffer-name", s("")),
        ("editor-instrument-run-mode", s("instrument")),
        // The surface is kept for the next open.
        ("editor-surface", s("patch")),
    ] {
        assert_eq!(legacy(&rt, "SEQ", field), value, "{field}");
    }
    // The macro sidebar's rows, as `build_*_sidebar_value` built them.
    present_editor_sidebar(&mut rt, |sidebar| {
        sidebar.patch_macros = vec![EditorMacro {
            name: "lfo".to_string(),
            params: vec!["rate".to_string()],
            ..Default::default()
        }];
        sidebar.library_macros = vec![EditorMacro {
            name: "env".to_string(),
            outputs: vec!["out".to_string()],
            summary: "An envelope".to_string(),
            used: true,
            ..Default::default()
        }];
        sidebar.assets = vec![EditorAsset {
            reference: "tables/saw".to_string(),
            tier: "factory".to_string(),
            source_path: "/f/saw.json".to_string(),
        }];
        sidebar.selected_asset = Some(AssetInfo {
            reference: "tables/missing".to_string(),
            metadata: None,
        });
    });
    let patch = rows(legacy(&rt, "SEQ", "editor-patch-macros"));
    assert_eq!(get(&patch[0], "name"), s("lfo"));
    assert_eq!(rows(get(&patch[0], "params")), vec![s("rate")]);
    let Value::Map(row) = &patch[0] else { panic!() };
    assert_eq!(row.len(), 3, "a patch macro is name, params and calls");
    let library = rows(legacy(&rt, "SEQ", "editor-library-macros"));
    assert_eq!(get(&library[0], "summary"), s("An envelope"));
    assert_eq!(get(&library[0], "used"), Value::Bool(true));
    assert_eq!(rows(get(&library[0], "outputs")), vec![s("out")]);
    let assets = rows(legacy(&rt, "SEQ", "editor-assets"));
    for (key, value) in [
        ("label", s("tables/saw")),
        ("name", s("tables/saw")),
        ("kind", s("patcher-asset")),
        ("detail", s("factory")),
        ("tier", s("factory")),
        ("file", s("tables/saw")),
        ("source-path", s("/f/saw.json")),
        ("drag-type", s("dgen-asset")),
        ("draggable", Value::Bool(true)),
        ("drop-target", Value::Bool(false)),
    ] {
        assert_eq!(get(&assets[0], key), value, "{key}");
    }
    // An unresolvable asset: its reference alone.
    let selected = legacy(&rt, "SEQ", "editor-selected-asset");
    assert_eq!(get(&selected, "reference"), s("tables/missing"));
    let Value::Map(selected) = selected else {
        panic!()
    };
    assert_eq!(selected.len(), 1);
    present_editor_sidebar(&mut rt, |sidebar| sidebar.selected_asset = None);
    assert_eq!(legacy(&rt, "SEQ", "editor-selected-asset"), Value::Nil);
}

#[test]
fn learn_mirror_matches_the_legacy_publishers() {
    let mut rt = runtime();
    present_learn(&mut rt, |l| {
        l.phase = "training".to_string();
        l.plan_params = vec![LearnPlanParam {
            name: "cutoff".to_string(),
            status: "frozen".to_string(),
            reason: "noise".to_string(),
        }];
        l.epoch_params = vec![LearnEpochParam {
            name: "cutoff".to_string(),
            from: 1.0,
            value: 3.0,
            change: 2.0,
            step: 0.5,
        }];
        l.result_deltas = vec![LearnDelta {
            name: "q".to_string(),
            from: 0.5,
            to: 0.75,
            change: 0.25,
        }];
        l.losses = vec![0.5, 0.25];
        l.checkpoint_wav = "/tmp/c.wav".to_string();
    });
    assert_eq!(legacy(&rt, "SEQ", "learn-phase"), s("training"));
    let plan = rows(legacy(&rt, "SEQ", "learn-plan-params"));
    assert_eq!(get(&plan[0], "status"), s("frozen"));
    assert_eq!(get(&plan[0], "reason"), s("noise"));
    let epoch = rows(legacy(&rt, "SEQ", "learn-epoch-params"));
    for (key, value) in [
        ("from", 1.0),
        ("value", 3.0),
        ("change", 2.0),
        ("step", 0.5),
    ] {
        assert_eq!(get(&epoch[0], key), Value::Number(value), "{key}");
    }
    let deltas = rows(legacy(&rt, "SEQ", "learn-result-deltas"));
    assert_eq!(get(&deltas[0], "to"), Value::Number(0.75));
    assert_eq!(
        rows(legacy(&rt, "SEQ", "learn-losses")),
        vec![Value::Number(0.5), Value::Number(0.25)]
    );
    assert_eq!(legacy(&rt, "SEQ", "learn-checkpoint-wav"), s("/tmp/c.wav"));
    // An error, then a reset: the settings and target stay.
    present_learn(&mut rt, |l| {
        l.epochs = 700.0;
        l.target_name = "Kick".to_string();
    });
    present_learn_error(&mut rt, "boom");
    assert_eq!(legacy(&rt, "SEQ", "learn-phase"), s("error"));
    assert_eq!(legacy(&rt, "SEQ", "learn-error"), s("boom"));
    present_learn(&mut rt, LearnView::reset);
    assert_eq!(legacy(&rt, "SEQ", "learn-phase"), s("pick"));
    assert_eq!(legacy(&rt, "SEQ", "learn-error"), s(""));
    assert_eq!(rows(legacy(&rt, "SEQ", "learn-plan-params")), vec![]);
    assert_eq!(legacy(&rt, "SEQ", "learn-checkpoint-wav"), s(""));
    assert_eq!(legacy(&rt, "SEQ", "learn-epochs"), Value::Number(700.0));
    assert_eq!(legacy(&rt, "SEQ", "learn-target-name"), s("Kick"));
}

#[test]
fn export_settings_and_agent_mirrors_match_the_legacy_publishers() {
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
        |p| &mut p.settings,
        |st| {
            st.workers_choice = "Auto (6)".to_string();
            st.workers_options = vec!["Auto (6)".to_string(), "1".to_string()];
            st.midi_devices = vec![MidiDevice {
                id: "a".to_string(),
                name: "Keys".to_string(),
                enabled: true,
                connected: false,
                status: "Disconnected".to_string(),
            }];
            st.midi_error = "boom".to_string();
        },
        legacy::mirror_settings,
    );
    assert_eq!(writes.get("AUDIO", "workers-choice"), s("Auto (6)"));
    assert_eq!(
        rows(writes.get("AUDIO", "workers-options")),
        vec![s("Auto (6)"), s("1")]
    );
    let devices = rows(writes.get("MIDI", "devices"));
    for (key, value) in [
        ("id", s("a")),
        ("name", s("Keys")),
        ("enabled", Value::Bool(true)),
        ("connected", Value::Bool(false)),
        ("status", s("Disconnected")),
    ] {
        assert_eq!(get(&devices[0], key), value, "{key}");
    }
    assert_eq!(writes.get("MIDI", "error"), s("boom"));
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
        ("editor-instrument-run-mode", s("instrument")),
        ("editor-selected-asset", Value::Nil),
        ("learn-phase", s("pick")),
        ("learn-method", s(LEARN_METHODS[0])),
        ("learn-epochs", Value::Number(300.0)),
        ("learn-cma-sigma", Value::Number(0.2)),
        ("learn-cma-refine-mode", s("Batched")),
        ("learn-checkpoint-wav", s("")),
    ] {
        assert_eq!(seq.get(field), Some(&value), "{field}");
    }
    let export: HashMap<&str, Value> = export_registration().into_iter().collect();
    assert_eq!(export.get("export-percent"), Some(&Value::Number(-1.0)));
    // A seeded value registers as seeded.
    seed_midi_persistent(true);
    let (_, midi) = settings_registration();
    assert!(midi.contains(&("persistent", Value::Bool(true))));
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
    let error = fixture::present_fixture(&mut writes, "song-export", &map_value([("nope", s(""))]));
    assert!(error.unwrap_err().contains("no field nope"));
}
