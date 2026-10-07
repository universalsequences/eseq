//! The factory device panels' shared plumbing (param controls, the custom-UI
//! runtime, the panel frame, widgets and bodies, the effect strip's view
//! state), ported to the kinds (kind-bindings spec §13 stage 8,
//! eseq-0l17.14).

use super::views::{assert_ported, distro, instance_bindings, legacy_forms, widgets_with_prop};
use super::*;

/// The ported files that read host kinds.
const PORTED: [(&str, &str); 5] = [
    (
        "ui/effects/devices.lisp",
        include_str!("../../../../../../content/ui/effects/devices.lisp"),
    ),
    (
        "ui/effects/param-controls.lisp",
        include_str!("../../../../../../content/ui/effects/param-controls.lisp"),
    ),
    (
        "ui/effects/panel-frame.lisp",
        include_str!("../../../../../../content/ui/effects/panel-frame.lisp"),
    ),
    (
        "ui/effects/panel-widgets.lisp",
        include_str!("../../../../../../content/ui/effects/panel-widgets.lisp"),
    ),
    (
        "ui/effects/panel-bodies.lisp",
        include_str!("../../../../../../content/ui/effects/panel-bodies.lisp"),
    ),
];

/// The ported files that read no host kind of their own: they reach params
/// through eseq.effects.param-controls, or hold view state only.
const PORTED_WITHOUT_KINDS: [(&str, &str); 7] = [
    (
        "ui/effects/state.lisp",
        include_str!("../../../../../../content/ui/effects/state.lisp"),
    ),
    (
        "ui/effects/custom-ui-runtime.lisp",
        include_str!("../../../../../../content/ui/effects/custom-ui-runtime.lisp"),
    ),
    (
        "ui/effects/custom-ui-sections.lisp",
        include_str!("../../../../../../content/ui/effects/custom-ui-sections.lisp"),
    ),
    (
        "ui/effects/custom-ui-controls.lisp",
        include_str!("../../../../../../content/ui/effects/custom-ui-controls.lisp"),
    ),
    (
        "ui/effects/custom-ui-lego.lisp",
        include_str!("../../../../../../content/ui/effects/custom-ui-lego.lisp"),
    ),
    (
        "ui/effects/custom-effect-ui.lisp",
        include_str!("../../../../../../content/ui/effects/custom-effect-ui.lisp"),
    ),
    (
        "ui/effects/param-grid.lisp",
        include_str!("../../../../../../content/ui/effects/param-grid.lisp"),
    ),
];

#[test]
fn ported_panel_plumbing_uses_no_legacy_binding_forms() {
    assert_ported(&PORTED);
    for (file, source) in PORTED_WITHOUT_KINDS {
        assert_eq!(legacy_forms(source), Vec::<&str>::new(), "{file}");
    }
}

/// Groups B–D of the panels (eseq-0l17.61) that read host kinds.
const PORTED_61: [(&str, &str); 5] = [
    (
        "ui/effects/sampler-panel.lisp",
        include_str!("../../../../../../content/ui/effects/sampler-panel.lisp"),
    ),
    (
        "ui/effects/process-panel.lisp",
        include_str!("../../../../../../content/ui/effects/process-panel.lisp"),
    ),
    (
        "ui/effects/scale-editor.lisp",
        include_str!("../../../../../../content/ui/effects/scale-editor.lisp"),
    ),
    (
        "ui/effects/builtin/filter-table.lisp",
        include_str!("../../../../../../content/ui/effects/builtin/filter-table.lisp"),
    ),
    (
        "ui/effects/builtin/phaser-flanger.lisp",
        include_str!("../../../../../../content/ui/effects/builtin/phaser-flanger.lisp"),
    ),
];

