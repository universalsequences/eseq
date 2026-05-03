use std::collections::HashMap;

use super::{
    CellBuffer, MetalPrimitive, MetalProportionalTextPrimitive, MetalRectPrimitive,
    WidgetDefinition, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num, get_prop_str,
    usize_to_f32,
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
    resolve_named_color(props, "color", theme::WIDGET_LABEL_FG())
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

    // If label has a width wider than text, fill with spaces for background
    for (i, ch) in text.chars().enumerate() {
        let col = col_u16 + i as u16;
        if col >= col_u16 + width_u16 {
            break;
        }
        buf.set(row_u16, col, styled_cell(ch, fg, None));
    }
    // Fill remaining width with spaces (needed for focus highlight to look clean)
    let text_len = text.chars().count() as u16;
    for i in text_len..width_u16 {
        let col = col_u16 + i;
        buf.set(row_u16, col, styled_cell(' ', fg, None));
    }
}

pub fn label_text_row(props: &HashMap<String, Value>, rect: Rect) -> f32 {
    match props.get("v-align") {
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "center" => {
            rect.row + (rect.height - 1.0).max(0.0) * 0.5
        }
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "end" => {
            rect.row + (rect.height - 1.0).max(0.0)
        }
        _ => rect.row,
    }
}

impl WidgetDefinition for LabelWidget {
    fn names(&self) -> &'static [&'static str] {
        &["label"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["text", "width", "height", "font-size"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        // If a TextMeasurer is available (Metal backend), use proportional measurement.
        if let Some(measurer) = ctx.text_measurer {
            let text = get_prop_str(node, "text").unwrap_or_default();
            let font_size = get_prop_num(node, "font-size")
                .map(f64_to_f32)
                .unwrap_or(ctx.inherited_font_size);
            let px_width = if let Some(explicit_w) = get_prop_num(node, "width") {
                // Explicit width is in cell units, convert to pixels.
                f64_to_f32(explicit_w) * ctx.cell_w
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
            width: get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or_else(|| {
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
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: label_text_row(&node.props, node.rect),
                col: node.rect.col,
                align_width: 0.0,
                h_align: 0.0,
                text: text.clone(),
                font_size,
                fg,
                bg,
            },
        ));
        prims
    }
}
