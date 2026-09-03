use std::collections::HashMap;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, GpuPrimitive, MouseEventOutcome, WidgetDefinition, WidgetEvent,
    get_f32_prop, gpu_widget_instance, ndc_bounds, styled_cell,
};
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct KnobWidget;

pub static KNOB_WIDGET: KnobWidget = KnobWidget;

const KNOB_FRAGMENT_SHADER: super::ShaderSources = super::ShaderSources::both(r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;

    float2 scale = aspect >= 1.0
        ? float2(aspect, 1.0)
        : float2(1.0, 1.0 / max(aspect, 0.0001));
    float2 p = float2((uv.x - 0.5) * 2.0, (uv.y - 0.5) * 2.0) * scale;
    float r = length(p);
    float a = atan2(p.y, p.x);

    float start = 1.57079633;
    float sweep = 4.71238898;
    float rel = fmod((a - start + 6.2831853), 6.2831853);
    float inRange = step(rel, sweep);
    float active = step(rel, sweep * in.value_t);

    float aa = max(fwidth(r), 0.0015);
    float ring = abs(r - 0.74) - 0.11;
    float activeRing = abs(r - 0.74) - 0.125;
    float ringMask = smoothstep(aa, -aa, ring) * inRange;
    float activeMask = smoothstep(aa, -aa, activeRing) * inRange * active;
    float trackMask = ringMask * (1.0 - active);

    float notchAngle = start + sweep * in.value_t;
    float2 valueDir = float2(cos(notchAngle), sin(notchAngle));
    float2 notchPos = valueDir * 0.74;
    float notch = length(p - notchPos) - 0.105;
    float notchMask = smoothstep(aa, -aa, notch);
    float lineAlong = dot(p, valueDir);
    float lineAcross = abs(p.x * valueDir.y - p.y * valueDir.x);
    float lineSegment = step(0.0, lineAlong) * step(lineAlong, 0.68);
    float line = lineAcross - 0.11;
    float lineMask = smoothstep(aa, -aa, line) * lineSegment;

    float4 col = float4(0.0);
    col = mix(col, in.color_b, trackMask);
    col = mix(col, in.color_a, activeMask);
    col = mix(col, in.color_b, lineMask);
    col = mix(col, in.color_a, notchMask);
    if (col.a < 0.01) { discard_fragment(); }
    return col;
}
"#, super::wgsl::KNOB_FRAGMENT_SHADER);

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
    let denom = (node.rect.height - 1.0).max(1.0);
    let offset = (local_row - node.rect.row) / denom;
    (1.0 - offset).clamp(0.0, 1.0)
}

fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let t = normalized_value(props);
    let size = rect.width.min(rect.height).max(1.0);
    let outer_radius = 0.48 * size;
    let fill_radius = (0.10 + 0.38 * t) * size;
    let center_x = rect.col + rect.width * 0.5;
    let center_y = rect.row + rect.height * 0.5;

    let row_u16 = rect.row.round() as u16;
    let col_u16 = rect.col.round() as u16;
    let height_u16 = rect.height.round() as u16;
    let width_u16 = rect.width.round() as u16;

    for row_offset in 0..height_u16 {
        for col_offset in 0..width_u16 {
            let sample_x = rect.col + col_offset as f32 + 0.5;
            let sample_y = rect.row + row_offset as f32 + 0.5;
            let dx = sample_x - center_x;
            let dy = sample_y - center_y;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > outer_radius {
                continue;
            }

            let (ch, fg) = if dist <= fill_radius {
                ('●', theme::WIDGET_KNOB_FILLED())
            } else {
                ('○', theme::WIDGET_KNOB_TRACK())
            };
            buf.set(
                row_u16 + row_offset,
                col_u16 + col_offset,
                styled_cell(ch, fg, None),
            );
        }
    }
}

impl WidgetDefinition for KnobWidget {
    fn names(&self) -> &'static [&'static str] {
        &["knob", "inline-knob"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["size"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let size = get_prop_num(node, "size")
            .map(f64_to_f32)
            .unwrap_or(2.0)
            .max(1.0);
        let aspect = constraints.aspect.max(f32::EPSILON);
        Some(Size {
            width: size,
            height: size / aspect,
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
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        match (node.widget_type.as_str(), mouse_kind) {
            ("inline-knob", MouseEventKind::Down(MouseButton::Left)) => MouseEventOutcome::Consume,
            ("inline-knob", MouseEventKind::Drag(MouseButton::Left)) => {
                let Some(Value::List(gesture)) = gesture else {
                    return MouseEventOutcome::Consume;
                };
                let number = |index: usize| {
                    gesture.get(index).and_then(|value| match &*value.borrow() {
                        Value::Number(value) => Some(*value as f32),
                        _ => None,
                    })
                };
                let (Some(start_value), Some(start_col), Some(start_row)) =
                    (number(0), number(1), number(2))
                else {
                    return MouseEventOutcome::Consume;
                };
                let min = get_f32_prop(&node.props, "min", 0.0);
                let max = get_f32_prop(&node.props, "max", 1.0);
                let range = (max - min).max(0.0001);
                let delta = ((start_row - local_row) + (local_col - start_col) * 0.15) / 8.0;
                MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::Number(
                    (start_value + delta * range).clamp(min, max) as f64,
                )))
            }
            (
                _,
                MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left),
            ) => MouseEventOutcome::Dispatch(WidgetEvent::SetNormalized(knob_t_from_local_row(
                node, local_row,
            ))),
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn begin_gesture(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
        _modifiers: KeyModifiers,
    ) -> Option<Value> {
        Some(Value::List(vec![
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(get_f32_prop(
                &node.props,
                "value",
                0.0,
            ) as f64))),
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(local_col as f64))),
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(local_row as f64))),
        ]))
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn unclamped_drag(&self) -> bool {
        true
    }

    fn hidden_drag(&self) -> bool {
        true
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let value = match event {
            WidgetEvent::SetNormalized(t) => {
                let min = get_f32_prop(&node.props, "min", 0.0);
                let max = get_f32_prop(&node.props, "max", 1.0);
                min + (max - min) * t.clamp(0.0, 1.0)
            }
            WidgetEvent::Custom(Value::Number(value)) => value as f32,
            _ => return None,
        };
        let callback = node.props.get("on-change")?.clone();
        Some(EventOutput {
            callback,
            args: vec![Value::Number(value as f64)],
        })
    }

    fn fragment_shader(
        &self,
        _widget_type: &str,
        backend: super::ShaderBackend,
    ) -> Option<&'static str> {
        KNOB_FRAGMENT_SHADER.source(backend)
    }

    fn build_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: super::WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
        let px_w = node.rect.width * viewport.cell_w;
        let px_h = node.rect.height * viewport.cell_h;
        gpu_widget_instance(
            widget_type,
            super::WidgetInstance {
                ndc_min,
                ndc_max,
                value_t: normalized_value(&node.props),
                orientation: 0.0,
                itime: 0.0,
                uniform_a: [0.0; 4],
                uniform_b: [0.0; 4],
                uniform_c: [0.0; 4],
                uniform_d: [0.0; 4],
                color_a: theme::WIDGET_KNOB_FILLED().to_rgba(),
                color_b: theme::WIDGET_KNOB_TRACK().to_rgba(),
                color_c: [0.0; 4],
                color_d: [0.0; 4],
                corner_radius: 0.0,
                pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
            },
        )
    }
}
