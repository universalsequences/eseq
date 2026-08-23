use std::collections::HashMap;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, GpuPrimitive, MouseEventOutcome, WidgetDefinition, WidgetEvent,
    get_bool_prop, gpu_widget_instance, ndc_bounds, resolve_named_color, styled_cell,
};
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size};
use crate::theme;
use crate::vm::Value;

pub struct ToggleWidget;

pub static TOGGLE_WIDGET: ToggleWidget = ToggleWidget;

fn on_color(props: &HashMap<String, Value>) -> crate::backend::Color {
    resolve_named_color(props, "color", theme::WIDGET_TOGGLE_ON())
}

fn off_color(props: &HashMap<String, Value>) -> crate::backend::Color {
    resolve_named_color(props, "off-color", theme::WIDGET_TOGGLE_OFF())
}

fn knob_on_color(props: &HashMap<String, Value>) -> crate::backend::Color {
    resolve_named_color(props, "knob-color", theme::WIDGET_TOGGLE_KNOB_ON())
}

fn knob_off_color(props: &HashMap<String, Value>) -> crate::backend::Color {
    resolve_named_color(props, "off-knob-color", theme::WIDGET_TOGGLE_KNOB_OFF())
}

/// TUI render for toggle: "(●)" or "(○)"
fn tui_render(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let on = get_bool_prop(props, "value", false);
    let track = if on {
        on_color(props)
    } else {
        off_color(props)
    };
    let knob = if on {
        knob_on_color(props)
    } else {
        knob_off_color(props)
    };

    let row_u16 = rect.row.round() as u16;
    let col_u16 = rect.col.round() as u16;
    let width_u16 = rect.width.round() as u16;
    let glyphs = [
        ('(', track),
        (if on { '●' } else { '○' }, knob),
        (')', track),
    ];

    for (i, (ch, fg)) in glyphs.into_iter().enumerate() {
        let col = col_u16 + i as u16;
        if col >= col_u16 + width_u16 {
            break;
        }
        buf.set(row_u16, col, styled_cell(ch, fg, None));
    }
}

const TOGGLE_FRAGMENT_SHADER: super::ShaderSources = super::ShaderSources::msl(r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;

    float2 localPos = float2((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);
    float2 sdfSize = float2(aspect, 1.0);
    float cornerRadius = 1.0;

    float3 borderColor = float3(0.25, 0.25, 0.28);
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

    float4 knobColor = mix(in.color_d, in.color_c, on);
    float3 rgb = mix(bg.rgb, borderColor, borderMask);
    rgb = mix(rgb, knobColor.rgb, knobMask);

    return float4(rgb, outerMask);
}
"#);

impl WidgetDefinition for ToggleWidget {
    fn names(&self) -> &'static [&'static str] {
        &["toggle"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["value"]
    }

    fn measure(
        &self,
        _node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        Some(Size {
            width: 4.0,
            height: 1.0,
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
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                MouseEventOutcome::Dispatch(WidgetEvent::Activate(modifiers))
            }
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Activate(_) = event else {
            return None;
        };
        let callback = node.props.get("on-change")?.clone();
        let current = get_bool_prop(&node.props, "value", false);
        Some(EventOutput {
            callback,
            args: vec![Value::Bool(!current)],
        })
    }

    fn fragment_shader(
        &self,
        _widget_type: &str,
        backend: super::ShaderBackend,
    ) -> Option<&'static str> {
        TOGGLE_FRAGMENT_SHADER.source(backend)
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
                value_t: if get_bool_prop(&node.props, "value", false) {
                    1.0
                } else {
                    0.0
                },
                orientation: 0.0,
                itime: 0.0,
                uniform_a: [0.0; 4],
                uniform_b: [0.0; 4],
                uniform_c: [0.0; 4],
                uniform_d: [0.0; 4],
                color_a: on_color(&node.props).to_rgba(),
                color_b: off_color(&node.props).to_rgba(),
                color_c: knob_on_color(&node.props).to_rgba(),
                color_d: knob_off_color(&node.props).to_rgba(),
                corner_radius: 0.0,
                pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;
    use crate::widget_render::CellBuffer;

    #[test]
    fn value_is_bindable_and_accepts_a_reactive_numeric_parameter() {
        assert_eq!(TOGGLE_WIDGET.bindable_props(), &["value"]);
        assert!(!TOGGLE_WIDGET.size_affecting_props().contains(&"value"));

        let slots = crate::reactive::ReactiveBindingStore::default();
        slots.write_float("TOGGLE_TEST", "osc2-on", 1.0);
        let reactive_value = Value::ReactiveRef {
            namespace: "TOGGLE_TEST".to_string(),
            field: "osc2-on".to_string(),
            index: None,
            kind: crate::vm::BindingKind::Float,
            slot: slots.slot("TOGGLE_TEST", "osc2-on"),
        };
        assert!(get_bool_prop(
            &HashMap::from([("value".to_string(), reactive_value.clone())]),
            "value",
            false
        ));

        let widget = crate::widgets::build_widget(
            "toggle",
            vec![Value::Keyword("value".to_string()), reactive_value],
        );

        let Value::Map(map) = widget else {
            panic!("expected toggle widget map");
        };
        assert!(!map.contains_key("__widget-diagnostic"));
    }

    #[test]
    fn toggle_uses_theme_default_and_color_overrides() {
        let mut buf = CellBuffer::new(4, 1);
        let mut props = HashMap::new();
        props.insert("value".to_string(), Value::Bool(true));

        tui_render(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 4.0,
                height: 1.0,
            },
            &mut buf,
        );

        assert_eq!(buf.get(0, 0).unwrap().style.fg, theme::WIDGET_TOGGLE_ON());
        assert_eq!(
            buf.get(0, 1).unwrap().style.fg,
            theme::WIDGET_TOGGLE_KNOB_ON()
        );

        props.insert("value".to_string(), Value::Bool(false));
        props.insert("off-color".to_string(), Value::Keyword("red".to_string()));
        props.insert(
            "off-knob-color".to_string(),
            Value::Keyword("blue".to_string()),
        );
        tui_render(
            &props,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 4.0,
                height: 1.0,
            },
            &mut buf,
        );

        assert_eq!(buf.get(0, 0).unwrap().style.fg, theme::RED());
        assert_eq!(buf.get(0, 1).unwrap().style.fg, theme::BLUE());
    }
}
