use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MetalPrimitive, MetalProportionalTextPrimitive, MouseEventOutcome,
    WidgetDefinition, WidgetEvent, label::label_text_row, ndc_bounds, resolve_named_color,
    styled_cell,
};
use crate::backend::Color;
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
    get_prop_str,
};
use crate::theme;
use crate::vm::Value;

pub struct ButtonWidget;

pub static BUTTON_WIDGET: ButtonWidget = ButtonWidget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ButtonVariant {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

fn variant(props: &HashMap<String, Value>) -> ButtonVariant {
    match props.get("variant") {
        Some(Value::Keyword(value)) | Some(Value::String(value)) => match value.as_str() {
            "secondary" => ButtonVariant::Secondary,
            "ghost" => ButtonVariant::Ghost,
            "danger" => ButtonVariant::Danger,
            _ => ButtonVariant::Primary,
        },
        _ => ButtonVariant::Primary,
    }
}

fn prop_text(props: &HashMap<String, Value>) -> String {
    match props.get("text") {
        Some(Value::String(text)) => text.clone(),
        Some(value) => crate::vm::format_lisp_value(value),
        None => String::new(),
    }
}

fn variant_bg(props: &HashMap<String, Value>) -> Color {
    let default = match variant(props) {
        ButtonVariant::Primary => theme::BUTTON_PRIMARY_BG(),
        ButtonVariant::Secondary => theme::BUTTON_SECONDARY_BG(),
        ButtonVariant::Ghost => theme::BUTTON_GHOST_BG(),
        ButtonVariant::Danger => theme::BUTTON_DANGER_BG(),
    };
    resolve_named_color(props, "background-color", default)
}

fn variant_fg(props: &HashMap<String, Value>) -> Color {
    let default = match variant(props) {
        ButtonVariant::Primary => theme::BUTTON_PRIMARY_FG(),
        ButtonVariant::Secondary => theme::BUTTON_SECONDARY_FG(),
        ButtonVariant::Ghost => theme::BUTTON_GHOST_FG(),
        ButtonVariant::Danger => theme::BUTTON_DANGER_FG(),
    };
    resolve_named_color(props, "color", default)
}

fn button_border(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "border-color", theme::BUTTON_BORDER())
}

fn button_highlight(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "highlight-color", theme::BUTTON_HIGHLIGHT())
}

fn button_shadow(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "shadow-color", theme::BUTTON_SHADOW())
}

fn icon_value(props: &HashMap<String, Value>) -> Option<f32> {
    match props.get("icon") {
        Some(Value::Keyword(value)) | Some(Value::String(value)) => match value.as_str() {
            "plus" | "new" => Some(0.0),
            "sampler" => Some(1.0),
            _ => None,
        },
        _ => None,
    }
}

fn click_info(phase: &str, modifiers: KeyModifiers) -> Value {
    let mut info = HashMap::new();
    info.insert(
        "phase".to_string(),
        std::rc::Rc::new(std::cell::RefCell::new(Value::String(phase.to_string()))),
    );
    info.insert(
        "shift".to_string(),
        std::rc::Rc::new(std::cell::RefCell::new(Value::Bool(
            modifiers.contains(KeyModifiers::SHIFT),
        ))),
    );
    info.insert(
        "alt".to_string(),
        std::rc::Rc::new(std::cell::RefCell::new(Value::Bool(
            modifiers.contains(KeyModifiers::ALT),
        ))),
    );
    let super_pressed = modifiers.contains(KeyModifiers::SUPER);
    info.insert(
        "super".to_string(),
        std::rc::Rc::new(std::cell::RefCell::new(Value::Bool(super_pressed))),
    );
    info.insert(
        "meta".to_string(),
        std::rc::Rc::new(std::cell::RefCell::new(Value::Bool(super_pressed))),
    );
    Value::Map(info)
}

