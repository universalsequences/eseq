//! `defwidget` instance state (docs/kind-bindings-spec.md §7.3; stage 5),
//! driven by a fake host's keyed kinds and a Lisp singleton.

use std::collections::HashMap;

use crate::layout::{LayoutNode, Rect};
use crate::reactive::read_float_slot;
use crate::runtime::Runtime;
use crate::vm::{InstanceId, VMError, Value};
use crate::widget_render::sdf_widget::{
    SdfFieldSource, sdf_widget_background_primitives, sdf_widget_def, sdf_widget_primitives,
};
use crate::widget_render::{GpuPrimitive, WidgetViewport};

/// A host-keyed `lane` with `cell`s under it, and a Lisp singleton `clock`.
const KINDS: &str = "
(def-kind lane :key (index)
  :host ((color :rgb) (level :number) (name :string) (cells (list-of cell))))
(def-kind cell :key (lane index)
  :host ((active :bool) (playing :bool) (selected :bool)))
(def-kind clock :key () :state ((playing false) (tempo 120)))";

/// `(sdf/fill (sdf/circle r) color)`: the step-cell of the spec, reading a
/// scalar, a cell's `:bool`, a lane's `:rgb` and a singleton's `:bool`.
const STEP_CELL: &str = "
(defwidget step-cell
  :width 4 :height 2
  :state (cell lane seed)
  :shader
  (sdf/layer
    (sdf/fill (sdf/circle (+ 0.3 (* 0.2 cell.active) seed))
      (rgba lane.color (if clock.playing 1 0.5)))))";

struct Host {
    runtime: Runtime,
    lane: InstanceId,
    cell: InstanceId,
}

fn host() -> Host {
    let mut runtime = Runtime::new();
    runtime.set_layout_viewport(40, 10);
    runtime.eval_str(KINDS).expect("kinds");
    let lane = runtime.register_keyed_instance("lane", &[0]).expect("lane");
    let cell = runtime
        .register_keyed_instance("cell", &[lane, 3])
        .expect("cell");
    runtime
        .set_instance_field(lane, "color", rgb(0.25, 0.5, 0.75))
        .expect("color");
    runtime
        .set_instance_field(
            lane,
            "cells",
            Value::List(vec![std::rc::Rc::new(std::cell::RefCell::new(
                Value::Instance(cell),
            ))]),
        )
        .expect("cells");
    runtime
        .set_instance_field(cell, "active", Value::Bool(true))
        .expect("active");
    runtime
        .eval_str("(def l0 (lane 0)) (def c0 (first l0.cells))")
        .expect("instances");
    Host {
        runtime,
        lane,
        cell,
    }
}

fn rgb(r: f64, g: f64, b: f64) -> Value {
    crate::vm::tagged_list(
        "rgb",
        vec![Value::Number(r), Value::Number(g), Value::Number(b)],
    )
}

fn error(runtime: &mut Runtime, code: &str) -> String {
    match runtime.eval_str(code) {
        Err(VMError::Instance(message)) => message,
        other => panic!("{code}: expected an error, got {other:?}"),
    }
}

fn viewport() -> WidgetViewport {
    WidgetViewport {
        vp_w: 400.0,
        vp_h: 200.0,
        cell_w: 10.0,
        cell_h: 20.0,
        scroll_top: 0.0,
        focused_widget_id: None,
        focused_branch: false,
        overlay_viewport_bottom: 10.0,
        inherited_hover: false,
        time_seconds: 0.0,
        scroll_left: 0.0,
    }
}

/// The 16 state floats a primitive carries, in slot order.
fn uniforms(primitives: &[GpuPrimitive]) -> Vec<f32> {
    let [GpuPrimitive::WidgetInstance { instance, .. }] = primitives else {
        panic!("expected one widget instance");
    };
    [
        instance.uniform_a,
        instance.uniform_b,
        instance.uniform_c,
        instance.uniform_d,
    ]
    .concat()
}

fn layout(runtime: &Runtime) -> LayoutNode {
    (**runtime.current_layout.as_ref().expect("layout")).clone()
}

fn assert_floats(actual: &[f32], expected: &[f32]) {
    assert!(
        actual.len() >= expected.len()
            && actual
                .iter()
                .zip(expected)
                .all(|(a, e)| (a - e).abs() < 1e-6),
        "uniforms {actual:?}, expected {expected:?}…"
    );
}

