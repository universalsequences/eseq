//! `drum-pad`: a circular drum head with several draggable points on it.
//!
//! The Modalys-style placer: a dark disc (the head) and up to eight numbered,
//! coloured dots (where the stick lands, where snares touch, where the mics
//! listen). Each dot's position is `(x, y)` in `-1..1` across the disc, with
//! `x` growing rightward and `y` growing UPWARD (the head's own coordinates,
//! centre `(0, 0)`), unlike `xy-pad`'s top-left screen origin.
//!
//! A press grabs the nearest dot (on empty head too) for the whole drag; the
//! widget reports `(k x y)` and does not move itself, so the handler writes
//! the values back into `:xk`/`:yk`. Drawing is plain primitives (circles for
//! the rim, disc and dots, proportional text for the labels): no shader.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, GpuCirclePrimitive, GpuCircleVisibleHalf, GpuPrimitive,
    GpuProportionalTextPrimitive, MouseEventOutcome, WidgetDefinition, WidgetEvent,
    WidgetViewport, get_f32_prop, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct DrumPadWidget;

pub static DRUM_PAD_WIDGET: DrumPadWidget = DrumPadWidget;

const DEFAULT_WIDTH: f32 = 14.0;
/// The most dots a pad shows.
pub const MAX_DOTS: usize = 8;
/// How far from the centre a dragged dot may go (a point on the rim itself
/// is clamped there and would not move).
const MAX_RADIUS: f32 = 0.95;
const X_PROPS: [&str; MAX_DOTS] = ["x1", "x2", "x3", "x4", "x5", "x6", "x7", "x8"];
const Y_PROPS: [&str; MAX_DOTS] = ["y1", "y2", "y3", "y4", "y5", "y6", "y7", "y8"];

/// How many dots the pad shows: `:count`, 1..8 (default 1).
pub fn dot_count(props: &HashMap<String, Value>) -> usize {
    (get_f32_prop(props, "count", 1.0).round().max(1.0) as usize).min(MAX_DOTS)
}

/// Dot `k`'s `(x, y)` (0-based), clamped onto the unit disc.
pub fn dot_position(props: &HashMap<String, Value>, k: usize) -> (f32, f32) {
    let x = get_f32_prop(props, X_PROPS[k], 0.0);
    let y = get_f32_prop(props, Y_PROPS[k], 0.0);
    clamp_to_disc(x, y, 1.0)
}

fn clamp_to_disc(x: f32, y: f32, radius: f32) -> (f32, f32) {
    let r = (x * x + y * y).sqrt();
    if r > radius && r > 0.0 {
        (x * radius / r, y * radius / r)
    } else {
        (x, y)
    }
}

/// The disc's centre and radii in cell units (a circle on screen: the
/// inscribed circle of the rect, in pixels, so the radii in cells differ when
/// cells are not square).
fn disc(rect: Rect, cell_w: f32, cell_h: f32) -> (f32, f32, f32, f32) {
    let radius_px = (rect.width * cell_w).min(rect.height * cell_h) * 0.5;
    (
        rect.col + rect.width * 0.5,
        rect.row + rect.height * 0.5,
        radius_px / cell_w.max(f32::EPSILON),
        radius_px / cell_h.max(f32::EPSILON),
    )
}

/// The cell size a node's geometry is read with when the event carries none:
/// the rect's own aspect, as if it were square on screen.
fn assumed_cells(rect: Rect) -> (f32, f32) {
    let w = rect.width.max(0.001);
    let h = rect.height.max(0.001);
    (h.max(w) / w, h.max(w) / h)
}

/// Maps a pointer position (layout cell units, as `mouse_event` receives)
/// onto the head's `-1..1` axes, y up, clamped to `MAX_RADIUS`.
pub fn position_from_local(node: &LayoutNode, local_col: f32, local_row: f32, cell_w: f32, cell_h: f32) -> (f32, f32) {
    let (cx, cy, rx, ry) = disc(node.rect, cell_w, cell_h);
    let x = (local_col - cx) / rx.max(0.001);
    let y = -(local_row - cy) / ry.max(0.001);
    clamp_to_disc(x, y, MAX_RADIUS)
}