/// Groups B–D that read no host kind of their own (values through
/// eseq.effects.devices / param-controls / the custom-UI runtime, view
/// state only).
const PORTED_61_WITHOUT_KINDS: [(&str, &str); 12] = [
    (
        "ui/effects/instrument-panel.lisp",
        include_str!("../../../../../../content/ui/effects/instrument-panel.lisp"),
    ),
    (
        "ui/effects/modulator-panel.lisp",
        include_str!("../../../../../../content/ui/effects/modulator-panel.lisp"),
    ),
    (
        "ui/effects/instrument-modulation.lisp",
        include_str!("../../../../../../content/ui/effects/instrument-modulation.lisp"),
    ),
    (
        "ui/effects/instrument-sources.lisp",
        include_str!("../../../../../../content/ui/effects/instrument-sources.lisp"),
    ),
    (
        "ui/effects/effect-panels.lisp",
        include_str!("../../../../../../content/ui/effects/effect-panels.lisp"),
    ),
    (
        "ui/effects/effect-modulation.lisp",
        include_str!("../../../../../../content/ui/effects/effect-modulation.lisp"),
    ),
    (
        "ui/effects/step-buffer.lisp",
        include_str!("../../../../../../content/ui/effects/step-buffer.lisp"),
    ),
    (
        "ui/effects/drum-surface.lisp",
        include_str!("../../../../../../content/ui/effects/drum-surface.lisp"),
    ),
    (
        "ui/effects/mnm-surface.lisp",
        include_str!("../../../../../../content/ui/effects/mnm-surface.lisp"),
    ),
    (
        "ui/effects/identified-drum.lisp",
        include_str!("../../../../../../content/ui/effects/identified-drum.lisp"),
    ),
    (
        "ui/effects/physical-model-surface.lisp",
        include_str!("../../../../../../content/ui/effects/physical-model-surface.lisp"),
    ),
    (
        "ui/materials.lisp",
        include_str!("../../../../../../content/ui/materials.lisp"),
    ),
];

#[test]
fn ported_panels_use_no_legacy_binding_forms() {
    assert_ported(&PORTED_61);
    for (file, source) in PORTED_61_WITHOUT_KINDS {
        assert_eq!(legacy_forms(source), Vec::<&str>::new(), "{file}");
    }
    // Every built-in effect panel (their view state is singletons now).
    let builtin =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/ui/effects/builtin");
    let mut count = 0;
    for entry in std::fs::read_dir(&builtin).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "lisp") {
            let source = std::fs::read_to_string(&path).unwrap();
            assert_eq!(
                legacy_forms(&source),
                Vec::<&str>::new(),
                "{}",
                path.display()
            );
            count += 1;
        }
    }
    assert!(count > 20, "found only {count} built-in panels");
}

#[test]
fn panel_layout_and_plock_table_keep_their_compat_reads() {
    // COMPAT (eseq-0l17.22): the panels lay out from the host's panel dicts
    // (SEQ.instrument-panel / midi-effects / effects / bus-effects), read by
    // the *fx* buffer and eseq.effects/device-panel alone; the p-lock table
    // reads its rows (SEQ.track-plocks, the variant chips) and binds a row's
    // value field: no kind holds a lock row yet.
    for (file, source, forms) in [
        (
            "ui/effects/buffers.lisp",
            include_str!("../../../../../../content/ui/effects/buffers.lisp"),
            vec!["SEQ."],
        ),
        (
            "ui/effects/index.lisp",
            include_str!("../../../../../../content/ui/effects/index.lisp"),
            vec!["SEQ."],
        ),
        (
            "ui/effects/track-panels.lisp",
            include_str!("../../../../../../content/ui/effects/track-panels.lisp"),
            vec!["bind-seq", "SEQ."],
        ),
    ] {
        assert_eq!(legacy_forms(source), forms, "{file}");
    }
}

/// A distro harness showing track 2 (a sampler, with a Filter effect) in
/// the *fx* buffer, its panels published as the tick does while *fx* shows.
fn sampler_with_filter() -> Harness {
    let mut h = distro();
    h.app
        .graph_controller()
        .add_blank_sampler_track()
        .expect("sampler track");
    h.app.add_builtin_effect_sync(2, "Filter").expect("filter");
    h.publish_panels(2);
    h
}

