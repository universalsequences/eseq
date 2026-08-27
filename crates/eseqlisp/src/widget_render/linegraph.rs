use std::collections::HashMap;

use super::{resolve_named_color, styled_cell, CellBuffer, WidgetDefinition};
use super::{
    GpuPrimitive, GpuProportionalTextPrimitive, GpuRectPrimitive, GpuTrianglePrimitive,
    WidgetViewport,
};
use crate::backend::Color;
use crate::layout::{f64_to_f32, get_prop_num, Constraints, LayoutNode, MeasureCtx, Rect, Size};
use crate::theme;
use crate::vm::Value;

pub struct LineGraphWidget;

pub static LINEGRAPH_WIDGET: LineGraphWidget = LineGraphWidget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GraphScale {
    Linear,
    Log,
}

struct GraphSeries {
    values: Vec<Option<f32>>,
    axis_min: f32,
    axis_max: f32,
}

fn number_value(value: &Value) -> Option<f32> {
    let value = match value {
        Value::Number(value) => *value,
        Value::ReactiveRef { slot, .. } => crate::reactive::read_float_slot(slot),
        _ => return None,
    };
    let value = value as f32;
    value.is_finite().then_some(value)
}

fn series_values(props: &HashMap<String, Value>) -> Vec<Option<f32>> {
    match props.get("values") {
        Some(Value::List(items)) => items
            .iter()
            .map(|item| number_value(&item.borrow()))
            .collect(),
        Some(value) => vec![number_value(value)],
        None => Vec::new(),
    }
}

fn explicit_total_points(props: &HashMap<String, Value>) -> Option<usize> {
    props
        .get("total-points")
        .and_then(number_value)
        .filter(|value| *value >= 1.0)
        .map(|value| value.round() as usize)
}

fn x_domain_points(props: &HashMap<String, Value>, observed_points: usize) -> usize {
    explicit_total_points(props)
        .unwrap_or(observed_points)
        .max(observed_points)
        .max(1)
}

fn graph_scale(props: &HashMap<String, Value>) -> GraphScale {
    match props.get("scale") {
        Some(Value::Keyword(scale)) | Some(Value::String(scale)) if scale == "log" => {
            GraphScale::Log
        }
        _ => GraphScale::Linear,
    }
}

fn transformed_series(
    props: &HashMap<String, Value>,
    raw: &[Option<f32>],
) -> (Vec<Option<f32>>, GraphScale, Option<f32>) {
    let requested_scale = graph_scale(props);
    if requested_scale == GraphScale::Linear {
        return (raw.to_vec(), GraphScale::Linear, None);
    }

    let Some(smallest_positive) = raw
        .iter()
        .flatten()
        .copied()
        .filter(|value| *value > 0.0)
        .min_by(f32::total_cmp)
    else {
        // Logarithmic axes are undefined without a positive datum. Falling
        // back to linear preserves a meaningful graph instead of hiding it.
        return (raw.to_vec(), GraphScale::Linear, None);
    };
    let floor = (smallest_positive * 0.1).max(f32::MIN_POSITIVE);
    let transformed = raw
        .iter()
        .map(|value| value.map(|value| value.max(floor).log10()))
        .collect();
    (transformed, GraphScale::Log, Some(floor))
}

fn transformed_bound(value: f32, scale: GraphScale, log_floor: Option<f32>) -> Option<f32> {
    if !value.is_finite() {
        return None;
    }
    Some(match scale {
        GraphScale::Linear => value,
        GraphScale::Log => value.max(log_floor?).log10(),
    })
}

