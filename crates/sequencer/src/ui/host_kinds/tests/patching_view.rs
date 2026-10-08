//! The factory patching, macro, menu and settings views ported to the kinds
//! (kind-bindings spec §13 stage 8, eseq-0l17.18): Patch Learn, the patch
//! macros sidebar, the macro controls and the macro mapping table, the
//! application menus and Settings.

use super::views::{
    assert_ported, distro, instance_bindings, legacy_forms, tree_has_string_prop, widgets_with_prop,
};
use super::*;
use sequencer::sequencer::{RackMacroCurve, RackMacroId, RackMacroMapping, RackMacroTarget};

/// The ported views' sources.
const PORTED: [(&str, &str); 5] = [
    (
        "ui/settings.lisp",
        include_str!("../../../../../../content/ui/settings.lisp"),
    ),
    (
        "ui/patch-learn.lisp",
        include_str!("../../../../../../content/ui/patch-learn.lisp"),
    ),
    (
        "ui/patch-macros.lisp",
        include_str!("../../../../../../content/ui/patch-macros.lisp"),
    ),
    (
        "ui/macros.lisp",
        include_str!("../../../../../../content/ui/macros.lisp"),
    ),
    (
        "ui/application-menus.lisp",
        include_str!("../../../../../../content/ui/application-menus.lisp"),
    ),
];

const MACRO_STATE: &str = include_str!("../../../../../../content/ui/macro-state.lisp");

/// The Filter's `cutoff` and the sampler's `start` (a percent param:
/// stored 0–1, shown 0–100).
const CUTOFF: usize = 2;
const START: usize = 2;

#[test]
fn ported_patching_views_use_no_legacy_binding_forms() {
    assert_ported(&PORTED);
    // The arm state is a singleton (the rack macro name COMPAT read went
    // with the effects port, eseq-0l17.61: the rack panel reads rm.name).
    assert_eq!(legacy_forms(MACRO_STATE), Vec::<&str>::new());
}

impl Harness {
    fn eval_macros(&mut self, code: &str) -> Value {
        self.eval_with("(import eseq.kinds :refer (macros tracks selection))", code)
    }

    /// Arm the mapping table: project macro `mid`, or rack macro `rack`.
    fn arm(&mut self, mid: i64, rack: i64) {
        let code = if mid >= 0 {
            format!("(eseq.macro-state/arm-macro! {mid})")
        } else {
            format!("(let ((a eseq.macro-state/macro-arm)) (set! a.rack-index {rack}))")
        };
        self.eval_macros(&code);
        self.sync();
        self.show_all();
    }

    /// The props of each widget of buffer `buffer` named `debug`.
    fn widgets_named(&self, buffer: &str, debug: &str) -> Vec<HashMap<String, Value>> {
        let mut widgets = Vec::new();
        widgets_with_prop(&self.buffer_tree(buffer).0, "debug-name", &mut widgets);
        widgets.retain(|w| matches!(&w["debug-name"], Value::String(name) if name == debug));
        widgets
    }

    /// The one widget of buffer `buffer` named `debug`.
    fn widget_named(&self, buffer: &str, debug: &str) -> HashMap<String, Value> {
        let mut widgets = self.widgets_named(buffer, debug);
        assert_eq!(widgets.len(), 1, "one {debug} in {buffer}");
        widgets.remove(0)
    }

    /// Call `widget`'s handler `prop` with `args`; the host commands it
    /// queued, not yet applied.
    fn fire(
        &mut self,
        widget: &HashMap<String, Value>,
        prop: &str,
        args: Vec<Value>,
    ) -> Vec<(String, Value)> {
        let handler = widget
            .get(prop)
            .cloned()
            .unwrap_or_else(|| panic!("no {prop}"));
        self.editor.runtime_mut().invoke(handler, args).expect(prop);
        self.custom_commands()
    }

    /// Apply `commands` as the event loop does, then sync and render.
    fn apply(&mut self, commands: Vec<(String, Value)>) {
        for (name, payload) in commands {
            self.command(&name, payload);
        }
        self.sync();
        self.show_all();
    }
}