#[test]
fn instance_state_allocates_only_the_fields_the_shader_reads() {
    let mut host = host();
    host.runtime.eval_str(STEP_CELL).expect("defwidget");
    let def = sdf_widget_def("step-cell").expect("registered");
    assert_eq!(
        def.state_uniforms,
        [
            "seed",
            "cell.active",
            "lane.color|r",
            "lane.color|g",
            "lane.color|b",
            "clock.playing"
        ]
    );
    let sources: Vec<(&SdfFieldSource, &str, bool)> = def
        .state
        .plan
        .fields
        .iter()
        .map(|field| (&field.source, field.field.as_str(), field.rgb))
        .collect();
    assert_eq!(
        sources,
        [
            (&SdfFieldSource::Prop("cell".into()), "active", false),
            (&SdfFieldSource::Prop("lane".into()), "color", true),
            (
                &SdfFieldSource::Singleton("scratch:clock".into()),
                "playing",
                false
            ),
        ]
    );
    // `lane.color` is a float3 the shader widens as a color.
    assert!(
        def.shader_source.contains("sdf_state_lane__color")
            && def.shader_source.contains("sdf_state_cell__active"),
        "{}",
        def.shader_source
    );
}

#[test]
fn an_instance_prop_fills_its_uniforms_and_a_field_write_only_repaints() {
    let mut host = host();
    host.runtime.eval_str(STEP_CELL).expect("defwidget");
    host.runtime
        .eval_str("(effect (step-cell :cell c0 :lane l0 :seed 0.1))")
        .expect("render");
    let node = layout(&host.runtime);
    assert_eq!(node.widget_type, "step-cell");
    assert!(matches!(
        node.props.get("shader-state-cell.active"),
        Some(Value::ReactiveRef { field, .. }) if field == "active"
    ));
    assert_floats(
        &uniforms(&sdf_widget_primitives("step-cell", &node, viewport())),
        &[0.1, 1.0, 0.25, 0.5, 0.75, 0.0],
    );
    // Held by the widget, the host fields count as observed.
    assert!(host.runtime.host_field_observed(host.cell, "active"));
    assert!(host.runtime.host_field_observed(host.lane, "color"));
    assert!(!host.runtime.host_field_observed(host.lane, "level"));
    let _ = host.runtime.take_dirty_widget_ids();
    let _ = host.runtime.drain_rendered_layouts();

    // A singleton read through the shader: a Lisp write repaints.
    host.runtime
        .eval_str("(set! clock.playing true)")
        .expect("write");
    assert_eq!(host.runtime.take_dirty_widget_ids(), vec![node.widget_id]);
    // A host push repaints the same way.
    host.runtime
        .set_instance_field(host.cell, "active", Value::Bool(false))
        .expect("host write");
    host.runtime
        .set_instance_field(host.lane, "color", rgb(1.0, 0.0, 0.5))
        .expect("host write");
    assert_eq!(host.runtime.take_dirty_widget_ids(), vec![node.widget_id]);
    assert!(
        host.runtime.drain_rendered_layouts().is_empty(),
        "a bound field write must not re-render"
    );
    let node = layout(&host.runtime);
    assert_floats(
        &uniforms(&sdf_widget_primitives("step-cell", &node, viewport())),
        &[0.1, 0.0, 1.0, 0.0, 0.5, 1.0],
    );
}

#[test]
fn box_background_passes_instance_props_through() {
    let mut host = host();
    host.runtime.eval_str(STEP_CELL).expect("defwidget");
    let value = host
        .runtime
        .eval_str(r#"(box :background "step-cell" :cell c0 :lane l0 :seed 0.2 :width 4 :height 2)"#)
        .expect("box")
        .expect("widget");
    let Value::Map(map) = value else {
        panic!("box map");
    };
    let props: HashMap<String, Value> = map
        .iter()
        .map(|(key, value)| (key.clone(), value.borrow().clone()))
        .collect();
    let Some(Value::ReactiveRef { slot, .. }) = props.get("shader-state-lane.color|b") else {
        panic!("lane.color b binding: {props:?}");
    };
    assert_eq!(read_float_slot(slot), 0.75);
    let rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 4.0,
        height: 2.0,
    };
    assert_floats(
        &uniforms(&sdf_widget_background_primitives(
            "step-cell",
            7,
            rect,
            viewport(),
            &props,
        )),
        &[0.2, 1.0, 0.25, 0.5, 0.75, 0.0],
    );
}

