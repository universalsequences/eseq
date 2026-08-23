use std::collections::HashMap;

use super::{
    CellBuffer, WidgetDefinition, get_bool_prop, get_f32_prop, resolve_named_color, styled_cell,
};
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

use super::{GpuPrimitive, GpuProportionalTextPrimitive, GpuRectPrimitive, WidgetViewport};

pub struct TransportClockWidget;
pub static TRANSPORT_CLOCK_WIDGET: TransportClockWidget = TransportClockWidget;

fn clock_sixteenth_position(props: &HashMap<String, Value>) -> f32 {
    if get_bool_prop(props, "use-song-position", false) {
        // Song position is expressed in quarter-note beats; the legacy
        // session transport playhead is already an absolute sixteenth count.
        get_f32_prop(props, "song-position-beats", 0.0) * 4.0
    } else {
        get_f32_prop(props, "playhead", 0.0)
    }
}

fn clock_parts(sixteenth_position: f32) -> (String, String, String) {
    let step = sixteenth_position.max(0.0).floor() as u64;
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
        &["playhead", "song-position-beats"]
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
        let (bar, beat, sixteenth) = clock_parts(clock_sixteenth_position(props));
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

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        _viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let (bar, beat, sixteenth) = clock_parts(clock_sixteenth_position(&node.props));
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
            prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: node.rect,
                color: bg,
            }));
        }
        for (text, col, align_width) in columns {
            prims.push(GpuPrimitive::ProportionalText(
                GpuProportionalTextPrimitive {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_clock_uses_elapsed_transport_sixteenths() {
        let props = HashMap::from([
            ("playhead".to_string(), Value::Number(222.0)),
            ("song-position-beats".to_string(), Value::Number(512.0)),
            ("use-song-position".to_string(), Value::Bool(false)),
        ]);

        assert_eq!(
            clock_parts(clock_sixteenth_position(&props)),
            (" 14".to_string(), "  4".to_string(), "  3".to_string())
        );
    }

    #[test]
    fn song_clock_uses_absolute_arrangement_beats() {
        let props = HashMap::from([
            ("playhead".to_string(), Value::Number(222.0)),
            ("song-position-beats".to_string(), Value::Number(513.5)),
            ("use-song-position".to_string(), Value::Bool(true)),
        ]);

        assert_eq!(
            clock_parts(clock_sixteenth_position(&props)),
            ("129".to_string(), "  2".to_string(), "  3".to_string())
        );
    }

    #[test]
    fn song_position_is_a_supported_reactive_binding() {
        assert_eq!(
            TRANSPORT_CLOCK_WIDGET.bindable_props(),
            &["playhead", "song-position-beats"]
        );
    }
}
