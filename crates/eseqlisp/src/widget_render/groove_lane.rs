//! `groove-lane`: one row of a rack groove, drawn as a bar of slots on a
//! centre line (the eseq rack groove buffer).
//!
//! - `:cells` — signed slot offsets (positive = late, negative = early). Each
//!   slot draws a bar up (late, `:late-color`) or down (early,
//!   `:early-color`) from the centre line; `0.4` of a slot fills the half
//!   height. `:measured` (parallel bools) dims the filled-in slots.
//! - `:hits` — per-slot bools. Used when `:cells` is empty (the rack plays
//!   straight): a dot sits on the centre line at every hit.
//! - `:slots` — slot count when both lists are empty (a blank bar).
//!
//! Beats are separated by a faint tick every quarter of the bar.

use std::collections::HashMap;

use super::{CellBuffer, WidgetDefinition, resolve_named_color, styled_cell};
use super::{GpuCirclePrimitive, GpuCircleVisibleHalf, GpuPrimitive, GpuRectPrimitive, WidgetViewport};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct GrooveLaneWidget;
pub static GROOVE_LANE_WIDGET: GrooveLaneWidget = GrooveLaneWidget;

/// The offset (in slots) that fills half the lane's height. Extraction keeps
/// offsets within half a slot, so a strongly swung slot reaches the edge.
const FULL_SCALE_SLOTS: f32 = 0.4;

fn numbers(props: &HashMap<String, Value>, key: &str) -> Vec<f32> {
    let Some(Value::List(items)) = props.get(key) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| match &*item.borrow() {
            Value::Number(value) => *value as f32,
            Value::Bool(value) => {
                if *value {
                    1.0
                } else {
                    0.0
                }
            }
            _ => 0.0,
        })
        .collect()
}

struct Lane {
    cells: Vec<f32>,
    measured: Vec<f32>,
    hits: Vec<f32>,
    slots: usize,
}

fn lane(props: &HashMap<String, Value>) -> Lane {
    let cells = numbers(props, "cells");
    let measured = numbers(props, "measured");
    let hits = numbers(props, "hits");
    let declared = props.get("slots").and_then(|value| match value {
        Value::Number(slots) if *slots >= 1.0 => Some(*slots as usize),
        _ => None,
    });
    let slots = if !cells.is_empty() {
        cells.len()
    } else {
        declared.unwrap_or(hits.len()).max(1)
    };
    Lane {
        cells,
        measured,
        hits,
        slots,
    }
}

fn with_alpha(color: Color, alpha: f32) -> Color {
    Color {
        a: color.a * alpha,
        ..color
    }
}

