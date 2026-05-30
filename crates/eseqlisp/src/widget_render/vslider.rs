use std::collections::HashMap;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

#[cfg(target_os = "macos")]
use super::sdf_widget;
use super::{
    CellBuffer, EventOutput, MetalPrimitive, MetalProportionalTextPrimitive, MouseEventOutcome,
    WidgetDefinition, WidgetEvent, get_f32_prop, mapped_haptic_value, metal_widget_instance,
    ndc_bounds, resolve_named_color, should_trigger_integer_haptic, styled_cell,
    trigger_level_change_haptic,
};
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
};
use crate::theme;
use crate::vm::Value;

pub struct VerticalSliderWidget;

pub static VSLIDER_WIDGET: VerticalSliderWidget = VerticalSliderWidget;

fn fill_color(props: &HashMap<String, Value>) -> crate::backend::Color {
    resolve_named_color(props, "fill", theme::WIDGET_SLIDER_FILLED())
}

fn dot_color(props: &HashMap<String, Value>) -> crate::backend::Color {
    resolve_named_color(props, "dot-color", theme::WIDGET_SLIDER_DOT())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_visual_props_are_bindable_but_not_size_affecting() {
        assert_eq!(
            VSLIDER_WIDGET.bindable_props(),
            &[
                "value",
                "haptic-value",
                "active",
                "track-r",
                "track-g",
                "track-b",
                "shader-state-active",
                "shader-state-track-r",
                "shader-state-track-g",
                "shader-state-track-b",
            ]
        );
        for prop in VSLIDER_WIDGET.bindable_props() {
            assert!(
                !VSLIDER_WIDGET.size_affecting_props().contains(prop),
                "{prop} should not force vslider layout"
            );
        }
    }
}