fn normalized_series(props: &HashMap<String, Value>) -> GraphSeries {
    let raw = series_values(props);
    let (transformed, scale, log_floor) = transformed_series(props, &raw);
    let mut values = transformed.iter().flatten().copied();
    let Some(first) = values.next() else {
        return GraphSeries {
            values: transformed,
            axis_min: 0.0,
            axis_max: 1.0,
        };
    };
    let (mut data_min, mut data_max) = values.fold((first, first), |(min, max), value| {
        (min.min(value), max.max(value))
    });

    let explicit_min = props
        .get("min")
        .and_then(number_value)
        .and_then(|value| transformed_bound(value, scale, log_floor));
    let explicit_max = props
        .get("max")
        .and_then(number_value)
        .and_then(|value| transformed_bound(value, scale, log_floor));
    if let Some(min) = explicit_min {
        data_min = min;
    }
    if let Some(max) = explicit_max {
        data_max = max;
    }
    if data_max < data_min {
        std::mem::swap(&mut data_min, &mut data_max);
    }

    let span = data_max - data_min;
    if span.abs() <= f32::EPSILON {
        let padding = match scale {
            GraphScale::Linear => data_min.abs().mul_add(0.05, 1.0e-6).max(1.0e-6),
            GraphScale::Log => 0.5,
        };
        match (explicit_min, explicit_max) {
            (Some(_), None) => data_max += padding,
            (None, Some(_)) => data_min -= padding,
            _ => {
                data_min -= padding;
                data_max += padding;
            }
        }
    } else if explicit_min.is_none() && explicit_max.is_none() {
        let padding = span * 0.05;
        data_min -= padding;
        data_max += padding;
    }

    let span = (data_max - data_min).max(f32::EPSILON);
    let values = transformed
        .into_iter()
        .map(|value| value.map(|value| ((value - data_min) / span).clamp(0.0, 1.0)))
        .collect();
    let to_axis_value = |value: f32| match scale {
        GraphScale::Linear => value,
        GraphScale::Log => 10.0f32.powf(value),
    };
    GraphSeries {
        values,
        axis_min: to_axis_value(data_min),
        axis_max: to_axis_value(data_max),
    }
}

fn bool_prop(props: &HashMap<String, Value>, key: &str, default: bool) -> bool {
    match props.get(key) {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => *value != 0.0,
        _ => default,
    }
}

fn format_axis_value(value: f32) -> String {
    let magnitude = value.abs();
    if magnitude >= 10_000.0 || (magnitude > 0.0 && magnitude < 0.001) {
        return format!("{value:.1e}");
    }
    let precision = if magnitude >= 100.0 {
        0
    } else if magnitude >= 10.0 {
        1
    } else if magnitude >= 1.0 {
        2
    } else {
        3
    };
    let mut label = format!("{value:.precision$}");
    if label.contains('.') {
        while label.ends_with('0') {
            label.pop();
        }
        if label.ends_with('.') {
            label.pop();
        }
    }
    label
}

fn line_color(props: &HashMap<String, Value>) -> Color {
    let fallback = resolve_named_color(props, "color", theme::WIDGET_SLIDER_FILLED());
    resolve_named_color(props, "line-color", fallback)
}

fn background_color(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "background-color", Color::rgba(0.0, 0.0, 0.0, 0.0))
}

fn area_enabled(props: &HashMap<String, Value>) -> bool {
    match props.get("area") {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => *value != 0.0,
        _ => false,
    }
}

fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let series = normalized_series(props);
    let full_width = rect.width.floor().max(1.0) as usize;
    let height = rect.height.floor().max(1.0) as usize;
    let row = rect.row.floor() as u16;
    let col = rect.col.floor() as u16;
    let line = line_color(props);
    let background = background_color(props);
    if background.a > 0.0 {
        for y in 0..height {
            for x in 0..full_width {
                buf.set(
                    row + y as u16,
                    col + x as u16,
                    styled_cell(' ', line, Some(background)),
                );
            }
        }
    }
    let show_y_axis = bool_prop(props, "y-axis", false);
    let top_label = format_axis_value(series.axis_max);
    let bottom_label = format_axis_value(series.axis_min);
    let label_width = if show_y_axis {
        top_label
            .chars()
            .count()
            .max(bottom_label.chars().count())
            .saturating_add(1)
            .min(full_width.saturating_sub(1))
    } else {
        0
    };
    if show_y_axis {
        let axis_color = resolve_named_color(props, "axis-color", theme::FG_MUTED());
        let axis_background = (background.a > 0.0).then_some(background);
        for (label_row, label) in [(0, &top_label), (height.saturating_sub(1), &bottom_label)] {
            for (index, character) in label.chars().take(label_width).enumerate() {
                buf.set(
                    row + label_row as u16,
                    col + index as u16,
                    styled_cell(character, axis_color, axis_background),
                );
            }
        }
    }
    let width = full_width.saturating_sub(label_width).max(1);
    let plot_col = col + label_width as u16;
    let values = &series.values;
    if values.is_empty() {
        return;
    }
    if values.len() == 1 {
        if let Some(value) = values[0] {
            let y = ((1.0 - value) * (height.saturating_sub(1)) as f32).round() as u16;
            let x = if explicit_total_points(props).is_some_and(|total| total > 1) {
                0
            } else {
                width / 2
            };
            buf.set(
                row + y,
                plot_col + x as u16,
                styled_cell('•', line, (background.a > 0.0).then_some(background)),
            );
        }
        return;
    }

    let domain_points = x_domain_points(props, values.len());
    for x in 0..width {
        let source_position = if width <= 1 || domain_points <= 1 {
            0.0
        } else {
            x as f32 * (domain_points - 1) as f32 / (width - 1) as f32
        };
        if source_position > (values.len() - 1) as f32 {
            continue;
        }
        let left = source_position.floor() as usize;
        let right = source_position.ceil() as usize;
        let value = match (
            values.get(left).copied().flatten(),
            values.get(right).copied().flatten(),
        ) {
            (Some(left_value), Some(right_value)) => {
                left_value + (right_value - left_value) * source_position.fract()
            }
            (Some(value), None) if left == right => value,
            _ => continue,
        };
        let y = ((1.0 - value) * (height.saturating_sub(1)) as f32).round() as u16;
        buf.set(
            row + y,
            plot_col + x as u16,
            styled_cell('•', line, (background.a > 0.0).then_some(background)),
        );
    }
}

fn push_polyline(
    primitives: &mut Vec<GpuPrimitive>,
    points: &[[f32; 2]],
    color: Color,
    viewport: WidgetViewport,
) {
    if points.len() < 2 {
        return;
    }
    let cell_w = viewport.cell_w.max(1.0);
    let cell_h = viewport.cell_h.max(1.0);
    let points_px: Vec<[f32; 2]> = points
        .iter()
        .map(|point| [point[0] * cell_w, point[1] * cell_h])
        .collect();
    let normalize = |vector: [f32; 2]| {
        let length = vector[0].hypot(vector[1]);
        if length > 1.0e-6 {
            [vector[0] / length, vector[1] / length]
        } else {
            [1.0, 0.0]
        }
    };
    let half_width_px = super::ui_design_px(0.9);
    let mut left = Vec::with_capacity(points.len());
    let mut right = Vec::with_capacity(points.len());
    for index in 0..points_px.len() {
        let previous_direction = if index > 0 {
            normalize([
                points_px[index][0] - points_px[index - 1][0],
                points_px[index][1] - points_px[index - 1][1],
            ])
        } else {
            normalize([
                points_px[index + 1][0] - points_px[index][0],
                points_px[index + 1][1] - points_px[index][1],
            ])
        };
        let next_direction = if index + 1 < points_px.len() {
            normalize([
                points_px[index + 1][0] - points_px[index][0],
                points_px[index + 1][1] - points_px[index][1],
            ])
        } else {
            previous_direction
        };
        let previous_normal = [-previous_direction[1], previous_direction[0]];
        let next_normal = [-next_direction[1], next_direction[0]];
        let miter = normalize([
            previous_normal[0] + next_normal[0],
            previous_normal[1] + next_normal[1],
        ]);
        let denominator = (miter[0] * next_normal[0] + miter[1] * next_normal[1]).max(1.0 / 3.0);
        let reach = half_width_px / denominator;
        left.push([
            (points_px[index][0] + miter[0] * reach) / cell_w,
            (points_px[index][1] + miter[1] * reach) / cell_h,
        ]);
        right.push([
            (points_px[index][0] - miter[0] * reach) / cell_w,
            (points_px[index][1] - miter[1] * reach) / cell_h,
        ]);
    }
    for index in 1..points.len() {
        primitives.push(GpuPrimitive::Triangle(GpuTrianglePrimitive {
            points: [left[index - 1], left[index], right[index]],
            color,
        }));
        primitives.push(GpuPrimitive::Triangle(GpuTrianglePrimitive {
            points: [left[index - 1], right[index], right[index - 1]],
            color,
        }));
    }
}