impl Harness {
    /// Make `track` current and publish its instrument and effect panel
    /// dicts, as the tick does while *fx* shows.
    fn publish_panels(&mut self, track: usize) {
        self.shared.current_track.store(track, Ordering::Relaxed);
        self.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
        self.sync();
        let instrument =
            build_instrument_panel_value(&self.app, track, &self.shared.selected_steps);
        let effects = build_effects_value(
            &self.shared.state,
            track,
            &self.app.graph.effect_descriptors,
            &self.shared.selected_steps,
        );
        let rt = self.editor.runtime_mut();
        rt.set_reactive_value_patch("SEQ", "current-track", Value::Number(track as f64));
        rt.set_reactive_value_patch("SEQ", "instrument-panel", instrument);
        rt.set_reactive_value_patch("SEQ", "effects", effects);
        rt.run_reactive_cycle();
        self.show_all();
        self.sync();
        self.show_all();
    }
}

#[test]
fn panel_controls_bind_their_params_through_kinds() {
    let mut h = sampler_with_filter();
    let (fx, _) = h.buffer_tree("*fx*");
    let mut bound = Vec::new();
    let mut legacy = Vec::new();
    instance_bindings(&fx, &mut bound, &mut legacy);
    let params = |h: &mut Harness, device: &str| {
        let list = h.eval_all(&format!("{device}.params"));
        h.instances(list)
    };
    h.eval_all("(def t2 (track 2)) (def flt-device (first (filter (lambda (d) (= d.name \"Filter\")) t2.devices)))");
    let filter = params(&mut h, "flt-device");
    assert!(!filter.is_empty(), "the Filter's params are published");
    let shown = |ids: &[InstanceId]| {
        ids.iter()
            .filter(|id| bound.contains(&(**id, "value".to_string())))
            .count()
    };
    assert!(
        shown(&filter) >= 3,
        "the Filter's knobs bind param.value: {bound:?}"
    );
    h.eval_all("(def smp-device (first (filter (lambda (d) (= d.slot -1)) t2.devices)))");
    let sampler = params(&mut h, "smp-device");
    assert!(
        shown(&sampler) >= 3,
        "the sampler's knobs bind param.value: {bound:?}"
    );
    assert!(
        !legacy.iter().any(|field| field.contains("-fx-")),
        "no effect knob binds a legacy value field: {legacy:?}"
    );
    // The p-lock accent is the view's own singleton.
    assert!(
        bound.iter().any(|(id, field)| field == "r"
            && h.rt()
                .instance_kind(*id)
                .is_some_and(|kind| kind.ends_with(":plock-color"))),
        "knobs bind the p-lock accent"
    );
}

#[test]
fn a_percent_param_shows_display_units_and_sets_stored_ones() {
    // Effect commands store a % param as 0-1; the panel shows the param in
    // display units (eseq.kinds, spec §14.2b), the dict included.
    let mut h = sampler_with_filter();
    let wet = h.eval_all(
        "(let ((fx (first (filter (lambda (fx) (= (get fx :name) \"Filter\")) SEQ.effects)))) \
           (first (filter (lambda (p) (= (get p :name) \"wet\")) (get fx :params))))",
    );
    assert_eq!(get(&wet, "max"), Value::Number(100.0), "{wet:?}");
    let stored = h.eval_all(
        "(let ((fx (first (filter (lambda (fx) (= (get fx :name) \"Filter\")) SEQ.effects)))) \
           (let ((p (first (filter (lambda (p) (= (get p :name) \"wet\")) (get fx :params))))) \
             (eseq.effects.devices/param-stored-value fx (eseq.effects.devices/param-of fx p) 40)))",
    );
    assert_eq!(stored, Value::Number(0.4));
}

