//! The factory device panels' shared plumbing (param controls, the custom-UI
//! runtime, the panel frame, widgets and bodies, the effect strip's view
//! state), ported to the kinds (kind-bindings spec §13 stage 8,
//! eseq-0l17.14).

use super::views::{assert_ported, distro, instance_bindings, legacy_forms};
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