impl WidgetDefinition for LineGraphWidget {
    fn names(&self) -> &'static [&'static str] {
        &["linegraph"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["values", "total-points"]
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
                .max(1.0),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        tui_render(props, rect, buf);
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let series = normalized_series(&node.props);
        let values = &series.values;
        let background = background_color(&node.props);
        let line = line_color(&node.props);
        let grid = resolve_named_color(
            &node.props,
            "grid-color",
            Color::rgba(0.36, 0.36, 0.38, 0.20),
        );
        let area = resolve_named_color(
            &node.props,
            "area-color",
            Color::rgba(line.r, line.g, line.b, line.a * 0.16),
        );
        let mut primitives = Vec::new();
        if background.a > 0.0 {
            primitives.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: node.rect,
                color: background,
            }));
        }

        let show_y_axis = bool_prop(&node.props, "y-axis", false);
        let top_label = format_axis_value(series.axis_max);
        let bottom_label = format_axis_value(series.axis_min);
        let axis_gutter = if show_y_axis {
            top_label.len().max(bottom_label.len()) as f32 * 0.48 + 0.55
        } else {
            0.0
        };
        let horizontal_padding = 0.18f32.min(node.rect.width * 0.1);
        let vertical_padding = 0.16f32.min(node.rect.height * 0.1);
        let plot_left = node.rect.col + horizontal_padding + axis_gutter;
        let plot_top = node.rect.row + vertical_padding;
        let plot_width = (node.rect.width - horizontal_padding * 2.0 - axis_gutter).max(0.01);
        let plot_height = (node.rect.height - vertical_padding * 2.0).max(0.01);
        let plot_bottom = plot_top + plot_height;
        let grid_lines = node
            .props
            .get("grid-lines")
            .and_then(number_value)
            .unwrap_or(3.0)
            .round()
            .clamp(0.0, 12.0) as usize;
        for index in 1..=grid_lines {
            let fraction = index as f32 / (grid_lines + 1) as f32;
            primitives.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    row: plot_top + plot_height * fraction,
                    col: plot_left,
                    width: plot_width,
                    height: (1.0 / viewport.cell_h.max(1.0)).max(0.035),
                },
                color: grid,
            }));
        }
        if show_y_axis {
            let axis_color = resolve_named_color(&node.props, "axis-color", theme::FG_MUTED());
            let transparent = Color::rgba(0.0, 0.0, 0.0, 0.0);
            for (row, label) in [
                (node.rect.row + 0.02, top_label),
                (node.rect.row + node.rect.height - 0.72, bottom_label),
            ] {
                primitives.push(GpuPrimitive::ProportionalText(
                    GpuProportionalTextPrimitive {
                        row,
                        col: node.rect.col + horizontal_padding,
                        align_width: axis_gutter - 0.25,
                        h_align: 1.0,
                        text: label,
                        font_size: 6.5,
                        scale: 1.0,
                        fg: axis_color,
                        bg: transparent,
                    },
                ));
            }
        }
        if values.is_empty() {
            return primitives;
        }

        let domain_points = x_domain_points(&node.props, values.len());
        let point_for = |index: usize, value: f32| {
            let x = if domain_points <= 1 && explicit_total_points(&node.props).is_none() {
                plot_left + plot_width * 0.5
            } else {
                plot_left + plot_width * index as f32 / (domain_points - 1).max(1) as f32
            };
            [x, plot_top + (1.0 - value) * plot_height]
        };

        let mut current_path = Vec::new();
        let flush_path = |path: &mut Vec<[f32; 2]>, primitives: &mut Vec<GpuPrimitive>| {
            if area_enabled(&node.props) && path.len() >= 2 {
                for index in 1..path.len() {
                    let previous = path[index - 1];
                    let current = path[index];
                    primitives.push(GpuPrimitive::Triangle(GpuTrianglePrimitive {
                        points: [previous, current, [current[0], plot_bottom]],
                        color: area,
                    }));
                    primitives.push(GpuPrimitive::Triangle(GpuTrianglePrimitive {
                        points: [
                            previous,
                            [current[0], plot_bottom],
                            [previous[0], plot_bottom],
                        ],
                        color: area,
                    }));
                }
            }
            push_polyline(primitives, path, line, viewport);
            path.clear();
        };
        for (index, value) in values.iter().enumerate() {
            if let Some(value) = value {
                current_path.push(point_for(index, *value));
            } else {
                flush_path(&mut current_path, &mut primitives);
            }
        }
        flush_path(&mut current_path, &mut primitives);

        if let Some((index, value)) = values
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, value)| value.map(|value| (index, value)))
        {
            let point = point_for(index, value);
            let marker_width = (3.0 / viewport.cell_w.max(1.0)).max(0.08);
            let marker_height = (3.0 / viewport.cell_h.max(1.0)).max(0.04);
            primitives.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    row: point[1] - marker_height * 0.5,
                    col: point[0] - marker_width * 0.5,
                    width: marker_width,
                    height: marker_height,
                },
                color: line,
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

    fn list(values: impl IntoIterator<Item = f64>) -> Value {
        Value::List(
            values
                .into_iter()
                .map(|value| Rc::new(RefCell::new(Value::Number(value))))
                .collect(),
        )
    }

    #[test]
    fn live_data_props_are_bindable_but_do_not_affect_layout_size() {
        assert_eq!(
            LINEGRAPH_WIDGET.bindable_props(),
            &["values", "total-points"]
        );
        assert!(!LINEGRAPH_WIDGET.size_affecting_props().contains(&"values"));
        assert!(!LINEGRAPH_WIDGET
            .size_affecting_props()
            .contains(&"total-points"));
    }

    #[test]
    fn changing_values_are_auto_ranged_into_distinct_heights() {
        let props = HashMap::from([("values".to_string(), list([8.0, 4.0, 2.0, 1.0]))]);
        let normalized = normalized_series(&props).values;
        assert!(normalized[0] > normalized[1]);
        assert!(normalized[1] > normalized[2]);
        assert!(normalized[2] > normalized[3]);
    }

    #[test]
    fn log_scale_preserves_each_order_of_magnitude() {
        let props = HashMap::from([
            ("values".to_string(), list([100.0, 10.0, 1.0])),
            ("scale".to_string(), Value::Keyword("log".to_string())),
        ]);
        let normalized = normalized_series(&props).values;
        let upper_step = normalized[0].unwrap() - normalized[1].unwrap();
        let lower_step = normalized[1].unwrap() - normalized[2].unwrap();
        assert!((upper_step - lower_step).abs() < 1.0e-5);
    }

    #[test]
    fn reactive_values_are_read_at_render_time() {
        let slots = crate::reactive::ReactiveBindingStore::default();
        let reactive = slots.seeded_float_ref("APP", "loss", 10.0);
        let props = HashMap::from([
            (
                "values".to_string(),
                Value::List(vec![
                    Rc::new(RefCell::new(reactive)),
                    Rc::new(RefCell::new(Value::Number(1.0))),
                ]),
            ),
            ("min".to_string(), Value::Number(0.0)),
            ("max".to_string(), Value::Number(10.0)),
        ]);
        assert_eq!(normalized_series(&props).values[0], Some(1.0));
        slots.write_float("APP", "loss", 5.0);
        assert_eq!(normalized_series(&props).values[0], Some(0.5));
    }

    #[test]
    fn constructor_accepts_reactive_values_and_total_point_bindings() {
        let slots = crate::reactive::ReactiveBindingStore::default();
        let widget = crate::widgets::build_widget(
            "linegraph",
            vec![
                Value::Keyword("values".to_string()),
                slots.seeded_float_ref("APP", "loss", 1.0),
                Value::Keyword("total-points".to_string()),
                slots.seeded_float_ref("APP", "epochs", 50.0),
            ],
        );
        let Value::Map(map) = widget else {
            panic!("expected linegraph widget map");
        };
        assert!(!map.contains_key("__widget-diagnostic"));
    }

    #[test]
    fn zero_anchored_axis_reports_its_real_bounds() {
        let props = HashMap::from([
            ("values".to_string(), list([16.0, 15.6])),
            ("min".to_string(), Value::Number(0.0)),
        ]);
        let series = normalized_series(&props);
        assert_eq!(series.axis_min, 0.0);
        assert_eq!(series.axis_max, 16.0);
        assert_eq!(series.values[1], Some(15.6 / 16.0));
    }

    #[test]
    fn total_points_keeps_partial_trajectories_at_the_left() {
        let props = HashMap::from([
            ("values".to_string(), list([8.0, 7.0, 6.0])),
            ("total-points".to_string(), Value::Number(10.0)),
        ]);
        assert_eq!(x_domain_points(&props, 3), 10);
    }

    #[test]
    fn metal_primitives_form_a_filled_connected_trace() {
        let node = LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "linegraph".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 20.0,
                height: 4.0,
            },
            props: HashMap::from([
                ("values".to_string(), list([1.0, 0.5, 0.25])),
                ("total-points".to_string(), Value::Number(10.0)),
                ("y-axis".to_string(), Value::Bool(true)),
                ("area".to_string(), Value::Bool(true)),
            ]),
            children: Vec::new(),
            focusable: false,
            animation: Default::default(),
        };
        let primitives = LINEGRAPH_WIDGET.build_primitives(
            "linegraph",
            &node,
            WidgetViewport {
                cell_w: 10.0,
                cell_h: 20.0,
                vp_w: 640.0,
                vp_h: 360.0,
                time_seconds: 0.0,
                focused_widget_id: None,
                focused_branch: false,
                overlay_viewport_bottom: 18.0,
                scroll_top: 0.0,
                scroll_left: 0.0,
                inherited_hover: false,
            },
        );
        let triangles = primitives
            .iter()
            .filter(|primitive| matches!(primitive, GpuPrimitive::Triangle(_)))
            .count();
        assert_eq!(
            triangles, 8,
            "two area and two stroke triangles per segment"
        );
        let marker = primitives
            .iter()
            .rev()
            .find_map(|primitive| match primitive {
                GpuPrimitive::Rect(rect) if rect.rect.width < 1.0 && rect.rect.height < 1.0 => {
                    Some(rect)
                }
                _ => None,
            })
            .expect("latest-point marker");
        assert!(
            marker.rect.col < node.rect.width * 0.5,
            "three of ten points should occupy only the left side: {:?}",
            marker.rect
        );
        let labels = primitives
            .iter()
            .filter(|primitive| matches!(primitive, GpuPrimitive::ProportionalText(_)))
            .count();
        assert_eq!(labels, 2, "top and bottom y-axis labels");
    }
}
