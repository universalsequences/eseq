use std::collections::HashMap;

use super::{
    CellBuffer, MetalPrimitive, MetalProportionalTextPrimitive, MetalRectPrimitive,
    WidgetDefinition, get_f32_prop, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, MeasureCtx, Rect, Size, f64_to_f32, get_map, get_prop_num,
    get_prop_str, usize_to_f32,
};
use crate::theme;
use crate::vm::Value;

pub struct LabelWidget;

pub static LABEL_WIDGET: LabelWidget = LabelWidget;

fn resolve_color(props: &HashMap<String, Value>, hovered: bool) -> Color {
    if hovered {
        if let Some(value) = props.get("hover-color") {
            return crate::theme::parse_color_value(value).unwrap_or(theme::WIDGET_LABEL_FG());
        }
    }
    if get_f32_prop(props, "active", 0.0) != 0.0
        && let Some(value) = props.get("active-color")
    {
        return crate::theme::parse_color_value(value).unwrap_or(theme::WIDGET_LABEL_FG());
    }
    resolve_named_color(props, "color", theme::WIDGET_LABEL_FG())
}

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

fn wrap_enabled(props: &HashMap<String, Value>) -> bool {
    matches!(props.get("wrap"), Some(Value::Bool(true)))
}

fn node_wrap_enabled(node: &Value) -> bool {
    get_map(node).is_some_and(|props| wrap_enabled(&props))
}

fn wrap_text_by_columns(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();

    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            let word_len = word.chars().count();
            if current.is_empty() {
                if word_len <= width {
                    current.push_str(word);
                } else {
                    push_hard_wrapped_word(&mut lines, word, width, &mut current);
                }
            } else {
                let current_len = current.chars().count();
                if current_len + 1 + word_len <= width {
                    current.push(' ');
                    current.push_str(word);
                } else {
                    lines.push(std::mem::take(&mut current));
                    if word_len <= width {
                        current.push_str(word);
                    } else {
                        push_hard_wrapped_word(&mut lines, word, width, &mut current);
                    }
                }
            }
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        } else if paragraph.is_empty() {
            lines.push(String::new());
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn push_hard_wrapped_word(lines: &mut Vec<String>, word: &str, width: usize, current: &mut String) {
    let mut chunk = String::new();
    for ch in word.chars() {
        chunk.push(ch);
        if chunk.chars().count() == width {
            lines.push(std::mem::take(&mut chunk));
        }
    }
    *current = chunk;
}

fn measure_wrapped_text_lines(
    text: &str,
    max_width_cells: f32,
    font_size: f32,
    ctx: &MeasureCtx<'_>,
) -> Vec<String> {
    let width_cells = max_width_cells.max(1.0);
    let Some(measurer) = ctx.text_measurer else {
        return wrap_text_by_columns(text, width_cells.floor() as usize);
    };
    let max_px = width_cells * ctx.cell_w;
    let mut lines = Vec::new();

    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if measurer.measure_text_px(&candidate, font_size) <= max_px || current.is_empty() {
                current = candidate;
            } else {
                lines.push(std::mem::take(&mut current));
                current.push_str(word);
            }

            while measurer.measure_text_px(&current, font_size) > max_px
                && current.chars().count() > 1
            {
                let mut fit = String::new();
                let mut rest = String::new();
                for ch in current.chars() {
                    let next = format!("{fit}{ch}");
                    if !fit.is_empty() && measurer.measure_text_px(&next, font_size) > max_px {
                        rest.push(ch);
                    } else {
                        fit.push(ch);
                    }
                }
                if fit.is_empty() || rest.is_empty() {
                    break;
                }
                lines.push(fit);
                current = rest;
            }
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        } else if paragraph.is_empty() {
            lines.push(String::new());
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_text_row_respects_vertical_alignment() {
        let rect = Rect {
            row: 2.0,
            col: 0.0,
            width: 8.0,
            height: 3.0,
        };
        let mut props = HashMap::new();

        assert_eq!(label_text_row(&props, rect), 2.0);

        props.insert("v-align".to_string(), Value::Keyword("center".to_string()));
        assert_eq!(label_text_row(&props, rect), 3.0);

        props.insert("v-align".to_string(), Value::Keyword("end".to_string()));
        assert_eq!(label_text_row(&props, rect), 4.0);
    }

    #[test]
    fn label_text_row_centers_inside_sub_cell_height_rects() {
        let rect = Rect {
            row: 2.0,
            col: 0.0,
            width: 8.0,
            height: 0.82,
        };
        let props = HashMap::from([("v-align".to_string(), Value::Keyword("center".to_string()))]);

        assert!((label_text_row(&props, rect) - 1.91).abs() < 0.0001);
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

        assert_eq!(resolve_color(&props, false), theme::DIM());

        props.insert("active".to_string(), Value::Number(1.0));

        assert_eq!(resolve_color(&props, false), theme::YELLOW());
    }
}

fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let text = match props.get("text") {
        Some(Value::String(s)) => s.clone(),
        _ => return,
    };

    let fg = resolve_color(props, false);
    let row_u16 = label_text_row(props, rect).round() as u16;
    let col_u16 = rect.col.round() as u16;
    let width_u16 = rect.width.round() as u16;

    let lines = if wrap_enabled(props) {
        wrap_text_by_columns(&text, width_u16 as usize)
    } else {
        vec![text]
    };

    for (line_idx, line) in lines.iter().enumerate() {
        let row = row_u16 + line_idx as u16;
        if row >= row_u16 + rect.height.round() as u16 {
            break;
        }
        for (i, ch) in line.chars().enumerate() {
            let col = col_u16 + i as u16;
            if col >= col_u16 + width_u16 {
                break;
            }
            buf.set(row, col, styled_cell(ch, fg, None));
        }
        let text_len = line.chars().count() as u16;
        for i in text_len..width_u16 {
            let col = col_u16 + i;
            buf.set(row, col, styled_cell(' ', fg, None));
        }
    }
}

