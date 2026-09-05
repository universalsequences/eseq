//! `xy-pad`: an absolute-position 2D control.
//!
//! A rectangle you drag a dot around in. It reports a normalized `(x, y)` pair,
//! both in `0..1`, with `x` increasing to the right and `y` increasing
//! *downward* — a top-left origin, matching screen coordinates and the
//! row/col order of a tensor grid. That makes it the natural UI for
//! "where on the drum head did the stick land", replacing the strike-position
//! `matrix` widget.
//!
//! Drawing is plain primitives (rects for the chassis and crosshair, one
//! `Circle` for the dot), so there is no fragment shader and no WGSL/MSL pair
//! to keep in sync.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, GpuCirclePrimitive, GpuCircleVisibleHalf, GpuPrimitive,
    GpuRectPrimitive, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetViewport,
    get_f32_prop, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct XyPadWidget;

pub static XY_PAD_WIDGET: XyPadWidget = XyPadWidget;

const DEFAULT_WIDTH: f32 = 12.0;

/// The `(x, y)` the widget currently displays, clamped to `0..1`.
pub fn pad_position(props: &HashMap<String, Value>) -> (f32, f32) {
    (
        get_f32_prop(props, "x", 0.5).clamp(0.0, 1.0),
        get_f32_prop(props, "y", 0.5).clamp(0.0, 1.0),
    )
}

/// Maps an absolute pointer position (in layout cell units, the same space
/// `mouse_event` receives) onto the pad's normalized `0..1` axes.
///
/// Exposed so tests and hosts can simulate a drag without reconstructing the
/// widget's internal geometry.
pub fn position_from_local(node: &LayoutNode, local_col: f32, local_row: f32) -> (f32, f32) {
    let x = (local_col - node.rect.col) / node.rect.width.max(0.001);
    let y = (local_row - node.rect.row) / node.rect.height.max(0.001);
    (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0))
}

fn position_event(x: f32, y: f32) -> WidgetEvent {
    WidgetEvent::Custom(Value::List(vec![
        Rc::new(RefCell::new(Value::Number(x as f64))),
        Rc::new(RefCell::new(Value::Number(y as f64))),
    ]))
}

fn accent_color(props: &HashMap<String, Value>) -> Color {
    match props.get("accent").or_else(|| props.get("color")) {
        Some(value) => theme::parse_color_value(value).unwrap_or_else(theme::WIDGET_KNOB_FILLED),
        None => theme::WIDGET_KNOB_FILLED(),
    }
}

fn background_color(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "background", theme::BUTTON_GHOST_BG())
}

fn border_color(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "border-color", theme::WIDGET_KNOB_TRACK())
}