/// Whether a label in `tree` shows `text`.
fn mentions(tree: &Value, text: &str) -> bool {
    tree_has_string_prop(tree, "text", text)
}

/// The one command named `name` and its payload.
fn command<'a>(commands: &'a [(String, Value)], name: &str) -> &'a Value {
    let mut named = commands.iter().filter(|(n, _)| n == name);
    let (_, payload) = named
        .next()
        .unwrap_or_else(|| panic!("no {name}: {commands:?}"));
    assert!(named.next().is_none(), "one {name}: {commands:?}");
    payload
}

/// A widget prop's value, a binding read through its slot.
fn shown(widget: &HashMap<String, Value>, prop: &str) -> f64 {
    match &widget[prop] {
        Value::Number(n) => *n,
        Value::ReactiveRef { slot, .. } => read_float_slot(slot),
        other => panic!("{prop} is {other:?}"),
    }
}

#[test]
fn the_project_mapping_table_reads_mappings_and_edits_them_through_their_setters() {
    let mut h = distro();
    let slot = h.add_effect(0, "Filter");
    let ensure = app::AppCommand::MacroEnsure {
        key: "delay-push".to_string(),
        name: "Delay Push".to_string(),
    };
    app::apply_command(&mut h.app, ensure);
    let id = h.app.macro_engine.macros()[0].id;
    let target = sequencer::process::ParamTarget::EffectParam {
        slot,
        effect: "Filter".to_string(),
        param: "cutoff".to_string(),
        param_id: None,
    };
    app::apply_command(
        &mut h.app,
        app::AppCommand::MacroMapParam {
            id,
            track: 0,
            target,
        },
    );
    h.sync();
    h.arm(i64::from(id), -1);
    let table = "*macro-mappings*";
    assert_eq!(h.widgets_named(table, "macro-mapping-table-row").len(), 1);
    let (tree, _) = h.buffer_tree(table);
    for text in [
        "MACRO MAPPINGS",
        "Delay Push",
        "T1 · Filter",
        "cutoff",
        "live",
    ] {
        assert!(mentions(&tree, text), "{text}");
    }
    // The range in the target's display units, within the param's range.
    let mapping = h.app.macro_engine.macros()[0].mappings[0].clone();
    let min = h.widget_named(table, "macro-mapping-min");
    assert_eq!(shown(&min, "value"), f64::from(mapping.range_min));
    let range = h.eval_macros(
        "(let ((m (first (macros))) (mm (first m.mappings))) (list mm.target.min mm.target.max))",
    );
    assert_eq!(range, list_value([min["min"].clone(), min["max"].clone()]));
    // Edits go through the mapping's setters.
    let commands = h.fire(&min, "on-change", vec![Value::Number(300.0)]);
    assert_eq!(
        get(command(&commands, "set-macro-mapping"), "field"),
        s("min")
    );
    h.apply(commands);
    assert_eq!(h.app.macro_engine.macros()[0].mappings[0].range_min, 300.0);
    let curve = h.widget_named(table, "macro-mapping-curve");
    let commands = h.fire(&curve, "on-change", vec![s("exp")]);
    assert_eq!(
        get(command(&commands, "set-macro-mapping"), "field"),
        s("curve")
    );
    h.apply(commands);
    assert_eq!(
        h.app.macro_engine.macros()[0].mappings[0].curve,
        sequencer::macro_engine::MacroCurve::Exp
    );
    let unmap = h.widget_named(table, "macro-mapping-unmap");
    let commands = h.fire(&unmap, "on-click", vec![Value::Nil]);
    let payload = command(&commands, "macro-unmap");
    assert_eq!(get(payload, "id"), Value::Number(f64::from(id)));
    assert_eq!(get(payload, "mapping-idx"), Value::Number(0.0));
    h.apply(commands);
    assert!(h.app.macro_engine.macros()[0].mappings.is_empty());
    assert!(h.widgets_named(table, "macro-mapping-table-row").is_empty());
    assert_eq!(
        h.widgets_named(table, "macro-mapping-editor-empty").len(),
        1
    );
}