#[cfg(target_os = "macos")]
fn normalized_corner_radius(rect: Rect, viewport: super::WidgetViewport, radius_px: f32) -> f32 {
    if radius_px <= 0.0 {
        return 0.001;
    }
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((radius_px * 2.0) / px_h).clamp(0.001, 0.5)
}

#[cfg(target_os = "macos")]
const BUTTON_ICON_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;
    float2 p = float2((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);
    float4 col = in.color_a;

    float d = 1.0;
    if (in.value_t < 0.5) {
        // plus
        float2 bar_v = abs(p) - float2(0.07, 0.36);
        float dv = length(max(bar_v, 0.0)) + min(max(bar_v.x, bar_v.y), 0.0);
        float2 bar_h = abs(p) - float2(0.36, 0.07);
        float dh = length(max(bar_h, 0.0)) + min(max(bar_h.x, bar_h.y), 0.0);
        d = min(dv, dh);
    } else {
        // sampler: stroked SP/MPC-style pad sampler glyph
        p *= 1.24;
        float2 body_q = abs(p) - float2(0.46, 0.34);
        float body_base = length(max(body_q, 0.0)) + min(max(body_q.x, body_q.y), 0.0) - 0.13;
        float body = abs(body_base) - 0.055;
        float2 screen_q = abs(p - float2(-0.16, -0.18)) - float2(0.17, 0.05);
        float screen_base = length(max(screen_q, 0.0)) + min(max(screen_q.x, screen_q.y), 0.0) - 0.04;
        float screen = abs(screen_base) - 0.035;
        float pad = 1.0;
        for (int ix = 0; ix < 2; ix++) {
            for (int iy = 0; iy < 2; iy++) {
                float2 center = float2(0.07 + float(ix) * 0.20, -0.06 + float(iy) * 0.19);
                float2 q = abs(p - center) - float2(0.065, 0.06);
                float pd_base = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - 0.045;
                float pd = abs(pd_base) - 0.028;
                pad = min(pad, pd);
            }
        }
        float knob = abs(length(p - float2(-0.31, 0.16)) - 0.055) - 0.03;
        d = min(min(body, screen), min(pad, knob));
    }

    float edge = max(fwidth(d), 0.001) * 1.2;
    float mask = smoothstep(edge, -edge, d);
    if (mask < 0.002) { discard_fragment(); }
    return float4(col.rgb, col.a * mask);
}
"#;