fn items(props: &HashMap<String, Value>) -> Vec<String> {
    match props.get("items") {
        Some(Value::List(list)) => list
            .iter()
            .filter_map(|v| match &*v.borrow() {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

fn selected_label(props: &HashMap<String, Value>) -> String {
    let labels = items(props);
    let value = get_f32_prop(props, "value", 0.0).round() as usize;
    labels.get(value).cloned().unwrap_or_default()
}

fn render_item_chars(
    props: &HashMap<String, Value>,
    rect: Rect,
    mut put: impl FnMut(u16, u16, char, crate::backend::Color),
) {
    let text = selected_label(props);
    let chars: Vec<char> = text.chars().take(rect.height.round() as usize).collect();
    let fg = resolve_named_color(props, "color", theme::WIDGET_LABEL_FG());
    let row_u16 = rect.row.round() as u16;
    let col_u16 = rect.col.round() as u16;
    let height_u16 = rect.height.round() as u16;
    let width_u16 = rect.width.round() as u16;
    let text_col = col_u16 + width_u16.saturating_sub(1) / 2;
    let start = if chars.len() < height_u16 as usize {
        (height_u16 as usize - chars.len()) / 2
    } else {
        0
    };
    for row_off in 0..height_u16 {
        let ch = row_off
            .checked_sub(start as u16)
            .and_then(|idx| chars.get(idx as usize))
            .copied()
            .unwrap_or(' ');
        put(row_u16 + row_off, text_col, ch, fg);
    }
}

/// Compute normalized value and origin for bipolar support.
/// `:origin` sets the fill start point in value-space (default: min).
fn normalize_with_origin(props: &HashMap<String, Value>) -> (f32, f32) {
    let value = get_f32_prop(props, "value", 0.0);
    let min = get_f32_prop(props, "min", 0.0);
    let max = get_f32_prop(props, "max", 1.0);
    let range = max - min;
    let t = if range > 0.0 {
        ((value - min) / range).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let origin = get_f32_prop(props, "origin", min);
    let origin_t = if range > 0.0 {
        ((origin - min) / range).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (t, origin_t)
}

/// TUI render for vertical slider: fill from origin toward value.
fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    if !items(props).is_empty() {
        render_item_chars(props, rect, |row, col, ch, fg| {
            buf.set(row, col, styled_cell(ch, fg, None));
        });
        return;
    }

    let (t, origin_t) = normalize_with_origin(props);

    let row_u16 = rect.row.round() as u16;
    let col_u16 = rect.col.round() as u16;
    let height_u16 = rect.height.round() as u16;
    let width_u16 = rect.width.round() as u16;

    // In screen coords, row 0 = top. t=1 is top, t=0 is bottom.
    // fill_top_t and fill_bot_t are in normalized 0–1 (bottom=0, top=1).
    let fill_top_t = t.max(origin_t);
    let fill_bot_t = t.min(origin_t);
    let fill_top_row = height_u16.saturating_sub((fill_top_t * rect.height).round() as u16);
    let fill_bot_row = height_u16.saturating_sub((fill_bot_t * rect.height).round() as u16);

    for row_offset in 0..height_u16 {
        let row = row_u16 + row_offset;
        for col_offset in 0..width_u16.min(2) {
            let col = col_u16 + col_offset;
            if row_offset >= fill_top_row && row_offset < fill_bot_row {
                buf.set(row, col, styled_cell('\u{2588}', fill_color(props), None));
            } else {
                let ch = if row_offset % 2 == 0 { '\u{2022}' } else { ' ' };
                buf.set(row, col, styled_cell(ch, dot_color(props), None));
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
    float t = in.value_t;
    float origin_t = in.uniform_a[0]; // 0 = fill from bottom (default), 0.5 = bipolar center

    // ── Fill bar: rounded rect between origin and value ──
    float fill_lo = min(t, origin_t);
    float fill_hi = max(t, origin_t);
    float fill_span = fill_hi - fill_lo;

    float xPad = 0.18;
    float halfW = 0.5 - xPad;
    float halfH = max(fill_span * 0.5, 0.0);
    float cr = 0.063;
    cr = min(cr, min(halfW * aspect, max(halfH, 0.001)));

    // Center of fill bar in uv space (uv.y: 0=top, 1=bottom; t=1 → top)
    float fillCenterY = 1.0 - (fill_lo + fill_hi) * 0.5;
    float2 p = float2((uv.x - 0.5) * aspect, uv.y - fillCenterY);
    float2 b = float2(halfW * aspect, halfH);
    float2 q = abs(p) - b + cr;
    float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - cr;
    float aa = max(fwidth(d), 0.001);
    float fillMask = smoothstep(aa, -aa, d) * step(0.005, fill_span);

    // ── Track dots: fixed grid, only visible outside fill ──
    float dotSpacing = 0.6 * aspect;
    float dotR = 0.08 * aspect;
    float dotMask = 0.0;

    float snapY = round(uv.y / dotSpacing) * dotSpacing;
    float fillTopUV = 1.0 - fill_hi;
    float fillBotUV = 1.0 - fill_lo;
    float margin = dotSpacing * 0.4;
    if ((snapY < fillTopUV - margin || snapY > fillBotUV + margin)
        && snapY > margin * 0.5 && snapY < 1.0 - margin * 0.5) {
        float2 dp = float2((uv.x - 0.5) * aspect, uv.y - snapY);
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

impl WidgetDefinition for VerticalSliderWidget {
    fn names(&self) -> &'static [&'static str] {
        &["vslider"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &[
            "value",
            "haptic-value",
            "active",
            "track-r",
            "track-g",
            "track-b",
            "shader-state-active",
            "shader-state-track-r",
            "shader-state-track-g",
            "shader-state-track-b",
        ]
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
            width: 2.0,
            height: get_prop_num(node, "height").map(f64_to_f32).unwrap_or(8.0),
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
        _modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                let denom = node.rect.height.max(0.001);
                let offset = (local_row - node.rect.row) / denom;
                let t = (1.0 - offset).clamp(0.0, 1.0);
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
        let haptic_previous = get_f32_prop(&node.props, "haptic-value", previous);
        let haptic_min = get_f32_prop(&node.props, "haptic-min", min);
        let haptic_max = get_f32_prop(&node.props, "haptic-max", max);
        let haptic_value = mapped_haptic_value(&node.props, t, value);
        if should_trigger_integer_haptic(
            node.widget_id,
            haptic_previous,
            haptic_value,
            haptic_min,
            haptic_max,
        ) {
            trigger_level_change_haptic();
        }
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
    fn build_metal_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: super::WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        if !items(&node.props).is_empty() {
            let font_size = node
                .props
                .get("font-size")
                .and_then(|v| match v {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                })
                .unwrap_or(DEFAULT_FONT_SIZE);
            let mut prims = Vec::new();
            render_item_chars(&node.props, node.rect, |row, col, ch, fg| {
                if ch != ' ' {
                    prims.push(MetalPrimitive::ProportionalText(
                        MetalProportionalTextPrimitive {
                            row: row as f32,
                            col: col as f32,
                            align_width: 0.0,
                            h_align: 0.0,
                            text: ch.to_string(),
                            font_size,
                            scale: 1.0,
                            fg,
                            bg: theme::BG(),
                        },
                    ));
                }
            });
            return prims;
        }

        let (t, origin_t) = normalize_with_origin(&node.props);
        let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
        let px_w = node.rect.width * viewport.cell_w;
        let px_h = node.rect.height * viewport.cell_h;
        let pixel_aspect = if px_h > 0.0 { px_w / px_h } else { 1.0 };

        // Check for :material shader override.
        // Emit default shader first (track dots + flat fill), then material on top.
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
                        color_b: dot_color(&node.props).to_rgba(),
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
                uniform_a: [origin_t, 0.0, 0.0, 0.0],
                uniform_b: [0.0; 4],
                color_a: fill_color(&node.props).to_rgba(),
                color_b: dot_color(&node.props).to_rgba(),
                color_c: [0.0; 4],
                color_d: [0.0; 4],
                corner_radius: 0.0,
                pixel_aspect,
            },
        )
    }
}
