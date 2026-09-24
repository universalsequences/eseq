//! A row of small numeric chips, e.g. the transposes a neuron is sounding.
//!
//! `:values` is a list of numbers or float bindings (`bind-nth` handles) and
//! `:count` how many of them are live, so a host can publish a fixed-width
//! numeric list and a write that changes one row's values dirties only the
//! widget bound to those indices: no Lisp re-eval, no relayout. Chips are a
//! fixed `:chip-width` so the widget's geometry never depends on its values;
//! when more are live than fit, the last chip reads `…k`. An optional
//! `:levels` list (0..1, parallel to `:values`) fades each chip, e.g. by the
//! note's velocity; `:min-alpha` keeps a zero level faintly visible.

use std::collections::HashMap;

use super::{
    CellBuffer, GpuPrimitive, GpuProportionalTextPrimitive, WidgetDefinition, WidgetInstance,
    WidgetViewport, get_f32_prop, ndc_bounds, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
};
use crate::theme;
use crate::vm::Value;

pub struct NumberListWidget;

pub static NUMBER_LIST_WIDGET: NumberListWidget = NumberListWidget;

const DEFAULT_CHIP_WIDTH: f32 = 2.4;
const DEFAULT_GAP: f32 = 0.25;

fn number_value(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Number(number) => *number,
        Value::ReactiveRef { slot, .. } => crate::reactive::read_float_slot(slot),
        _ => return None,
    };
    number.is_finite().then_some(number)
}

/// The live `(value, level)` pairs: the first `:count` entries of `:values`
/// (all of them when `:count` is absent), each with its `:levels` entry
/// clamped to 0..1 (1 when absent).
fn live_values(props: &HashMap<String, Value>) -> Vec<(f64, f32)> {
    let Some(Value::List(items)) = props.get("values") else {
        return Vec::new();
    };
    let count = match props.get("count") {
        Some(value) => number_value(value)
            .map(|count| count.round().max(0.0) as usize)
            .unwrap_or(0),
        None => items.len(),
    };
    let levels = match props.get("levels") {
        Some(Value::List(levels)) => Some(levels),
        _ => None,
    };
    items
        .iter()
        .take(count)
        .enumerate()
        .filter_map(|(idx, item)| {
            let value = number_value(&item.borrow())?;
            let level = levels
                .and_then(|levels| levels.get(idx))
                .and_then(|level| number_value(&level.borrow()))
                .map(|level| level.clamp(0.0, 1.0) as f32)
                .unwrap_or(1.0);
            Some((value, level))
        })
        .collect()
}

fn format_value(value: f64, decimals: usize, signed: bool) -> String {
    let text = format!("{value:.decimals$}");
    // `-0` reads as noise on an integer readout.
    let text = if text.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') {
        text.trim_start_matches('-').to_string()
    } else {
        text
    };
    if signed && value > 0.0 && !text.chars().all(|c| c == '0' || c == '.') {
        format!("+{text}")
    } else {
        text
    }
}

/// The chips that fit in `slots`: every value when they fit, otherwise the
/// first `slots - 1` and a `…k` overflow chip at the loudest hidden level.
fn chips(props: &HashMap<String, Value>, slots: usize) -> Vec<(String, f32)> {
    let decimals = get_f32_prop(props, "decimals", 0.0).round().clamp(0.0, 6.0) as usize;
    let signed = super::get_bool_prop(props, "signed", false);
    let values = live_values(props);
    if slots == 0 {
        return Vec::new();
    }
    if values.len() <= slots {
        return values
            .iter()
            .map(|(value, level)| (format_value(*value, decimals, signed), *level))
            .collect();
    }
    let shown = slots - 1;
    let mut chips: Vec<(String, f32)> = values[..shown]
        .iter()
        .map(|(value, level)| (format_value(*value, decimals, signed), *level))
        .collect();
    let hidden_level = values[shown..].iter().map(|(_, level)| *level).fold(0.0, f32::max);
    // Not `+k`: on a signed readout that reads as a value.
    chips.push((format!("\u{2026}{}", values.len() - shown), hidden_level));
    chips
}

fn with_alpha(color: Color, alpha: f32) -> Color {
    Color { a: color.a * alpha, ..color }
}

/// Text fades by mixing toward the chip color: glyph rendering does not
/// honor foreground alpha.
fn fade_toward(fg: Color, under: Color, amount: f32) -> Color {
    let mix = |to: f32, from: f32| from + (to - from) * amount;
    Color { r: mix(fg.r, under.r), g: mix(fg.g, under.g), b: mix(fg.b, under.b), a: fg.a }
}

fn chip_geometry(props: &HashMap<String, Value>, rect: Rect) -> (f32, f32, usize) {
    let chip_width = get_f32_prop(props, "chip-width", DEFAULT_CHIP_WIDTH).max(0.5);
    let gap = get_f32_prop(props, "gap", DEFAULT_GAP).max(0.0);
    let slots = ((rect.width + gap) / (chip_width + gap)).floor().max(0.0) as usize;
    (chip_width, gap, slots)
}

fn normalized_corner_radius(rect: Rect, viewport: WidgetViewport, radius_px: f32) -> f32 {
    if radius_px <= 0.0 {
        return 0.001;
    }
    let radius_px = super::ui_design_px(radius_px);
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((radius_px * 2.0) / px_h).clamp(0.001, 0.5)
}

