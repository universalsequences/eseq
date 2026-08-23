use std::collections::HashMap;

use super::{
    CellBuffer, GpuPrimitive, GpuProportionalTextPrimitive, GpuRectPrimitive,
    WidgetDefinition, get_f32_prop, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, MeasureCtx, Rect, Size, f64_to_f32, get_map, get_prop_num,
};
use crate::theme;
use crate::vm::Value;

pub struct NumberLabelWidget;

pub static NUMBER_LABEL_WIDGET: NumberLabelWidget = NumberLabelWidget;

fn resolve_h_align(props: &HashMap<String, Value>) -> f32 {
    match props.get("h-align") {
        Some(Value::Number(value)) => (*value as f32).clamp(0.0, 1.0),
        Some(Value::Keyword(value)) | Some(Value::String(value))
            if value == "center" || value == "middle" =>
        {
            0.5
        }
        Some(Value::Keyword(value)) | Some(Value::String(value))
            if value == "right" || value == "end" =>
        {
            1.0
        }
        _ => 0.0,
    }
}

fn resolve_v_align(props: &HashMap<String, Value>) -> f32 {
    match props.get("v-align") {
        Some(Value::Number(value)) => (*value as f32).clamp(0.0, 1.0),
        Some(Value::Keyword(value)) | Some(Value::String(value))
            if value == "center" || value == "middle" =>
        {
            0.5
        }
        Some(Value::Keyword(value)) | Some(Value::String(value))
            if value == "bottom" || value == "end" =>
        {
            1.0
        }
        _ => 0.0,
    }
}

fn resolve_color(props: &HashMap<String, Value>) -> Color {
    if get_f32_prop(props, "active", 0.0) != 0.0
        && let Some(value) = props.get("active-color")
    {
        return crate::theme::parse_color_value(value).unwrap_or(theme::WIDGET_LABEL_FG());
    }
    resolve_named_color(props, "color", theme::WIDGET_LABEL_FG())
}

fn bg_is_transparent(props: &HashMap<String, Value>) -> bool {
    matches!(
        props.get("bg"),
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "transparent"
    )
}

fn string_prop<'a>(props: &'a HashMap<String, Value>, key: &str) -> &'a str {
    match props.get(key) {
        Some(Value::String(value)) | Some(Value::Keyword(value)) => value,
        _ => "",
    }
}

fn decimals(props: &HashMap<String, Value>) -> usize {
    get_f32_prop(props, "decimals", 0.0).round().clamp(0.0, 6.0) as usize
}

fn min_integer_digits(props: &HashMap<String, Value>) -> usize {
    get_f32_prop(props, "min-integer-digits", 0.0)
        .round()
        .clamp(0.0, 12.0) as usize
}

fn pad_integer_part(mut text: String, min_digits: usize) -> String {
    if min_digits == 0 {
        return text;
    }
    let sign_len = usize::from(text.starts_with('-') || text.starts_with('+'));
    let number_start = sign_len;
    let number_end = text[number_start..]
        .find('.')
        .map(|idx| number_start + idx)
        .unwrap_or(text.len());
    let digit_count = text[number_start..number_end].chars().count();
    if digit_count >= min_digits {
        return text;
    }
    let padding = " ".repeat(min_digits - digit_count);
    text.insert_str(number_start, &padding);
    text
}

fn formatted_value(props: &HashMap<String, Value>) -> String {
    let value = get_f32_prop(props, "value", 0.0) as f64;
    let decimals = decimals(props);
    let number = pad_integer_part(format!("{value:.decimals$}"), min_integer_digits(props));
    let prefix = string_prop(props, "prefix");
    let suffix = string_prop(props, "suffix");
    format!("{prefix}{number}{suffix}")
}

fn text_row(props: &HashMap<String, Value>, rect: Rect) -> f32 {
    let line_height = 1.0;
    rect.row + (rect.height - line_height).max(0.0) * resolve_v_align(props)
}

fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let text = formatted_value(props);
    let fg = resolve_color(props);
    let row = text_row(props, rect).round() as u16;
    let col = rect.col.round() as u16;
    let width = rect.width.round().max(0.0) as u16;
    if width == 0 {
        return;
    }

    let text_len = text.chars().count() as u16;
    let available_pad = width.saturating_sub(text_len);
    let start_offset = (available_pad as f32 * resolve_h_align(props)).round() as u16;
    for (idx, ch) in text.chars().take(width as usize).enumerate() {
        let target_col = col + start_offset + idx as u16;
        if target_col < col + width {
            buf.set(row, target_col, styled_cell(ch, fg, None));
        }
    }
}

impl WidgetDefinition for NumberLabelWidget {
    fn names(&self) -> &'static [&'static str] {
        &["number-label"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["value", "active"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "font-size"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let props = get_map(node).unwrap_or_default();
        let font_size = get_prop_num(node, "font-size")
            .map(f64_to_f32)
            .unwrap_or(ctx.inherited_font_size);
        let height = if let Some(explicit) = get_prop_num(node, "height").map(f64_to_f32) {
            explicit
        } else if let Some(measurer) = ctx.text_measurer {
            measurer.line_height_px(font_size) / ctx.cell_h
        } else {
            1.0
        };
        Some(Size {
            width: get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or_else(|| get_f32_prop(&props, "default-width", 4.0)),
            height,
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        tui_render(props, rect, buf);
    }

    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &crate::layout::LayoutNode,
        viewport: super::WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let fg = resolve_color(&node.props);
        let bg = if viewport.focused_branch {
            theme::WIDGET_FOCUS_BG()
        } else {
            theme::BG()
        };
        let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);
        let mut prims = Vec::new();
        if !bg_is_transparent(&node.props) {
            prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: node.rect,
                color: bg,
            }));
        }
        prims.push(GpuPrimitive::ProportionalText(
            GpuProportionalTextPrimitive {
                row: text_row(&node.props, node.rect),
                col: node.rect.col,
                align_width: node.rect.width,
                h_align: resolve_h_align(&node.props),
                text: formatted_value(&node.props),
                font_size,
                scale: 1.0,
                fg,
                bg,
            },
        ));
        prims
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_value_with_fixed_decimals_and_suffix() {
        let mut props = HashMap::new();
        props.insert("value".to_string(), Value::Number(12.345));
        props.insert("decimals".to_string(), Value::Number(1.0));
        props.insert("suffix".to_string(), Value::String("%".to_string()));

        assert_eq!(formatted_value(&props), "12.3%");
    }

    #[test]
    fn pads_integer_part_before_suffix() {
        let mut props = HashMap::new();
        props.insert("value".to_string(), Value::Number(3.0));
        props.insert("decimals".to_string(), Value::Number(0.0));
        props.insert("min-integer-digits".to_string(), Value::Number(2.0));
        props.insert("suffix".to_string(), Value::String("%".to_string()));

        assert_eq!(formatted_value(&props), " 3%");
    }

    #[test]
    fn value_is_bindable_but_not_size_affecting() {
        assert_eq!(NUMBER_LABEL_WIDGET.bindable_props(), &["value", "active"]);
        assert!(
            !NUMBER_LABEL_WIDGET
                .size_affecting_props()
                .contains(&"value")
        );
        assert!(
            !NUMBER_LABEL_WIDGET
                .size_affecting_props()
                .contains(&"active")
        );
    }

    #[test]
    fn active_color_overrides_base_color_when_active() {
        let mut props = HashMap::from([
            ("color".to_string(), Value::Keyword("dim".to_string())),
            (
                "active-color".to_string(),
                Value::Keyword("yellow".to_string()),
            ),
        ]);

        assert_eq!(resolve_color(&props), theme::DIM());

        props.insert("active".to_string(), Value::Number(1.0));

        assert_eq!(resolve_color(&props), theme::YELLOW());
    }
}
