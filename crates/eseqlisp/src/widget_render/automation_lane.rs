//! `automation-lane`: the piano roll's parameter-lock strip.
//!
//! One horizontal lane under a `timeline` sharing its `view-start` /
//! `view-duration` axis. Each `:points` entry is a step: a dot at the step's
//! onset and, for a triggered note, a horizontal bar spanning the note's
//! duration at the parameter's value (the Ableton velocity-lane idiom).
//! Locked points draw in `:color`; points showing the base (unlocked) value
//! draw in `:base-color`, so a device parameter's "what applies without a
//! lock" reads as gray until the user drags it into a lock.
//!
//! The widget is value-agnostic: it maps `:min..:max` onto its height and
//! reports `(kind step value)` to `on-change`, where `kind` is `:set` for a
//! press/drag frame, `:finish` on release and `:clear` on double-click. The
//! host decides what a value means for the selected parameter.
//!
//! Plain primitives only (rects + circles), so no shader pair to maintain.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::{
    get_f32_prop, resolve_named_color, styled_cell, CellBuffer, EventOutput, GpuCirclePrimitive,
    GpuCircleVisibleHalf, GpuPrimitive, GpuRectPrimitive, MouseEventOutcome, WidgetDefinition,
    WidgetEvent, WidgetViewport,
};
use crate::backend::Color;
use crate::layout::{f64_to_f32, get_prop_num, Constraints, LayoutNode, MeasureCtx, Rect, Size};
use crate::theme;
use crate::vm::Value;

pub struct AutomationLaneWidget;

pub static AUTOMATION_LANE_WIDGET: AutomationLaneWidget = AutomationLaneWidget;

const DEFAULT_HEIGHT: f32 = 4.0;
/// Horizontal hit tolerance around a point's onset, in design pixels.
const HIT_TOLERANCE_PX: f32 = 7.0;

thread_local! {
    // Only one lane can be under the pointer. Avoid retaining state for every
    // lane ever mounted while switching tracks and parameters.
    static HOVERED_POINT: Cell<Option<(u64, usize)>> = const { Cell::new(None) };
}

fn set_hovered_point(widget_id: u64, point: Option<LanePoint>) {
    let next = point.map(|point| (widget_id, point.step));
    HOVERED_POINT.with(|hovered| {
        let previous = hovered.replace(next);
        if previous != next {
            if let Some((previous_id, _)) = previous {
                super::bump_widget_state_revision(previous_id);
            }
            super::bump_widget_state_revision(widget_id);
        }
    });
}

