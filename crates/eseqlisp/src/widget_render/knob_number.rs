use std::cell::RefCell;
use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent,
    get_f32_prop, resolve_named_color, should_trigger_integer_haptic, styled_cell,
    trigger_level_change_haptic,
};
use crate::backend::Color;
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_map,
    get_prop_num,
};
use crate::theme;
use crate::vm::Value;

#[cfg(target_os = "macos")]
use super::{
    FocusCornerStyle, FocusDecoration, MetalPrimitive, MetalProportionalTextPrimitive,
    MetalRectPrimitive, WidgetInstance, WidgetViewport, ndc_bounds,
};

#[derive(Clone, Debug, Default)]
struct KnobNumberState {
    editing: bool,
    edit_text: String,
    cursor_pos: usize,
}

thread_local! {
    static STATES: RefCell<HashMap<u64, KnobNumberState>> = RefCell::new(HashMap::new());
    static CHAR_WIDTHS: RefCell<HashMap<(u32, u32), HashMap<char, f32>>> = RefCell::new(HashMap::new());
    static LINE_HEIGHTS: RefCell<HashMap<(u32, u32), f32>> = RefCell::new(HashMap::new());
}

fn get_state(widget_id: u64) -> KnobNumberState {
    STATES.with(|s| s.borrow().get(&widget_id).cloned().unwrap_or_default())
}

fn set_state(widget_id: u64, state: KnobNumberState) {
    STATES.with(|s| s.borrow_mut().insert(widget_id, state));
    super::bump_widget_state_generation();
}

fn format_value(value: f64, decimals: u32) -> String {
    format!("{:.*}", decimals as usize, value)
}

/// Display text for the current value: the formatted number plus an optional
/// `unit` suffix (e.g. "dB", "%"). Edit mode seeds the plain number so typed
/// input parses without stripping the unit.
fn format_display(props: &HashMap<String, Value>, value: f32, decimals: u32) -> String {
    let text = format_value(display_value(props, value) as f64, decimals);
    match props.get("unit") {
        Some(Value::String(unit)) if !unit.is_empty() => format!("{text} {unit}"),
        _ => text,
    }
}

fn knob_edit_color(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(
        props,
        "edit-color",
        Color {
            r: 1.0,
            g: 0.95,
            b: 0.25,
            a: 1.0,
        },
    )
}