/// The current track's effect named `name`, as `fx`, and its param `param`,
/// as `p` (its panel dict) and `prm` (its kinds param).
fn bind_effect_param(h: &mut Harness, name: &str, param: &str) {
    h.eval_all(&format!(
        "(def fx (first (filter (lambda (fx) (= (get fx :name) \"{name}\")) SEQ.effects))) \
         (def p (first (filter (lambda (p) (= (get p :name) \"{param}\")) (get fx :params)))) \
         (def prm (eseq.effects.devices/param-of fx p)) \
         (def lane (first prm.mod-targets)) \
         (def depth lane.depth)"
    ));
}

/// Open fx's modulation view (slot 1), or close it.
fn set_effect_mods(h: &mut Harness, open: bool) {
    h.eval_all(&format!(
        "(let ((v eseq.effects.state/effect-mods)) \
           (do (set! v.open {open}) (set! v.chain \"audio\") (set! v.track 2) \
               (set! v.slot (get fx :slot-idx)) (set! v.rack-slot -1) (set! v.bus -1) \
               (set! v.mod-slot 1)))"
    ));
}

#[test]
fn a_percent_lane_depth_reads_display_units_and_round_trips() {
    // Str8 Delay's wet lanes: a % depth param stored as -1..1 (param.percent)
    // reads its value and its lane's range x 100 (spec §14.2b).
    let mut h = sampler_with_filter();
    h.app
        .add_builtin_effect_sync(2, "Str8 Delay")
        .expect("str8 delay");
    h.publish_panels(2);
    bind_effect_param(&mut h, "Str8 Delay", "wet");
    assert_eq!(h.eval_all("depth.percent"), Value::Bool(true));
    assert_eq!(h.eval_all("lane.depth-min"), Value::Number(-100.0));
    assert_eq!(h.eval_all("lane.depth-max"), Value::Number(100.0));
    // A depth set in display units lands as its stored ratio and reads back.
    h.eval_all(
        "(eseq.effects.param-controls/fx-set-effect-value fx \
           (dict :idx depth.index :control \"param\") 50)",
    );
    h.drain();
    h.sync();
    assert_eq!(h.eval_all("depth.value"), Value::Number(50.0));
    // The knob shows the depth as it reads: no further x 100.
    set_effect_mods(&mut h, true);
    assert_eq!(
        h.eval_all("(eseq.effects.param-controls/percent-scale fx p)"),
        Value::Number(1.0)
    );
}

#[test]
fn a_percent_lane_with_a_plain_depth_shows_it_times_100() {
    // Chorus mix: a % param (display units) whose lane depth param has no
    // unit (a 0-1 fraction) under the lane's % unit.
    let mut h = sampler_with_filter();
    h.app.add_builtin_effect_sync(2, "Chorus").expect("chorus");
    h.publish_panels(2);
    bind_effect_param(&mut h, "Chorus", "mix");
    assert_eq!(h.eval_all("depth.percent"), Value::Bool(false));
    assert_eq!(h.eval_all("lane.unit"), Value::String("%".into()));
    let scale = |h: &mut Harness| h.eval_all("(eseq.effects.param-controls/percent-scale fx p)");
    set_effect_mods(&mut h, false);
    assert_eq!(
        scale(&mut h),
        Value::Number(1.0),
        "the mix itself: display units"
    );
    set_effect_mods(&mut h, true);
    assert_eq!(scale(&mut h), Value::Number(100.0), "its depth: a fraction");
}