fn hovered_step(widget_id: u64) -> Option<usize> {
    if !super::pointer_hovered(widget_id) {
        return None;
    }
    HOVERED_POINT.with(|hovered| {
        hovered.get().filter(|(id, _)| *id == widget_id).map(|(_, step)| step)
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LanePoint {
    pub step: usize,
    pub start: f64,
    pub end: f64,
    pub value: f32,
    pub locked: bool,
    /// A triggered note (draws the duration bar); an off-step lock has none.
    pub active: bool,
}

fn map_num(map: &HashMap<String, Rc<RefCell<Value>>>, key: &str) -> Option<f64> {
    match &*map.get(key)?.borrow() {
        Value::Number(n) if n.is_finite() => Some(*n),
        _ => None,
    }
}

fn map_bool(map: &HashMap<String, Rc<RefCell<Value>>>, key: &str, default: bool) -> bool {
    match map.get(key).map(|cell| cell.borrow().clone()) {
        Some(Value::Bool(b)) => b,
        Some(Value::Number(n)) => n > 0.5,
        Some(Value::Nil) => false,
        _ => default,
    }
}

pub fn lane_points(props: &HashMap<String, Value>) -> Vec<LanePoint> {
    let Some(Value::List(items)) = props.get("points") else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let Value::Map(map) = &*item.borrow() else {
                return None;
            };
            let step = map_num(map, "step")?.max(0.0) as usize;
            let start = map_num(map, "start").unwrap_or(step as f64);
            let end = map_num(map, "end").unwrap_or(start).max(start);
            let value = map_num(map, "value")? as f32;
            Some(LanePoint {
                step,
                start,
                end,
                value,
                locked: map_bool(map, "locked", true),
                active: map_bool(map, "active", true),
            })
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub struct LaneScale {
    pub min: f32,
    pub max: f32,
    pub increment: f32,
}

pub fn lane_scale(props: &HashMap<String, Value>) -> LaneScale {
    let min = get_f32_prop(props, "min", 0.0);
    let mut max = get_f32_prop(props, "max", 1.0);
    if max <= min {
        max = min + 1.0;
    }
    LaneScale {
        min,
        max,
        increment: get_f32_prop(props, "increment", 0.0).max(0.0),
    }
}

fn view_axis(props: &HashMap<String, Value>) -> (f64, f64) {
    let start = get_f32_prop(props, "view-start", 0.0) as f64;
    let duration = (get_f32_prop(props, "view-duration", 16.0) as f64).max(0.0001);
    (start, duration)
}

/// Column for `time`, in the same cell space the sibling `timeline` uses for
/// its content area: the lane's whole width is the visible view span.
pub fn x_for_time(rect: Rect, view_start: f64, view_duration: f64, time: f64) -> f32 {
    let t = (time - view_start) / view_duration;
    rect.col + (rect.width as f64 * t) as f32
}

/// Vertical inset of the value axis, in design pixels: a dot at `min` or
/// `max` stays whole instead of straddling the lane's edge.
const VALUE_INSET_PX: f32 = 6.0;

/// The rect the value axis spans: the widget rect inset top and bottom.
pub fn value_rect(rect: Rect, cell_h: f32) -> Rect {
    let inset = (super::ui_design_px(VALUE_INSET_PX) / cell_h.max(1.0)).min(rect.height * 0.25);
    Rect {
        row: rect.row + inset,
        height: (rect.height - inset * 2.0).max(0.0),
        ..rect
    }
}

/// Row for `value`: `max` at the top edge, `min` at the bottom edge of
/// `rect` (pass the `value_rect`).
pub fn y_for_value(rect: Rect, scale: LaneScale, value: f32) -> f32 {
    let t = ((value - scale.min) / (scale.max - scale.min)).clamp(0.0, 1.0);
    rect.row + rect.height * (1.0 - t)
}

/// Inverse of `y_for_value`, snapped to `increment` when one is set.
pub fn value_for_row(rect: Rect, scale: LaneScale, local_row: f32) -> f32 {
    let t = if rect.height > 0.0 {
        (1.0 - (local_row - rect.row) / rect.height).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let raw = scale.min + t * (scale.max - scale.min);
    if scale.increment > 0.0 {
        (((raw - scale.min) / scale.increment).round() * scale.increment + scale.min)
            .clamp(scale.min, scale.max)
    } else {
        raw
    }
}

/// Hit the full duration of an active point, not just its onset. Like the
/// original velocity-lane interaction, the hit region spans the lane's height
/// so pressing above/below a bar can immediately set its value.
/// Onsets take priority over bars; overlapping bars prefer the latest onset.
/// Selection depends only on time, so value changes cannot retarget a drag.
pub fn point_at_col(node: &LayoutNode, local_col: f32, cell_w: f32) -> Option<LanePoint> {
    let (view_start, view_duration) = view_axis(&node.props);
    let tolerance = super::ui_design_px(HIT_TOLERANCE_PX) / cell_w.max(1.0);
    if local_col < node.rect.col || local_col > node.rect.col + node.rect.width {
        return None;
    }
    let points = lane_points(&node.props);
    let onset = points.iter().filter_map(|point| {
        let x = x_for_time(node.rect, view_start, view_duration, point.start);
        let distance = (x - local_col).abs();
        // A clipped-away dot must not steal the visible end of another bar.
        (x >= node.rect.col && x <= node.rect.col + node.rect.width && distance <= tolerance)
            .then_some((distance, *point))
    }).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, point)| point);
    onset.or_else(|| {
        points.into_iter().filter(|point| {
            let x0 = x_for_time(node.rect, view_start, view_duration, point.start);
            let x1 = x_for_time(node.rect, view_start, view_duration, point.end);
            point.active && x1 > x0 && local_col >= x0 && local_col <= x1
        }).max_by(|a, b| a.start.total_cmp(&b.start).then(a.step.cmp(&b.step)))
    })
}

fn lane_event(kind: &str, step: usize, value: f32) -> WidgetEvent {
    WidgetEvent::Custom(Value::List(vec![
        Rc::new(RefCell::new(Value::Keyword(kind.to_string()))),
        Rc::new(RefCell::new(Value::Number(step as f64))),
        Rc::new(RefCell::new(Value::Number(value as f64))),
    ]))
}

fn accent_color(props: &HashMap<String, Value>) -> Color {
    match props.get("color") {
        Some(value) => theme::parse_color_value(value).unwrap_or_else(theme::WIDGET_KNOB_FILLED),
        None => theme::WIDGET_KNOB_FILLED(),
    }
}

fn base_color(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(
        props,
        "base-color",
        Color {
            r: 0.55,
            g: 0.55,
            b: 0.55,
            a: 1.0,
        },
    )
}

fn background_color(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "background", theme::BUTTON_GHOST_BG())
}

impl WidgetDefinition for AutomationLaneWidget {
    fn names(&self) -> &'static [&'static str] {
        &["automation-lane"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "points",
            "min",
            "max",
            "default",
            "increment",
            "view-start",
            "view-duration",
            "width",
            "height",
            "color",
            "base-color",
            "background",
            "on-change",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or(constraints.max_width)
            .max(1.0);
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(DEFAULT_HEIGHT)
            .max(1.0);
        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let scale = lane_scale(props);
        let (view_start, view_duration) = view_axis(props);
        let accent = accent_color(props);
        let base = base_color(props);
        let row_max = (rect.row + rect.height - 1.0).max(rect.row);
        for point in lane_points(props) {
            let col = x_for_time(rect, view_start, view_duration, point.start).round();
            if col < rect.col || col >= rect.col + rect.width {
                continue;
            }
            // Terminal cells are far coarser than the pixel inset; map over
            // the full rect.
            let row = y_for_value(rect, scale, point.value)
                .round()
                .clamp(rect.row, row_max);
            let fg = if point.locked { accent } else { base };
            buf.set(row as u16, col as u16, styled_cell('●', fg, None));
        }
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        modifiers: KeyModifiers,
        cell_w: f32,
        cell_h: f32,
    ) -> MouseEventOutcome {
        // The anchor is the point hit where the press LANDED, so a drag
        // that wanders sideways keeps editing the same step.
        let anchor_col = drag_start.map(|(col, _)| col).unwrap_or(local_col);
        let axis = value_rect(node.rect, cell_h);
        match mouse_kind {
            MouseEventKind::Moved => {
                set_hovered_point(node.widget_id, point_at_col(node, local_col, cell_w));
                MouseEventOutcome::Consume
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(point) = point_at_col(node, local_col, cell_w) else {
                    return MouseEventOutcome::Ignore;
                };
                set_hovered_point(node.widget_id, Some(point));
                if modifiers.contains(KeyModifiers::ALT) {
                    return MouseEventOutcome::Dispatch(lane_event(
                        "clear",
                        point.step,
                        point.value,
                    ));
                }
                let scale = lane_scale(&node.props);
                let value = value_for_row(axis, scale, local_row);
                MouseEventOutcome::Dispatch(lane_event("set", point.step, value))
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(point) = point_at_col(node, anchor_col, cell_w) else {
                    return MouseEventOutcome::Consume;
                };
                let scale = lane_scale(&node.props);
                let value = value_for_row(axis, scale, local_row);
                MouseEventOutcome::Dispatch(lane_event("set", point.step, value))
            }
            MouseEventKind::Up(MouseButton::Left) => match point_at_col(node, anchor_col, cell_w) {
                Some(point) => {
                    MouseEventOutcome::Dispatch(lane_event("finish", point.step, point.value))
                }
                None => MouseEventOutcome::Consume,
            },
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn double_click_event(
        &self,
        node: &LayoutNode,
        local_col: f32,
        _local_row: f32,
    ) -> Option<WidgetEvent> {
        // Double-click cannot see the cell width; the design-pixel tolerance
        // is small enough that one cell is a fair stand-in.
        let point = point_at_col(node, local_col, 1.0)?;
        Some(lane_event("clear", point.step, point.value))
    }

    fn captures_drag(&self) -> bool {
        true
    }

    /// Values clamp to the lane's range, so the pointer may leave the rect
    /// mid-drag and keep pinning the value to an edge.
    fn unclamped_drag(&self) -> bool {
        true
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(value) = event else {
            return None;
        };
        let callback = node.props.get("on-change")?.clone();
        let args = match &value {
            Value::List(items) => items.iter().map(|item| item.borrow().clone()).collect(),
            other => vec![other.clone()],
        };
        Some(EventOutput { callback, args })
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let rect = node.rect;
        if !(rect.width.is_finite() && rect.height.is_finite())
            || rect.width <= 0.0
            || rect.height <= 0.0
        {
            return Vec::new();
        }
        let scale = lane_scale(&node.props);
        let (view_start, view_duration) = view_axis(&node.props);
        let accent = accent_color(&node.props);
        let base = base_color(&node.props);
        let cell_h = viewport.cell_h.max(1.0);
        let hairline_h = (super::ui_design_px(1.0) / cell_h).min(rect.height * 0.25);
        let bar_h = (super::ui_design_px(3.5) / cell_h).min(rect.height * 0.25);
        let right = rect.col + rect.width;
        let axis = value_rect(rect, cell_h);

        let mut prims = vec![GpuPrimitive::Rect(GpuRectPrimitive {
            rect,
            color: background_color(&node.props),
        })];
        // Faint rule at the parameter's default, so a lane full of gray base
        // points still reads against "unchanged".
        if let Some(default) = node.props.get("default").and_then(|value| match value {
            Value::Number(n) if n.is_finite() => Some(*n as f32),
            _ => None,
        }) {
            let y = y_for_value(axis, scale, default);
            prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    row: y - hairline_h * 0.5,
                    height: hairline_h,
                    ..rect
                },
                color: Color {
                    a: base.a * 0.25,
                    ..base
                },
            }));
        }

        let hovered = hovered_step(node.widget_id);
        let dot_px = super::ui_design_px(get_f32_prop(&node.props, "dot-size", 5.5)).max(1.0);
        let mut dots = Vec::new();
        for point in lane_points(&node.props) {
            let x0 = x_for_time(rect, view_start, view_duration, point.start);
            let x1 = x_for_time(rect, view_start, view_duration, point.end);
            if x1 < rect.col || x0 > right {
                continue;
            }
            let y = y_for_value(axis, scale, point.value);
            let mut color = if point.locked { accent } else { base };
            if hovered == Some(point.step) {
                // Lift the existing locked/base color toward white, preserving
                // the distinction between locked and inherited values.
                color = Color {
                    r: color.r + (1.0 - color.r) * 0.4,
                    g: color.g + (1.0 - color.g) * 0.4,
                    b: color.b + (1.0 - color.b) * 0.4,
                    ..color
                };
            }
            if point.active && x1 > x0 {
                let bar_left = x0.max(rect.col);
                let bar_right = x1.min(right);
                prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                    rect: Rect {
                        col: bar_left,
                        row: y - bar_h * 0.5,
                        width: (bar_right - bar_left).max(0.0),
                        height: bar_h,
                    },
                    color: Color {
                        a: color.a * 0.85,
                        ..color
                    },
                }));
            }
            if x0 >= rect.col && x0 <= right {
                // Dots last so they sit above every bar.
                dots.push(GpuPrimitive::Circle(GpuCirclePrimitive {
                    center: [x0, y],
                    radius_px: dot_px,
                    color,
                    visible_half: GpuCircleVisibleHalf::Full,
                }));
            }
        }
        prims.extend(dots);
        prims
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::LayoutAnimationHints;

    fn point_value(step: f64, start: f64, end: f64, value: f64, locked: bool) -> Value {
        let mut map = HashMap::new();
        for (key, value) in [
            ("step", Value::Number(step)),
            ("start", Value::Number(start)),
            ("end", Value::Number(end)),
            ("value", Value::Number(value)),
            ("locked", Value::Bool(locked)),
            ("active", Value::Bool(true)),
        ] {
            map.insert(key.to_string(), Rc::new(RefCell::new(value)));
        }
        Value::Map(map)
    }

    fn node(props: HashMap<String, Value>) -> LayoutNode {
        LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "automation-lane".to_string(),
            rect: Rect {
                col: 10.0,
                row: 4.0,
                width: 16.0,
                height: 4.0,
            },
            props,
            children: Vec::new(),
            focusable: false,
            animation: LayoutAnimationHints::default(),
        }
    }

    fn velocity_props() -> HashMap<String, Value> {
        let mut props = HashMap::new();
        props.insert(
            "points".to_string(),
            Value::List(vec![
                Rc::new(RefCell::new(point_value(0.0, 0.0, 1.0, 100.0, true))),
                Rc::new(RefCell::new(point_value(4.0, 4.0, 6.0, 64.0, false))),
            ]),
        );
        props.insert("min".to_string(), Value::Number(0.0));
        props.insert("max".to_string(), Value::Number(127.0));
        props.insert("increment".to_string(), Value::Number(1.0));
        props.insert("view-start".to_string(), Value::Number(0.0));
        props.insert("view-duration".to_string(), Value::Number(16.0));
        props.insert("on-change".to_string(), Value::Keyword("cb".to_string()));
        props
    }

    fn viewport() -> WidgetViewport {
        WidgetViewport {
            cell_w: 8.0,
            cell_h: 16.0,
            vp_w: 640.0,
            vp_h: 360.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 36.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        }
    }

    #[test]
    fn value_for_row_maps_top_to_max_and_snaps_to_increment() {
        let rect = Rect {
            col: 0.0,
            row: 0.0,
            width: 10.0,
            height: 4.0,
        };
        let scale = LaneScale {
            min: 0.0,
            max: 127.0,
            increment: 1.0,
        };
        assert_eq!(value_for_row(rect, scale, 0.0), 127.0);
        assert_eq!(value_for_row(rect, scale, 4.0), 0.0);
        assert_eq!(value_for_row(rect, scale, 2.0), 64.0);
        assert_eq!(value_for_row(rect, scale, -50.0), 127.0, "clamps above");
    }

    #[test]
    fn press_hits_nearest_onset_and_drag_keeps_the_anchor_step() {
        let node = node(velocity_props());
        // Step 4 sits at col 10 + 16 * 4/16 = 14.
        let outcome = AUTOMATION_LANE_WIDGET.mouse_event(
            &node,
            MouseEventKind::Down(MouseButton::Left),
            14.2,
            4.0,
            None,
            None,
            KeyModifiers::NONE,
            8.0,
            16.0,
        );
        let MouseEventOutcome::Dispatch(event) = outcome else {
            panic!("press on a point dispatches");
        };
        let output = AUTOMATION_LANE_WIDGET
            .handle_event(&node, event)
            .expect("on-change output");
        assert_eq!(output.args[0], Value::Keyword("set".to_string()));
        assert_eq!(output.args[1], Value::Number(4.0));
        assert_eq!(output.args[2], Value::Number(127.0));

        // Dragging sideways over step 0's column still edits step 4.
        let outcome = AUTOMATION_LANE_WIDGET.mouse_event(
            &node,
            MouseEventKind::Drag(MouseButton::Left),
            10.0,
            8.0,
            Some((14.2, 4.0)),
            None,
            KeyModifiers::NONE,
            8.0,
            16.0,
        );
        let MouseEventOutcome::Dispatch(event) = outcome else {
            panic!("drag dispatches");
        };
        let output = AUTOMATION_LANE_WIDGET.handle_event(&node, event).unwrap();
        assert_eq!(output.args[1], Value::Number(4.0));
        assert_eq!(output.args[2], Value::Number(0.0));

        let outcome = AUTOMATION_LANE_WIDGET.mouse_event(
            &node,
            MouseEventKind::Up(MouseButton::Left),
            10.0,
            8.0,
            Some((14.2, 4.0)),
            None,
            KeyModifiers::NONE,
            8.0,
            16.0,
        );
        let MouseEventOutcome::Dispatch(event) = outcome else {
            panic!("release dispatches finish");
        };
        let output = AUTOMATION_LANE_WIDGET.handle_event(&node, event).unwrap();
        assert_eq!(output.args[0], Value::Keyword("finish".to_string()));
        assert_eq!(output.args[1], Value::Number(4.0));
    }

    #[test]
    fn press_between_points_is_ignored_and_double_click_clears() {
        let node = node(velocity_props());
        let outcome = AUTOMATION_LANE_WIDGET.mouse_event(
            &node,
            MouseEventKind::Down(MouseButton::Left),
            12.0,
            4.0,
            None,
            None,
            KeyModifiers::NONE,
            8.0,
            16.0,
        );
        assert!(matches!(outcome, MouseEventOutcome::Ignore));
        let event = AUTOMATION_LANE_WIDGET
            .double_click_event(&node, 10.1, 5.0)
            .expect("double-click on a point");
        let output = AUTOMATION_LANE_WIDGET.handle_event(&node, event).unwrap();
        assert_eq!(output.args[0], Value::Keyword("clear".to_string()));
        assert_eq!(output.args[1], Value::Number(0.0));
    }

    #[test]
    fn duration_bar_press_drag_and_release_keep_the_original_step() {
        let mut props = velocity_props();
        props.insert("points".to_string(), Value::List(vec![
            Rc::new(RefCell::new(point_value(0.0, 0.0, 1.0, 100.0, true))),
            Rc::new(RefCell::new(point_value(4.0, 4.0, 12.0, 64.0, false))),
        ]));
        let mut node = node(props);
        // Deep inside step 4's duration, many hit tolerances from its circle.
        let anchor = (20.0, 6.0);
        for (kind, col, row, drag_start, expected_kind, expected_value) in [
            (MouseEventKind::Down(MouseButton::Left), anchor.0, anchor.1, None, "set", 64.0),
            (MouseEventKind::Drag(MouseButton::Left), 10.0, -5.0, Some(anchor), "set", 127.0),
            (MouseEventKind::Up(MouseButton::Left), 40.0, -5.0, Some(anchor), "finish", 127.0),
        ] {
            let outcome = AUTOMATION_LANE_WIDGET.mouse_event(
                &node, kind, col, row, drag_start, None, KeyModifiers::NONE, 8.0, 16.0,
            );
            let MouseEventOutcome::Dispatch(event) = outcome else { panic!("bar dispatches"); };
            let output = AUTOMATION_LANE_WIDGET.handle_event(&node, event).unwrap();
            assert_eq!(output.args[0], Value::Keyword(expected_kind.to_string()));
            assert_eq!(output.args[1], Value::Number(4.0));
            assert_eq!(output.args[2], Value::Number(expected_value));
            // Model the host publishing a fresh points list after each edit.
            node.props.insert("points".to_string(), Value::List(vec![
                Rc::new(RefCell::new(point_value(0.0, 0.0, 1.0, 100.0, true))),
                Rc::new(RefCell::new(point_value(4.0, 4.0, 12.0, expected_value, true))),
            ]));
        }
    }

    #[test]
    fn bar_hits_respect_clipping_inactive_points_and_onset_priority() {
        let mut props = velocity_props();
        let inactive = point_value(8.0, 8.0, 12.0, 80.0, true);
        if let Value::Map(map) = &inactive {
            *map["active"].borrow_mut() = Value::Bool(false);
        }
        props.insert("points".to_string(), Value::List(vec![
            Rc::new(RefCell::new(point_value(0.0, -4.0, 7.0, 100.0, true))),
            Rc::new(RefCell::new(point_value(4.0, 4.0, 6.0, 64.0, false))),
            Rc::new(RefCell::new(inactive)),
        ]));
        let node = node(props);
        assert_eq!(point_at_col(&node, 10.0, 8.0).unwrap().step, 0, "clipped bar remains hittable");
        assert_eq!(point_at_col(&node, 14.1, 8.0).unwrap().step, 4, "onset beats overlapping bar");
        assert_eq!(point_at_col(&node, 15.5, 8.0).unwrap().step, 4, "latest overlapping bar wins");
        assert_eq!(point_at_col(&node, 16.5, 8.0).unwrap().step, 0);
        assert!(point_at_col(&node, 20.0, 8.0).is_none(), "off-step locks have no bar");
        assert!(point_at_col(&node, 9.9, 8.0).is_none(), "outside visible lane");
        assert!(point_at_col(&node, 27.0, 8.0).is_none());
    }

    #[test]
    fn hovering_a_bar_highlights_only_its_point_and_invalidates_render_state() {
        let node = node(velocity_props());
        let colors = |node: &LayoutNode| -> Vec<Color> {
            AUTOMATION_LANE_WIDGET.build_primitives("automation-lane", node, viewport())
                .iter().map(|primitive| match primitive {
                    GpuPrimitive::Circle(circle) => circle.color,
                    GpuPrimitive::Rect(rect) => rect.color,
                    _ => panic!("unexpected lane primitive"),
                }).collect()
        };
        super::super::set_pointer_hover_widget(Some(node.widget_id));
        set_hovered_point(node.widget_id, None);
        let normal = colors(&node);
        let revision = super::super::widget_state_revision(node.widget_id);
        let outcome = AUTOMATION_LANE_WIDGET.mouse_event(
            &node, MouseEventKind::Moved, 15.5, 6.0, None, None, KeyModifiers::NONE, 8.0, 16.0,
        );
        assert!(matches!(outcome, MouseEventOutcome::Consume), "hover never writes a value");
        assert!(super::super::widget_state_revision(node.widget_id) > revision);
        assert_eq!(hovered_step(node.widget_id), Some(4));
        let highlighted = colors(&node);
        assert_eq!(normal[0], highlighted[0], "background unchanged");
        assert_eq!(normal[1], highlighted[1], "other bar unchanged");
        assert_ne!(normal[2], highlighted[2], "hovered bar changes");
        assert_eq!(normal[3], highlighted[3], "other dot unchanged");
        assert_ne!(normal[4], highlighted[4], "associated dot changes");
        super::super::set_pointer_hover_widget(None);
        assert_eq!(colors(&node), normal, "leaving the lane removes feedback");
        super::super::set_pointer_hover_widget(Some(node.widget_id));
        AUTOMATION_LANE_WIDGET.mouse_event(
            &node, MouseEventKind::Moved, 23.0, 6.0, None, None, KeyModifiers::NONE, 8.0, 16.0,
        );
        assert_eq!(colors(&node), normal, "empty lane space clears hover");
        super::super::set_pointer_hover_widget(None);
    }

    #[test]
    fn build_primitives_draws_duration_bars_and_gray_base_dots() {
        let node = node(velocity_props());
        let prims = AUTOMATION_LANE_WIDGET.build_primitives("automation-lane", &node, viewport());
        let dots: Vec<&GpuCirclePrimitive> = prims
            .iter()
            .filter_map(|primitive| match primitive {
                GpuPrimitive::Circle(circle) => Some(circle),
                _ => None,
            })
            .collect();
        assert_eq!(dots.len(), 2);
        assert!(
            (dots[0].center[0] - 10.0).abs() < 1e-6,
            "step 0 onset at the left edge"
        );
        assert!((dots[1].center[0] - 14.0).abs() < 1e-6, "step 4 onset");
        assert_ne!(dots[0].color, dots[1].color, "locked vs base colors differ");
        // Background + one bar per active point (no default rule: none set).
        let rects = prims
            .iter()
            .filter(|primitive| matches!(primitive, GpuPrimitive::Rect(_)))
            .count();
        assert_eq!(rects, 3);
    }
}