impl WidgetDefinition for GrooveLaneWidget {
    fn names(&self) -> &'static [&'static str] {
        &["groove-lane"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        Some(Size {
            width: get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or(constraints.max_width)
                .min(constraints.max_width),
            height: get_prop_num(node, "height")
                .map(f64_to_f32)
                .unwrap_or(1.0)
                .max(0.5),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let lane = lane(props);
        let width = rect.width.floor().max(1.0) as usize;
        let row = rect.row.floor() as u16 + rect.height.floor().max(1.0) as u16 / 2;
        let late = resolve_named_color(props, "late-color", theme::WIDGET_SLIDER_FILLED());
        let early = resolve_named_color(props, "early-color", theme::FG_MUTED());
        let muted = resolve_named_color(props, "line-color", theme::FG_MUTED());
        for slot in 0..lane.slots {
            let x = slot * width / lane.slots;
            if x >= width {
                break;
            }
            let (ch, color) = match lane.cells.get(slot) {
                Some(offset) if *offset > 0.02 => ('▴', late),
                Some(offset) if *offset < -0.02 => ('▾', early),
                Some(_) => ('·', muted),
                None if lane.hits.get(slot).copied().unwrap_or(0.0) > 0.0 => ('●', late),
                None => ('·', muted),
            };
            buf.set(
                row,
                rect.col.floor() as u16 + x as u16,
                styled_cell(ch, color, None),
            );
        }
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        _viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let props = &node.props;
        let lane = lane(props);
        let rect = node.rect;
        let late = resolve_named_color(props, "late-color", Color::rgba(0.29, 0.56, 1.0, 1.0));
        let early = resolve_named_color(props, "early-color", Color::rgba(0.90, 0.66, 0.24, 1.0));
        let line = resolve_named_color(props, "line-color", Color::rgba(1.0, 1.0, 1.0, 0.12));
        let dot = resolve_named_color(props, "dot-color", Color::rgba(0.85, 0.85, 0.88, 1.0));
        let mut primitives = Vec::new();
        if props.contains_key("background-color") {
            primitives.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect,
                color: resolve_named_color(props, "background-color", Color::rgba(0.0, 0.0, 0.0, 0.0)),
            }));
        }
        let mid = rect.row + rect.height * 0.5;
        let half = (rect.height * 0.5 - 0.08).max(0.05);
        let line_height = (rect.height * 0.03).clamp(0.02, 0.06);
        primitives.push(GpuPrimitive::Rect(GpuRectPrimitive {
            rect: Rect {
                row: mid - line_height * 0.5,
                col: rect.col,
                width: rect.width,
                height: line_height,
            },
            color: line,
        }));
        let slot_width = rect.width / lane.slots as f32;
        let per_beat = (lane.slots / 4).max(1);
        for beat in 1..4 {
            let slot = beat * per_beat;
            if slot >= lane.slots {
                break;
            }
            primitives.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    row: rect.row + rect.height * 0.2,
                    col: rect.col + slot as f32 * slot_width - 0.02,
                    width: 0.04,
                    height: rect.height * 0.6,
                },
                color: with_alpha(line, 0.8),
            }));
        }
        let bar_width = (slot_width * 0.34).clamp(0.08, 0.6);
        let dot_radius = match props.get("dot-radius") {
            Some(Value::Number(radius)) => *radius as f32,
            _ => 2.5,
        };
        let mut dots = Vec::new();
        for slot in 0..lane.slots {
            let center = rect.col + (slot as f32 + 0.5) * slot_width;
            if let Some(&offset) = lane.cells.get(slot) {
                if !offset.is_finite() || offset.abs() < 0.01 {
                    continue;
                }
                let measured = lane.measured.get(slot).copied().unwrap_or(1.0) > 0.0;
                let magnitude = (offset.abs() / FULL_SCALE_SLOTS).min(1.0);
                let height = (half * magnitude).max(0.06);
                let color = if offset > 0.0 { late } else { early };
                let color = if measured { color } else { with_alpha(color, 0.45) };
                let row = if offset > 0.0 { mid - height } else { mid };
                primitives.push(GpuPrimitive::Rect(GpuRectPrimitive {
                    rect: Rect {
                        row,
                        col: center - bar_width * 0.5,
                        width: bar_width,
                        height,
                    },
                    color,
                }));
            } else if lane.cells.is_empty() && lane.hits.get(slot).copied().unwrap_or(0.0) > 0.0 {
                dots.push(GpuPrimitive::Circle(GpuCirclePrimitive {
                    center: [center, mid],
                    radius_px: dot_radius,
                    color: dot,
                    visible_half: GpuCircleVisibleHalf::Full,
                }));
            }
        }
        primitives.extend(dots);
        primitives
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn list(values: &[f64]) -> Value {
        Value::List(
            values
                .iter()
                .map(|value| Rc::new(RefCell::new(Value::Number(*value))))
                .collect(),
        )
    }

    #[test]
    fn cells_set_the_slot_count_and_hits_fill_a_straight_lane() {
        let grooved = lane(&HashMap::from([
            ("cells".to_string(), list(&[0.0, 0.2, 0.0, -0.1])),
            ("slots".to_string(), Value::Number(16.0)),
        ]));
        assert_eq!(grooved.slots, 4);
        let straight = lane(&HashMap::from([
            ("hits".to_string(), list(&[1.0, 0.0, 0.0])),
            ("slots".to_string(), Value::Number(16.0)),
        ]));
        assert_eq!(straight.slots, 16);
        assert_eq!(straight.hits, vec![1.0, 0.0, 0.0]);
    }
}