#[test]
fn the_rack_mapping_table_shows_the_armed_rack_macro_and_edits_its_mapping() {
    let mut h = distro();
    h.rack_track();
    h.sync();
    let id = RackMacroId::from_index(1).unwrap();
    let mapping = RackMacroMapping {
        target: RackMacroTarget::SlotInstrumentParam {
            slot: 0,
            param: "start".to_string(),
            param_index: START,
        },
        range_min: 0.0,
        range_max: 0.5,
        curve: RackMacroCurve::Linear,
    };
    h.app.map_rack_macro(2, id, mapping).expect("map the macro");
    h.shared.current_track.store(2, Ordering::Relaxed);
    h.sync();
    h.arm(-1, 1);
    let table = "*macro-mappings*";
    let (tree, _) = h.buffer_tree(table);
    for text in ["RACK MACRO MAPPINGS", "Layer 1", "start"] {
        assert!(mentions(&tree, text), "{text}");
    }
    // A percent target's range shows 0–100.
    let max = h.widget_named(table, "macro-mapping-max");
    assert_eq!(shown(&max, "value"), 50.0);
    assert_eq!(max["max"], Value::Number(100.0));
    let commands = h.fire(&max, "on-change", vec![Value::Number(40.0)]);
    let payload = command(&commands, "set-macro-mapping");
    assert_eq!(get(payload, "macro"), Value::Number(1.0));
    h.apply(commands);
    let rack = h.shared.state.live_rack_track_snapshot(2).unwrap();
    assert!((rack.macros[1].mappings[0].range_max - 0.4).abs() < 1e-6);
    let unmap = h.widget_named(table, "macro-mapping-unmap");
    let commands = h.fire(&unmap, "on-click", vec![Value::Nil]);
    let payload = command(&commands, "unmap-rack-macro-param");
    assert_eq!(get(payload, "track"), Value::Number(2.0));
    assert_eq!(get(payload, "id"), Value::Number(1.0));
    assert_eq!(get(payload, "mapping-idx"), Value::Number(0.0));
}

#[test]
fn player_controls_resolve_a_scripted_macro_by_key_and_bind_its_value() {
    let mut h = distro();
    h.eval_macros(r#"(eseq.macros/macro-ensure :player/delay-push "Delay Push")"#);
    let commands = h.custom_commands();
    let ensure = command(&commands, "macro-ensure");
    assert_eq!(get(ensure, "key"), s("player/delay-push"));
    let ensure = app::AppCommand::MacroEnsure {
        key: "player/delay-push".to_string(),
        name: "Delay Push".to_string(),
    };
    app::apply_command(&mut h.app, ensure);
    h.sync();
    let id = h.app.macro_engine.macros()[0].id;
    assert_eq!(
        h.eval_macros("(eseq.macros/macro-id-for-key :player/delay-push)"),
        Value::Number(f64::from(id))
    );
    h.eval_macros(
        r#"(effect-buffer "*macro-controls-test*"
             (h-stack :gap 1
               (eseq.macros/macro-knob :macro :player/delay-push)
               (eseq.macros/macro-momentary :macro :player/delay-push)
               (eseq.macros/macro-map-button :macro :player/delay-push)))"#,
    );
    h.show_all();
    let buffer = "*macro-controls-test*";
    let (tree, _) = h.buffer_tree(buffer);
    let (mut bound, mut legacy) = (Vec::new(), Vec::new());
    instance_bindings(&tree, &mut bound, &mut legacy);
    assert_eq!(legacy, Vec::<String>::new());
    let macros = h.eval_macros("(macros)");
    let m = h.instances(macros)[0];
    assert!(bound.contains(&(m, "value".to_string())), "{bound:?}");
    // The knob sets the macro's value (a performance control).
    let knob = h.widget_named(buffer, "macro-knob");
    let commands = h.fire(&knob, "on-change", vec![Value::Number(0.75)]);
    assert_eq!(get(command(&commands, "set-macro"), "field"), s("value"));
    h.apply(commands);
    assert_eq!(h.app.macro_engine.macros()[0].value, 0.75);
    assert_eq!(shown(&h.widget_named(buffer, "macro-knob"), "value"), 0.75);
    // Hold drives it fully on; letting go releases it.
    let hold = h.widget_named(buffer, "macro-momentary");
    let commands = h.fire(&hold, "on-press", vec![Value::Nil]);
    assert_eq!(
        get(command(&commands, "set-macro"), "value"),
        Value::Number(1.0)
    );
    let commands = h.fire(&hold, "on-release", vec![Value::Nil]);
    let payload = command(&commands, "macro-release");
    assert_eq!(get(payload, "id"), Value::Number(f64::from(id)));
    // Map arms the macro for mapping.
    let map = h.widget_named(buffer, "macro-map-button");
    h.fire(&map, "on-click", vec![Value::Nil]);
    assert_eq!(
        h.eval_macros("(let ((a eseq.macro-state/macro-arm)) (list a.open a.mid))"),
        list_value([Value::Bool(true), Value::Number(f64::from(id))])
    );
}

