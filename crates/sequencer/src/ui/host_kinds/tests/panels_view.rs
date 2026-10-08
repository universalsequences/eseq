//! The factory device panels' shared plumbing (param controls, the custom-UI
//! runtime, the panel frame, widgets and bodies, the effect strip's view
//! state), ported to the kinds (kind-bindings spec §13 stage 8,
//! eseq-0l17.14).

use super::views::{
    assert_ported, bound, distro, instance_bindings, legacy_forms, tree_has_string_prop,
    widgets_with_prop,
};
use super::*;

/// The ported files that read host kinds.
const PORTED: [(&str, &str); 6] = [
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
    // The p-lock table reads selection.plock-rows (eseq-0l17.74).
    (
        "ui/effects/track-panels.lisp",
        include_str!("../../../../../../content/ui/effects/track-panels.lisp"),
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

/// The device panels' layout (eseq-0l17.82): the *fx* buffer and
/// eseq.effects/device-panel lay out from the devices, params and racks
/// through eseq.effects.panel-data; no file reads a panel dict from SEQ.
const PORTED_82: [(&str, &str); 2] = [
    (
        "ui/effects/buffers.lisp",
        include_str!("../../../../../../content/ui/effects/buffers.lisp"),
    ),
    (
        "ui/effects/panel-data.lisp",
        include_str!("../../../../../../content/ui/effects/panel-data.lisp"),
    ),
];

#[test]
fn panel_layout_reads_the_kinds() {
    assert_ported(&PORTED_82);
    let index = include_str!("../../../../../../content/ui/effects/index.lisp");
    assert_eq!(legacy_forms(index), Vec::<&str>::new(), "ui/effects/index.lisp");
    // Not vacuous: the scanner flags the reads these files had.
    assert_eq!(legacy_forms("(each SEQ.instrument-panel |inst| inst)"), vec!["SEQ."]);
}

/// A distro harness showing track 2 (a sampler, with a Filter effect) in
/// the *fx* buffer.
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
    /// Make `track` current, its chain synced (the panels lay out from its
    /// devices), and show the buffers.
    fn publish_panels(&mut self, track: usize) {
        self.shared.current_track.store(track, Ordering::Relaxed);
        self.shared.fx_epoch.fetch_add(1, Ordering::Relaxed);
        self.sync();
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
        "(let ((fx (first (filter (lambda (fx) (= (get fx :name) \"Filter\")) (eseq.effects.panel-data/current-effect-panels))))) \
           (first (filter (lambda (p) (= (get p :name) \"wet\")) (get fx :params))))",
    );
    assert_eq!(get(&wet, "max"), Value::Number(100.0), "{wet:?}");
    let stored = h.eval_all(
        "(let ((fx (first (filter (lambda (fx) (= (get fx :name) \"Filter\")) (eseq.effects.panel-data/current-effect-panels))))) \
           (let ((p (first (filter (lambda (p) (= (get p :name) \"wet\")) (get fx :params))))) \
             (eseq.effects.devices/param-stored-value fx (eseq.effects.devices/param-of fx p) 40)))",
    );
    assert_eq!(stored, Value::Number(0.4));
}

/// The current track's effect named `name`, as `fx`, and its param `param`,
/// as `p` (its panel dict) and `prm` (its kinds param).
fn bind_effect_param(h: &mut Harness, name: &str, param: &str) {
    h.eval_all(&format!(
        "(def fx (first (filter (lambda (fx) (= (get fx :name) \"{name}\")) \
           (eseq.effects.panel-data/current-effect-panels)))) \
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

/// Track 2's Filter and its cutoff param (`flt`, `cutoff`), and the
/// cutoff's instance.
fn filter_cutoff(h: &mut Harness) -> InstanceId {
    h.eval_all(
        "(def t2 (track 2)) \
         (def flt (first (filter (lambda (d) (= d.name \"Filter\")) t2.devices))) \
         (def cutoff (first (filter (lambda (p) (= p.name \"cutoff\")) flt.params)))",
    );
    match h.eval_all("cutoff") {
        Value::Instance(id) => id,
        other => panic!("no cutoff param: {other:?}"),
    }
}

/// eseq-0l17.82: the panels lay out from the kinds, and a param's value is
/// a binding: editing it repaints the knob and rebuilds no panel.
#[test]
fn editing_a_param_repaints_without_rebuilding_the_panels() {
    let mut h = sampler_with_filter();
    let cutoff = filter_cutoff(&mut h);
    let (fx, revision) = h.buffer_tree("*fx*");
    assert!(
        tree_has_string_prop(&fx, "debug-name", "audio-fx-panel-root-0-Filter"),
        "the Filter's panel shows"
    );
    let mut knobs = Vec::new();
    widgets_with_prop(&fx, "value", &mut knobs);
    let knob = knobs
        .iter()
        .find(|knob| bound(knob, "value") == Some((cutoff, "value".to_string())))
        .expect("a control binds cutoff.value");
    let Value::ReactiveRef { slot, .. } = &knob["value"] else {
        unreachable!("bound")
    };
    let slot = slot.clone();
    let before = read_float_slot(&slot);
    let target = if (before - 1234.0).abs() < 1.0 { 2345.0 } else { 1234.0 };
    h.eval_all(&format!("(set! cutoff.base {target})"));
    h.drain_and_sync();
    h.show_all();
    assert!(
        (read_float_slot(&slot) - target).abs() < 1e-3,
        "the knob repaints the new value"
    );
    assert_eq!(h.buffer_tree("*fx*").1, revision, "no panel rebuilt");
}

/// eseq-0l17.82: adding or removing an effect rebuilds the panels, which
/// lay out from the track's devices.
#[test]
fn adding_or_removing_an_effect_rebuilds_the_panels() {
    let mut h = sampler_with_filter();
    let (fx, revision) = h.buffer_tree("*fx*");
    let chorus = |slot: usize| format!("audio-fx-panel-root-{slot}-Chorus");
    let slot = h.app.add_builtin_effect_sync(2, "Chorus").expect("chorus");
    assert!(!tree_has_string_prop(&fx, "debug-name", &chorus(slot)));
    h.publish_panels(2);
    let (fx, added) = h.buffer_tree("*fx*");
    assert_ne!(added, revision, "the panels rebuild");
    assert!(
        tree_has_string_prop(&fx, "debug-name", &chorus(slot)),
        "the Chorus panel shows"
    );
    h.app
        .graph_controller()
        .delete_custom_effect_slot(2, slot)
        .expect("delete");
    h.publish_panels(2);
    let (fx, removed) = h.buffer_tree("*fx*");
    assert_ne!(removed, added, "the panels rebuild");
    assert!(!tree_has_string_prop(&fx, "debug-name", &chorus(slot)));
    assert!(tree_has_string_prop(&fx, "debug-name", "audio-fx-panel-root-0-Filter"));
}

impl Harness {
    /// Play with track `track`'s playhead at `step`, synced and shown.
    fn play_panels_at(&mut self, track: usize, step: usize) {
        let transport = &self.shared.state.transport;
        transport.playing.store(true, Ordering::Relaxed);
        transport.track_playheads[track].store(step as u32, Ordering::Relaxed);
        self.sync();
        self.show_all();
        self.sync();
        self.show_all();
    }

    /// The `:value-index` slot of the dropdown in the *fx* buffer bound to
    /// `param`'s value, and the buffer's revision.
    fn option_slot(&self, param: InstanceId) -> (Arc<std::sync::atomic::AtomicU64>, u64) {
        let (fx, revision) = self.buffer_tree("*fx*");
        let mut dropdowns = Vec::new();
        widgets_with_prop(&fx, "value-index", &mut dropdowns);
        let dropdown = dropdowns
            .iter()
            .find(|props| bound(props, "value-index") == Some((param, "value".to_string())))
            .unwrap_or_else(|| panic!("a dropdown binds the option param's value"));
        let Value::ReactiveRef { slot, .. } = &dropdown["value-index"] else {
            unreachable!("bound")
        };
        (slot.clone(), revision)
    }

    fn panel_instance(&mut self, code: &str) -> InstanceId {
        match self.eval_all(code) {
            Value::Instance(id) => id,
            other => panic!("{code}: not an instance: {other:?}"),
        }
    }
}

/// eseq-0l17.82: an option param p-locked on two steps shows each step's
/// option as the playhead steps onto it, through its dropdown's bound index:
/// the *fx* buffer's tree is not rebuilt (no panel reads the option by
/// value).
#[test]
fn a_p_locked_instrument_option_steps_under_the_playhead_without_rebuilding() {
    let mut h = sampler_with_filter();
    h.eval_all(
        "(def t2 (track 2)) (def inst (first t2.devices)) \
         (def lp (first (filter (lambda (p) (= p.name \"loop\")) inst.params))) \
         (lock-param! lp (list (nth t2.steps 0)) 0) \
         (lock-param! lp (list (nth t2.steps 1)) 1)",
    );
    h.drain_and_sync();
    let lp = h.panel_instance("lp");
    assert_eq!(h.eval_all("lp.type"), s("enum"));
    h.play_panels_at(2, 0);
    let (slot, revision) = h.option_slot(lp);
    assert_eq!(read_float_slot(&slot), 0.0);
    h.play_panels_at(2, 1);
    assert_eq!(read_float_slot(&slot), 1.0, "the dropdown shows step 1's option");
    assert_eq!(h.buffer_tree("*fx*").1, revision, "no panel rebuilt");
    h.play_panels_at(2, 0);
    assert_eq!(read_float_slot(&slot), 0.0);
    assert_eq!(h.buffer_tree("*fx*").1, revision, "no panel rebuilt");
}

/// The same for an option param of a drum rack slot's effect (the rack's
/// selected chain, each effect's panel in its own subtree).
#[test]
fn a_p_locked_rack_slot_effect_option_steps_under_the_playhead_without_rebuilding() {
    let mut h = distro();
    h.rack_track();
    h.sync();
    let add = h.eval(r#"(dict :track 2 :rack-slot 0 :name "Filter" :builtin true)"#);
    h.command("add-rack-slot-effect", add);
    let select = h.eval("(dict :track 2 :slot 0)");
    h.command("select-rack-slot", select);
    h.publish_panels(2);
    h.eval_all(
        "(def t2 (track 2)) (def rk (first t2.devices)) (def rs (first rk.devices)) \
         (def flt (first rs.devices)) \
         (def opt (first (filter (lambda (p) (= p.name \"lfo wave\")) flt.params))) \
         (eseq.effects.state/rack-panel-set-view (str t2.tid) false false true) \
         (lock-param! opt (list (nth t2.steps 0)) 0) \
         (lock-param! opt (list (nth t2.steps 1)) 1)",
    );
    h.drain_and_sync();
    let opt = h.panel_instance("opt");
    assert_eq!(h.eval_all("opt.type"), s("enum"));
    assert!(num(h.eval_all("flt.node-id")) > 0.0, "the slot's Filter runs");
    h.play_panels_at(2, 0);
    let (slot, revision) = h.option_slot(opt);
    assert_eq!(read_float_slot(&slot), 0.0);
    h.play_panels_at(2, 1);
    assert_eq!(read_float_slot(&slot), 1.0, "the dropdown shows step 1's option");
    assert_eq!(h.buffer_tree("*fx*").1, revision, "no panel rebuilt");
}

/// A synced Delay's time picks a division: editing it moves the dropdown's
/// bound index and rebuilds no panel (the dict reads no value).
#[test]
fn a_synced_delay_time_edit_rebuilds_no_panel() {
    let mut h = sampler_with_filter();
    h.app.add_builtin_effect_sync(2, "Delay").expect("delay");
    h.publish_panels(2);
    h.eval_all(
        "(def t2 (track 2)) \
         (def dly (first (filter (lambda (d) (= d.name \"Delay\")) t2.devices))) \
         (def sync (nth dly.params 1)) (def time (nth dly.params 2)) \
         (set! sync.base 1)",
    );
    h.drain_and_sync();
    h.publish_panels(2);
    assert_eq!(h.eval_all("sync.name"), s("synced"));
    let time = h.panel_instance("time");
    let (slot, revision) = h.option_slot(time);
    let target = if read_float_slot(&slot) == 3.0 { 5.0 } else { 3.0 };
    h.eval_all(&format!("(set! time.base {target})"));
    h.drain_and_sync();
    h.show_all();
    assert_eq!(read_float_slot(&slot), target, "the division follows the edit");
    assert_eq!(h.buffer_tree("*fx*").1, revision, "no panel rebuilt");
}
