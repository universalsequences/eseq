use std::collections::HashMap;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

#[cfg(target_os = "macos")]
use super::sdf_widget;
use super::{
    CellBuffer, EventOutput, MetalPrimitive, MouseEventOutcome, WidgetDefinition, WidgetEvent,
    get_f32_prop, metal_widget_instance, ndc_bounds, resolve_named_color,
    should_trigger_integer_haptic, styled_cell, trigger_level_change_haptic,
};
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct HorizontalSliderWidget;

pub static HSLIDER_WIDGET: HorizontalSliderWidget = HorizontalSliderWidget;

fn fill_color(props: &HashMap<String, Value>) -> crate::backend::Color {
    resolve_named_color(props, "fill", theme::WIDGET_SLIDER_FILLED())
}

/// TUI render for horizontal slider: filled bar + dot track.
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

    let row_u16 = rect.row.round() as u16;
    let col_u16 = rect.col.round() as u16;
    let width_u16 = rect.width.round() as u16;
    let fg = fill_color(props);

    let filled = (t * rect.width).round() as u16;

    for col_offset in 0..width_u16 {
        let col = col_u16 + col_offset;
        if col_offset < filled {
            buf.set(row_u16, col, styled_cell('\u{2588}', fg, None));
        } else {
            let ch = if col_offset % 2 == 0 { '\u{2022}' } else { ' ' };
            buf.set(
                row_u16,
                col,
                styled_cell(ch, theme::WIDGET_SLIDER_TRACK(), None),
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
    float t = in.value_t;

    // ── Fill bar: rounded rect from x=0..t, vertically inset ──
    float yPad = 0.18;
    float halfH = 0.5 - yPad;
    float halfW = max(t * 0.5, 0.0);
    float cr = 0.18;
    cr = min(cr, min(halfH, max(halfW * aspect, 0.001)));

    float2 p = float2((uv.x - halfW) * aspect, uv.y - 0.5);
    float2 b = float2(halfW * aspect, halfH);
    float2 q = abs(p) - b + cr;
    float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - cr;
    float aa = max(fwidth(d), 0.001);
    float fillMask = smoothstep(aa, -aa, d) * step(0.005, t);

    // ── Track dots: fixed grid, only visible past fill ──
    float dotSpacing = 0.6 / aspect;
    float dotR = 0.08;
    float dotMask = 0.0;

    float snapX = round(uv.x / dotSpacing) * dotSpacing;
    float margin = dotSpacing * 0.4;
    if (snapX > t + margin && snapX > margin && snapX < 1.0 - margin * 0.5) {
        float2 dp = float2((uv.x - snapX) * aspect, uv.y - 0.5);
        float dd = length(dp) - dotR;
        float da = max(fwidth(dd), 0.001);
        dotMask = smoothstep(da, -da, dd);
    }

    // Composite
    float3 rgb = in.color_a.rgb * fillMask + in.color_b.rgb * dotMask * (1.0 - fillMask);
    float alpha = max(fillMask, dotMask);
    if (alpha < 0.001) discard_fragment();
    return float4(rgb, alpha);
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
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        Some(Size {
            width: get_prop_num(node, "width").map(f64_to_f32).unwrap_or(16.0),
            height: 1.0,
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
        _modifiers: KeyModifiers,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                let denom = node.rect.width.max(0.001);
                let t = ((local_col - node.rect.col) / denom).clamp(0.0, 1.0);
                MouseEventOutcome::Dispatch(WidgetEvent::SetNormalized(t))
            }
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::SetNormalized(t) = event else {
            return None;
        };
        let callback = node.props.get("on-change")?.clone();
        let previous = get_f32_prop(&node.props, "value", 0.0);
        let min = get_f32_prop(&node.props, "min", 0.0);
        let max = get_f32_prop(&node.props, "max", 1.0);
        let value = min + (max - min) * t.clamp(0.0, 1.0);
        if should_trigger_integer_haptic(previous, value, min, max) {
            trigger_level_change_haptic();
        }
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
        let px_w = node.rect.width * viewport.cell_w;
        let px_h = node.rect.height * viewport.cell_h;
        let pixel_aspect = if px_h > 0.0 { px_w / px_h } else { 1.0 };

        // Check for :material shader override.
        // Emit the default hardcoded shader first (draws track dots + flat fill),
        // then the material shader on top (overdraws just the fill region).
        // The dots survive because they're outside the material's fill area.
        if let Some(Value::String(shader_type)) = node.props.get(sdf_widget::SHADER_TYPE_PROP) {
            if let Some(def) = sdf_widget::sdf_widget_def(shader_type) {
                // Layer 1: dots only (value_t=0 hides flat fill)
                let mut prims = metal_widget_instance(
                    widget_type,
                    super::WidgetInstance {
                        ndc_min,
                        ndc_max,
                        value_t: 0.0,
                        orientation: 0.0,
                        itime: 0.0,
                        uniform_a: [0.0; 4],
                        uniform_b: [0.0; 4],
                        color_a: fill_color(&node.props).to_rgba(),
                        color_b: theme::WIDGET_SLIDER_TRACK().to_rgba(),
                        color_c: [0.0; 4],
                        color_d: [0.0; 4],
                        corner_radius: 0.0,
                        pixel_aspect,
                    },
                );
                // Layer 2: material overlay
                prims.push(sdf_widget::build_material_overlay(
                    node,
                    &def,
                    shader_type,
                    viewport,
                    t,
                ));
                return prims;
            }
        }

        // Default hardcoded shader path
        metal_widget_instance(
            widget_type,
            super::WidgetInstance {
                ndc_min,
                ndc_max,
                value_t: t,
                orientation: 0.0,
                itime: 0.0,
                uniform_a: [0.0; 4],
                uniform_b: [0.0; 4],
                color_a: fill_color(&node.props).to_rgba(),
                color_b: theme::WIDGET_SLIDER_TRACK().to_rgba(),
                color_c: [0.0; 4],
                color_d: [0.0; 4],
                corner_radius: 0.0,
                pixel_aspect,
            },
        )
    }
}