#[cfg(target_os = "macos")]
const BUTTON_SURFACE_SHADER: &str = r#"
float button_surface_rounded_rect(float2 p, float2 size, float radius)
{
    float2 q = abs(p) - (size - float2(radius));
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

float button_surface_smooth(float2 p, float2 size, float radius, float edge_min, float edge_max)
{
    return smoothstep(edge_min, edge_max, button_surface_rounded_rect(p, size, radius));
}

float3 button_surface_normal(float2 p, float2 size, float radius, float eps)
{
    float right = button_surface_smooth(p + float2(eps, 0.0), size, radius, -0.10, 0.92);
    float left = button_surface_smooth(p - float2(eps, 0.0), size, radius, -0.10, 0.92);
    float down = button_surface_smooth(p + float2(0.0, eps), size, radius, -0.10, 0.92);
    float up = button_surface_smooth(p - float2(0.0, eps), size, radius, -0.10, 0.92);
    return normalize(float3((right - left) / (2.0 * eps), (down - up) / (2.0 * eps), 1.0));
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float aspect = max(in.aspect, 0.001);
    float2 p = float2((in.uv.x - 0.5) * 2.0 * aspect, (in.uv.y - 0.5) * 2.0);
    float2 size = float2(aspect, 1.0);

    float r = in.corner_radius > 0.0 ? in.corner_radius : 0.75;
    r = min(r, min(aspect, 1.0));
    float d = button_surface_rounded_rect(p, size, r);

    float edge = fwidth(d) * 1.2;
    float mask = smoothstep(edge, -edge, d);
    if (mask < 0.002) { discard_fragment(); }

    float px = max(max(fwidth(p.x), fwidth(p.y)), 0.001);
    float border_width = 1.35 * px;
    float2 inner_size = max(size - float2(border_width), float2(0.001));
    float inner_d = button_surface_rounded_rect(p, inner_size, max(r - border_width, 0.0));
    float inner_edge = fwidth(inner_d) * 1.2;
    float inner_mask = smoothstep(inner_edge, -inner_edge, inner_d);
    float border_mask = clamp(mask - inner_mask, 0.0, 1.0);

    float3 normal = button_surface_normal(p, size, r, max(px * 1.5, 0.004));
    float3 view_dir = float3(0.0, 0.0, 1.0);
    float3 key_light = normalize(float3(-0.72, -0.92, 1.30));
    float3 bounce_light = normalize(float3(0.82, 0.78, 1.10));
    float key_diffuse = max(0.0, dot(normal, key_light));
    float bounce_diffuse = max(0.0, dot(normal, bounce_light));
    float key_specular = pow(max(0.0, dot(normal, normalize(key_light + view_dir))), 56.0);
    float bounce_specular = pow(max(0.0, dot(normal, normalize(bounce_light + view_dir))), 42.0);
    float edge_fade = smoothstep(0.12, -0.04, d);

    float quadrant_shade = (key_diffuse - 0.34 * bounce_diffuse) * 0.16;
    quadrant_shade += (bounce_diffuse - 0.30 * key_diffuse) * 0.10;
    float3 fill_lit = in.color_a.rgb * (1.0 + quadrant_shade * 0.08);
    fill_lit += in.color_c.rgb * in.color_c.a * key_specular * edge_fade * 0.10;
    fill_lit = mix(fill_lit, in.color_d.rgb, in.color_d.a * (1.0 - key_diffuse) * 0.05);

    float3 border_lit = in.color_b.rgb * (0.72 + 0.34 * key_diffuse + 0.26 * bounce_diffuse);
    border_lit += in.color_c.rgb * in.color_c.a * (key_specular * 1.8 + bounce_specular * 1.25) * edge_fade;
    border_lit = mix(border_lit, in.color_d.rgb, in.color_d.a * (1.0 - max(key_diffuse, bounce_diffuse)) * 0.42);

    float4 fill = float4(fill_lit, in.color_a.a * inner_mask);
    float4 border = float4(border_lit, in.color_b.a * border_mask);
    float out_alpha = fill.a + border.a * (1.0 - fill.a);
    if (out_alpha <= 0.002) { discard_fragment(); }
    float3 out_rgb = (fill.rgb * fill.a + border.rgb * border.a * (1.0 - fill.a)) / out_alpha;
    return float4(out_rgb, out_alpha);
}
"#;

impl WidgetDefinition for ButtonWidget {
    fn names(&self) -> &'static [&'static str] {
        &["button", "button-icon", "badge"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["text", "width", "height", "font-size", "padding"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let text = get_prop_str(node, "text").unwrap_or_default();
        let font_size = get_prop_num(node, "font-size")
            .map(f64_to_f32)
            .unwrap_or(ctx.inherited_font_size);
        let padding = get_prop_num(node, "padding").map(f64_to_f32).unwrap_or(1.2);
        let text_width = ctx
            .text_measurer
            .map(|measurer| measurer.measure_text_px(&text, font_size) / ctx.cell_w)
            .unwrap_or_else(|| text.chars().count() as f32);
        Some(Size {
            width: get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or(text_width + padding * 2.0),
            height: get_prop_num(node, "height").map(f64_to_f32).unwrap_or(1.5),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let text = prop_text(props);
        let fg = variant_fg(props);
        let bg = variant_bg(props);
        let row = label_text_row(props, rect).round() as u16;
        let col = rect.col.round() as u16;
        let width = rect.width.round() as u16;
        let text_len = text.chars().count() as u16;
        let start = col + width.saturating_sub(text_len) / 2;
        for i in 0..width {
            buf.set(row, col + i, styled_cell(' ', fg, Some(bg)));
        }
        for (idx, ch) in text.chars().enumerate() {
            let cell_col = start + idx as u16;
            if cell_col < col + width {
                buf.set(row, cell_col, styled_cell(ch, fg, Some(bg)));
            }
        }
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        _local_col: f32,
        _local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        if node.widget_type == "badge" {
            return MouseEventOutcome::Ignore;
        }
        if matches!(node.props.get("disabled"), Some(Value::Bool(true))) {
            return MouseEventOutcome::Consume;
        }
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                MouseEventOutcome::Dispatch(WidgetEvent::Activate(modifiers))
            }
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn key_event(&self, node: &LayoutNode, key: super::WidgetKeyEvent) -> Option<WidgetEvent> {
        if node.widget_type == "badge" {
            return None;
        }
        if matches!(node.props.get("disabled"), Some(Value::Bool(true))) {
            return None;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => Some(WidgetEvent::Activate(key.modifiers)),
            _ => None,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        if node.widget_type == "badge" {
            return None;
        }
        let WidgetEvent::Activate(modifiers) = event else {
            return None;
        };
        let callback = node.props.get("on-click")?.clone();
        Some(EventOutput {
            callback,
            args: vec![click_info("click", modifiers)],
        })
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, widget_type: &str) -> Option<&'static str> {
        if widget_type == "button-icon" {
            Some(BUTTON_ICON_SHADER)
        } else {
            Some(BUTTON_SURFACE_SHADER)
        }
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: super::WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let mut prims = Vec::new();
        let bg = variant_bg(&node.props);
        if bg.a > 0.0 {
            let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
            let px_w = node.rect.width * viewport.cell_w;
            let px_h = node.rect.height * viewport.cell_h;
            prims.push(MetalPrimitive::WidgetInstance {
                widget_type: "button".to_string(),
                instance: super::WidgetInstance {
                    ndc_min,
                    ndc_max,
                    value_t: 0.0,
                    orientation: 0.0,
                    itime: viewport.time_seconds,
                    uniform_a: [0.0; 4],
                    uniform_b: [0.0; 4],
                    color_a: bg.to_rgba(),
                    color_b: button_border(&node.props).to_rgba(),
                    color_c: button_highlight(&node.props).to_rgba(),
                    color_d: button_shadow(&node.props).to_rgba(),
                    corner_radius: normalized_corner_radius(node.rect, viewport, 12.0),
                    pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                },
                is_background: true,
            });
        }

        if let Some(icon) = icon_value(&node.props) {
            let icon_size = node.rect.height.min(1.0);
            let icon_rect = Rect {
                row: node.rect.row + (node.rect.height - icon_size).max(0.0) * 0.5 - 0.08,
                col: node.rect.col + 0.47,
                width: icon_size + 0.16,
                height: icon_size + 0.16,
            };
            let (ndc_min, ndc_max) = ndc_bounds(icon_rect, viewport);
            let px_w = icon_rect.width * viewport.cell_w;
            let px_h = icon_rect.height * viewport.cell_h;
            prims.push(MetalPrimitive::WidgetInstance {
                widget_type: "button-icon".to_string(),
                instance: super::WidgetInstance {
                    ndc_min,
                    ndc_max,
                    value_t: icon,
                    orientation: 0.0,
                    itime: viewport.time_seconds,
                    uniform_a: [0.0; 4],
                    uniform_b: [0.0; 4],
                    color_a: variant_fg(&node.props).to_rgba(),
                    color_b: [0.0; 4],
                    color_c: [0.0; 4],
                    color_d: [0.0; 4],
                    corner_radius: 0.0,
                    pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                },
                is_background: false,
            });
        }

        let text = prop_text(&node.props);
        if !text.is_empty() {
            let font_size = super::get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);
            let has_icon = icon_value(&node.props).is_some();
            let text_col = node.rect.col + if has_icon { 1.25 } else { 0.0 };
            let align_width = if has_icon {
                (node.rect.width - 1.35).max(0.0)
            } else {
                node.rect.width
            };
            let mut text_props = node.props.clone();
            text_props
                .entry("v-align".to_string())
                .or_insert_with(|| Value::Keyword("center".to_string()));
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: label_text_row(&text_props, node.rect),
                    col: text_col,
                    align_width,
                    h_align: 0.5,
                    text,
                    font_size,
                    scale: 1.0,
                    fg: variant_fg(&node.props),
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
    use std::cell::RefCell;
    use std::rc::Rc;

    fn color_value(r: f64, g: f64, b: f64, a: f64) -> Value {
        Value::List(
            [r, g, b, a]
                .into_iter()
                .map(|value| Rc::new(RefCell::new(Value::Number(value))))
                .collect(),
        )
    }

    #[cfg(target_os = "macos")]
    fn test_viewport() -> super::super::WidgetViewport {
        super::super::WidgetViewport {
            vp_w: 100.0,
            vp_h: 100.0,
            cell_w: 10.0,
            cell_h: 10.0,
            scroll_top: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 10.0,
            inherited_hover: false,
            time_seconds: 0.0,
            scroll_left: 0.0,
        }
    }

    fn test_button_node(props: HashMap<String, Value>) -> LayoutNode {
        LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "button".to_string(),
            rect: Rect {
                row: 2.0,
                col: 4.0,
                width: 8.0,
                height: 3.0,
            },
            props,
            children: Vec::new(),
            focusable: true,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn button_metal_centers_text_vertically() {
        let props = HashMap::from([
            ("text".to_string(), Value::String("Go".to_string())),
            ("variant".to_string(), Value::Keyword("primary".to_string())),
            ("font-size".to_string(), Value::Number(11.0)),
        ]);
        let node = test_button_node(props);
        let viewport = test_viewport();

        let prims = ButtonWidget.build_metal_primitives("button", &node, viewport);
        let background_is_marked_background = prims.iter().any(|prim| {
            matches!(
                prim,
                MetalPrimitive::WidgetInstance {
                    widget_type,
                    is_background: true,
                    ..
                } if widget_type == "button"
            )
        });
        assert!(
            background_is_marked_background,
            "button background must render before proportional text"
        );
        let text = prims
            .iter()
            .find_map(|prim| match prim {
                MetalPrimitive::ProportionalText(text) => Some(text),
                _ => None,
            })
            .expect("button should emit centered text");

        assert!((text.row - 3.0).abs() < 0.0001);
        assert!((text.col - node.rect.col).abs() < 0.0001);
        assert!((text.align_width - node.rect.width).abs() < 0.0001);
        assert!((text.h_align - 0.5).abs() < 0.0001);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn button_metal_background_carries_surface_detail_colors() {
        let props = HashMap::from([
            ("text".to_string(), Value::String("Go".to_string())),
            (
                "background-color".to_string(),
                color_value(0.1, 0.2, 0.3, 1.0),
            ),
            ("border-color".to_string(), color_value(0.4, 0.5, 0.6, 0.7)),
            (
                "highlight-color".to_string(),
                color_value(0.8, 0.85, 0.9, 0.2),
            ),
            ("shadow-color".to_string(), color_value(0.0, 0.0, 0.0, 0.3)),
        ]);
        let node = test_button_node(props);
        let prims = ButtonWidget.build_metal_primitives("button", &node, test_viewport());
        let instance = prims
            .iter()
            .find_map(|prim| match prim {
                MetalPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    is_background: true,
                } if widget_type == "button" => Some(instance),
                _ => None,
            })
            .expect("button should emit a detailed background primitive");

        assert_eq!(instance.color_a, [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(instance.color_b, [0.4, 0.5, 0.6, 0.7]);
        assert_eq!(instance.color_c, [0.8, 0.85, 0.9, 0.2]);
        assert_eq!(instance.color_d, [0.0, 0.0, 0.0, 0.3]);
    }
}