#[test]
fn missing_instance_props_read_zero_and_the_wrong_kind_is_an_error() {
    let mut host = host();
    host.runtime.eval_str(STEP_CELL).expect("defwidget");
    host.runtime
        .eval_str("(effect (step-cell :seed 0.1))")
        .expect("no instances");
    let node = layout(&host.runtime);
    assert_floats(
        &uniforms(&sdf_widget_primitives("step-cell", &node, viewport())),
        &[0.1, 0.0, 0.0, 0.0, 0.0, 0.0],
    );
    let message = error(&mut host.runtime, "(step-cell :cell l0 :lane l0)");
    assert!(
        message.contains("step-cell: :cell takes an instance of kind 'scratch:cell'; got <lane#"),
        "{message}"
    );
}

#[test]
fn bad_fields_are_defwidget_errors_naming_the_fields() {
    let mut host = host();
    let message = error(
        &mut host.runtime,
        "(defwidget bad-cell :state (cell) :shader (sdf/circle cell.activ))",
    );
    assert_eq!(
        message,
        "bad-cell: kind 'scratch:cell' has no field 'activ'; bindable fields: active, playing, \
         selected"
    );
    let message = error(
        &mut host.runtime,
        "(defwidget bad-lane :state (lane) :shader (sdf/circle lane.name))",
    );
    assert_eq!(
        message,
        "bad-lane: field 'name' of kind 'scratch:lane' is :string, which is not bindable; \
         bindable fields: color, level"
    );
    let message = error(
        &mut host.runtime,
        "(defwidget bad-id :state (lane) :shader (sdf/circle lane.id))",
    );
    assert!(
        message.contains("'id' of kind 'scratch:lane' is a built-in field"),
        "{message}"
    );
    let message = error(
        &mut host.runtime,
        "(defwidget not-a-kind :state (knob) :shader (sdf/circle knob.value))",
    );
    assert!(
        message.contains(
            "not-a-kind: knob.value reads a field of :state 'knob', but no kind is named 'knob'"
        ),
        "{message}"
    );
    let message = error(
        &mut host.runtime,
        "(defwidget both-ways :state (cell) :shader (sdf/circle (+ cell cell.active)))",
    );
    assert_eq!(
        message,
        "both-ways: :state 'cell' is read both as a number (cell) and as an instance (cell.active)"
    );
    let message = error(
        &mut host.runtime,
        "(defwidget keyed-without-state :shader (sdf/circle cell.active))",
    );
    assert!(
        message.contains("cell.active reads kind 'scratch:cell', which is not a singleton"),
        "{message}"
    );
    assert!(sdf_widget_def("bad-cell").is_none());
}

#[test]
fn an_ambiguous_kind_name_lists_the_candidates() {
    let mut runtime = Runtime::new();
    runtime
        .eval_str("(module alpha) (def-kind dup :key () :state ((on false)))")
        .expect("alpha");
    runtime
        .eval_str("(module beta) (def-kind dup :key () :state ((on false)))")
        .expect("beta");
    let message = error(
        &mut runtime,
        "(defwidget dup-cell :state (dup) :shader (sdf/circle dup.on))",
    );
    assert_eq!(
        message,
        "dup-cell: :state 'dup' names several kinds (alpha:dup, beta:dup); use its kind id"
    );
    // The kind id resolves exactly.
    runtime
        .eval_str("(defwidget dup-cell :state (alpha:dup) :shader (sdf/circle alpha:dup.on))")
        .expect("exact kind id");
    let def = sdf_widget_def("dup-cell").expect("registered");
    assert_eq!(def.state.plan.fields[0].kind, "alpha:dup");
}

