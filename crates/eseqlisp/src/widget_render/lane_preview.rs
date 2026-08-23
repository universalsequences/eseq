use std::collections::HashMap;

use super::{CellBuffer, WidgetDefinition, resolve_named_color, styled_cell};
use super::{GpuPrimitive, GpuRectPrimitive, WidgetViewport};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct LanePreviewWidget;
pub static LANE_PREVIEW_WIDGET: LanePreviewWidget = LanePreviewWidget;

fn lane_values(props: &HashMap<String, Value>) -> Vec<f32> {
    let Some(Value::List(items)) = props.get("values") else {
        return Vec::new();
    };
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| match &*item.borrow() {
            Value::Keyword(tag) if index == 0 && tag == "__process-lane" => None,
            Value::Number(value) => Some(*value as f32),
            Value::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
            _ => None,
        })
        .collect()
}

fn value_range(values: &[f32]) -> (f32, f32) {
    let min = values.iter().copied().fold(0.0, f32::min);
    let max = values.iter().copied().fold(1.0, f32::max);
    if (max - min).abs() < f32::EPSILON {
        (min, min + 1.0)
    } else {
        (min, max)
    }
}

impl WidgetDefinition for LanePreviewWidget {
    fn names(&self) -> &'static [&'static str] {
        &["lane-preview"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "values"]
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
                .unwrap_or(4.0)
                .max(2.0),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let values = lane_values(props);
        if values.is_empty() {
            return;
        }
        let (min, max) = value_range(&values);
        let width = rect.width.floor().max(1.0) as usize;
        let step_width = (width / values.len()).max(1);
        let row = rect.row.floor() as u16 + rect.height.floor().max(1.0) as u16 / 2;
        let active = resolve_named_color(props, "active-color", theme::WIDGET_SLIDER_FILLED());
        let inactive = resolve_named_color(props, "inactive-color", theme::FG_MUTED());
        for (index, value) in values.iter().enumerate() {
            let normalized = ((*value - min) / (max - min)).clamp(0.0, 1.0);
            for column in 0..step_width.saturating_sub(1).max(1) {
                let x = index * step_width + column;
                if x >= width {
                    break;
                }
                buf.set(
                    row,
                    rect.col.floor() as u16 + x as u16,
                    styled_cell(
                        if normalized > 0.0 { '■' } else { '·' },
                        if normalized > 0.0 { active } else { inactive },
                        None,
                    ),
                );
            }
        }
    }

    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        _viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let values = lane_values(&node.props);
        let background = resolve_named_color(
            &node.props,
            "background-color",
            Color::rgba(0.025, 0.03, 0.035, 0.92),
        );
        let active = resolve_named_color(
            &node.props,
            "active-color",
            Color::rgba(0.95, 0.55, 0.2, 1.0),
        );
        let mut primitives = vec![GpuPrimitive::Rect(GpuRectPrimitive {
            rect: node.rect,
            color: background,
        })];
        if values.is_empty() {
            return primitives;
        }
        let (min, max) = value_range(&values);
        let step_width = node.rect.width / values.len() as f32;
        for (index, value) in values.iter().enumerate() {
            let normalized = ((*value - min) / (max - min)).clamp(0.0, 1.0);
            let height = (node.rect.height - 0.8).max(0.1) * normalized.max(0.035);
            primitives.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    row: node.rect.row + node.rect.height - 0.4 - height,
                    col: node.rect.col + index as f32 * step_width + 0.08,
                    width: (step_width - 0.16).max(0.04),
                    height,
                },
                color: active,
            }));
        }
        primitives
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn tagged_process_lane_values_are_extracted_without_the_tag() {
        let values = Value::List(vec![
            Rc::new(RefCell::new(Value::Keyword("__process-lane".to_string()))),
            Rc::new(RefCell::new(Value::Number(0.0))),
            Rc::new(RefCell::new(Value::Number(2.0))),
        ]);
        assert_eq!(
            lane_values(&HashMap::from([("values".to_string(), values)])),
            vec![0.0, 2.0]
        );
    }
}