/// A custom UI's ADSR readouts bind their editor's stage flags, one
/// view-local `adsr-gesture` per (scope, section) (eseq-0l17.73): two
/// custom UIs sharing a section number keep their own flags, and a drag
/// repaints its readouts without re-rendering any buffer (eseq-eeng).
#[test]
fn adsr_gesture_flags_are_per_scope_and_a_drag_only_repaints() {
    let mut h = distro();
    // As the generated custom-UI functions do: name the scope, then render
    // the readouts (section -1, the panel's own envelope).
    h.eval_all(
        r#"(def adsr-readouts (scope stages)
             (do (set! eseq.vanilla/custom-ui-current-kind "instrument")
                 (set! eseq.vanilla/synth-ui-current-name scope)
                 (h-stack
                   (map (lambda (stage)
                          (number-picker :value 0 :min 0 :max 1 :noui true
                            :active (eseq.effects.custom-ui-sections/custom-ui-adsr-stage-active-binding -1 stage)))
                        stages))))
           (effect-buffer "*adsr-core*" (adsr-readouts "core" (list :attack :decay)))
           (effect-buffer "*adsr-triton*" (adsr-readouts "triton" (list :attack)))"#,
    );
    h.show_all();
    let bound = |h: &Harness, buffer: &str| {
        let (tree, revision) = h.buffer_tree(buffer);
        let mut bound = Vec::new();
        let mut legacy = Vec::new();
        instance_bindings(&tree, &mut bound, &mut legacy);
        assert!(legacy.is_empty(), "{buffer}: {legacy:?}");
        (bound, revision)
    };
    let (core, core_revision) = bound(&h, "*adsr-core*");
    let (triton, triton_revision) = bound(&h, "*adsr-triton*");
    assert_eq!(core.len(), 2, "{core:?}");
    assert_eq!(core[0].0, core[1].0, "one gesture per editor");
    assert_eq!(
        [core[0].1.as_str(), core[1].1.as_str()],
        ["attack", "decay"]
    );
    assert_eq!(triton.len(), 1, "{triton:?}");
    assert_ne!(triton[0].0, core[0].0, "same section, another scope");
    assert!(h
        .rt()
        .instance_kind(core[0].0)
        .is_some_and(|kind| kind.ends_with(":adsr-gesture")));

    // The readouts' flags, read from their bound slots.
    let flags = |h: &Harness, buffer: &str| {
        let (tree, _) = h.buffer_tree(buffer);
        let mut widgets = Vec::new();
        widgets_with_prop(&tree, "active", &mut widgets);
        widgets
            .iter()
            .map(|widget| match &widget["active"] {
                Value::ReactiveRef { slot, .. } => read_float_slot(slot),
                other => panic!("not a binding: {other:?}"),
            })
            .collect::<Vec<_>>()
    };
    let drag = |h: &mut Harness, scope: &str, active: &str| {
        let before = h.rt().ui_work_counters();
        h.eval_all(&format!(
            "(eseq.effects.custom-ui-sections/custom-ui-set-active-adsr (dict :name \"{scope}\") -1 {active})"
        ));
        h.show_all();
        let after = h.rt().ui_work_counters();
        (
            after.full_buffer_reruns - before.full_buffer_reruns,
            after.subtree_reruns - before.subtree_reruns,
        )
    };
    assert_eq!(flags(&h, "*adsr-core*"), [0.0, 0.0]);
    assert_eq!(drag(&mut h, "core", ":attack"), (0, 0), "the first drag");
    assert_eq!(flags(&h, "*adsr-core*"), [1.0, 0.0]);
    assert_eq!(flags(&h, "*adsr-triton*"), [0.0], "triton's own flags");
    assert_eq!(drag(&mut h, "core", ":decay"), (0, 0));
    assert_eq!(flags(&h, "*adsr-core*"), [0.0, 1.0]);
    // A drag elsewhere clears the held editor's flags.
    assert_eq!(drag(&mut h, "triton", ":attack"), (0, 0));
    assert_eq!(flags(&h, "*adsr-core*"), [0.0, 0.0]);
    assert_eq!(flags(&h, "*adsr-triton*"), [1.0]);
    assert_eq!(drag(&mut h, "triton", "false"), (0, 0), "the drag ends");
    assert_eq!(flags(&h, "*adsr-triton*"), [0.0]);
    assert_eq!(h.buffer_tree("*adsr-core*").1, core_revision);
    assert_eq!(h.buffer_tree("*adsr-triton*").1, triton_revision);
}
