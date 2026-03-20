use std::collections::HashMap;

use crossterm::event::{MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, get_f32_prop,
    ndc_bounds, styled_cell,
};
use crate::theme;
use crate::layout::{
    Constraints, LayoutNode, Rect, Size, f64_to_u16, get_prop_num,
};
use crate::vm::Value;

pub struct VerticalSliderWidget;

pub static VSLIDER_WIDGET: VerticalSliderWidget = VerticalSliderWidget;

/// TUI render for vertical slider: fill from bottom up.
fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let value = get_f32_prop(props, "value", 0.0);
    let min = get_f32_prop(props, "min", 0.0);
    let max = get_f32_prop(props, "max", 1.0);
    let range = max - min;
    let t = if range > 0.0 {
        (value - min) / range
    } else {
        0.0
    };
    let t = t.clamp(0.0, 1.0);

    let height = rect.height;
    let filled = (t * height as f32).round() as u16;
    let threshold = height.saturating_sub(filled);

    for row_offset in 0..height {
        let row = rect.row + row_offset;
        for col_offset in 0..rect.width.min(2) {
            let col = rect.col + col_offset;
            if row_offset >= threshold {
                buf.set(row, col, styled_cell('\u{2588}', theme::WIDGET_SLIDER_FILLED, None));
            } else {
                buf.set(row, col, styled_cell(' ', theme::WIDGET_SLIDER_TRACK, None));
            }
        }
    }
}

#[cfg(target_os = "macos")]
const VSLIDER_FRAGMENT_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;

    float2 localPos = float2((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);
    float2 sdfSize = float2(aspect, 1.0);
    float cornerRadius = min(aspect, 1.0);

    float3 borderColor = float3(0.45, 0.45, 0.5);
    float outerMask;
    float borderMask = compute_border_mask(localPos, sdfSize, cornerRadius, 1.5, outerMask);
    if (outerMask <= 0.001) { discard_fragment(); }

    float threshold = 1.0 - in.value_t;
    float fillDist = uv.y - threshold;
    float fillDeriv = max(fwidth(fillDist), 0.001);
    float edge = smoothstep(-fillDeriv, fillDeriv, fillDist);
    float4 interior = mix(in.color_b, in.color_a, edge);

    float3 final_rgb = mix(interior.rgb, borderColor, borderMask);
    return float4(final_rgb, outerMask);
}
"#;

impl WidgetDefinition for VerticalSliderWidget {
    fn names(&self) -> &'static [&'static str] {
        &["vslider"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["height"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        Some(Size {
            width: 2,
            height: get_prop_num(node, "height").map(f64_to_u16).unwrap_or(8),
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
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => MouseEventOutcome::Consume,
            MouseEventKind::Drag(MouseButton::Left) => {
                let denom = node.rect.height.saturating_sub(1).max(1) as f32;
                let offset = (local_row - node.rect.row as f32) / denom;
                let t = (1.0 - offset).clamp(0.0, 1.0);
                MouseEventOutcome::Dispatch(WidgetEvent::SetNormalized(t))
            }
            _ => MouseEventOutcome::Consume,
        }
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
            args: vec![Value::Number(value as f64)],
        })
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(VSLIDER_FRAGMENT_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_instance(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: super::WidgetViewport,
    ) -> Option<super::WidgetInstance> {
        let value = get_f32_prop(&node.props, "value", 0.0);
        let min = get_f32_prop(&node.props, "min", 0.0);
        let max = get_f32_prop(&node.props, "max", 1.0);
        let range = max - min;
        let t = if range > 0.0 {
            ((value - min) / range).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
        let px_w = node.rect.width as f32 * viewport.cell_w;
        let px_h = node.rect.height as f32 * viewport.cell_h;
        Some(super::WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: t,
            orientation: 0.0,
            color_a: theme::WIDGET_SLIDER_FILLED.to_rgba(),
            color_b: theme::WIDGET_SLIDER_TRACK.to_rgba(),
            corner_radius: 0.0,
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        })
    }
}