#[test]
fn scene_macro_controls_read_and_set_the_scene_config() {
    let mut h = distro();
    let create = app::AppCommand::MacroCreateScene {
        name: "Scene Push".to_string(),
        target_scene: 0,
    };
    app::apply_command(&mut h.app, create);
    let id = h.app.macro_engine.macros()[0].id;
    h.sync();
    h.eval_macros(&format!(
        r#"(effect-buffer "*scene-macro-test*" (eseq.macros/scene-macro-controls :macro {id}))"#
    ));
    h.show_all();
    h.sync();
    h.show_all();
    let buffer = "*scene-macro-test*";
    for name in [
        "scene-macro-controls",
        "scene-macro-title",
        "scene-macro-target",
        "scene-macro-knob",
        "scene-macro-momentary",
        "scene-macro-morph-params",
        "scene-macro-steal-patterns",
        "scene-macro-quantize",
        "scene-macro-track-mask",
        "scene-macro-diff",
    ] {
        h.widget_named(buffer, name);
    }
    let target = h.widget_named(buffer, "scene-macro-target");
    assert_eq!(target["value"], s("Scene 1"));
    let (tree, _) = h.buffer_tree(buffer);
    assert!(mentions(&tree, "PUSH · SCENE 1"));
    let steal = h.widget_named(buffer, "scene-macro-steal-patterns");
    let steals = steal["value"] == Value::Bool(true);
    let commands = h.fire(&steal, "on-change", vec![Value::Bool(!steals)]);
    let payload = command(&commands, "set-macro");
    assert_eq!(get(payload, "field"), s("steal-patterns"));
    h.apply(commands);
    let config = h.app.macro_engine.scene_config(id).unwrap().clone();
    assert_eq!(config.steal_patterns, !steals);
    assert_eq!(
        h.widget_named(buffer, "scene-macro-steal-patterns")["value"],
        Value::Bool(!steals)
    );
}