/// The dot nearest a pointer position, measured on screen.
pub fn nearest_dot(node: &LayoutNode, local_col: f32, local_row: f32, cell_w: f32, cell_h: f32) -> usize {
    let (cx, cy, rx, ry) = disc(node.rect, cell_w, cell_h);
    (0..dot_count(&node.props))
        .map(|k| {
            let (x, y) = dot_position(&node.props, k);
            let dx = (cx + x * rx - local_col) * cell_w;
            let dy = (cy - y * ry - local_row) * cell_h;
            (k, dx * dx + dy * dy)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(0, |(k, _)| k)
}

fn dot_event(k: usize, x: f32, y: f32) -> WidgetEvent {
    WidgetEvent::Custom(Value::List(
        [k as f64, x as f64, y as f64]
            .into_iter()
            .map(|v| Rc::new(RefCell::new(Value::Number(v))))
            .collect(),
    ))
}

fn accent_color(props: &HashMap<String, Value>) -> Color {
    match props.get("accent").or_else(|| props.get("color")) {
        Some(value) => theme::parse_color_value(value).unwrap_or_else(theme::WIDGET_KNOB_FILLED),
        None => theme::WIDGET_KNOB_FILLED(),
    }
}

fn list_item(props: &HashMap<String, Value>, key: &str, k: usize) -> Option<Value> {
    match props.get(key) {
        Some(Value::List(items)) => items.get(k).map(|item| item.borrow().clone()),
        _ => None,
    }
}

/// Dot `k`'s colour: `:colors`'s k-th, else the accent.
fn dot_color(props: &HashMap<String, Value>, k: usize) -> Color {
    list_item(props, "colors", k)
        .and_then(|value| theme::parse_color_value(&value))
        .unwrap_or_else(|| accent_color(props))
}

/// Dot `k`'s label: `:labels`'s k-th, else its number from 1.
fn dot_label(props: &HashMap<String, Value>, k: usize) -> String {
    match list_item(props, "labels", k) {
        Some(Value::String(text)) => text,
        Some(Value::Number(n)) => format!("{n}"),
        _ => format!("{}", k + 1),
    }
}

fn darker(color: Color, by: f32) -> Color {
    Color { r: color.r * by, g: color.g * by, b: color.b * by, ..color }
}

impl WidgetDefinition for DrumPadWidget {
    fn names(&self) -> &'static [&'static str] {
        &["drum-pad"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "size"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &[
            "x1", "y1", "x2", "y2", "x3", "y3", "x4", "y4", "x5", "y5", "x6", "y6", "x7", "y7", "x8", "y8",
        ]
    }

    fn doc(&self) -> &'static str {
        "A round drum head with up to eight numbered dots on it, for placing things \
(where a stick lands, where snares touch, where mics listen). :count dots (1..8) sit \
at :x1 :y1 .. :x8 :y8 (bindable), each -1..1 across the head with x growing \
rightward and y growing UPWARD (centre 0 0). A press grabs the nearest dot (on empty \
head too) and dragging it calls :on-change with three numbers, (k x y): k the dot \
from 0, x and y clamped inside radius 0.95. The pad does not move itself, so write \
them back. :labels (strings, default \"1\"..) and :colors (one per dot, default \
:accent) style the dots; :background fills the head, :rim-color edges it; :dot-size \
is a dot's radius in pixels (default 9), :font-size its label's. It is :size cells \
wide (default 14) and square unless :height is given. Example: (drum-pad :count 3 \
:x1 sx :y1 sy :x2 ax :y2 ay :x3 bx :y3 by :labels (list \"1\" \"2\" \"3\") \
:colors (list :blue :gray :green) :on-change (lambda (k x y) (place! k x y)))"
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "count", "x1", "y1", "x2", "y2", "x3", "y3", "x4", "y4", "x5", "y5", "x6", "y6", "x7", "y7", "x8",
            "y8", "labels", "colors", "width", "height", "size", "accent", "color", "background", "rim-color",
            "dot-size", "font-size", "on-change",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[&Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let default_size = get_prop_num(node, "size").map(f64_to_f32).unwrap_or(DEFAULT_WIDTH);
        let width = get_prop_num(node, "width").map(f64_to_f32).unwrap_or(default_size).max(1.0);
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or_else(|| width / constraints.aspect.max(f32::EPSILON))
            .max(1.0);
        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let width = rect.width.round().max(1.0) as u16;
        let height = rect.height.round().max(1.0) as u16;
        let row0 = rect.row.round() as u16;
        let col0 = rect.col.round() as u16;
        let track = theme::WIDGET_KNOB_TRACK();
        let half_w = (width as f32 - 1.0).max(1.0) * 0.5;
        let half_h = (height as f32 - 1.0).max(1.0) * 0.5;
        for row in 0..height {
            for col in 0..width {
                let x = (col as f32 - half_w) / half_w;
                let y = (row as f32 - half_h) / half_h;
                let r = (x * x + y * y).sqrt();
                if (r - 1.0).abs() < 0.5 / half_w.min(half_h).max(1.0) + 0.08 {
                    buf.set(row0 + row, col0 + col, styled_cell('·', track, None));
                }
            }
        }
        for k in 0..dot_count(props) {
            let (x, y) = dot_position(props, k);
            let col = ((half_w + x * half_w).round() as u16).min(width - 1);
            let row = ((half_h - y * half_h).round() as u16).min(height - 1);
            let ch = dot_label(props, k).chars().next().unwrap_or('●');
            buf.set(row0 + row, col0 + col, styled_cell(ch, dot_color(props, k), None));
        }
    }

    fn captures_drag(&self) -> bool {
        true
    }

    /// The pointer may leave the rect during a drag; the dot then follows it
    /// to the rim (`position_from_local` clamps).
    fn unclamped_drag(&self) -> bool {
        true
    }

    fn begin_gesture(&self, node: &LayoutNode, local_col: f32, local_row: f32, _modifiers: KeyModifiers) -> Option<Value> {
        let local_row = super::scroll::event_content_row(local_row);
        let (cell_w, cell_h) = assumed_cells(node.rect);
        Some(Value::Number(nearest_dot(node, local_col, local_row, cell_w, cell_h) as f64))
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        cell_w: f32,
        cell_h: f32,
    ) -> MouseEventOutcome {
        let local_row = super::scroll::event_content_row(local_row);
        let (cell_w, cell_h) = if cell_w > 0.0 && cell_h > 0.0 { (cell_w, cell_h) } else { assumed_cells(node.rect) };
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                let k = match gesture {
                    Some(Value::Number(n)) => (*n as usize).min(dot_count(&node.props) - 1),
                    _ => nearest_dot(node, local_col, local_row, cell_w, cell_h),
                };
                let (x, y) = position_from_local(node, local_col, local_row, cell_w, cell_h);
                MouseEventOutcome::Dispatch(dot_event(k, x, y))
            }
            MouseEventKind::Up(MouseButton::Left) => MouseEventOutcome::Consume,
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(Value::List(items)) = event else {
            return None;
        };
        let callback = node.props.get("on-change")?.clone();
        let number = |k: usize| match items.get(k).map(|item| item.borrow().clone()) {
            Some(Value::Number(n)) => n,
            _ => 0.0,
        };
        let (x, y) = clamp_to_disc(number(1) as f32, number(2) as f32, MAX_RADIUS);
        Some(EventOutput {
            callback,
            args: vec![Value::Number(number(0).max(0.0).floor()), Value::Number(x as f64), Value::Number(y as f64)],
        })
    }

    fn build_primitives(&self, _widget_type: &str, node: &LayoutNode, viewport: WidgetViewport) -> Vec<GpuPrimitive> {
        let rect = node.rect;
        if !(rect.width.is_finite() && rect.height.is_finite()) || rect.width <= 0.0 || rect.height <= 0.0 {
            return Vec::new();
        }
        let props = &node.props;
        let cell_w = viewport.cell_w.max(1.0);
        let cell_h = viewport.cell_h.max(1.0);
        let (cx, cy, rx, ry) = disc(rect, cell_w, cell_h);
        let radius_px = rx * cell_w;
        let rim_px = super::ui_design_px(1.5).min(radius_px * 0.2);
        let background = resolve_named_color(props, "background", theme::BUTTON_GHOST_BG());
        let rim = resolve_named_color(props, "rim-color", theme::WIDGET_KNOB_TRACK());
        let circle = |center: [f32; 2], radius_px: f32, color: Color| {
            GpuPrimitive::Circle(GpuCirclePrimitive { center, radius_px, color, visible_half: GpuCircleVisibleHalf::Full })
        };
        let mut prims = vec![
            circle([cx, cy], radius_px, rim),
            circle([cx, cy], (radius_px - rim_px).max(0.5), background),
        ];
        let dot_px = super::ui_design_px(get_f32_prop(props, "dot-size", 9.0)).max(2.0);
        let outline_px = super::ui_design_px(2.0).min(dot_px * 0.4);
        let font_size = get_f32_prop(props, "font-size", 8.0);
        for k in 0..dot_count(props) {
            let (x, y) = dot_position(props, k);
            let center = [cx + x * rx, cy - y * ry];
            let color = dot_color(props, k);
            prims.push(circle(center, dot_px, darker(color, 0.55)));
            prims.push(circle(center, dot_px - outline_px, color));
            let label_w = dot_px * 2.0 / cell_w;
            prims.push(GpuPrimitive::ProportionalText(GpuProportionalTextPrimitive {
                row: center[1] - 0.5,
                col: center[0] - label_w * 0.5,
                align_width: label_w,
                h_align: 0.5,
                text: dot_label(props, k),
                font_size,
                scale: 1.0,
                fg: Color::rgba(0.92, 0.92, 0.92, 1.0),
                bg: Color::rgba(0.0, 0.0, 0.0, 0.0),
                face: crate::ui::glyph_atlas::TextFace::Bold,
            }));
        }
        prims
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::LayoutAnimationHints;

    fn node(props: HashMap<String, Value>) -> LayoutNode {
        LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "drum-pad".to_string(),
            rect: Rect { col: 10.0, row: 4.0, width: 20.0, height: 10.0 },
            props,
            children: Vec::new(),
            focusable: false,
            animation: LayoutAnimationHints::default(),
        }
    }

    fn dots(points: &[(f64, f64)]) -> HashMap<String, Value> {
        let mut props = HashMap::new();
        props.insert("count".to_string(), Value::Number(points.len() as f64));
        for (k, (x, y)) in points.iter().enumerate() {
            props.insert(X_PROPS[k].to_string(), Value::Number(*x));
            props.insert(Y_PROPS[k].to_string(), Value::Number(*y));
        }
        props
    }

    // a 20 x 10 cell rect with 8 x 16 px cells is 160 x 160 px: a disc of
    // radius 80 px, 10 cells across and 5 down, centred at (20, 9)
    const CW: f32 = 8.0;
    const CH: f32 = 16.0;

    #[test]
    fn drum_pad_maps_the_centre_to_zero_and_clamps_inside_the_rim() {
        let node = node(HashMap::new());
        let (x, y) = position_from_local(&node, 20.0, 9.0, CW, CH);
        assert!(x.abs() < 1e-6 && y.abs() < 1e-6, "centre is (0,0): {x} {y}");
        // the top of the disc is y = +1 (up), clamped to the 0.95 rim
        let (x, y) = position_from_local(&node, 20.0, 4.0, CW, CH);
        assert!(x.abs() < 1e-6 && (y - 0.95).abs() < 1e-6, "top: {x} {y}");
        // half way to the right edge of the disc
        let (x, y) = position_from_local(&node, 25.0, 9.0, CW, CH);
        assert!((x - 0.5).abs() < 1e-6 && y.abs() < 1e-6, "right: {x} {y}");
        let (x, y) = position_from_local(&node, 500.0, -500.0, CW, CH);
        assert!(((x * x + y * y).sqrt() - 0.95).abs() < 1e-5, "far outside clamps to the rim");
    }

    #[test]
    fn drum_pad_gesture_grabs_the_nearest_dot() {
        let node = node(dots(&[(-0.5, 0.0), (0.5, 0.5), (0.0, -0.6)]));
        let (cell_w, cell_h) = assumed_cells(node.rect);
        assert_eq!(nearest_dot(&node, 25.0, 6.5, cell_w, cell_h), 1);
        assert_eq!(nearest_dot(&node, 20.0, 12.0, cell_w, cell_h), 2);
        let gesture = DRUM_PAD_WIDGET.begin_gesture(&node, 14.0, 9.0, KeyModifiers::NONE).unwrap();
        assert_eq!(gesture, Value::Number(0.0));
    }

    #[test]
    fn drum_pad_drag_dispatches_the_grabbed_dot_and_its_clamped_position() {
        let mut props = dots(&[(-0.5, 0.0), (0.5, 0.5)]);
        props.insert("on-change".to_string(), Value::Keyword("cb".to_string()));
        let node = node(props);
        // dot 0 grabbed, dragged far up and right: it stays dot 0 and lands on the rim
        let gesture = Value::Number(0.0);
        let outcome = DRUM_PAD_WIDGET.mouse_event(
            &node,
            MouseEventKind::Drag(MouseButton::Left),
            100.0,
            -50.0,
            None,
            Some(&gesture),
            KeyModifiers::NONE,
            CW,
            CH,
        );
        let MouseEventOutcome::Dispatch(event) = outcome else { panic!("drag should dispatch") };
        let output = DRUM_PAD_WIDGET.handle_event(&node, event).expect("on-change output");
        let numbers: Vec<f64> = output
            .args
            .iter()
            .map(|arg| match arg {
                Value::Number(n) => *n,
                other => panic!("expected a number, got {other:?}"),
            })
            .collect();
        assert_eq!(numbers.len(), 3);
        assert_eq!(numbers[0], 0.0);
        assert!(numbers[1] > 0.0 && numbers[2] > 0.0, "up and right: {numbers:?}");
        assert!(((numbers[1].powi(2) + numbers[2].powi(2)).sqrt() - 0.95).abs() < 1e-5);
    }

    #[test]
    fn drum_pad_draws_a_disc_and_a_circle_per_dot_without_a_shader() {
        let node = node(dots(&[(0.0, 0.0), (0.5, 0.0), (0.0, 0.5)]));
        let prims = DRUM_PAD_WIDGET.build_primitives(
            "drum-pad",
            &node,
            WidgetViewport {
                cell_w: CW,
                cell_h: CH,
                vp_w: 640.0,
                vp_h: 360.0,
                time_seconds: 0.0,
                focused_widget_id: None,
                focused_branch: false,
                overlay_viewport_bottom: 36.0,
                scroll_top: 0.0,
                scroll_left: 0.0,
                inherited_hover: false,
            },
        );
        assert!(DRUM_PAD_WIDGET.fragment_shader("drum-pad", super::super::ShaderBackend::Wgsl).is_none());
        let circles: Vec<&GpuCirclePrimitive> = prims
            .iter()
            .filter_map(|p| match p {
                GpuPrimitive::Circle(c) => Some(c),
                _ => None,
            })
            .collect();
        // rim + disc, then an outline and a fill per dot
        assert_eq!(circles.len(), 2 + 2 * 3);
        assert!((circles[0].radius_px - 80.0).abs() < 1e-3, "the disc fills the 160 px square");
        assert_eq!(circles[0].center, [20.0, 9.0]);
        // dot 1 at x = 0.5: half the disc's 10-cell half-width right of centre
        assert!((circles[4].center[0] - 25.0).abs() < 1e-5);
        // dot 2 at y = 0.5 is ABOVE the centre (y up): 2.5 rows up
        assert!((circles[6].center[1] - 6.5).abs() < 1e-5);
        let labels = prims.iter().filter(|p| matches!(p, GpuPrimitive::ProportionalText(_))).count();
        assert_eq!(labels, 3);
    }
}