pub fn label_text_row(props: &HashMap<String, Value>, rect: Rect) -> f32 {
    match props.get("v-align") {
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "center" => {
            rect.row + (rect.height - 1.0) * 0.5
        }
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "end" => {
            rect.row + rect.height - 1.0
        }
        _ => rect.row,
    }
}

impl WidgetDefinition for LabelWidget {
    fn names(&self) -> &'static [&'static str] {
        &["label"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["text", "width", "height", "font-size", "wrap"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["active"]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "text", "color", "active", "active-color", "hover-color", "bg", "font-size",
            "width", "height", "wrap", "h-align", "v-align",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let text = get_prop_str(node, "text").unwrap_or_default();
        let font_size = get_prop_num(node, "font-size")
            .map(f64_to_f32)
            .unwrap_or(ctx.inherited_font_size);
        let explicit_width = get_prop_num(node, "width").map(f64_to_f32);
        if node_wrap_enabled(node)
            && (explicit_width.is_some() || constraints.max_width.is_finite())
        {
            let width = explicit_width.unwrap_or(constraints.max_width).max(1.0);
            let line_count = measure_wrapped_text_lines(&text, width, font_size, ctx).len();
            let line_height = ctx
                .text_measurer
                .map(|measurer| measurer.line_height_px(font_size) / ctx.cell_h)
                .unwrap_or(1.0);
            return Some(Size {
                width,
                height: get_prop_num(node, "height")
                    .map(f64_to_f32)
                    .unwrap_or(line_height * line_count as f32),
            });
        }

        // If a TextMeasurer is available (Metal backend), use proportional measurement.
        if let Some(measurer) = ctx.text_measurer {
            let px_width = if let Some(explicit_w) = explicit_width {
                // Explicit width is in cell units, convert to pixels.
                explicit_w * ctx.cell_w
            } else {
                measurer.measure_text_px(&text, font_size)
            };
            let px_height = measurer.line_height_px(font_size);
            return Some(Size {
                width: px_width / ctx.cell_w,
                height: get_prop_num(node, "height")
                    .map(f64_to_f32)
                    .unwrap_or(px_height / ctx.cell_h),
            });
        }

        // TUI fallback: monospace char-count measurement.
        Some(Size {
            width: explicit_width.unwrap_or_else(|| {
                get_prop_str(node, "text")
                    .map(|text| usize_to_f32(text.chars().count()))
                    .unwrap_or(0.0)
            }),
            height: get_prop_num(node, "height").map(f64_to_f32).unwrap_or(1.0),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        tui_render(props, rect, buf);
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &crate::layout::LayoutNode,
        viewport: super::WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let Some(Value::String(text)) = node.props.get("text") else {
            return Vec::new();
        };
        let fg = resolve_color(&node.props, viewport.inherited_hover);
        let bg_transparent = matches!(
            node.props.get("bg"),
            Some(Value::Keyword(k)) if k == "transparent"
        );
        let bg = if viewport.focused_branch {
            theme::WIDGET_FOCUS_BG()
        } else {
            theme::BG()
        };
        let font_size = node
            .props
            .get("font-size")
            .and_then(|v| match v {
                Value::Number(n) => Some(*n as f32),
                _ => None,
            })
            .unwrap_or(DEFAULT_FONT_SIZE);
        let mut prims = Vec::new();
        if !bg_transparent {
            prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: node.rect,
                color: bg,
            }));
        }
        let lines = if wrap_enabled(&node.props) {
            wrap_text_by_columns(text, node.rect.width.floor().max(1.0) as usize)
        } else {
            vec![text.clone()]
        };
        let start_row = label_text_row(&node.props, node.rect);
        for (line_idx, line) in lines.into_iter().enumerate() {
            let row = start_row + line_idx as f32;
            if row >= node.rect.row + node.rect.height {
                break;
            }
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row,
                    col: node.rect.col,
                    align_width: node.rect.width,
                    h_align: resolve_h_align(&node.props),
                    text: line,
                    font_size,
                    scale: 1.0,
                    fg,
                    bg,
                },
            ));
        }
        prims
    }
}