#[test]
fn effect_devices_say_whether_they_are_built_in() {
    let (mut h, _) = Harness::with_devices();
    let roles =
        "(let ((t (nth (tracks) {track}))) (map (lambda (d) (list d.role d.builtin)) t.devices))";
    assert_eq!(
        h.eval_macros(&roles.replace("{track}", "0")),
        h.eval_macros(r#"(list (list "effect" true))"#)
    );
    assert_eq!(
        h.eval_macros(&roles.replace("{track}", "2")),
        h.eval_macros(r#"(list (list "instrument" false))"#)
    );
}

#[test]
fn a_built_in_effect_armed_for_deletion_offers_no_effect_editor() {
    let mut h = distro();
    h.add_effect(0, "Filter");
    h.sync();
    let enabled = |h: &mut Harness| {
        let item =
            crate::application_menu::menu_item_prop("Edit", "edit-menu-effect", "enabled-when");
        h.eval_macros(&format!("({item})"))
    };
    assert_eq!(enabled(&mut h), Value::Bool(false));
    h.eval_macros(
        "(let ((t (nth (tracks) 0)) (fx (first t.devices))) (set! fx.delete-target true))",
    );
    h.drain();
    h.sync();
    assert_eq!(
        h.eval_macros("(let ((t (nth (tracks) 0)) (fx (first t.devices))) fx.delete-target)"),
        Value::Bool(true)
    );
    assert_eq!(
        enabled(&mut h),
        Value::Bool(false),
        "a built-in has no source to edit"
    );
}

/// The candidates picker steps over 1–3 (a population is auto, 0, or at
/// least 4): to 4 going up, back to auto going down; `set-learn` never sees
/// a value it rejects.
#[test]
fn the_population_picker_steps_over_the_rejected_sizes() {
    let mut h = distro();
    h.eval_macros(r#"(effect-buffer "*learn-cma-test*" (eseq.patch-learn/cma-config))"#);
    h.show_all();
    let picker = |h: &Harness| {
        let mut widgets = Vec::new();
        widgets_with_prop(&h.buffer_tree("*learn-cma-test*").0, "key", &mut widgets);
        widgets.retain(
            |w| matches!(&w["key"], Value::String(key) if key.ends_with("learn-cma-population")),
        );
        assert_eq!(widgets.len(), 1, "one population picker");
        widgets.remove(0)
    };
    let population =
        |h: &mut Harness| h.eval_with("(import eseq.kinds :refer (learn))", "learn.cma-population");
    for (from, step, set) in [
        (0.0, 1.0, 4.0),
        (4.0, 3.0, 0.0),
        (0.0, 2.0, 4.0),
        (4.0, 5.0, 5.0),
    ] {
        assert_eq!(population(&mut h), Value::Number(from));
        let commands = h.fire(&picker(&h), "on-change", vec![Value::Number(step)]);
        let payload = command(&commands, "set-learn");
        assert_eq!(get(payload, "field"), s("cma-population"));
        assert_eq!(
            get(payload, "value"),
            Value::Number(set),
            "{from} -> {step}"
        );
        h.apply(commands);
    }
    assert_eq!(population(&mut h), Value::Number(5.0));
}

/// An epoch re-renders the training readouts (their subtrees), never the
/// whole pane with its target picker.
#[test]
fn a_training_epoch_rerenders_only_the_readouts() {
    let mut h = distro();
    crate::present_learn(h.editor.runtime_mut(), |l| {
        l.phase = "training".to_string();
        l.stage = "train".to_string();
        l.method = "Local fit + basin check".to_string();
        l.current_epoch = 3.0;
        l.total_epochs = 50.0;
        l.loss = 0.5;
        l.losses = vec![1.0, 0.75, 0.5];
    });
    h.sync();
    // `renders` is a plain global: its write re-runs nothing.
    h.eval_macros(
        r#"(def renders 0)
           (effect-buffer "*learn-train-test*"
             (do (set! renders (+ renders 1)) (eseq.patch-learn/panel)))"#,
    );
    h.show_all();
    let buffer = "*learn-train-test*";
    assert!(mentions(&h.buffer_tree(buffer).0, "Epoch 3 / 50"));
    let renders = h.eval_macros("renders");
    assert!(
        matches!(renders, Value::Number(n) if n >= 1.0),
        "{renders:?}"
    );
    crate::present_learn(h.editor.runtime_mut(), |l| {
        l.current_epoch = 4.0;
        l.loss = 0.25;
        l.losses.push(0.25);
        l.epoch_params = vec![crate::presented::LearnEpochParam {
            name: "cutoff".to_string(),
            from: 520.0,
            value: 610.0,
            change: 90.0,
            step: 0.2,
        }];
    });
    h.sync();
    h.show_all();
    let (tree, _) = h.buffer_tree(buffer);
    assert!(mentions(&tree, "Epoch 4 / 50"));
    assert!(mentions(&tree, "loss 0.250000"));
    assert!(mentions(&tree, "520.0000 → 610.0000"));
    assert_eq!(
        h.eval_macros("renders"),
        renders,
        "the pane did not re-render"
    );
}
