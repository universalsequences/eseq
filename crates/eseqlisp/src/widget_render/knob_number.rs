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
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
};
use crate::theme;
use crate::vm::Value;

#[cfg(target_os = "macos")]
use super::{
    MetalPrimitive, MetalProportionalTextPrimitive, MetalRectPrimitive, WidgetInstance,
    WidgetViewport, ndc_bounds,
};

#[derive(Clone, Debug, Default)]
struct KnobNumberState {
    editing: bool,
    edit_text: String,
    cursor_pos: usize,
}

thread_local! {
    static STATES: RefCell<HashMap<u64, KnobNumberState>> = RefCell::new(HashMap::new());
    static CHAR_WIDTHS: RefCell<HashMap<u32, HashMap<char, f32>>> = RefCell::new(HashMap::new());
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
            tile_content_rows: 36.0,
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
        }
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

fn normalized_value(props: &HashMap<String, Value>) -> f32 {
    let value = quantized_value(props, get_f32_prop(props, "value", 0.0));
    let min = get_f32_prop(props, "min", 0.0);
    let max = get_f32_prop(props, "max", 1.0);
    let range = max - min;
    if range > 0.0 {
        ((value - min) / range).clamp(0.0, 1.0)
    } else {
        0.0
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
fn cursor_x_from_cache(text: &str, cursor_pos: usize, font_size: f32, cell_w: f32) -> f32 {
    let key = font_size.to_bits();
    CHAR_WIDTHS.with(|cw| {
        let cache = cw.borrow();
        if let Some(widths) = cache.get(&key) {
            let fallback = font_size * 0.55 / cell_w;
            text.chars()
                .take(cursor_pos)
                .map(|ch| widths.get(&ch).copied().unwrap_or(fallback))
                .sum()
        } else {
            cursor_pos as f32 * font_size * 0.55 / cell_w
        }
    })
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
            let key = font_size.to_bits();
            CHAR_WIDTHS.with(|cw| {
                let mut cache = cw.borrow_mut();
                cache.entry(key).or_insert_with(|| {
                    let mut widths = HashMap::new();
                    for ch in "0123456789.-".chars() {
                        let px = measurer.measure_text_px(&ch.to_string(), font_size);
                        widths.insert(ch, px / ctx.cell_w);
                    }
                    widths
                });
            });
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

    fn begin_gesture(&self, node: &LayoutNode, local_col: f32, local_row: f32) -> Option<Value> {
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
            format!(
                "{} {}",
                label,
                format_value(display_value(props, value) as f64, decimals)
            )
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
        let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);
        let label_size = get_f32_prop(&node.props, "label-font-size", font_size * 0.88);
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
        let edit_color = resolve_named_color(
            &node.props,
            "edit-color",
            Color {
                r: 1.0,
                g: 0.95,
                b: 0.25,
                a: 1.0,
            },
        );
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
        let arc_color = resolve_named_color(&node.props, "arc-color", theme::WIDGET_KNOB_FILLED());
        let track_color =
            resolve_named_color(&node.props, "track-color", theme::WIDGET_KNOB_TRACK());

        // `:value-align :center` switches to a stacked layout — label / knob /
        // value — with the value horizontally centered below the knob (Ableton
        // Operator-style). Default layout keeps the value tucked into the
        // bottom-right of the knob arc (Cirklon-style).
        let center_value = matches!(
            node.props.get("value-align"),
            Some(Value::Keyword(k)) if k == "center"
        );
        let (knob_size, knob_top, value_row, label_row) = if center_value {
            // The metal text renderer takes `row` as the top of a 1-cell-tall
            // band and visually centers the glyph inside it. To place a label
            // inside an arbitrary band, we put `row` at band-center − 0.5 so
            // the glyph's center lands at the band's center.
            //
            // Bands are sized to the actual font (with a small margin) so the
            // knob keeps whatever vertical space is left — taller widgets get
            // proportionally bigger knobs, not bigger label/value bands.
            let cell_h = viewport.cell_h.max(0.000_001);
            let label_band = (label_size * 1.15 / cell_h).max(0.55);
            let value_band = (font_size * 1.05 / cell_h).max(0.55);
            let knob_band = (node.rect.height - label_band - value_band).max(0.5);
            let knob_size = get_f32_prop(&node.props, "knob-size", knob_band)
                .max(0.6)
                .min(knob_band);
            let label_center = node.rect.row + label_band * 0.5;
            let value_center = node.rect.row + node.rect.height - value_band * 0.5;
            let knob_top = node.rect.row + label_band + (knob_band - knob_size) * 0.5;
            (knob_size, knob_top, value_center - 0.5, label_center - 0.5)
        } else {
            // `:label-height N` lets callers shrink the band reserved above
            // the knob for the label — useful when the widget is short and
            // the default 45%-of-height reservation eats too much room.
            let label_height_override = node.props.get("label-height").and_then(|v| {
                if let Value::Number(n) = v {
                    Some(*n as f32)
                } else {
                    None
                }
            });
            let default_top_band = (node.rect.height * 0.45).max(0.72);
            let top_band = label_height_override.unwrap_or(default_top_band);
            // When the label band is overridden, also grow the knob to fill
            // the remaining vertical space (callers can still pin it via
            // `:knob-size`).
            let default_knob_size = if label_height_override.is_some() {
                (node.rect.height - top_band - 0.05).max(0.6)
            } else {
                node.rect.height * 0.53
            };
            let knob_size = get_f32_prop(&node.props, "knob-size", default_knob_size)
                .max(0.6)
                .min(node.rect.height * 0.9);
            let large_knob_value_offset = ((knob_size - 1.45).max(0.0) * 1.1).min(0.32);
            let value_row = (node.rect.row + node.rect.height * 0.62 + large_knob_value_offset)
                .min(node.rect.row + node.rect.height - 0.55);
            let knob_top = node.rect.row + top_band;
            // When the label band is overridden small, the label glyph (which
            // normally fills a 1-cell band centered on `label_row`) would dip
            // into the knob. Shift label_row up so the glyph centers in the
            // narrower label band instead of in a 1-cell window.
            let label_row = if label_height_override.is_some() {
                node.rect.row + top_band * 0.5 - 0.5
            } else {
                node.rect.row
            };
            (knob_size, knob_top, value_row, label_row)
        };
        let knob_width = if viewport.cell_w > 0.0 {
            knob_size * viewport.cell_h / viewport.cell_w
        } else {
            knob_size
        };
        let knob_rect = Rect {
            row: knob_top,
            col: node.rect.col + (node.rect.width - knob_width) * 0.5,
            width: knob_width,
            height: knob_size,
        };
        let (ndc_min, ndc_max) = ndc_bounds(knob_rect, viewport);
        let px_w = knob_rect.width * viewport.cell_w;
        let px_h = knob_rect.height * viewport.cell_h;
        let mut prims = vec![MetalPrimitive::WidgetInstance {
            widget_type: widget_type.to_string(),
            instance: WidgetInstance {
                ndc_min,
                ndc_max,
                value_t: normalized_value(&node.props),
                orientation: 0.0,
                itime: viewport.time_seconds,
                uniform_a: [if is_focused { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
                uniform_b: [0.0; 4],
                color_a: [arc_color.r, arc_color.g, arc_color.b, arc_color.a],
                color_b: [track_color.r, track_color.g, track_color.b, track_color.a],
                color_c: [edit_color.r, edit_color.g, edit_color.b, edit_color.a],
                color_d: [0.0; 4],
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
                        value_t: normalized_value(&node.props),
                        orientation: 0.0,
                        itime: viewport.time_seconds,
                        uniform_a: [if is_focused { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
                        uniform_b: [
                            mod_range_ring_radius(range_index, selected),
                            base_t,
                            end_t,
                            if selected { 1.0 } else { 0.0 },
                        ],
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

        if let Some(Value::String(label)) = node.props.get("label") {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: label_row,
                    col: node.rect.col + node.rect.width * 0.5
                        - (label.chars().count() as f32 * 0.31),
                    align_width: 0.0,
                    h_align: 0.0,
                    text: label.clone(),
                    font_size: label_size,
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

        let (display_text, fg) = if state.editing {
            (state.edit_text.clone(), edit_color)
        } else if is_focused {
            (
                format_value(display_value(&node.props, value) as f64, decimals),
                edit_color,
            )
        } else {
            (
                format_value(display_value(&node.props, value) as f64, decimals),
                text_color,
            )
        };
        let text_width_cells = CHAR_WIDTHS.with(|cw| {
            let cache = cw.borrow();
            let fallback = font_size * 0.55 / viewport.cell_w.max(0.000_001);
            cache
                .get(&font_size.to_bits())
                .map(|widths| {
                    display_text
                        .chars()
                        .map(|ch| widths.get(&ch).copied().unwrap_or(fallback))
                        .sum::<f32>()
                })
                .unwrap_or_else(|| display_text.chars().count() as f32 * fallback)
        });
        let (text_col, align_width, h_align, value_font_size) = if center_value {
            let available_cells = (node.rect.width - 0.2).max(0.5);
            let scaled = if text_width_cells > available_cells && text_width_cells > 0.0 {
                font_size * (available_cells / text_width_cells)
            } else {
                font_size
            };
            (node.rect.col, node.rect.width, 0.5_f32, scaled)
        } else {
            (knob_rect.col + knob_width * 0.55, 0.0, 0.0_f32, font_size)
        };
        let text_row = value_row;
        if show_value || state.editing || is_focused {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: text_row,
                    col: text_col,
                    align_width,
                    h_align,
                    text: display_text.clone(),
                    font_size: value_font_size,
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

        if is_focused && state.editing {
            let text_left = if center_value {
                let scale = if font_size > 0.0 {
                    value_font_size / font_size
                } else {
                    1.0
                };
                let rendered_text_width = text_width_cells * scale;
                node.rect.col + (node.rect.width - rendered_text_width) * 0.5
            } else {
                text_col
            };
            let cursor_x = cursor_x_from_cache(
                &display_text,
                state.cursor_pos,
                value_font_size,
                viewport.cell_w,
            );
            prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: Rect {
                    row: text_row + 0.08,
                    col: text_left + cursor_x,
                    width: 0.08,
                    height: node.rect.height * 0.36,
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
    float active = step(rel, sweep * in.value_t);

    float knobRadius = 0.58;
    float ring = abs(r - knobRadius) - 0.070;
    float activeRing = abs(r - knobRadius) - 0.082;
    float aa = max(fwidth(r), 0.0015);
    float ringMask = smoothstep(aa, -aa, ring) * inRange;
    float activeMask = smoothstep(aa, -aa, activeRing) * inRange * active;
    float trackMask = ringMask * (1.0 - active);

    float notchAngle = start + sweep * in.value_t;
    float2 n = float2(cos(notchAngle), sin(notchAngle));
    float notch = length(p - n * knobRadius) - 0.070;
    float notchMask = smoothstep(aa, -aa, notch);
    float lineAlong = dot(p, n);
    float lineAcross = abs(p.x * n.y - p.y * n.x);
    float lineSegment = step(0.0, lineAlong) * step(lineAlong, 0.52);
    float line = lineAcross - 0.070;
    float lineMask = smoothstep(aa, -aa, line) * lineSegment;

    float4 col = float4(0.0);
    col = mix(col, in.color_b, trackMask);
    col = mix(col, in.color_a, activeMask);
    col = mix(col, in.color_b, lineMask);
    col = mix(col, in.color_a, notchMask);
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
