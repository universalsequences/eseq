use std::collections::HashMap;

use crossterm::event::{MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MetalPrimitive, MouseEventOutcome, WidgetDefinition, WidgetEvent,
    get_f32_prop, metal_widget_instance, ndc_bounds, styled_cell,
};
use crate::theme;
use crate::layout::{
    Constraints, LayoutNode, Rect, Size, f64_to_u16, get_prop_num,
};
use crate::vm::Value;

pub struct HorizontalSliderWidget;

pub static HSLIDER_WIDGET: HorizontalSliderWidget = HorizontalSliderWidget;

/// TUI render for horizontal slider: filled bar from left to right.
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

    let width = rect.width;
    let filled = (t * width as f32).round() as u16;

    for col_offset in 0..width {
        let col = rect.col + col_offset;
        if col_offset < filled {
            buf.set(rect.row, col, styled_cell('\u{2588}', theme::WIDGET_SLIDER_FILLED, None));
        } else {
            buf.set(
                rect.row,
                col,
                styled_cell('\u{2591}', theme::WIDGET_SLIDER_TRACK, None),
            );
        }
    }
}

#[cfg(target_os = "macos")]
const HSLIDER_FRAGMENT_SHADER: &str = r#"
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

    float fillDist = uv.x - in.value_t;
    float fillDeriv = max(fwidth(fillDist), 0.001);
    float edge = smoothstep(-fillDeriv, fillDeriv, fillDist);
    float4 interior = mix(in.color_a, in.color_b, edge);

    float3 final_rgb = mix(interior.rgb, borderColor, borderMask);
    return float4(final_rgb, outerMask);
}
"#;

impl WidgetDefinition for HorizontalSliderWidget {
    fn names(&self) -> &'static [&'static str] {
        &["slider", "hslider"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        Some(Size {
            width: get_prop_num(node, "width").map(f64_to_u16).unwrap_or(16),
            height: 1,
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        tui_render(props, rect, buf);
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        _local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => MouseEventOutcome::Consume,
            MouseEventKind::Drag(MouseButton::Left) => {
                let denom = node.rect.width.saturating_sub(1).max(1) as f32;
                let t = ((local_col - node.rect.col as f32) / denom).clamp(0.0, 1.0);
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
        Some(HSLIDER_FRAGMENT_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: super::WidgetViewport,
    ) -> Vec<MetalPrimitive> {
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
        metal_widget_instance(widget_type, super::WidgetInstance {
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
