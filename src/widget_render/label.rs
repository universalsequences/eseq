use std::collections::HashMap;

use super::{CellBuffer, MetalGlyphRunPrimitive, MetalPrimitive, MetalRectPrimitive, WidgetDefinition, resolve_named_color, styled_cell};
use crate::backend::Color;
use crate::theme;
use crate::layout::{
    Constraints, Rect, Size, f64_to_u16, get_prop_num, get_prop_str, saturating_usize_to_u16,
};
use crate::vm::Value;

pub struct LabelWidget;

pub static LABEL_WIDGET: LabelWidget = LabelWidget;

fn resolve_color(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "color", theme::WIDGET_LABEL_FG)
}

fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let text = match props.get("text") {
        Some(Value::String(s)) => s.clone(),
        _ => return,
    };

    let fg = resolve_color(props);

    // If label has a width wider than text, fill with spaces for background
    for (i, ch) in text.chars().enumerate() {
        let col = rect.col + i as u16;
        if col >= rect.col + rect.width {
            break;
        }
        buf.set(rect.row, col, styled_cell(ch, fg, None));
    }
    // Fill remaining width with spaces (needed for focus highlight to look clean)
    let text_len = text.chars().count() as u16;
    for i in text_len..rect.width {
        let col = rect.col + i;
        buf.set(rect.row, col, styled_cell(' ', fg, None));
    }
}

impl WidgetDefinition for LabelWidget {
    fn names(&self) -> &'static [&'static str] {
        &["label"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["text", "width"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        Some(Size {
            width: get_prop_num(node, "width")
                .map(f64_to_u16)
                .unwrap_or_else(|| {
                    get_prop_str(node, "text")
                        .map(|text| saturating_usize_to_u16(text.chars().count()))
                        .unwrap_or(0)
                }),
            height: 1,
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
            theme::STATUS_BG
        } else {
            theme::BG
        };
        vec![
            MetalPrimitive::Rect(MetalRectPrimitive {
                rect: node.rect,
                color: bg,
            }),
            MetalPrimitive::GlyphRun(MetalGlyphRunPrimitive {
                row: node.rect.row as i32,
                col: node.rect.col as i32,
                text: text.clone(),
                fg,
                bg,
            }),
        ]
    }
}
