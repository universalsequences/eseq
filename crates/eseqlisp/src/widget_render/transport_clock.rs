use std::collections::HashMap;

use super::{CellBuffer, WidgetDefinition, get_f32_prop, resolve_named_color, styled_cell};
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

#[cfg(target_os = "macos")]
use super::{MetalPrimitive, MetalProportionalTextPrimitive, MetalRectPrimitive, WidgetViewport};

pub struct TransportClockWidget;
pub static TRANSPORT_CLOCK_WIDGET: TransportClockWidget = TransportClockWidget;

fn clock_parts(playhead: f32) -> (String, String, String) {
    let step = playhead.max(0.0).floor() as u64;
    let bar = step / 16 + 1;
    let beat = (step % 16) / 4 + 1;
    let sixteenth = step % 4 + 1;
    (
        format!("{bar:>3}"),
        format!("{beat:>3}"),
        format!("{sixteenth:>3}"),
    )
}

impl WidgetDefinition for TransportClockWidget {
    fn names(&self) -> &'static [&'static str] {
        &["transport-clock"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "font-size"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["playhead"]
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
            width: get_prop_num(node, "width").map(f64_to_f32).unwrap_or(11.0),
            height: get_prop_num(node, "height").map(f64_to_f32).unwrap_or(1.2),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let (bar, beat, sixteenth) = clock_parts(get_f32_prop(props, "playhead", 0.0));
        let text = format!("{bar} {beat} {sixteenth}");
        let fg = resolve_named_color(props, "color", theme::WIDGET_LABEL_FG());
        let row = rect.row.round() as u16;
        let col = rect.col.round() as u16;
        for (idx, ch) in text.chars().enumerate() {
            if idx as f32 >= rect.width {
                break;
            }
            buf.set(row, col + idx as u16, styled_cell(ch, fg, None));
        }
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        _viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let (bar, beat, sixteenth) = clock_parts(get_f32_prop(&node.props, "playhead", 0.0));
        let font_size = get_f32_prop(&node.props, "font-size", 15.0);
        let fg = resolve_named_color(&node.props, "color", theme::WIDGET_LABEL_FG());
        let bg = theme::BG();
        let row = node.rect.row;
        let columns = [
            (bar, node.rect.col, 4.0),
            (beat, node.rect.col + 4.0, 3.0),
            (sixteenth, node.rect.col + 7.0, 3.0),
        ];
        let mut prims = Vec::with_capacity(4);
        if !matches!(node.props.get("bg"), Some(Value::Keyword(k)) if k == "transparent") {
            prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: node.rect,
                color: bg,
            }));
        }
        for (text, col, align_width) in columns {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row,
                    col,
                    align_width,
                    h_align: 0.0,
                    text,
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
