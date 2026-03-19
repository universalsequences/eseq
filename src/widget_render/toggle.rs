use std::collections::HashMap;

use crossterm::event::{MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, get_bool_prop,
    ndc_bounds, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, Rect, Size};
use crate::vm::Value;

pub struct ToggleWidget;

pub static TOGGLE_WIDGET: ToggleWidget = ToggleWidget;

/// TUI render for toggle: "[×]" or "[ ]"
fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let on = get_bool_prop(props, "value", false);

    let (text, fg) = if on {
        ("[×]", Color::GREEN)
    } else {
        ("[ ]", Color::DARK_GRAY)
    };

    for (i, ch) in text.chars().enumerate() {
        let col = rect.col + i as u16;
        if col >= rect.col + rect.width {
            break;
        }
        buf.set(rect.row, col, styled_cell(ch, fg, None));
    }
}

#[cfg(target_os = "macos")]
const TOGGLE_FRAGMENT_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;

    float2 localPos = float2((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);
    float2 sdfSize = float2(aspect, 1.0);
    float cornerRadius = 1.0;

    float3 borderColor = float3(0.5, 0.5, 0.55);
    float outerMask;
    float borderMask = compute_border_mask(localPos, sdfSize, cornerRadius, 1.5, outerMask);
    if (outerMask <= 0.001) { discard_fragment(); }

    float on = in.value_t;
    float4 bg = mix(in.color_b, in.color_a, on);

    float knob_x = mix(0.3, 0.7, on);
    float2 knob_pos = float2((uv.x - knob_x) * aspect, uv.y - 0.5);
    float knob_radius = 0.28;
    float knobDist = length(knob_pos) - knob_radius;
    float knobDeriv = max(fwidth(knobDist), 0.001);
    float knobMask = smoothstep(knobDeriv, -knobDeriv, knobDist);

    float3 knobColor = float3(0.96, 0.96, 0.98);
    float3 rgb = mix(bg.rgb, borderColor, borderMask);
    rgb = mix(rgb, knobColor, knobMask);

    return float4(rgb, outerMask);
}
"#;

impl WidgetDefinition for ToggleWidget {
    fn names(&self) -> &'static [&'static str] {
        &["toggle"]
    }

    fn measure(
        &self,
        _node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        Some(Size {
            width: 4,
            height: 1,
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        tui_render(props, rect, buf);
    }

    fn mouse_event(
        &self,
        _node: &LayoutNode,
        mouse_kind: MouseEventKind,
        _local_col: f32,
        _local_row: f32,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                MouseEventOutcome::Dispatch(WidgetEvent::Activate)
            }
            _ => MouseEventOutcome::Consume,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Activate = event else {
            return None;
        };
        let callback = node.props.get("on-change")?.clone();
        let current = get_bool_prop(&node.props, "value", false);
        Some(EventOutput {
            callback,
            value: Value::Bool(!current),
        })
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(TOGGLE_FRAGMENT_SHADER)
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
            value_t: if get_bool_prop(&node.props, "value", false) {
                1.0
            } else {
                0.0
            },
            orientation: 0.0,
            color_a: [0.2, 0.78, 0.35, 1.0],
            color_b: [0.3, 0.3, 0.35, 1.0],
            corner_radius: 0.0,
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        })
    }
}
