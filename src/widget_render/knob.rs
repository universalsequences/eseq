use std::collections::HashMap;

use crossterm::event::{MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, get_f32_prop,
    ndc_bounds, styled_cell,
};
use crate::theme;
use crate::layout::{Constraints, LayoutNode, Rect, Size, f64_to_u16, get_prop_num};
use crate::vm::Value;

pub struct KnobWidget;

pub static KNOB_WIDGET: KnobWidget = KnobWidget;

#[cfg(target_os = "macos")]
const KNOB_FRAGMENT_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;

    float2 scale = float2(max(aspect, 1.0), max(1.0 / max(aspect, 0.0001), 1.0));
    float2 centerPos = float2((uv.x - 0.5) * 2.0, (uv.y - 0.5) * 2.0) * scale;
    float dist = length(centerPos);
    float distDeriv = max(fwidth(dist), 0.001);

    float outerRadius = 0.90;
    float ringInnerRadius = 0.85;
    float fillRadius = mix(0.12, 0.83, in.value_t);

    float outerMask = smoothstep(outerRadius + distDeriv, outerRadius - distDeriv, dist);
    if (outerMask <= 0.001) { discard_fragment(); }

    float ringMask = smoothstep(ringInnerRadius + distDeriv, ringInnerRadius - distDeriv, dist);
    float fillMask = smoothstep(fillRadius + distDeriv, fillRadius - distDeriv, dist);

    float3 baseRgb = mix(in.color_b.rgb, in.color_a.rgb, fillMask);
    float3 borderRgb = mix(baseRgb, float3(0.55, 0.55, 0.60), 1.0 - ringMask);
    return float4(borderRgb, outerMask);
}
"#;

fn normalized_value(props: &HashMap<String, Value>) -> f32 {
    let value = get_f32_prop(props, "value", 0.0);
    let min = get_f32_prop(props, "min", 0.0);
    let max = get_f32_prop(props, "max", 1.0);
    let range = max - min;
    if range > 0.0 {
        ((value - min) / range).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn knob_t_from_local_row(node: &LayoutNode, local_row: f32) -> f32 {
    let denom = node.rect.height.saturating_sub(1).max(1) as f32;
    let offset = (local_row - node.rect.row as f32) / denom;
    (1.0 - offset).clamp(0.0, 1.0)
}

fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let t = normalized_value(props);
    let size = rect.width.min(rect.height).max(1) as f32;
    let outer_radius = 0.48 * size;
    let fill_radius = (0.10 + 0.38 * t) * size;
    let center_x = rect.col as f32 + rect.width as f32 * 0.5;
    let center_y = rect.row as f32 + rect.height as f32 * 0.5;

    for row_offset in 0..rect.height {
        for col_offset in 0..rect.width {
            let sample_x = rect.col as f32 + col_offset as f32 + 0.5;
            let sample_y = rect.row as f32 + row_offset as f32 + 0.5;
            let dx = sample_x - center_x;
            let dy = sample_y - center_y;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > outer_radius {
                continue;
            }

            let (ch, fg) = if dist <= fill_radius {
                ('●', theme::WIDGET_KNOB_FILLED)
            } else {
                ('○', theme::WIDGET_KNOB_TRACK)
            };
            buf.set(rect.row + row_offset, rect.col + col_offset, styled_cell(ch, fg, None));
        }
    }
}

impl WidgetDefinition for KnobWidget {
    fn names(&self) -> &'static [&'static str] {
        &["knob"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["size"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let size = get_prop_num(node, "size").map(f64_to_u16).unwrap_or(2).max(1);
        Some(Size {
            width: size,
            height: size,
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        tui_render(props, rect, buf);
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        _local_col: f32,
        local_row: f32,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                MouseEventOutcome::Dispatch(WidgetEvent::SetNormalized(knob_t_from_local_row(
                    node, local_row,
                )))
            }
            _ => MouseEventOutcome::Consume,
        }
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::SetNormalized(t) = event else {
            return None;
        };
        let callback = node.props.get("on-change")?.clone();
        let min = get_f32_prop(&node.props, "min", 0.0);
        let max = get_f32_prop(&node.props, "max", 1.0);
        let value = min + (max - min) * t.clamp(0.0, 1.0);
        Some(EventOutput {
            callback,
            value: Value::Number(value as f64),
        })
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(KNOB_FRAGMENT_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_instance(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: super::WidgetViewport,
    ) -> Option<super::WidgetInstance> {
        let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
        let px_w = node.rect.width as f32 * viewport.cell_w;
        let px_h = node.rect.height as f32 * viewport.cell_h;
        Some(super::WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: normalized_value(&node.props),
            orientation: 0.0,
            color_a: theme::WIDGET_KNOB_FILLED.to_rgba(),
            color_b: theme::WIDGET_KNOB_TRACK.to_rgba(),
            corner_radius: 0.0,
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        })
    }
}