#[test]
fn the_uniform_budget_is_an_error_listing_the_allocation() {
    let mut host = host();
    let names: Vec<String> = (0..14).map(|i| format!("s{i}")).collect();
    let message = error(
        &mut host.runtime,
        &format!(
            "(defwidget too-many :state ({} lane) :shader (rgba lane.color (+ {})))",
            names.join(" "),
            names.join(" ")
        ),
    );
    assert_eq!(
        message,
        format!(
            "too-many: shader state needs 17 floats, over the budget of 16: {}, lane.color 3",
            names
                .iter()
                .map(|name| format!("{name} 1"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    );
    // Scalar state alone is held to the same budget (no silent truncation).
    let names: Vec<String> = (0..17).map(|i| format!("s{i}")).collect();
    let message = error(
        &mut host.runtime,
        &format!(
            "(defwidget too-many-scalars :state ({}) :shader (sdf/circle (+ {})))",
            names.join(" "),
            names.join(" ")
        ),
    );
    assert!(
        message.starts_with("too-many-scalars: shader state needs 17 floats"),
        "{message}"
    );
    // Fields the shader never reads cost nothing.
    let names: Vec<String> = (0..16).map(|i| format!("s{i}")).collect();
    host.runtime
        .eval_str(&format!(
            "(defwidget just-fits :state ({} lane cell) :shader (sdf/circle (+ {})))",
            names.join(" "),
            names.join(" ")
        ))
        .expect("16 floats fit");
}

#[test]
fn a_kind_named_state_read_bare_stays_scalar() {
    let mut host = host();
    host.runtime
        .eval_str("(defwidget legacy-lane :state (lane) :shader (sdf/circle lane))")
        .expect("scalar lane");
    let def = sdf_widget_def("legacy-lane").expect("registered");
    assert_eq!(def.state_uniforms, ["lane"]);
    assert!(def.state.plan.fields.is_empty());
}

#[test]
fn bindable_is_ignored_and_every_state_accepts_refs() {
    let mut runtime = Runtime::new();
    runtime.register_reactive(
        "APP",
        vec![("a", Value::Number(0.25)), ("b", Value::Number(0.5))],
        true,
    );
    let value = runtime
        .eval_str(
            r#"
            (defwidget partial-bindable
              :state (a b)
              :bindable (a)
              :shader (sdf/circle (+ a b)))
            (partial-bindable :a (bind "APP" "a") :b (bind "APP" "b"))
            (box :background "partial-bindable" :a 1 :b (bind "APP" "b"))
            "#,
        )
        .expect("evaluate")
        .expect("box");
    let Value::Map(map) = value else {
        panic!("box map");
    };
    assert!(!map.contains_key("__widget-diagnostic"));
    assert!(matches!(
        map.get("b").map(|value| value.borrow().clone()),
        Some(Value::ReactiveRef { .. })
    ));
    let value = runtime
        .eval_str(r#"(partial-bindable :a (bind "APP" "a") :b (bind "APP" "b"))"#)
        .expect("evaluate")
        .expect("widget");
    let Value::Map(map) = value else {
        panic!("widget map");
    };
    assert!(!map.contains_key("__widget-diagnostic"), "{map:?}");
    assert!(matches!(
        map.get("shader-state-b")
            .map(|value| value.borrow().clone()),
        Some(Value::ReactiveRef { .. })
    ));
}

#[test]
fn a_kind_reload_that_changes_a_field_the_shader_reads_is_an_error() {
    let mut host = host();
    host.runtime.eval_str(STEP_CELL).expect("defwidget");
    host.runtime
        .eval_str("(step-cell :cell c0 :lane l0)")
        .expect("construct");
    // Re-registering the kinds unchanged re-plans to the same fields.
    host.runtime.eval_str(KINDS).expect("reload kinds");
    host.runtime
        .eval_str("(step-cell :cell c0 :lane l0)")
        .expect("same plan after a reload");
    // lane.color turned from :rgb to :number: the compiled shader reads a
    // float3, so construction is an error (never three silent zeros).
    host.runtime
        .eval_str(
            "(def-kind lane :key (index)
               :host ((color :number) (level :number) (name :string) (cells (list-of cell))))",
        )
        .expect("reload lane");
    let message = error(&mut host.runtime, "(step-cell :cell c0 :lane l0)");
    assert_eq!(
        message,
        "step-cell: kind 'scratch:lane' changed since defwidget; re-evaluate it"
    );
    // Re-evaluating the defwidget plans against the new kind.
    host.runtime.eval_str(STEP_CELL).expect("re-evaluate");
    let def = sdf_widget_def("step-cell").expect("registered");
    assert!(def.state_uniforms.iter().any(|name| name == "lane.color"));
    host.runtime
        .eval_str("(step-cell :cell c0 :lane l0)")
        .expect("construct after re-evaluating");
}

#[test]
fn a_stale_rgb_instance_binds_detached_slots() {
    let mut host = host();
    host.runtime
        .eval_str(
            "(defwidget lane-dot :state (lane) :shader (sdf/fill (sdf/circle 0.5) lane.color))",
        )
        .expect("defwidget");
    assert!(host.runtime.drop_instance(host.lane));
    let store = host.runtime.reactive_binding_store();
    let before = store.slot_count();
    let value = host
        .runtime
        .eval_str("(lane-dot :lane l0)")
        .expect("construct")
        .expect("widget");
    assert_eq!(
        store.slot_count(),
        before,
        "a stale instance must not add slots under its namespace"
    );
    let Value::Map(map) = value else {
        panic!("widget map");
    };
    for component in ["r", "g", "b"] {
        let prop = format!("shader-state-lane.color|{component}");
        let Some(Value::ReactiveRef { slot, .. }) = map.get(&prop).map(|v| v.borrow().clone())
        else {
            panic!("{prop}: {map:?}");
        };
        assert_eq!(read_float_slot(&slot), 0.0);
    }
}

#[test]
fn colliding_shader_uniform_names_are_an_error() {
    let mut host = host();
    let message = error(
        &mut host.runtime,
        "(defwidget clash :state (lane__color lane) :shader (rgba lane.color lane__color))",
    );
    assert_eq!(
        message,
        "clash: state 'lane__color' and 'lane.color' name the same shader uniform; rename one"
    );
    assert!(sdf_widget_def("clash").is_none());
}

#[test]
fn a_shader_that_does_not_compile_is_a_defwidget_error() {
    // A material macro whose module is not loaded is left as a call the
    // shader compiler does not know (eseq-0l17.25: `rec-arm-dot` before
    // `eseq.materials`): an evaluation error naming the widget, nothing
    // registered. Once the macro exists the same form compiles.
    let mut host = host();
    let code =
        "(defwidget dot :shader (sdf/fill (sdf/circle 0.4) (shade.materials/color 0.5 0.2)))";
    let message = error(&mut host.runtime, code);
    assert!(
        message.starts_with("dot: shader error: "),
        "unexpected message: {message}"
    );
    assert!(message.contains("shade.materials/color"), "{message}");
    assert!(sdf_widget_def("dot").is_none());
    host.runtime
        .eval_str("(defmacro shade.materials/color (a b) `(rgba ,a ,b ,a 1))")
        .expect("macro");
    host.runtime
        .eval_str(code)
        .expect("compiles once the macro exists");
    assert!(sdf_widget_def("dot").is_some());
}

#[test]
fn only_scalar_states_accept_a_binding_ref() {
    let mut host = host();
    host.runtime.eval_str(STEP_CELL).expect("defwidget");
    let diagnostic = |runtime: &mut Runtime, code: &str| {
        let Some(Value::Map(map)) = runtime.eval_str(code).expect("evaluate") else {
            panic!("{code}: widget map");
        };
        map.get("__widget-diagnostic")
            .map(|value| value.borrow().clone())
    };
    assert_eq!(
        diagnostic(&mut host.runtime, "(step-cell :cell #'c0.active :lane l0)"),
        Some(Value::String(
            "step-cell: :cell does not accept reactive bindings".into()
        ))
    );
    // A uniform name is never a prop.
    assert!(
        diagnostic(
            &mut host.runtime,
            "(step-cell :cell c0 :cell.active #'c0.active)"
        )
        .is_some()
    );
    // A scalar state takes any ref, even one the shader never reads (a
    // view may bind a state only the host reads).
    assert_eq!(
        diagnostic(&mut host.runtime, "(step-cell :cell c0 :seed #'c0.active)"),
        None
    );
    host.runtime
        .eval_str("(defwidget unread-state :state (cell extra) :shader (sdf/circle cell.active))")
        .expect("defwidget");
    assert_eq!(
        diagnostic(
            &mut host.runtime,
            "(unread-state :cell c0 :extra #'c0.active)"
        ),
        None
    );
}

#[test]
fn singleton_fields_read_in_materials_and_sdf_to_metal() {
    let mut host = host();
    let Some(Value::String(shader)) = host
        .runtime
        .eval_str("(sdf->metal '(sdf/fill (sdf/circle 0.5) (rgba 1 0 0 clock.playing)))")
        .expect("sdf->metal")
    else {
        panic!("shader string");
    };
    assert!(shader.contains("sdf_state_clock__playing"), "{shader}");

    host.runtime
        .eval_str("(set! clock.playing true)")
        .expect("write");
    let Some(Value::Map(map)) = host
        .runtime
        .eval_str("(hslider :min 0 :max 1 :value 0.5 :material (material :color (rgba 1 0 0 clock.playing)))")
        .expect("material slider")
    else {
        panic!("slider map");
    };
    assert!(map.contains_key(crate::widget_render::sdf_widget::SHADER_TYPE_PROP));
    let Some(Value::ReactiveRef { slot, .. }) = map
        .get("shader-state-clock.playing")
        .map(|value| value.borrow().clone())
    else {
        panic!("clock.playing binding: {map:?}");
    };
    assert_eq!(read_float_slot(&slot), 1.0);
}