fn push_rounded_rect(
    prims: &mut Vec<GpuPrimitive>,
    rect: Rect,
    color: Color,
    viewport: WidgetViewport,
    radius_px: f32,
) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    prims.push(GpuPrimitive::WidgetInstance {
        widget_type: "box".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [0.0; 4],
            uniform_b: [0.0; 4],
            uniform_c: [0.0; 4],
            uniform_d: [0.0; 4],
            color_a: color.to_rgba(),
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: normalized_corner_radius(rect, viewport, radius_px),
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
        is_background: false,
    });
}

impl WidgetDefinition for NumberListWidget {
    fn names(&self) -> &'static [&'static str] {
        &["number-list"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["count"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "values", "count", "levels", "min-alpha", "decimals", "signed", "chip-width", "gap", "chip-color",
            "color", "font-size", "corner-radius", "width", "height", "key",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        Some(Size {
            width: get_prop_num(node, "width").map(f64_to_f32).unwrap_or(10.0),
            height: get_prop_num(node, "height").map(f64_to_f32).unwrap_or(1.0),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let (_, _, slots) = chip_geometry(props, rect);
        let text = chips(props, slots)
            .into_iter()
            .map(|(label, _)| label)
            .collect::<Vec<_>>()
            .join(" ");
        let fg = resolve_named_color(props, "color", theme::WIDGET_LABEL_FG());
        let row = rect.row.round() as u16;
        let col = rect.col.round() as u16;
        let width = rect.width.round().max(0.0) as usize;
        for (idx, ch) in text.chars().take(width).enumerate() {
            buf.set(row, col + idx as u16, styled_cell(ch, fg, None));
        }
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &crate::layout::LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let props = &node.props;
        let (chip_width, gap, slots) = chip_geometry(props, node.rect);
        let chips = chips(props, slots);
        let min_alpha = get_f32_prop(props, "min-alpha", 0.2).clamp(0.0, 1.0);
        let fg = resolve_named_color(props, "color", theme::WIDGET_LABEL_FG());
        let chip_color = resolve_named_color(props, "chip-color", theme::WIDGET_SLIDER_TRACK());
        let font_size = get_f32_prop(props, "font-size", DEFAULT_FONT_SIZE);
        let radius_px = get_f32_prop(props, "corner-radius", 4.0);
        let text_row = node.rect.row + (node.rect.height - 1.0).max(0.0) * 0.5;
        let mut prims = Vec::with_capacity(chips.len() * 2);
        for (idx, (text, level)) in chips.into_iter().enumerate() {
            let alpha = min_alpha + (1.0 - min_alpha) * level;
            let text_color = fade_toward(fg, chip_color, alpha);
            let chip_color = with_alpha(chip_color, alpha);
            let chip = Rect {
                row: node.rect.row,
                col: node.rect.col + idx as f32 * (chip_width + gap),
                width: chip_width,
                height: node.rect.height,
            };
            push_rounded_rect(&mut prims, chip, chip_color, viewport, radius_px);
            prims.push(GpuPrimitive::ProportionalText(GpuProportionalTextPrimitive {
                row: text_row,
                col: chip.col,
                align_width: chip.width,
                h_align: 0.5,
                text,
                font_size,
                scale: 1.0,
                fg: text_color,
                bg: chip_color,
                mono: false,
            }));
        }
        prims
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

    fn labels(props: &HashMap<String, Value>, slots: usize) -> Vec<String> {
        chips(props, slots).into_iter().map(|(label, _)| label).collect()
    }

    #[test]
    fn count_limits_the_live_values() {
        let props = HashMap::from([
            ("values".to_string(), list(&[-3.0, 5.0, 12.0, 0.0])),
            ("count".to_string(), Value::Number(2.0)),
            ("signed".to_string(), Value::Bool(true)),
        ]);
        assert_eq!(labels(&props, 8), vec!["-3", "+5"]);
    }

    #[test]
    fn overflow_collapses_into_a_more_chip() {
        let props = HashMap::from([("values".to_string(), list(&[1.0, 2.0, 3.0, 4.0, 5.0]))]);
        assert_eq!(labels(&props, 3), vec!["1", "2", "\u{2026}3"]);
    }

    #[test]
    fn levels_ride_along_and_the_overflow_chip_takes_the_loudest_hidden() {
        let props = HashMap::from([
            ("values".to_string(), list(&[1.0, 2.0, 3.0, 4.0])),
            ("levels".to_string(), list(&[0.5, 2.0, 0.25, 0.75])),
        ]);
        let levels: Vec<f32> = chips(&props, 3).into_iter().map(|(_, level)| level).collect();
        assert_eq!(levels, vec![0.5, 1.0, 0.75]);
    }

    #[test]
    fn zero_never_carries_a_sign() {
        assert_eq!(format_value(0.0, 0, true), "0");
        assert_eq!(format_value(-0.2, 0, true), "0");
    }

    #[test]
    fn count_is_bindable_and_values_never_resize_the_widget() {
        assert_eq!(NUMBER_LIST_WIDGET.bindable_props(), &["count"]);
        assert!(!NUMBER_LIST_WIDGET.size_affecting_props().contains(&"values"));
        assert!(!NUMBER_LIST_WIDGET.size_affecting_props().contains(&"count"));
    }
}
