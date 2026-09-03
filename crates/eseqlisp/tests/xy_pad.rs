//! End-to-end cover for the `xy-pad` builtin widget: it has to survive
//! evaluation, reach the layout engine with a real rect, and turn a simulated
//! press/drag into clamped `0..1` positional callback args.

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};
use eseqlisp::{
    Runtime,
    layout::{LayoutEngine, LayoutNode},
    vm::Value,
    widget_render::{EventOutput, MouseEventOutcome, widget_definition},
};

const UI: &str = r#"
(v-stack :width 40 :height 20
  (label "STRIKE" :font-size 8.2)
  (xy-pad :x 0.25 :y 0.75 :width 20 :height 10 :accent :cyan
          :on-change (lambda (x y) (list x y))))
"#;

fn find_widget<'a>(node: &'a LayoutNode, widget_type: &str) -> Option<&'a LayoutNode> {
    if node.widget_type == widget_type {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_widget(child, widget_type))
}

fn layout_pad() -> LayoutNode {
    let mut rt = Runtime::new();
    let tree = rt
        .eval_str(UI)
        .expect("xy-pad ui evaluates")
        .expect("ui tree");
    let layout = LayoutEngine::new(80, 30, 1.0)
        .layout(&tree)
        .expect("xy-pad ui layout");
    find_widget(&layout, "xy-pad")
        .expect("xy-pad node in layout")
        .clone()
}

/// Drives the real widget definition the way the app loop does: a mouse event
/// produces a `WidgetEvent`, which `handle_event` turns into a callback plus
/// positional args.
fn drag_to(node: &LayoutNode, kind: MouseEventKind, col: f32, row: f32) -> EventOutput {
    let definition = widget_definition("xy-pad").expect("xy-pad is a registered widget");
    let outcome = definition.mouse_event(
        node,
        kind,
        col,
        row,
        None,
        None,
        KeyModifiers::NONE,
        1.0,
        1.0,
    );
    let MouseEventOutcome::Dispatch(event) = outcome else {
        panic!("xy-pad should dispatch a position for {kind:?}");
    };
    definition
        .handle_event(node, event)
        .expect("xy-pad dispatches to :on-change")
}

fn args_as_pair(output: &EventOutput) -> (f64, f64) {
    assert_eq!(
        output.args.len(),
        2,
        "on-change takes two positional args, got {:?}",
        output.args.len()
    );
    let number = |value: &Value| match value {
        Value::Number(number) => *number,
        other => panic!("expected a number arg, got {other:?}"),
    };
    (number(&output.args[0]), number(&output.args[1]))
}

#[test]
fn xy_pad_lays_out_with_a_finite_nonzero_rect() {
    let pad = layout_pad();
    let rect = pad.rect;
    assert!(
        rect.width.is_finite()
            && rect.width > 0.0
            && rect.height.is_finite()
            && rect.height > 0.0
            && rect.col.is_finite()
            && rect.row.is_finite(),
        "xy-pad should have a finite nonzero rect, got {rect:?}"
    );
}

#[test]
fn press_and_drag_report_clamped_top_left_origin_positions() {
    let pad = layout_pad();
    let rect = pad.rect;

    // Press dead center.
    let center = drag_to(
        &pad,
        MouseEventKind::Down(MouseButton::Left),
        rect.col + rect.width * 0.5,
        rect.row + rect.height * 0.5,
    );
    let (x, y) = args_as_pair(&center);
    assert!(
        (x - 0.5).abs() < 1e-5 && (y - 0.5).abs() < 1e-5,
        "center press should be (0.5, 0.5), got ({x}, {y})"
    );

    // Drag a quarter across and three quarters down: y grows DOWNWARD.
    let quarter = drag_to(
        &pad,
        MouseEventKind::Drag(MouseButton::Left),
        rect.col + rect.width * 0.25,
        rect.row + rect.height * 0.75,
    );
    let (x, y) = args_as_pair(&quarter);
    assert!(
        (x - 0.25).abs() < 1e-5 && (y - 0.75).abs() < 1e-5,
        "expected (0.25, 0.75), got ({x}, {y})"
    );

    // Drag far outside the rect in both directions: `captures_drag` keeps the
    // gesture, and the widget clamps to the unit square.
    let low = drag_to(
        &pad,
        MouseEventKind::Drag(MouseButton::Left),
        rect.col - 500.0,
        rect.row - 500.0,
    );
    assert_eq!(args_as_pair(&low), (0.0, 0.0));
    let high = drag_to(
        &pad,
        MouseEventKind::Drag(MouseButton::Left),
        rect.col + rect.width + 500.0,
        rect.row + rect.height + 500.0,
    );
    assert_eq!(args_as_pair(&high), (1.0, 1.0));
}

#[test]
fn xy_pad_captures_drag_and_uses_no_shader() {
    let definition = widget_definition("xy-pad").expect("xy-pad is a registered widget");
    assert!(
        definition.captures_drag(),
        "drag must keep tracking outside the rect"
    );
    assert!(
        !definition.hidden_drag(),
        "xy-pad is an absolute-position control, not an infinite drag"
    );
    assert!(definition.bindable_props().contains(&"x"));
    assert!(definition.bindable_props().contains(&"y"));
}
