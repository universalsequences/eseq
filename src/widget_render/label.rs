use std::collections::HashMap;

use super::{
    CellBuffer, MetalPrimitive, MetalProportionalTextPrimitive, MetalRectPrimitive,
    WidgetDefinition, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
    get_prop_str, usize_to_f32,
};
use crate::theme;
use crate::vm::Value;

pub struct LabelWidget;

pub static LABEL_WIDGET: LabelWidget = LabelWidget;

fn resolve_color(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "color", theme::WIDGET_LABEL_FG())
}

fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let text = match props.get("text") {
        Some(Value::String(s)) => s.clone(),
        _ => return,
    };

    let fg = resolve_color(props);
    let row_u16 = rect.row.round() as u16;
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

impl WidgetDefinition for LabelWidget {
    fn names(&self) -> &'static [&'static str] {
        &["label"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["text", "width", "font-size"]
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
                height: px_height / ctx.cell_h,
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
            height: 1.0,
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
        let fg = resolve_color(&node.props);
        let bg = if viewport.focused_widget_id == Some(node.widget_id) && node.focusable {
            theme::STATUS_BG()
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
        vec![
            MetalPrimitive::Rect(MetalRectPrimitive {
                rect: node.rect,
                color: bg,
            }),
            MetalPrimitive::ProportionalText(MetalProportionalTextPrimitive {
                row: node.rect.row,
                col: node.rect.col,
                text: text.clone(),
                font_size,
                fg,
                bg,
            }),
        ]
    }
}