fn display_decimals(props: &HashMap<String, Value>) -> u32 {
    let decimals = get_f32_prop(props, "decimals", 2.0) as u32;
    let min = get_f32_prop(props, "min", 0.0);
    let max = get_f32_prop(props, "max", 1.0);
    let display_range = ((max - min) * value_scale(props)).abs();
    if display_range < 10.0 {
        decimals
    } else if display_range < 100.0 {
        decimals.min(1)
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use std::rc::Rc;

    fn numeric_props(min: f64, max: f64, decimals: f64) -> HashMap<String, Value> {
        HashMap::from([
            ("min".to_string(), Value::Number(min)),
            ("max".to_string(), Value::Number(max)),
            ("decimals".to_string(), Value::Number(decimals)),
        ])
    }

    #[test]
    fn display_decimals_preserves_precision_for_small_ranges() {
        let props = numeric_props(0.0, 1.0, 2.0);
        assert_eq!(display_decimals(&props), 2);
    }

    #[test]
    fn display_decimals_removes_precision_for_large_ranges() {
        let props = numeric_props(20.0, 20_000.0, 2.0);
        assert_eq!(display_decimals(&props), 0);
    }

    #[test]
    fn display_decimals_keeps_one_decimal_for_mid_ranges() {
        let props = numeric_props(-24.0, 24.0, 1.0);
        assert_eq!(display_decimals(&props), 1);
        let props = numeric_props(-24.0, 24.0, 2.0);
        assert_eq!(display_decimals(&props), 1);
    }

    #[test]
    fn display_decimals_uses_scaled_display_range() {
        let mut props = numeric_props(0.0, 1.0, 2.0);
        props.insert("value-scale".to_string(), Value::Number(100.0));
        assert_eq!(display_decimals(&props), 0);
    }

    #[test]
    fn normalized_origin_defaults_to_min_and_supports_bipolar_zero() {
        let mut props = numeric_props(-1.0, 1.0, 2.0);
        props.insert("value".to_string(), Value::Number(0.5));
        assert_eq!(normalized_value_with_origin(&props), (0.75, 0.0));

        props.insert("origin".to_string(), Value::Number(0.0));
        assert_eq!(normalized_value_with_origin(&props), (0.75, 0.5));
    }

    #[cfg(target_os = "macos")]
    fn value_cell(value: Value) -> Rc<RefCell<Value>> {
        Rc::new(RefCell::new(value))
    }

    #[cfg(target_os = "macos")]
    fn mod_range(slot: f64, depth: f64) -> Value {
        Value::Map(HashMap::from([
            ("slot".to_string(), value_cell(Value::Number(slot))),
            ("depth".to_string(), value_cell(Value::Number(depth))),
        ]))
    }

    #[cfg(target_os = "macos")]
    fn test_viewport() -> WidgetViewport {
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 10.0,
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

    #[cfg(target_os = "macos")]
    fn test_knob_node(props: HashMap<String, Value>) -> LayoutNode {
        LayoutNode {
            widget_id: 42,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "knob-number".to_string(),
            rect: Rect {
                row: 2.0,
                col: 3.0,
                width: 4.0,
                height: 2.8,
            },
            props,
            children: Vec::new(),
            focusable: true,
            animation: Default::default(),
        }
    }

    #[cfg(target_os = "macos")]
    fn assert_rect_contains(outer: Rect, inner: Rect) {
        let epsilon = 0.000_01;
        assert!(
            inner.row + epsilon >= outer.row,
            "{inner:?} starts above {outer:?}"
        );
        assert!(
            inner.col + epsilon >= outer.col,
            "{inner:?} starts left of {outer:?}"
        );
        assert!(
            inner.row + inner.height <= outer.row + outer.height + epsilon,
            "{inner:?} extends below {outer:?}"
        );
        assert!(
            inner.col + inner.width <= outer.col + outer.width + epsilon,
            "{inner:?} extends right of {outer:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn oversized_knob_is_clamped_inside_default_overlay_layout() {
        let mut node = test_knob_node(HashMap::from([
            ("label".to_string(), Value::String("pan".to_string())),
            ("font-size".to_string(), Value::Number(9.0)),
            ("label-font-size".to_string(), Value::Number(8.0)),
            ("knob-size".to_string(), Value::Number(20.0)),
        ]));
        node.rect.width = 3.9;
        node.rect.height = 2.35;
        let viewport = test_viewport();
        let layout = knob_number_component_layout(&node, viewport, "pan", "0.00", "0.00", true);

        assert_rect_contains(node.rect, layout.knob_rect);
        assert_rect_contains(node.rect, layout.label_band.expect("label band"));
        assert_rect_contains(node.rect, layout.value_band.expect("value band"));
        assert!(layout.knob_rect.height < 20.0);
        assert_eq!(
            layout.knob_rect.width * viewport.cell_w,
            layout.knob_rect.height * viewport.cell_h
        );
        let label_bottom = layout.label_band.unwrap().row + layout.label_band.unwrap().height;
        assert!(
            layout.knob_rect.row - label_bottom <= 0.11,
            "label and knob should use only the fixed one-pixel gap: {layout:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn knob_size_is_clamped_by_width_with_non_square_cells() {
        let mut node = test_knob_node(HashMap::from([
            ("label".to_string(), Value::String("frequency".to_string())),
            ("font-size".to_string(), Value::Number(9.0)),
            ("label-font-size".to_string(), Value::Number(8.0)),
            ("knob-size".to_string(), Value::Number(20.0)),
        ]));
        node.rect.width = 1.5;
        node.rect.height = 6.0;
        let viewport = WidgetViewport {
            cell_w: 8.0,
            cell_h: 16.0,
            ..test_viewport()
        };
        let layout =
            knob_number_component_layout(&node, viewport, "frequency", "20 kHz", "20 kHz", true);

        assert_rect_contains(node.rect, layout.knob_rect);
        assert_rect_contains(node.rect, layout.label_band.expect("label band"));
        assert_rect_contains(node.rect, layout.value_band.expect("value band"));
        assert!(layout.knob_rect.width <= node.rect.width);
        assert_eq!(
            layout.knob_rect.width * viewport.cell_w,
            layout.knob_rect.height * viewport.cell_h
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn short_overlay_layout_does_not_overlap_label_and_value_bands() {
        let mut node = test_knob_node(HashMap::from([
            ("label".to_string(), Value::String("pan".to_string())),
            ("font-size".to_string(), Value::Number(12.0)),
            ("label-font-size".to_string(), Value::Number(12.0)),
            ("knob-size".to_string(), Value::Number(20.0)),
        ]));
        node.rect.height = 0.8;
        let layout =
            knob_number_component_layout(&node, test_viewport(), "pan", "0.00", "0.00", true);
        let label_band = layout.label_band.expect("label band");
        let value_band = layout.value_band.expect("value band");

        assert_rect_contains(node.rect, label_band);
        assert_rect_contains(node.rect, layout.knob_rect);
        assert_rect_contains(node.rect, value_band);
        assert!(label_band.row + label_band.height <= value_band.row);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn overlay_value_band_tracks_knob_bottom_instead_of_widget_bottom() {
        let mut node = test_knob_node(HashMap::from([
            ("label".to_string(), Value::String("speed".to_string())),
            ("font-size".to_string(), Value::Number(10.0)),
            ("label-font-size".to_string(), Value::Number(9.0)),
            ("knob-size".to_string(), Value::Number(1.0)),
        ]));
        node.rect.width = 4.7;
        node.rect.height = 5.0;
        let viewport = WidgetViewport {
            cell_w: 20.0,
            cell_h: 43.0,
            ..test_viewport()
        };
        let layout = knob_number_component_layout(&node, viewport, "speed", "1.0", "1.0", true);
        let value_band = layout.value_band.expect("value band");
        let knob_bottom = layout.knob_rect.row + layout.knob_rect.height;
        let value_bottom = value_band.row + value_band.height;

        assert_eq!(layout.value_h_align, 1.0);
        assert!((value_bottom - (knob_bottom + 4.0 / viewport.cell_h)).abs() < 0.000_01);
        assert!(
            value_bottom < node.rect.row + node.rect.height - 0.5,
            "value should remain attached to a small knob instead of falling to the widget bottom"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn wide_overlay_value_is_centered_in_the_lower_band() {
        let mut node = test_knob_node(HashMap::from([
            ("label".to_string(), Value::String("sr".to_string())),
            ("font-size".to_string(), Value::Number(10.5)),
            ("label-font-size".to_string(), Value::Number(10.0)),
            ("knob-size".to_string(), Value::Number(2.5)),
        ]));
        node.rect.width = 4.7;
        node.rect.height = 2.05;
        let viewport = WidgetViewport {
            cell_w: 20.0,
            cell_h: 43.0,
            ..test_viewport()
        };
        let layout = knob_number_component_layout(
            &node,
            viewport,
            "sr",
            "42645000000000000000",
            "42645000000000000000",
            true,
        );
        let value_band = layout.value_band.expect("value band");
        let value_bottom = value_band.row + value_band.height;
        let expected_content_bottom = node.rect.row + node.rect.height - 1.0 / viewport.cell_h;

        assert_eq!(layout.value_h_align, 0.5);
        assert!((value_bottom - expected_content_bottom).abs() < 0.000_01);
        assert_eq!(layout.value_text_rect, layout.text_rect);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn wider_widget_can_keep_the_same_range_value_in_the_compact_pocket() {
        let mut node = test_knob_node(HashMap::from([
            ("label".to_string(), Value::String("sr".to_string())),
            ("font-size".to_string(), Value::Number(10.5)),
            ("label-font-size".to_string(), Value::Number(10.0)),
            ("knob-size".to_string(), Value::Number(2.5)),
        ]));
        node.rect.width = 4.7;
        node.rect.height = 2.05;
        let viewport = WidgetViewport {
            cell_w: 20.0,
            cell_h: 43.0,
            ..test_viewport()
        };
        let narrow = knob_number_component_layout(
            &node,
            viewport,
            "sr",
            "42645000000000",
            "42645000000000",
            true,
        );

        node.rect.width = 8.0;
        let wide = knob_number_component_layout(
            &node,
            viewport,
            "sr",
            "42645000000000",
            "42645000000000",
            true,
        );

        assert_eq!(narrow.value_h_align, 0.5);
        assert_eq!(wide.value_h_align, 1.0);
        assert_ne!(wide.value_text_rect, wide.text_rect);
        assert_rect_contains(node.rect, wide.value_text_rect);
        assert_rect_contains(node.rect, wide.value_band.expect("value band"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn compact_value_pocket_starts_at_the_projected_45_degree_arc_endpoint() {
        let knob_rect = Rect {
            row: 1.0,
            col: 3.0,
            width: 4.0,
            height: 4.0,
        };
        let text_rect = Rect {
            row: 0.0,
            col: 1.0,
            width: 8.0,
            height: 6.0,
        };
        let ring_outer_radius = knob_rect.width * 0.331;
        let open_sector_left = knob_rect.col + knob_rect.width * 0.5
            - ring_outer_radius * std::f32::consts::FRAC_1_SQRT_2;
        let pocket =
            compact_overlay_value_text_rect(knob_rect, text_rect, 2.0, open_sector_left, 10.0)
                .expect("two-cell value should fit the open sector");

        assert_eq!(pocket.col, open_sector_left);
        assert!(pocket.col + pocket.width <= text_rect.col + text_rect.width);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn hidden_value_does_not_require_a_compact_value_pocket() {
        let node = test_knob_node(HashMap::from([
            ("label".to_string(), Value::String("gain".to_string())),
            ("font-size".to_string(), Value::Number(10.0)),
            ("knob-size".to_string(), Value::Number(1.5)),
        ]));
        let layout =
            knob_number_component_layout(&node, test_viewport(), "gain", "0.00", "0.00", false);

        assert!(layout.value_band.is_none());
        assert_rect_contains(node.rect, layout.knob_rect);
        assert_rect_contains(node.rect, layout.label_band.expect("label band"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pan_layout_uses_widest_range_endpoint_instead_of_current_value() {
        let mut props = numeric_props(-1.0, 1.0, 2.0);
        props.extend([
            ("label".to_string(), Value::String("pan".to_string())),
            ("font-size".to_string(), Value::Number(9.0)),
            ("label-font-size".to_string(), Value::Number(8.0)),
            ("knob-size".to_string(), Value::Number(1.88)),
        ]);
        let mut node = test_knob_node(props);
        node.rect.width = 3.9;
        node.rect.height = 2.35;
        let viewport = test_viewport();
        let range_width_text = widest_range_display_text(&node.props, 2, 9.0, viewport.cell_w);
        assert_eq!(range_width_text, "-1.00");

        let positive =
            knob_number_component_layout(&node, viewport, "pan", "0.16", &range_width_text, true);
        let negative =
            knob_number_component_layout(&node, viewport, "pan", "-0.33", &range_width_text, true);

        assert_eq!(positive.value_h_align, 0.5);
        assert_eq!(positive.value_h_align, negative.value_h_align);
        assert_eq!(positive.value_band, negative.value_band);
        assert_eq!(positive.value_text_rect, negative.value_text_rect);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn centered_value_layout_contains_all_three_component_bands() {
        let node = test_knob_node(HashMap::from([
            ("label".to_string(), Value::String("Frequency".to_string())),
            ("font-size".to_string(), Value::Number(10.0)),
            ("label-font-size".to_string(), Value::Number(10.0)),
            ("knob-size".to_string(), Value::Number(30.0)),
            (
                "value-align".to_string(),
                Value::Keyword("center".to_string()),
            ),
        ]));
        let layout = knob_number_component_layout(
            &node,
            test_viewport(),
            "Frequency",
            "191 Hz",
            "191 Hz",
            true,
        );
        let label_band = layout.label_band.expect("label band");
        let value_band = layout.value_band.expect("value band");

        assert_rect_contains(node.rect, label_band);
        assert_rect_contains(node.rect, layout.knob_rect);
        assert_rect_contains(node.rect, value_band);
        assert!(label_band.row + label_band.height <= layout.knob_rect.row);
        assert!(layout.knob_rect.row + layout.knob_rect.height <= value_band.row);
        assert_eq!(layout.value_h_align, 0.5);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rich_mod_ranges_emit_base_knob_text_and_range_primitives() {
        let node = test_knob_node(HashMap::from([
            ("label".to_string(), Value::String("cut".to_string())),
            ("value".to_string(), Value::Number(0.0)),
            ("min".to_string(), Value::Number(-1.0)),
            ("max".to_string(), Value::Number(1.0)),
            ("base-value".to_string(), Value::Number(0.0)),
            ("base-min".to_string(), Value::Number(-1.0)),
            ("base-max".to_string(), Value::Number(1.0)),
            ("selected-mod-slot".to_string(), Value::Number(1.0)),
            (
                "mod-ranges".to_string(),
                Value::List(vec![
                    value_cell(mod_range(1.0, 0.5)),
                    value_cell(mod_range(2.0, -0.25)),
                ]),
            ),
        ]));

        let primitives =
            KNOB_NUMBER_WIDGET.build_metal_primitives("knob-number", &node, test_viewport());
        let base_instances = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                MetalPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    ..
                } if widget_type == "knob-number" => Some(instance),
                _ => None,
            })
            .collect::<Vec<_>>();
        let range_instances = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                MetalPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    ..
                } if widget_type == "knob-number-mod-range" => Some(instance),
                _ => None,
            })
            .collect::<Vec<_>>();
        let text = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                MetalPrimitive::ProportionalText(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(base_instances.len(), 1);
        assert_eq!(base_instances[0].uniform_b[0], 0.0);
        assert_eq!(range_instances.len(), 2);
        assert_eq!(range_instances[0].uniform_b, [0.94, 0.5, 0.75, 1.0]);
        assert!((range_instances[1].uniform_b[0] - 0.895).abs() < 0.000_01);
        assert_eq!(&range_instances[1].uniform_b[1..], &[0.5, 0.375, 0.0]);
        assert!(
            text.contains(&"cut"),
            "knob-number should still emit label/value text primitives: {text:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn metal_knob_receives_normalized_bipolar_origin() {
        let node = test_knob_node(HashMap::from([
            ("label".to_string(), Value::String("pan".to_string())),
            ("value".to_string(), Value::Number(0.5)),
            ("min".to_string(), Value::Number(-1.0)),
            ("max".to_string(), Value::Number(1.0)),
            ("origin".to_string(), Value::Number(0.0)),
        ]));
        let primitives =
            KNOB_NUMBER_WIDGET.build_metal_primitives("knob-number", &node, test_viewport());
        let instance = primitives
            .iter()
            .find_map(|primitive| match primitive {
                MetalPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    ..
                } if widget_type == "knob-number" => Some(instance),
                _ => None,
            })
            .expect("knob-number base instance");

        assert_eq!(instance.value_t, 0.75);
        assert_eq!(instance.uniform_a[3], 0.5);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mod_range_arc_uses_display_domain_for_percent_depths() {
        let node = test_knob_node(HashMap::from([
            ("label".to_string(), Value::String("scrub".to_string())),
            ("value".to_string(), Value::Number(100.0)),
            ("min".to_string(), Value::Number(-100.0)),
            ("max".to_string(), Value::Number(100.0)),
            ("base-value".to_string(), Value::Number(0.0)),
            ("base-min".to_string(), Value::Number(-100.0)),
            ("base-max".to_string(), Value::Number(100.0)),
            ("selected-mod-slot".to_string(), Value::Number(1.0)),
            (
                "mod-ranges".to_string(),
                Value::List(vec![value_cell(mod_range(1.0, 100.0))]),
            ),
        ]));

        let primitives =
            KNOB_NUMBER_WIDGET.build_metal_primitives("knob-number", &node, test_viewport());
        let range = primitives
            .iter()
            .find_map(|primitive| match primitive {
                MetalPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    ..
                } if widget_type == "knob-number-mod-range" => Some(instance),
                _ => None,
            })
            .expect("full-range scrub modulation depth should emit a range arc");

        assert_eq!(range.uniform_b[1], 0.5);
        assert_eq!(range.uniform_b[2], 1.0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn outer_mod_range_radius_keeps_stroke_inside_primitive_bounds() {
        let selected_radius = mod_range_ring_radius(0, true);
        let unselected_radius = mod_range_ring_radius(0, false);

        assert!(selected_radius + mod_range_ring_half_width(true) < 1.0);
        assert!(unselected_radius + mod_range_ring_half_width(false) < 1.0);
        assert_eq!(selected_radius, 0.94);
        assert_eq!(unselected_radius, 0.956);
    }
}

fn quantized_value(props: &HashMap<String, Value>, value: f32) -> f32 {
    let min = get_f32_prop(props, "min", 0.0);
    let max = get_f32_prop(props, "max", 1.0);
    let step = get_f32_prop(props, "step", 0.0);
    let clamped = value.clamp(min, max);
    if step.is_finite() && step > 0.0 {
        (min + ((clamped - min) / step).round() * step).clamp(min, max)
    } else {
        clamped
    }
}

fn value_scale(props: &HashMap<String, Value>) -> f32 {
    get_f32_prop(props, "value-scale", 1.0).max(0.000001)
}

fn display_value(props: &HashMap<String, Value>, value: f32) -> f32 {
    value * value_scale(props)
}

fn model_value_from_display(props: &HashMap<String, Value>, value: f32) -> f32 {
    value / value_scale(props)
}

fn normalized_value_with_origin(props: &HashMap<String, Value>) -> (f32, f32) {
    let value = quantized_value(props, get_f32_prop(props, "value", 0.0));
    let min = get_f32_prop(props, "min", 0.0);
    let max = get_f32_prop(props, "max", 1.0);
    let range = max - min;
    if range > 0.0 {
        let value_t = ((value - min) / range).clamp(0.0, 1.0);
        let origin = get_f32_prop(props, "origin", min);
        let origin_t = ((origin - min) / range).clamp(0.0, 1.0);
        (value_t, origin_t)
    } else {
        (0.0, 0.0)
    }
}

#[cfg(target_os = "macos")]
fn value_as_f32(value: &Value) -> Option<f32> {
    let value = match value {
        Value::Number(n) => Some(*n as f32),
        Value::ReactiveRef { slot, .. } => Some(crate::reactive::read_float_slot(slot) as f32),
        _ => None,
    }?;
    value.is_finite().then_some(value)
}

#[cfg(target_os = "macos")]
fn map_f32(
    map: &HashMap<String, std::rc::Rc<std::cell::RefCell<Value>>>,
    key: &str,
) -> Option<f32> {
    map.get(key).and_then(|value| value_as_f32(&value.borrow()))
}

#[cfg(target_os = "macos")]
fn mod_slot_color(slot: i32, selected: bool) -> Color {
    let mut color = match slot {
        1 => Color {
            r: 0.10,
            g: 0.56,
            b: 1.0,
            a: 1.0,
        },
        2 => Color {
            r: 0.96,
            g: 0.50,
            b: 0.18,
            a: 1.0,
        },
        3 => Color {
            r: 0.23,
            g: 0.78,
            b: 0.42,
            a: 1.0,
        },
        4 => Color {
            r: 0.62,
            g: 0.42,
            b: 0.98,
            a: 1.0,
        },
        5 => Color {
            r: 0.00,
            g: 0.78,
            b: 0.86,
            a: 1.0,
        },
        6 => Color {
            r: 0.98,
            g: 0.72,
            b: 0.18,
            a: 1.0,
        },
        7 => Color {
            r: 0.92,
            g: 0.30,
            b: 0.22,
            a: 1.0,
        },
        8 => Color {
            r: 0.18,
            g: 0.70,
            b: 0.95,
            a: 1.0,
        },
        9 => Color {
            r: 0.74,
            g: 0.86,
            b: 0.24,
            a: 1.0,
        },
        10 => Color {
            r: 0.95,
            g: 0.42,
            b: 0.78,
            a: 1.0,
        },
        _ => Color {
            r: 0.85,
            g: 0.85,
            b: 0.85,
            a: 1.0,
        },
    };
    color.a = if selected { 0.95 } else { 0.58 };
    color
}

#[cfg(target_os = "macos")]
fn mod_range_ring_half_width(selected: bool) -> f32 {
    // Keep these in sync with the Metal shader's halfWidth constants below;
    // CPU radius clamping relies on the same stroke width to stay in bounds.
    if selected { 0.056 } else { 0.040 }
}

#[cfg(target_os = "macos")]
fn mod_range_ring_radius(range_index: usize, selected: bool) -> f32 {
    const OUTER_EDGE_MARGIN: f32 = 0.004;
    let preferred = (0.98 - (range_index.min(4) as f32 * 0.085)).max(0.64);
    preferred.min(1.0 - mod_range_ring_half_width(selected) - OUTER_EDGE_MARGIN)
}

#[cfg(target_os = "macos")]
fn cache_font_metrics(
    font_size: f32,
    chars: &str,
    measurer: &dyn crate::layout::TextMeasurer,
    ctx: &MeasureCtx<'_>,
) {
    let width_key = (font_size.to_bits(), ctx.cell_w.to_bits());
    CHAR_WIDTHS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let widths = cache.entry(width_key).or_default();
        for ch in chars.chars() {
            widths.entry(ch).or_insert_with(|| {
                measurer.measure_text_px(&ch.to_string(), font_size) / ctx.cell_w.max(0.000_001)
            });
        }
    });

    let height_key = (font_size.to_bits(), ctx.cell_h.to_bits());
    LINE_HEIGHTS.with(|cache| {
        cache
            .borrow_mut()
            .entry(height_key)
            .or_insert_with(|| measurer.line_height_px(font_size) / ctx.cell_h.max(0.000_001));
    });
}

#[cfg(target_os = "macos")]
fn text_width_cells(text: &str, font_size: f32, cell_w: f32) -> f32 {
    let fallback = font_size * 0.55 / cell_w.max(0.000_001);
    CHAR_WIDTHS.with(|cache| {
        cache
            .borrow()
            .get(&(font_size.to_bits(), cell_w.to_bits()))
            .map(|widths| {
                text.chars()
                    .map(|ch| widths.get(&ch).copied().unwrap_or(fallback))
                    .sum()
            })
            .unwrap_or_else(|| text.chars().count() as f32 * fallback)
    })
}

#[cfg(target_os = "macos")]
fn widest_range_display_text(
    props: &HashMap<String, Value>,
    decimals: u32,
    font_size: f32,
    cell_w: f32,
) -> String {
    let min_text = format_display(props, get_f32_prop(props, "min", 0.0), decimals);
    let max_text = format_display(props, get_f32_prop(props, "max", 1.0), decimals);
    if text_width_cells(&min_text, font_size, cell_w)
        >= text_width_cells(&max_text, font_size, cell_w)
    {
        min_text
    } else {
        max_text
    }
}

#[cfg(target_os = "macos")]
fn line_height_cells(font_size: f32, cell_h: f32) -> f32 {
    LINE_HEIGHTS.with(|cache| {
        cache
            .borrow()
            .get(&(font_size.to_bits(), cell_h.to_bits()))
            .copied()
            .unwrap_or(font_size * 1.2 / cell_h.max(0.000_001))
    })
}

#[cfg(target_os = "macos")]
fn compact_overlay_value_text_rect(
    knob_rect: Rect,
    text_rect: Rect,
    stable_value_width: f32,
    pocket_left: f32,
    cell_w: f32,
) -> Option<Rect> {
    // The right edge grows only as much as the stable range text needs beyond
    // its preferred knob-relative anchor, so widening the widget creates usable
    // room without pulling short values away from the knob.
    const VALUE_KNOB_OVERHANG_PX: f32 = 4.0;

    let knob_right = knob_rect.col + knob_rect.width;
    let text_right = text_rect.col + text_rect.width;
    let pocket_left = pocket_left.max(text_rect.col).min(text_right);
    let available_width = (text_right - pocket_left).max(0.0);
    if stable_value_width > available_width {
        return None;
    }

    let preferred_right = (knob_right + VALUE_KNOB_OVERHANG_PX / cell_w.max(0.000_001))
        .min(text_right)
        .max(pocket_left);
    let value_right = preferred_right
        .max(pocket_left + stable_value_width)
        .min(text_right);
    Some(Rect {
        row: text_rect.row,
        col: pocket_left,
        width: (value_right - pocket_left).max(0.0),
        height: text_rect.height,
    })
}

#[cfg(target_os = "macos")]
fn cursor_x_from_cache(
    text: &str,
    cursor_pos: usize,
    measured_font_size: f32,
    rendered_font_size: f32,
    cell_w: f32,
) -> f32 {
    let key = (measured_font_size.to_bits(), cell_w.to_bits());
    let scale = if measured_font_size > 0.0 {
        rendered_font_size / measured_font_size
    } else {
        1.0
    };
    CHAR_WIDTHS.with(|cw| {
        let cache = cw.borrow();
        if let Some(widths) = cache.get(&key) {
            let fallback = measured_font_size * 0.55 / cell_w;
            text.chars()
                .take(cursor_pos)
                .map(|ch| widths.get(&ch).copied().unwrap_or(fallback))
                .sum::<f32>()
                * scale
        } else {
            cursor_pos as f32 * rendered_font_size * 0.55 / cell_w
        }
    })
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug)]
struct KnobNumberComponentLayout {
    knob_rect: Rect,
    label_band: Option<Rect>,
    label_font_size: f32,
    value_band: Option<Rect>,
    value_font_size: f32,
    text_rect: Rect,
    value_text_rect: Rect,
    value_h_align: f32,
}

#[cfg(target_os = "macos")]
fn fit_font_size(
    text: &str,
    requested_font_size: f32,
    max_width: f32,
    max_height: f32,
    viewport: WidgetViewport,
) -> f32 {
    if text.is_empty() || requested_font_size <= 0.0 || max_width <= 0.0 || max_height <= 0.0 {
        return 0.0;
    }
    let width = text_width_cells(text, requested_font_size, viewport.cell_w);
    let height = line_height_cells(requested_font_size, viewport.cell_h);
    let width_scale = if width > 0.0 { max_width / width } else { 1.0 };
    let height_scale = if height > 0.0 {
        max_height / height
    } else {
        1.0
    };
    requested_font_size * width_scale.min(height_scale).min(1.0).max(0.0) * 0.98
}

#[cfg(target_os = "macos")]
fn knob_number_component_layout(
    node: &LayoutNode,
    viewport: WidgetViewport,
    label: &str,
    value_text: &str,
    range_width_text: &str,
    value_visible: bool,
) -> KnobNumberComponentLayout {
    const CONTENT_INSET_PX: f32 = 1.0;
    const TEXT_RASTER_PAD_PX: f32 = 3.0;
    const COMPONENT_GAP_PX: f32 = 1.0;

    let cell_w = viewport.cell_w.max(0.000_001);
    let cell_h = viewport.cell_h.max(0.000_001);
    let inset_x = (CONTENT_INSET_PX / cell_w).min(node.rect.width * 0.5);
    let inset_y = (CONTENT_INSET_PX / cell_h).min(node.rect.height * 0.5);
    let content = Rect {
        row: node.rect.row + inset_y,
        col: node.rect.col + inset_x,
        width: (node.rect.width - inset_x * 2.0).max(0.0),
        height: (node.rect.height - inset_y * 2.0).max(0.0),
    };
    let text_pad = (TEXT_RASTER_PAD_PX / cell_w).min(content.width * 0.5);
    let text_rect = Rect {
        row: content.row,
        col: content.col + text_pad,
        width: (content.width - text_pad * 2.0).max(0.0),
        height: content.height,
    };

    let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE).max(0.0);
    let requested_label_font =
        get_f32_prop(&node.props, "label-font-size", font_size * 0.88).max(0.0);
    let has_label = !label.is_empty();
    let has_value = value_visible && !value_text.is_empty();
    let natural_label_height = if has_label {
        line_height_cells(requested_label_font, cell_h)
    } else {
        0.0
    };
    let requested_label_height = node
        .props
        .get("label-height")
        .and_then(|value| match value {
            Value::Number(height) => Some((*height as f32).max(0.0)),
            _ => None,
        });
    let mut label_height = requested_label_height
        .map(|height| natural_label_height.min(height))
        .unwrap_or(natural_label_height);
    let natural_value_height = if has_value {
        line_height_cells(font_size, cell_h)
    } else {
        0.0
    };
    let gap = COMPONENT_GAP_PX / cell_h;
    let center_value = matches!(
        node.props.get("value-align"),
        Some(Value::Keyword(align)) if align == "center"
    );
    let mut label_gap = if has_label { gap } else { 0.0 };
    let mut value_gap = if center_value && has_value { gap } else { 0.0 };
    let mut value_height = if center_value {
        natural_value_height
    } else {
        0.0
    };

    let fixed_height = label_height + label_gap + value_gap + value_height;
    if fixed_height > content.height && fixed_height > 0.0 {
        let scale = content.height / fixed_height;
        label_height *= scale;
        label_gap *= scale;
        value_gap *= scale;
        value_height *= scale;
    }

    let available_knob_height =
        (content.height - label_height - label_gap - value_gap - value_height).max(0.0);
    let available_knob_width_as_height = content.width * cell_w / cell_h;
    let max_knob_size = available_knob_height.min(available_knob_width_as_height);
    let requested_knob_size = node
        .props
        .get("knob-size")
        .and_then(|value| match value {
            Value::Number(size) => Some(*size as f32),
            _ => None,
        })
        .unwrap_or(max_knob_size)
        .max(0.0);
    let knob_size = requested_knob_size.min(max_knob_size);
    let knob_width = knob_size * cell_h / cell_w;
    let stack_height = label_height + label_gap + knob_size + value_gap + value_height;
    let mut row = content.row + (content.height - stack_height).max(0.0) * 0.5;

    let label_band = has_label.then(|| {
        let band = Rect {
            row,
            col: text_rect.col,
            width: text_rect.width,
            height: label_height,
        };
        row += label_height + label_gap;
        band
    });
    let knob_rect = Rect {
        row,
        col: content.col + (content.width - knob_width).max(0.0) * 0.5,
        width: knob_width,
        height: knob_size,
    };
    row += knob_size + value_gap;
    const VALUE_KNOB_OVERHANG_PX: f32 = 4.0;
    const VALUE_BOTTOM_INSET_PX: f32 = 5.0;
    // Shader `activeRing` reaches p=0.662. Since p spans -1..1, its outer
    // radius occupies 0.331 of the square knob primitive.
    const RING_OUTER_RADIUS_FRACTION: f32 = 0.331;

    let overlay_top = if has_label {
        knob_rect.row
    } else {
        content.row
    };
    let content_bottom = content.row + content.height;
    let near_knob_bottom = (knob_rect.row + knob_rect.height + VALUE_KNOB_OVERHANG_PX / cell_h)
        .min(content_bottom - VALUE_BOTTOM_INSET_PX / cell_h)
        .max(overlay_top);
    let stable_value_width = text_width_cells(range_width_text, font_size, cell_w);

    // The arc endpoints are rotated 45 degrees away from the bottom axis. The
    // left edge of that open sector is the horizontal projection of the outer
    // ring radius, which gives the value text the space intentionally left
    // beneath the knob without crossing the drawn arc.
    let ring_outer_radius = knob_rect.width * RING_OUTER_RADIUS_FRACTION;
    let open_sector_left =
        knob_rect.col + knob_rect.width * 0.5 - ring_outer_radius * std::f32::consts::FRAC_1_SQRT_2;
    let compact_value_text_rect = (!center_value && has_value)
        .then(|| {
            compact_overlay_value_text_rect(
                knob_rect,
                text_rect,
                stable_value_width,
                open_sector_left,
                cell_w,
            )
        })
        .flatten();
    let wide_overlay_value = !center_value && has_value && compact_value_text_rect.is_none();
    let value_text_rect = if !has_value || center_value || wide_overlay_value {
        text_rect
    } else {
        compact_value_text_rect.expect("visible non-centered values have a compact text rect")
    };
    let value_band = if !has_value {
        None
    } else if center_value {
        Some(Rect {
            row,
            col: value_text_rect.col,
            width: value_text_rect.width,
            height: value_height,
        })
    } else {
        let preferred_bottom = if wide_overlay_value {
            content_bottom
        } else {
            near_knob_bottom
        }
        .max(overlay_top);
        let overlay_height = natural_value_height.min((preferred_bottom - overlay_top).max(0.0));
        Some(Rect {
            row: preferred_bottom - overlay_height,
            col: value_text_rect.col,
            width: value_text_rect.width,
            height: overlay_height,
        })
    };

    let label_font_size = label_band
        .map(|band| {
            fit_font_size(
                label,
                requested_label_font,
                band.width,
                band.height,
                viewport,
            )
        })
        .unwrap_or(0.0);
    let value_font_size = value_band
        .map(|band| fit_font_size(value_text, font_size, band.width, band.height, viewport))
        .unwrap_or(0.0);
    let value_h_align = if center_value || wide_overlay_value {
        0.5
    } else {
        1.0
    };

    KnobNumberComponentLayout {
        knob_rect,
        label_band,
        label_font_size,
        value_band,
        value_font_size,
        text_rect,
        value_text_rect,
        value_h_align,
    }
}

pub struct KnobNumberWidget;
pub static KNOB_NUMBER_WIDGET: KnobNumberWidget = KnobNumberWidget;

impl WidgetDefinition for KnobNumberWidget {
    fn names(&self) -> &'static [&'static str] {
        &["knob-number", "knob-number-mod-range"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &[
            "width",
            "height",
            "font-size",
            "decimals",
            "step",
            "show-value",
        ]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &[
            "value",
            "origin",
            "base-value",
            "base-min",
            "base-max",
            "selected-mod-slot",
            "mod-range-0-slot",
            "mod-range-0-depth",
            "mod-range-1-slot",
            "mod-range-1-depth",
            "mod-range-2-slot",
            "mod-range-2-depth",
            "mod-range-3-slot",
            "mod-range-3-depth",
            "mod-range-4-slot",
            "mod-range-4-depth",
            "mod-range-5-slot",
            "mod-range-5-depth",
            "mod-range-6-slot",
            "mod-range-6-depth",
            "mod-range-7-slot",
            "mod-range-7-depth",
            "mod-range-8-slot",
            "mod-range-8-depth",
            "mod-range-9-slot",
            "mod-range-9-depth",
            "plock-active",
            "plock-default",
            "plock-color-r",
            "plock-color-g",
            "plock-color-b",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        #[cfg(target_os = "macos")]
        if let Some(measurer) = ctx.text_measurer {
            let font_size = get_prop_num(node, "font-size")
                .map(f64_to_f32)
                .unwrap_or(ctx.inherited_font_size);
            let label_size = get_prop_num(node, "label-font-size")
                .map(f64_to_f32)
                .unwrap_or(font_size * 0.88);
            let props = get_map(node).unwrap_or_default();
            let label = props.get("label").and_then(|value| match value {
                Value::String(label) => Some(label.as_str()),
                _ => None,
            });
            let unit = props.get("unit").and_then(|value| match value {
                Value::String(unit) => Some(unit.as_str()),
                _ => None,
            });
            let mut value_chars = String::from("0123456789.- ");
            if let Some(unit) = unit {
                value_chars.push_str(unit);
            }
            cache_font_metrics(font_size, &value_chars, measurer, ctx);
            cache_font_metrics(label_size, label.unwrap_or(""), measurer, ctx);
        }
        Some(Size {
            width: get_prop_num(node, "width").map(f64_to_f32).unwrap_or(5.2),
            height: get_prop_num(node, "height").map(f64_to_f32).unwrap_or(2.8),
        })
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn unclamped_drag(&self) -> bool {
        true
    }

    fn begin_gesture(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
        _modifiers: KeyModifiers,
    ) -> Option<Value> {
        let value = get_f32_prop(&node.props, "value", 0.0);
        Some(Value::List(vec![
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(value as f64))),
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(local_col as f64))),
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(local_row as f64))),
        ]))
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
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let mut state = get_state(node.widget_id);
                state.editing = false;
                set_state(node.widget_id, state);
                MouseEventOutcome::Consume
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(Value::List(gesture_list)) = gesture else {
                    return MouseEventOutcome::Consume;
                };
                let Some(start_value) = gesture_list.first().and_then(|v| match &*v.borrow() {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                }) else {
                    return MouseEventOutcome::Consume;
                };
                let Some(start_col) = gesture_list.get(1).and_then(|v| match &*v.borrow() {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                }) else {
                    return MouseEventOutcome::Consume;
                };
                let Some(start_row) = gesture_list.get(2).and_then(|v| match &*v.borrow() {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                }) else {
                    return MouseEventOutcome::Consume;
                };

                let min = get_f32_prop(&node.props, "min", 0.0);
                let max = get_f32_prop(&node.props, "max", 1.0);
                let range = (max - min).max(0.0001);
                let dx = local_col - start_col;
                let dy = start_row - local_row;
                let drag_cells = get_f32_prop(&node.props, "drag-cells", 8.0).max(1.0);
                let delta = ((dy * 1.0) + (dx * 0.15)) / drag_cells * range;
                let new_value = quantized_value(&node.props, start_value + delta);
                MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::Number(new_value as f64)))
            }
            _ => MouseEventOutcome::Consume,
        }
    }

    fn key_event(&self, node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
        let mut state = get_state(node.widget_id);
        let value = get_f32_prop(&node.props, "value", 0.0);
        let decimals = display_decimals(&node.props);

        match key.code {
            KeyCode::Char(c)
                if (c.is_ascii_digit() || c == '.' || c == '-')
                    && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
            {
                if !state.editing {
                    state.editing = true;
                    state.edit_text.clear();
                    state.cursor_pos = 0;
                }
                state.edit_text.insert(state.cursor_pos, c);
                state.cursor_pos += 1;
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Backspace if state.editing => {
                if state.cursor_pos > 0 {
                    state.cursor_pos -= 1;
                    state.edit_text.remove(state.cursor_pos);
                }
                if state.edit_text.is_empty() {
                    state.editing = false;
                }
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Left if state.editing => {
                state.cursor_pos = state.cursor_pos.saturating_sub(1);
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Right if state.editing => {
                state.cursor_pos = (state.cursor_pos + 1).min(state.edit_text.len());
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Enter => {
                if state.editing {
                    let min = get_f32_prop(&node.props, "min", 0.0);
                    let max = get_f32_prop(&node.props, "max", 1.0);
                    let parsed = state
                        .edit_text
                        .parse::<f64>()
                        .map(|value| model_value_from_display(&node.props, value as f32) as f64)
                        .unwrap_or(value as f64)
                        .clamp(min as f64, max as f64);
                    let parsed = quantized_value(&node.props, parsed as f32) as f64;
                    state.editing = false;
                    state.edit_text.clear();
                    state.cursor_pos = 0;
                    set_state(node.widget_id, state);
                    Some(WidgetEvent::Custom(Value::Number(parsed)))
                } else {
                    state.editing = true;
                    state.edit_text =
                        format_value(display_value(&node.props, value) as f64, decimals);
                    state.cursor_pos = state.edit_text.len();
                    set_state(node.widget_id, state);
                    Some(WidgetEvent::Custom(Value::Nil))
                }
            }
            KeyCode::Esc => {
                state.editing = false;
                state.edit_text.clear();
                state.cursor_pos = 0;
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            _ => None,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let new_value = match event {
            WidgetEvent::SetNormalized(t) => {
                let min = get_f32_prop(&node.props, "min", 0.0);
                let max = get_f32_prop(&node.props, "max", 1.0);
                min + (max - min) * t.clamp(0.0, 1.0)
            }
            WidgetEvent::Custom(Value::Number(n)) => n as f32,
            WidgetEvent::Custom(Value::Nil) => return None,
            _ => return None,
        };
        let new_value = quantized_value(&node.props, new_value);
        let previous = get_f32_prop(&node.props, "value", 0.0);
        let min = get_f32_prop(&node.props, "min", 0.0);
        let max = get_f32_prop(&node.props, "max", 1.0);
        if should_trigger_integer_haptic(node.widget_id, previous, new_value, min, max) {
            trigger_level_change_haptic();
        }
        let callback = node.props.get("on-change")?.clone();
        Some(EventOutput {
            callback,
            args: vec![Value::Number(new_value as f64)],
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let label = props
            .get("label")
            .and_then(|v| match v {
                Value::String(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        let value = quantized_value(props, get_f32_prop(props, "value", 0.0));
        let decimals = display_decimals(props);
        let show_value = !matches!(props.get("show-value"), Some(Value::Bool(false)));
        let text = if show_value {
            format!("{} {}", label, format_display(props, value, decimals))
        } else {
            label.to_string()
        };
        let fg = Color {
            r: 0.9,
            g: 0.9,
            b: 0.9,
            a: 1.0,
        };
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16;
        for (i, ch) in text.chars().enumerate() {
            let c = col_start + i as u16;
            if c >= col_start + rect.width.round() as u16 {
                break;
            }
            buf.set(row, c, styled_cell(ch, fg, None));
        }
    }

    #[cfg(target_os = "macos")]
    fn renders_own_focus(&self) -> bool {
        true
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, widget_type: &str) -> Option<&'static str> {
        match widget_type {
            "knob-number" => Some(KNOB_NUMBER_SHADER),
            "knob-number-mod-range" => Some(KNOB_NUMBER_MOD_RANGE_SHADER),
            _ => None,
        }
    }

    #[cfg(target_os = "macos")]
    fn metal_focus_decoration(&self, node: &LayoutNode) -> FocusDecoration {
        FocusDecoration::Corners(FocusCornerStyle::new(resolve_named_color(
            &node.props,
            "focus-color",
            knob_edit_color(&node.props),
        )))
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let value = quantized_value(&node.props, get_f32_prop(&node.props, "value", 0.0));
        let decimals = display_decimals(&node.props);
        let state = get_state(node.widget_id);
        let is_focused = viewport.focused_widget_id == Some(node.widget_id);
        let show_value = !matches!(node.props.get("show-value"), Some(Value::Bool(false)));
        let text_color = resolve_named_color(
            &node.props,
            "text-color",
            Color {
                r: 0.90,
                g: 0.90,
                b: 0.92,
                a: 1.0,
            },
        );
        let label_color = resolve_named_color(
            &node.props,
            "label-color",
            Color {
                r: 0.52,
                g: 0.52,
                b: 0.55,
                a: 1.0,
            },
        );
        let edit_color = knob_edit_color(&node.props);
        let cursor_color = resolve_named_color(
            &node.props,
            "cursor-color",
            Color {
                r: 1.0,
                g: 0.95,
                b: 0.25,
                a: 1.0,
            },
        );
        let plock_active = get_f32_prop(&node.props, "plock-active", 0.0) > 0.5;
        let plock_color = Color {
            r: get_f32_prop(&node.props, "plock-color-r", 0.270_588_25),
            g: get_f32_prop(&node.props, "plock-color-g", 0.784_313_74),
            b: get_f32_prop(&node.props, "plock-color-b", 0.862_745_1),
            a: 1.0,
        };
        let arc_color = if plock_active {
            plock_color
        } else {
            resolve_named_color(&node.props, "arc-color", theme::WIDGET_KNOB_FILLED())
        };
        let track_color =
            resolve_named_color(&node.props, "track-color", theme::WIDGET_KNOB_TRACK());
        let (display_text, fg) = if state.editing {
            (state.edit_text.clone(), edit_color)
        } else if is_focused {
            (format_display(&node.props, value, decimals), edit_color)
        } else if plock_active {
            (format_display(&node.props, value, decimals), plock_color)
        } else {
            (format_display(&node.props, value, decimals), text_color)
        };
        let label = node
            .props
            .get("label")
            .and_then(|value| match value {
                Value::String(label) => Some(label.as_str()),
                _ => None,
            })
            .unwrap_or("");
        let value_visible = show_value || state.editing || is_focused;
        let range_width_text = widest_range_display_text(
            &node.props,
            decimals,
            get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE),
            viewport.cell_w,
        );
        let component_layout = knob_number_component_layout(
            node,
            viewport,
            label,
            &display_text,
            &range_width_text,
            value_visible,
        );
        let knob_rect = component_layout.knob_rect;
        let (ndc_min, ndc_max) = ndc_bounds(knob_rect, viewport);
        let px_w = knob_rect.width * viewport.cell_w;
        let px_h = knob_rect.height * viewport.cell_h;
        let min = get_f32_prop(&node.props, "min", 0.0);
        let max = get_f32_prop(&node.props, "max", 1.0);
        let (value_t, origin_t) = normalized_value_with_origin(&node.props);
        let default_t = if (max - min).abs() > 0.000_001 {
            ((get_f32_prop(&node.props, "plock-default", value) - min) / (max - min))
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut prims = vec![MetalPrimitive::WidgetInstance {
            widget_type: widget_type.to_string(),
            instance: WidgetInstance {
                ndc_min,
                ndc_max,
                value_t,
                orientation: 0.0,
                itime: viewport.time_seconds,
                uniform_a: [
                    if is_focused { 1.0 } else { 0.0 },
                    if plock_active { 1.0 } else { 0.0 },
                    default_t,
                    origin_t,
                ],
                uniform_b: [0.0; 4],
                uniform_c: [0.0; 4],
                uniform_d: [0.0; 4],
                color_a: [arc_color.r, arc_color.g, arc_color.b, arc_color.a],
                color_b: [track_color.r, track_color.g, track_color.b, track_color.a],
                color_c: [edit_color.r, edit_color.g, edit_color.b, edit_color.a],
                color_d: [plock_color.r, plock_color.g, plock_color.b, plock_color.a],
                corner_radius: 0.0,
                pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
            },
            is_background: false,
        }];

        let base_min = node
            .props
            .get("base-min")
            .and_then(value_as_f32)
            .unwrap_or_else(|| get_f32_prop(&node.props, "min", 0.0));
        let base_max = node
            .props
            .get("base-max")
            .and_then(value_as_f32)
            .unwrap_or_else(|| get_f32_prop(&node.props, "max", 1.0));
        let base_value = node
            .props
            .get("base-value")
            .and_then(value_as_f32)
            .unwrap_or(value);
        let base_range = base_max - base_min;
        let selected_slot = node
            .props
            .get("selected-mod-slot")
            .and_then(value_as_f32)
            .unwrap_or(0.0)
            .round() as i32;
        if base_range.abs() > 0.000_001 {
            let base_t = ((base_value - base_min) / base_range).clamp(0.0, 1.0);
            let mut ranges = Vec::new();

            if let Some(Value::List(mod_ranges)) = node.props.get("mod-ranges") {
                for range in mod_ranges {
                    if let Value::Map(map) = &*range.borrow()
                        && let (Some(slot), Some(depth)) =
                            (map_f32(map, "slot"), map_f32(map, "depth"))
                    {
                        ranges.push((slot, depth));
                    }
                }
            }
            for idx in 0..10 {
                let slot_key = format!("mod-range-{idx}-slot");
                let depth_key = format!("mod-range-{idx}-depth");
                if let (Some(slot), Some(depth)) = (
                    node.props.get(&slot_key).and_then(value_as_f32),
                    node.props.get(&depth_key).and_then(value_as_f32),
                ) {
                    ranges.push((slot, depth));
                }
            }

            for (range_index, (slot_f, depth)) in ranges.into_iter().enumerate() {
                let slot = slot_f.round() as i32;
                if slot <= 0 {
                    continue;
                }
                let end_t = ((base_value + depth - base_min) / base_range).clamp(0.0, 1.0);
                let selected = slot == selected_slot;
                let color = mod_slot_color(slot, selected);
                prims.push(MetalPrimitive::WidgetInstance {
                    widget_type: "knob-number-mod-range".to_string(),
                    instance: WidgetInstance {
                        ndc_min,
                        ndc_max,
                        value_t,
                        orientation: 0.0,
                        itime: viewport.time_seconds,
                        uniform_a: [if is_focused { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
                        uniform_b: [
                            mod_range_ring_radius(range_index, selected),
                            base_t,
                            end_t,
                            if selected { 1.0 } else { 0.0 },
                        ],
                        uniform_c: [0.0; 4],
                        uniform_d: [0.0; 4],
                        color_a: [color.r, color.g, color.b, color.a],
                        color_b: [track_color.r, track_color.g, track_color.b, track_color.a],
                        color_c: [edit_color.r, edit_color.g, edit_color.b, edit_color.a],
                        color_d: [0.0; 4],
                        corner_radius: 0.0,
                        pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                    },
                    is_background: false,
                });
            }
        }

        if let Some(label_band) = component_layout.label_band
            && component_layout.label_font_size >= 0.5
        {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: label_band.row + label_band.height * 0.5 - 0.5,
                    col: component_layout.text_rect.col,
                    align_width: component_layout.text_rect.width,
                    h_align: 0.5,
                    text: label.to_string(),
                    font_size: component_layout.label_font_size,
                    scale: 1.0,
                    fg: label_color,
                    bg: Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    },
                },
            ));
        }

        if let Some(value_band) = component_layout.value_band
            && component_layout.value_font_size >= 0.5
        {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: value_band.row + value_band.height * 0.5 - 0.5,
                    col: component_layout.value_text_rect.col,
                    align_width: component_layout.value_text_rect.width,
                    h_align: component_layout.value_h_align,
                    text: display_text.clone(),
                    font_size: component_layout.value_font_size,
                    scale: 1.0,
                    fg,
                    bg: Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    },
                },
            ));
        }

        if is_focused
            && state.editing
            && let Some(value_band) = component_layout.value_band
            && component_layout.value_font_size >= 0.5
        {
            let requested_value_font =
                get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE).max(0.0);
            let text_scale = if requested_value_font > 0.0 {
                component_layout.value_font_size / requested_value_font
            } else {
                1.0
            };
            let text_width =
                text_width_cells(&display_text, requested_value_font, viewport.cell_w) * text_scale;
            let text_left = component_layout.value_text_rect.col
                + (component_layout.value_text_rect.width - text_width).max(0.0)
                    * component_layout.value_h_align;
            let cursor_x = cursor_x_from_cache(
                &display_text,
                state.cursor_pos,
                requested_value_font,
                component_layout.value_font_size,
                viewport.cell_w,
            );
            let cursor_width =
                (1.0 / viewport.cell_w.max(0.000_001)).min(component_layout.value_text_rect.width);
            let cursor_col = (text_left + cursor_x).clamp(
                component_layout.value_text_rect.col,
                component_layout.value_text_rect.col + component_layout.value_text_rect.width
                    - cursor_width,
            );
            prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: Rect {
                    row: value_band.row + value_band.height * 0.08,
                    col: cursor_col,
                    width: cursor_width,
                    height: value_band.height * 0.84,
                },
                color: cursor_color,
            }));
        }

        prims
    }
}

#[cfg(target_os = "macos")]
const KNOB_NUMBER_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float2 p = float2((uv.x - 0.5) * 2.0, (uv.y - 0.5) * 2.0);
    float r = length(p);
    float a = atan2(p.y, p.x);

    float start = 1.57079633;
    float sweep = 4.71238898;
    float rel = fmod((a - start + 6.2831853), 6.2831853);
    float inRange = step(rel, sweep);
    float valueRel = sweep * clamp(in.value_t, 0.0, 1.0);
    float originRel = sweep * clamp(in.uniform_a.w, 0.0, 1.0);
    float fillLo = min(valueRel, originRel);
    float fillHi = max(valueRel, originRel);
    float fillSpan = fillHi - fillLo;
    float active = step(fillLo, rel) * step(rel, fillHi) * step(0.001, fillSpan);

    float knobRadius = 0.58;
    float ring = abs(r - knobRadius) - 0.070;
    float activeRing = abs(r - knobRadius) - 0.082;
    float aa = max(fwidth(r), 0.0015);
    float ringMask = smoothstep(aa, -aa, ring) * inRange;
    float activeMask = smoothstep(aa, -aa, activeRing) * inRange * active;
    float glowRing = abs(r - knobRadius) - 0.150;
    float glowMask = smoothstep(aa * 4.0, -aa * 4.0, glowRing) * inRange * active * step(0.5, in.uniform_a.y);
    float trackMask = ringMask * (1.0 - active);

    float notchAngle = start + valueRel;
    float2 n = float2(cos(notchAngle), sin(notchAngle));
    float notch = length(p - n * knobRadius) - 0.070;
    float notchMask = smoothstep(aa, -aa, notch);
    float lineAlong = dot(p, n);
    float lineAcross = abs(p.x * n.y - p.y * n.x);
    float lineSegment = step(0.0, lineAlong) * step(lineAlong, 0.52);
    float line = lineAcross - 0.070;
    float lineMask = smoothstep(aa, -aa, line) * lineSegment;
    float defaultAngle = start + sweep * clamp(in.uniform_a.z, 0.0, 1.0);
    float2 dn = float2(cos(defaultAngle), sin(defaultAngle));
    float defaultNotch = length(p - dn * knobRadius) - 0.046;
    float defaultMask = smoothstep(aa, -aa, defaultNotch)
        * step(0.5, in.uniform_a.y)
        * step(0.01, abs(in.uniform_a.z - in.value_t));

    float4 col = float4(0.0);
    col = mix(col, float4(in.color_d.rgb, 0.20), glowMask);
    col = mix(col, in.color_b, trackMask);
    col = mix(col, in.color_a, activeMask);
    col = mix(col, in.color_b, lineMask);
    col = mix(col, in.color_a, notchMask);
    col = mix(col, float4(0.36, 0.36, 0.41, 0.95), defaultMask);
    if (col.a < 0.01) { discard_fragment(); }
    return col;
}
"#;

#[cfg(target_os = "macos")]
const KNOB_NUMBER_MOD_RANGE_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float2 p = float2((uv.x - 0.5) * 2.0, (uv.y - 0.5) * 2.0);
    float r = length(p);
    float a = atan2(p.y, p.x);

    float start = 1.57079633;
    float sweep = 4.71238898;
    float rel = fmod((a - start + 6.2831853), 6.2831853);
    float inRange = step(rel, sweep);
    float aa = max(fwidth(r), 0.0015);

    float ringRadius = clamp(in.uniform_b.x, 0.62, 1.02);
    float t0 = clamp(in.uniform_b.y, 0.0, 1.0);
    float t1 = clamp(in.uniform_b.z, 0.0, 1.0);
    float lo = min(t0, t1) * sweep;
    float hi = max(t0, t1) * sweep;
    float selected = step(0.5, in.uniform_b.w);
    float radius = ringRadius;
    float halfWidth = mix(0.040, 0.056, selected);
    float modRing = abs(r - radius) - halfWidth;
    float arcMask = step(lo, rel) * step(rel, hi) * inRange;
    float mask = smoothstep(aa, -aa, modRing) * arcMask;
    float4 col = float4(in.color_a.rgb, in.color_a.a * mask);
    if (col.a < 0.01) { discard_fragment(); }
    return col;
}
"#;