impl WidgetDefinition for XyPadWidget {
    fn names(&self) -> &'static [&'static str] {
        &["xy-pad"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "size"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["x", "y"]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "x",
            "y",
            "width",
            "height",
            "size",
            "accent",
            "color",
            "background",
            "border-color",
            "dot-size",
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
        let default_size = get_prop_num(node, "size")
            .map(f64_to_f32)
            .unwrap_or(DEFAULT_WIDTH);
        let width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or(default_size)
            .max(1.0);
        // Square by default: the pad's two axes are symmetric, so honour the
        // cell aspect rather than the raw cell count.
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or_else(|| width / constraints.aspect.max(f32::EPSILON))
            .max(1.0);
        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let (x, y) = pad_position(props);
        let width = rect.width.round().max(1.0) as u16;
        let height = rect.height.round().max(1.0) as u16;
        let row0 = rect.row.round() as u16;
        let col0 = rect.col.round() as u16;
        let dot_col = ((x * (width.saturating_sub(1)) as f32).round() as u16).min(width - 1);
        let dot_row = ((y * (height.saturating_sub(1)) as f32).round() as u16).min(height - 1);
        let track = theme::WIDGET_KNOB_TRACK();
        let accent = accent_color(props);
        for row in 0..height {
            for col in 0..width {
                let edge = row == 0 || col == 0 || row == height - 1 || col == width - 1;
                let (ch, fg) = if row == dot_row && col == dot_col {
                    ('●', accent)
                } else if edge {
                    ('·', track)
                } else {
                    continue;
                };
                buf.set(row0 + row, col0 + col, styled_cell(ch, fg, None));
            }
        }
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                let (x, y) = position_from_local(node, local_col, local_row);
                MouseEventOutcome::Dispatch(position_event(x, y))
            }
            MouseEventKind::Up(MouseButton::Left) => MouseEventOutcome::Consume,
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn captures_drag(&self) -> bool {
        true
    }

    /// The pointer is allowed to leave the rect during a drag; the raw
    /// position arrives here and `position_from_local` clamps it. Without this
    /// the framework would clamp first, which is the same result, but keeping
    /// the clamp local makes the 0..1 contract the widget's own.
    fn unclamped_drag(&self) -> bool {
        true
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(value) = event else {
            return None;
        };
        let callback = node.props.get("on-change")?.clone();
        // `mouse_event` packs `(x y)`; spread it into two positional args.
        let args = match &value {
            Value::List(items) => items
                .iter()
                .map(|item| {
                    let number = match &*item.borrow() {
                        Value::Number(number) => *number,
                        _ => 0.0,
                    };
                    Value::Number(number.clamp(0.0, 1.0))
                })
                .collect(),
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
        let (x, y) = pad_position(&node.props);
        let accent = accent_color(&node.props);
        let bg = background_color(&node.props);
        let border = border_color(&node.props);

        let cell_w = viewport.cell_w.max(1.0);
        let cell_h = viewport.cell_h.max(1.0);
        // One design pixel of border / crosshair, expressed back in cell units.
        let border_w = (super::ui_design_px(1.0) / cell_w).min(rect.width * 0.25);
        let border_h = (super::ui_design_px(1.0) / cell_h).min(rect.height * 0.25);

        let dot_x = rect.col + x * rect.width;
        let dot_y = rect.row + y * rect.height;
        let crosshair = Color {
            a: accent.a * 0.35,
            ..accent
        };

        let mut prims = vec![
            GpuPrimitive::Rect(GpuRectPrimitive { rect, color: bg }),
            // Border: four hairline rects, so there is no SDF `fwidth` corner
            // fattening to work around.
            GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    height: border_h,
                    ..rect
                },
                color: border,
            }),
            GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    row: rect.row + rect.height - border_h,
                    height: border_h,
                    ..rect
                },
                color: border,
            }),
            GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    width: border_w,
                    ..rect
                },
                color: border,
            }),
            GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    col: rect.col + rect.width - border_w,
                    width: border_w,
                    ..rect
                },
                color: border,
            }),
            // Crosshair through the current position.
            GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    row: dot_y - border_h * 0.5,
                    height: border_h,
                    ..rect
                },
                color: crosshair,
            }),
            GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    col: dot_x - border_w * 0.5,
                    width: border_w,
                    ..rect
                },
                color: crosshair,
            }),
        ];

        let dot_px = super::ui_design_px(get_f32_prop(&node.props, "dot-size", 4.0)).max(1.0);
        prims.push(GpuPrimitive::Circle(GpuCirclePrimitive {
            center: [dot_x, dot_y],
            radius_px: dot_px,
            color: accent,
            visible_half: GpuCircleVisibleHalf::Full,
        }));
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
            widget_type: "xy-pad".to_string(),
            rect: Rect {
                col: 10.0,
                row: 4.0,
                width: 20.0,
                height: 10.0,
            },
            props,
            children: Vec::new(),
            focusable: false,
            animation: LayoutAnimationHints::default(),
        }
    }

    #[test]
    fn position_from_local_is_top_left_origin_and_clamped() {
        let node = node(HashMap::new());
        let (x, y) = position_from_local(&node, 10.0, 4.0);
        assert!(x.abs() < 1e-6 && y.abs() < 1e-6, "top-left is (0,0)");
        let (x, y) = position_from_local(&node, 30.0, 14.0);
        assert!(
            (x - 1.0).abs() < 1e-6 && (y - 1.0).abs() < 1e-6,
            "bottom-right is (1,1)"
        );
        // y grows downward, so a point above the rect clamps to 0.
        let (x, y) = position_from_local(&node, -100.0, -100.0);
        assert_eq!((x, y), (0.0, 0.0));
        let (x, y) = position_from_local(&node, 500.0, 500.0);
        assert_eq!((x, y), (1.0, 1.0));
    }

    #[test]
    fn drag_dispatches_clamped_pair_to_on_change() {
        let mut props = HashMap::new();
        props.insert("on-change".to_string(), Value::Keyword("cb".to_string()));
        let node = node(props);
        let outcome = XY_PAD_WIDGET.mouse_event(
            &node,
            MouseEventKind::Drag(MouseButton::Left),
            60.0,
            9.0,
            None,
            None,
            KeyModifiers::NONE,
            1.0,
            1.0,
        );
        let MouseEventOutcome::Dispatch(event) = outcome else {
            panic!("drag should dispatch");
        };
        let output = XY_PAD_WIDGET
            .handle_event(&node, event)
            .expect("on-change output");
        assert_eq!(output.args.len(), 2);
        let numbers: Vec<f64> = output
            .args
            .iter()
            .map(|arg| match arg {
                Value::Number(number) => *number,
                other => panic!("expected number, got {other:?}"),
            })
            .collect();
        assert!((numbers[0] - 1.0).abs() < 1e-6, "x clamps to 1.0");
        assert!((numbers[1] - 0.5).abs() < 1e-6, "y is mid-rect");
    }

    #[test]
    fn build_primitives_emits_a_dot_without_a_shader() {
        let mut props = HashMap::new();
        props.insert("x".to_string(), Value::Number(0.25));
        props.insert("y".to_string(), Value::Number(0.75));
        let node = node(props);
        let prims = XY_PAD_WIDGET.build_primitives(
            "xy-pad",
            &node,
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
            },
        );
        assert!(
            XY_PAD_WIDGET
                .fragment_shader("xy-pad", super::super::ShaderBackend::Wgsl)
                .is_none(),
            "xy-pad draws with primitives only"
        );
        let dot = prims
            .iter()
            .find_map(|primitive| match primitive {
                GpuPrimitive::Circle(circle) => Some(circle),
                _ => None,
            })
            .expect("dot circle");
        assert!((dot.center[0] - 15.0).abs() < 1e-6, "x=0.25 of 20 from 10");
        assert!((dot.center[1] - 11.5).abs() < 1e-6, "y=0.75 of 10 from 4");
    }
}
