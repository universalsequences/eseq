use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, GpuPrimitive, GpuProportionalTextPrimitive, MouseEventOutcome,
    PointerEvent, WidgetDefinition, WidgetEvent, get_f32_prop, label::label_text_row, ndc_bounds,
    plock_active, plock_color, resolve_named_color, styled_cell,
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

fn resolve_h_align(props: &HashMap<String, Value>) -> f32 {
    match props.get("h-align") {
        Some(Value::Number(value)) => (*value as f32).clamp(0.0, 1.0),
        Some(Value::Keyword(value)) | Some(Value::String(value))
            if value == "left" || value == "start" =>
        {
            0.0
        }
        Some(Value::Keyword(value)) | Some(Value::String(value))
            if value == "right" || value == "end" =>
        {
            1.0
        }
        Some(Value::Keyword(value)) | Some(Value::String(value))
            if value == "center" || value == "middle" =>
        {
            0.5
        }
        _ => 0.5,
    }
}

fn button_is_active(props: &HashMap<String, Value>) -> bool {
    match props.get("active") {
        Some(Value::Bool(active)) => *active,
        Some(Value::Number(active)) => *active != 0.0,
        Some(Value::ReactiveRef { .. }) => get_f32_prop(props, "active", 0.0) != 0.0,
        _ => false,
    }
}

fn button_is_active_tab(props: &HashMap<String, Value>) -> bool {
    matches!(
        props.get("shape"),
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "tab"
    ) && button_is_active(props)
}

fn variant_bg(props: &HashMap<String, Value>) -> Color {
    let default = match variant(props) {
        ButtonVariant::Primary => theme::BUTTON_PRIMARY_BG(),
        ButtonVariant::Secondary => theme::BUTTON_SECONDARY_BG(),
        ButtonVariant::Ghost => theme::BUTTON_GHOST_BG(),
        ButtonVariant::Danger => theme::BUTTON_DANGER_BG(),
    };
    if button_is_active(props)
        && let Some(active_bg) = props.get("active-background-color")
    {
        return theme::parse_color_value(active_bg).unwrap_or(default);
    }
    resolve_named_color(props, "background-color", default)
}

fn variant_fg(props: &HashMap<String, Value>) -> Color {
    let default = match variant(props) {
        ButtonVariant::Primary => theme::BUTTON_PRIMARY_FG(),
        ButtonVariant::Secondary => theme::BUTTON_SECONDARY_FG(),
        ButtonVariant::Ghost => theme::BUTTON_GHOST_FG(),
        ButtonVariant::Danger => theme::BUTTON_DANGER_FG(),
    };
    if button_is_active(props)
        && let Some(active_fg) = props.get("active-color")
    {
        return theme::parse_color_value(active_fg).unwrap_or(default);
    }
    resolve_named_color(props, "color", default)
}

fn button_border(props: &HashMap<String, Value>) -> Color {
    if plock_active(props) {
        return plock_color(props);
    }
    resolve_named_color(props, "border-color", theme::BUTTON_BORDER())
}

fn button_highlight(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "highlight-color", theme::BUTTON_HIGHLIGHT())
}

fn button_shadow(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "shadow-color", theme::BUTTON_SHADOW())
}

pub(super) fn icon_name_value(value: &str) -> Option<f32> {
    match value {
        "plus" | "new" => Some(0.0),
        "sampler" => Some(1.0),
        "waveform" => Some(2.0),
        "piano" | "keys" => Some(3.0),
        "sliders" | "controls" => Some(4.0),
        "note-arrow" | "midi-fx" => Some(5.0),
        "dial" | "preset" | "bookmark" => Some(6.0),
        "folder" | "project" => Some(7.0),
        "sine" | "lfo" => Some(8.0),
        _ => None,
    }
}

fn icon_value(props: &HashMap<String, Value>) -> Option<f32> {
    match props.get("icon") {
        Some(Value::Keyword(value)) | Some(Value::String(value)) => icon_name_value(value),
        _ => None,
    }
}

fn click_info(phase: &str, modifiers: KeyModifiers) -> Value {
    let mut info = super::pointer_modifier_info(modifiers);
    info.insert(
        "phase".to_string(),
        std::rc::Rc::new(std::cell::RefCell::new(Value::String(phase.to_string()))),
    );
    Value::Map(info)
}

fn normalized_corner_radius(rect: Rect, viewport: super::WidgetViewport, radius_px: f32) -> f32 {
    if radius_px <= 0.0 {
        return 0.001;
    }
    let radius_px = super::ui_design_px(radius_px);
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((radius_px * 2.0) / px_h).clamp(0.001, 1.0)
}

fn button_icon_rect(rect: Rect) -> Rect {
    let icon_size = rect.height.min(1.08);
    Rect {
        row: rect.row + (rect.height - icon_size).max(0.0) * 0.5,
        col: rect.col + 0.56,
        width: icon_size,
        height: icon_size,
    }
}

fn button_icon_text_inset(rect: Rect) -> f32 {
    let icon_rect = button_icon_rect(rect);
    (icon_rect.col - rect.col) + icon_rect.width + 0.42
}

const BUTTON_ICON_SHADER: super::ShaderSources = super::ShaderSources::both(r#"
float button_icon_box(float2 p, float2 b)
{
    float2 q = abs(p) - b;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);
}

float button_icon_round_rect(float2 p, float2 b, float r)
{
    float2 q = abs(p) - (b - float2(r));
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

float button_icon_segment(float2 p, float2 a, float2 b)
{
    float2 pa = p - a;
    float2 ba = b - a;
    float h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0);
    return length(pa - ba * h);
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;
    float2 p = float2((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);
    float4 col = in.color_a;

    float d = 1.0;
    float stroke = 0.070;
    if (in.value_t < 0.5) {
        // plus
        float2 bar_v = abs(p) - float2(0.09, 0.38);
        float dv = length(max(bar_v, 0.0)) + min(max(bar_v.x, bar_v.y), 0.0);
        float2 bar_h = abs(p) - float2(0.38, 0.09);
        float dh = length(max(bar_h, 0.0)) + min(max(bar_h.x, bar_h.y), 0.0);
        d = min(dv, dh);
    } else if (in.value_t < 1.5) {
        // sampler: compact Ableton-style device outline with heavier strokes
        p *= 1.08;
        float2 body_q = abs(p) - float2(0.50, 0.35);
        float body_base = length(max(body_q, 0.0)) + min(max(body_q.x, body_q.y), 0.0) - 0.08;
        float body = abs(body_base) - 0.075;
        float2 screen_q = abs(p - float2(-0.18, -0.17)) - float2(0.17, 0.045);
        float screen_base = length(max(screen_q, 0.0)) + min(max(screen_q.x, screen_q.y), 0.0) - 0.03;
        float screen = abs(screen_base) - 0.052;
        float pad = 1.0;
        for (int ix = 0; ix < 2; ix++) {
            for (int iy = 0; iy < 2; iy++) {
                float2 center = float2(0.08 + float(ix) * 0.20, -0.05 + float(iy) * 0.18);
                float2 q = abs(p - center) - float2(0.065, 0.055);
                float pd_base = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - 0.03;
                float pd = abs(pd_base) - 0.045;
                pad = min(pad, pd);
            }
        }
        float knob = abs(length(p - float2(-0.32, 0.16)) - 0.055) - 0.045;
        d = min(min(body, screen), min(pad, knob));
    } else if (in.value_t < 2.5) {
        // waveform: vertical sample bars, clearer than a tiny sine curve.
        float b0 = button_icon_segment(p, float2(-0.45, -0.16), float2(-0.45, 0.16)) - stroke;
        float b1 = button_icon_segment(p, float2(-0.25, -0.38), float2(-0.25, 0.38)) - stroke;
        float b2 = button_icon_segment(p, float2(-0.05, -0.26), float2(-0.05, 0.26)) - stroke;
        float b3 = button_icon_segment(p, float2(0.15, -0.46), float2(0.15, 0.46)) - stroke;
        float b4 = button_icon_segment(p, float2(0.35, -0.30), float2(0.35, 0.30)) - stroke;
        float b5 = button_icon_segment(p, float2(0.52, -0.14), float2(0.52, 0.14)) - stroke;
        d = min(min(min(b0, b1), min(b2, b3)), min(b4, b5));
    } else if (in.value_t < 3.5) {
        // piano keys: simple outline plus two dividers; avoids clutter at source-list size.
        float body = abs(button_icon_round_rect(p, float2(0.50, 0.34), 0.055)) - stroke;
        float div_a = button_icon_segment(p, float2(-0.16, -0.31), float2(-0.16, 0.34)) - 0.048;
        float div_b = button_icon_segment(p, float2(0.18, -0.31), float2(0.18, 0.34)) - 0.048;
        d = min(body, min(div_a, div_b));
    } else if (in.value_t < 4.5) {
        // sliders
        float a = button_icon_segment(p, float2(-0.52, -0.30), float2(0.52, -0.30)) - stroke;
        float b = button_icon_segment(p, float2(-0.52, 0.00), float2(0.52, 0.00)) - stroke;
        float c = button_icon_segment(p, float2(-0.52, 0.30), float2(0.52, 0.30)) - stroke;
        float ka = length(p - float2(-0.24, -0.30)) - 0.12;
        float kb = length(p - float2(0.20, 0.00)) - 0.12;
        float kc = length(p - float2(-0.02, 0.30)) - 0.12;
        d = min(min(a, b), min(c, min(ka, min(kb, kc))));
    } else if (in.value_t < 5.5) {
        // note with a rightward arrow
        float2 head_p = (p - float2(-0.30, 0.24)) * float2(1.08, 0.92);
        float head = abs(length(head_p) - 0.16) - 0.052;
        float stem = button_icon_segment(p, float2(-0.13, 0.21), float2(-0.13, -0.42)) - stroke;
        float beam = button_icon_segment(p, float2(-0.13, -0.40), float2(0.15, -0.32)) - stroke;
        float shaft = button_icon_segment(p, float2(0.00, 0.18), float2(0.46, 0.18)) - stroke;
        float arrow_a = button_icon_segment(p, float2(0.46, 0.18), float2(0.27, 0.00)) - stroke;
        float arrow_b = button_icon_segment(p, float2(0.46, 0.18), float2(0.27, 0.36)) - stroke;
        d = min(min(head, stem), min(beam, min(shaft, min(arrow_a, arrow_b))));
    } else if (in.value_t < 6.5) {
        // bookmark
        float left = button_icon_segment(p, float2(-0.34, -0.46), float2(-0.34, 0.46)) - stroke;
        float right = button_icon_segment(p, float2(0.34, -0.46), float2(0.34, 0.46)) - stroke;
        float top = button_icon_segment(p, float2(-0.34, -0.46), float2(0.34, -0.46)) - stroke;
        float fold_a = button_icon_segment(p, float2(-0.34, 0.46), float2(0.00, 0.20)) - stroke;
        float fold_b = button_icon_segment(p, float2(0.34, 0.46), float2(0.00, 0.20)) - stroke;
        d = min(min(left, right), min(top, min(fold_a, fold_b)));
    } else if (in.value_t < 7.5) {
        // folder
        float left = button_icon_segment(p, float2(-0.44, -0.16), float2(-0.44, 0.40)) - stroke;
        float right = button_icon_segment(p, float2(0.50, -0.04), float2(0.50, 0.40)) - stroke;
        float bottom = button_icon_segment(p, float2(-0.44, 0.40), float2(0.50, 0.40)) - stroke;
        float tab_top = button_icon_segment(p, float2(-0.44, -0.32), float2(-0.12, -0.32)) - stroke;
        float tab_side = button_icon_segment(p, float2(-0.12, -0.32), float2(0.02, -0.16)) - stroke;
        float lip = button_icon_segment(p, float2(-0.44, -0.16), float2(0.46, -0.16)) - stroke;
        d = min(min(left, right), min(bottom, min(tab_top, min(tab_side, lip))));
    } else {
        // sine: one cycle traced as short segments so it stays crisp at badge size.
        float s0 = button_icon_segment(p, float2(-0.54, 0.00), float2(-0.405, 0.24)) - stroke;
        float s1 = button_icon_segment(p, float2(-0.405, 0.24), float2(-0.27, 0.38)) - stroke;
        float s2 = button_icon_segment(p, float2(-0.27, 0.38), float2(-0.135, 0.24)) - stroke;
        float s3 = button_icon_segment(p, float2(-0.135, 0.24), float2(0.00, 0.00)) - stroke;
        float s4 = button_icon_segment(p, float2(0.00, 0.00), float2(0.135, -0.24)) - stroke;
        float s5 = button_icon_segment(p, float2(0.135, -0.24), float2(0.27, -0.38)) - stroke;
        float s6 = button_icon_segment(p, float2(0.27, -0.38), float2(0.405, -0.24)) - stroke;
        float s7 = button_icon_segment(p, float2(0.405, -0.24), float2(0.54, 0.00)) - stroke;
        d = min(min(min(s0, s1), min(s2, s3)), min(min(s4, s5), min(s6, s7)));
    }

    float edge = max(fwidth(d), 0.001) * 1.2;
    float mask = smoothstep(edge, -edge, d);
    if (mask < 0.002) { discard_fragment(); }
    return float4(col.rgb, col.a * mask);
}
"#, super::wgsl::BUTTON_ICON_SHADER);

pub(crate) const BUTTON_SURFACE_SHADER: super::ShaderSources = super::ShaderSources::both(r#"
float button_surface_rounded_rect(float2 p, float2 size, float radius)
{
    float2 q = abs(p) - (size - float2(radius));
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

float button_surface_tab(float2 p, float2 size, float radius)
{
    float top_splay = min(size.x * 0.20, 0.22);
    float top_half_width = max(size.x - top_splay, 0.001);
    float t = smoothstep(-size.y + radius * 1.40, size.y, p.y);
    float half_width = mix(top_half_width, size.x, t);
    float2 q = float2(abs(p.x) - half_width, abs(p.y) - size.y);
    float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);

    float2 top_left = p - float2(-top_half_width + radius, -size.y + radius);
    float2 top_right = p - float2(top_half_width - radius, -size.y + radius);
    float top_left_d = length(top_left) - radius;
    float top_right_d = length(top_right) - radius;
    d = (p.x < -top_half_width + radius && p.y < -size.y + radius) ? top_left_d : d;
    d = (p.x > top_half_width - radius && p.y < -size.y + radius) ? top_right_d : d;
    return d;
}

float button_surface_distance(float2 p, float2 size, float radius, float shape)
{
    if (shape > 0.5) {
        return button_surface_tab(p, size, radius);
    }
    return button_surface_rounded_rect(p, size, radius);
}

float button_surface_smooth(float2 p, float2 size, float radius, float edge_min, float edge_max)
{
    return smoothstep(edge_min, edge_max, button_surface_distance(p, size, radius, 0.0));
}

float3 button_surface_normal(float2 p, float2 size, float radius, float shape, float eps)
{
    float right = smoothstep(-0.10, 0.92, button_surface_distance(p + float2(eps, 0.0), size, radius, shape));
    float left = smoothstep(-0.10, 0.92, button_surface_distance(p - float2(eps, 0.0), size, radius, shape));
    float down = smoothstep(-0.10, 0.92, button_surface_distance(p + float2(0.0, eps), size, radius, shape));
    float up = smoothstep(-0.10, 0.92, button_surface_distance(p - float2(0.0, eps), size, radius, shape));
    return normalize(float3((right - left) / (2.0 * eps), (down - up) / (2.0 * eps), 1.0));
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float aspect = max(in.aspect, 0.001);
    float2 p = float2((in.uv.x - 0.5) * 2.0 * aspect, (in.uv.y - 0.5) * 2.0);
    float2 size = float2(aspect, 1.0);
    float shape = in.uniform_a.x;

    float r = in.corner_radius > 0.0 ? in.corner_radius : 0.75;
    r = min(r, min(aspect, 1.0));
    float d = button_surface_distance(p, size, r, shape);

    float edge = fwidth(d) * 1.2;
    float mask = smoothstep(edge, -edge, d);
    if (mask < 0.002) { discard_fragment(); }

    float px = max(max(fwidth(p.x), fwidth(p.y)), 0.001);
    float border_width = 1.35 * px;
    float2 inner_size = max(size - float2(border_width), float2(0.001));
    float inner_d = button_surface_distance(p, inner_size, max(r - border_width, 0.0), shape);
    float inner_edge = fwidth(inner_d) * 1.2;
    float inner_mask = smoothstep(inner_edge, -inner_edge, inner_d);
    float border_mask = clamp(mask - inner_mask, 0.0, 1.0);

    float3 normal = button_surface_normal(p, size, r, shape, max(px * 1.5, 0.004));
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
"#, super::wgsl::BUTTON_SURFACE_SHADER);

impl WidgetDefinition for ButtonWidget {
    fn names(&self) -> &'static [&'static str] {
        &["button", "button-icon", "badge"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["text", "icon", "width", "height", "font-size", "padding"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &[
            "active",
            "corner-radius",
            "plock-active",
            "plock-color-r",
            "plock-color-g",
            "plock-color-b",
        ]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "text", "icon", "width", "height", "font-size", "padding", "active", "disabled",
            "variant", "shape", "color", "active-color", "background-color",
            "active-background-color", "border-color", "highlight-color", "shadow-color",
            "corner-radius", "h-align", "v-align", "on-click", "on-press", "on-release",
            "plock-active", "plock-color-r", "plock-color-g", "plock-color-b",
        ]
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
        let icon_offset = if icon_value(props).is_some() { 2 } else { 0 };
        let align_width = width.saturating_sub(icon_offset);
        let align_pad = (align_width.saturating_sub(text_len) as f32 * resolve_h_align(props))
            .round()
            .max(0.0) as u16;
        let start = col + icon_offset + align_pad;
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

    fn begin_gesture(
        &self,
        node: &LayoutNode,
        _local_col: f32,
        _local_row: f32,
        _modifiers: KeyModifiers,
    ) -> Option<Value> {
        node.props
            .get("on-release")
            .filter(|callback| !matches!(callback, Value::Nil | Value::Bool(false)))
            .map(|_| Value::String("button-release".to_string()))
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
            MouseEventKind::Down(MouseButton::Left)
                if node.props.get("on-press").is_some_and(|callback| {
                    !matches!(callback, Value::Nil | Value::Bool(false))
                }) =>
            {
                MouseEventOutcome::Dispatch(WidgetEvent::PointerDown(PointerEvent {
                    local_col: _local_col,
                    local_row: _local_row,
                }))
            }
            MouseEventKind::Down(MouseButton::Left) => {
                MouseEventOutcome::Dispatch(WidgetEvent::Activate(modifiers))
            }
            MouseEventKind::Up(MouseButton::Left)
                if node.props.get("on-release").is_some_and(|callback| {
                    !matches!(callback, Value::Nil | Value::Bool(false))
                }) =>
            {
                MouseEventOutcome::Dispatch(WidgetEvent::PointerUp(PointerEvent {
                    local_col: _local_col,
                    local_row: _local_row,
                }))
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
        let (callback_name, phase, modifiers) = match event {
            WidgetEvent::Activate(modifiers) => ("on-click", "click", modifiers),
            WidgetEvent::PointerDown(_) => ("on-press", "press", KeyModifiers::empty()),
            WidgetEvent::PointerUp(_) => ("on-release", "release", KeyModifiers::empty()),
            _ => return None,
        };
        let callback = node.props.get(callback_name)?.clone();
        Some(EventOutput {
            callback,
            args: vec![click_info(phase, modifiers)],
        })
    }

    fn fragment_shader(
        &self,
        widget_type: &str,
        backend: super::ShaderBackend,
    ) -> Option<&'static str> {
        if widget_type == "button-icon" {
            BUTTON_ICON_SHADER.source(backend)
        } else {
            BUTTON_SURFACE_SHADER.source(backend)
        }
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: super::WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let mut prims = Vec::new();
        let bg = variant_bg(&node.props);
        if bg.a > 0.0 {
            let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
            let px_w = node.rect.width * viewport.cell_w;
            let px_h = node.rect.height * viewport.cell_h;
            let corner_radius_px = get_f32_prop(&node.props, "corner-radius", 12.0).max(0.0);
            prims.push(GpuPrimitive::WidgetInstance {
                widget_type: "button".to_string(),
                instance: super::WidgetInstance {
                    ndc_min,
                    ndc_max,
                    value_t: 0.0,
                    orientation: 0.0,
                    itime: viewport.time_seconds,
                    uniform_a: [
                        if button_is_active_tab(&node.props) {
                            1.0
                        } else {
                            0.0
                        },
                        0.0,
                        0.0,
                        0.0,
                    ],
                    uniform_b: [0.0; 4],
                    uniform_c: [0.0; 4],
                    uniform_d: [0.0; 4],
                    color_a: bg.to_rgba(),
                    color_b: button_border(&node.props).to_rgba(),
                    color_c: button_highlight(&node.props).to_rgba(),
                    color_d: button_shadow(&node.props).to_rgba(),
                    corner_radius: normalized_corner_radius(node.rect, viewport, corner_radius_px),
                    pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                },
                is_background: true,
            });
        }

        if let Some(icon) = icon_value(&node.props) {
            let icon_rect = button_icon_rect(node.rect);
            let (ndc_min, ndc_max) = ndc_bounds(icon_rect, viewport);
            let px_w = icon_rect.width * viewport.cell_w;
            let px_h = icon_rect.height * viewport.cell_h;
            prims.push(GpuPrimitive::WidgetInstance {
                widget_type: "button-icon".to_string(),
                instance: super::WidgetInstance {
                    ndc_min,
                    ndc_max,
                    value_t: icon,
                    orientation: 0.0,
                    itime: viewport.time_seconds,
                    uniform_a: [0.0; 4],
                    uniform_b: [0.0; 4],
                    uniform_c: [0.0; 4],
                    uniform_d: [0.0; 4],
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
            let text_inset = if has_icon {
                button_icon_text_inset(node.rect)
            } else {
                0.0
            };
            let text_col = node.rect.col + text_inset;
            let align_width = if has_icon {
                (node.rect.width - text_inset).max(0.0)
            } else {
                node.rect.width
            };
            let mut text_props = node.props.clone();
            text_props
                .entry("v-align".to_string())
                .or_insert_with(|| Value::Keyword("center".to_string()));
            prims.push(GpuPrimitive::ProportionalText(
                GpuProportionalTextPrimitive {
                    row: label_text_row(&text_props, node.rect),
                    col: text_col,
                    align_width,
                    h_align: resolve_h_align(&node.props),
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

    fn test_viewport() -> super::super::WidgetViewport {
        super::super::WidgetViewport {
            vp_w: 100.0,
            vp_h: 100.0,
            cell_w: 10.0,
            cell_h: 10.0,
            scroll_top: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 10.0,
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
            animation: Default::default(),
        }
    }

    #[test]
    fn momentary_button_dispatches_press_and_release_callbacks() {
        let node = test_button_node(HashMap::from([
            ("on-press".to_string(), Value::Symbol("press".to_string())),
            (
                "on-release".to_string(),
                Value::Symbol("release".to_string()),
            ),
        ]));

        assert_eq!(
            ButtonWidget.begin_gesture(&node, 5.0, 3.0, KeyModifiers::empty()),
            Some(Value::String("button-release".to_string())),
            "release-capable buttons must retain the original press target"
        );

        let press = ButtonWidget.mouse_event(
            &node,
            MouseEventKind::Down(MouseButton::Left),
            5.0,
            3.0,
            None,
            None,
            KeyModifiers::empty(),
            1.0,
            1.0,
        );
        let MouseEventOutcome::Dispatch(press) = press else {
            panic!("press should dispatch");
        };
        let press = ButtonWidget
            .handle_event(&node, press)
            .expect("press callback");
        assert_eq!(press.callback, Value::Symbol("press".to_string()));
        assert!(matches!(
            press.args.first(),
            Some(Value::Map(info))
                if matches!(
                    info.get("phase").map(|value| value.borrow().clone()),
                    Some(Value::String(phase)) if phase == "press"
                )
        ));

        let release = ButtonWidget.mouse_event(
            &node,
            MouseEventKind::Up(MouseButton::Left),
            20.0,
            20.0,
            None,
            Some(&Value::String("button-release".to_string())),
            KeyModifiers::empty(),
            1.0,
            1.0,
        );
        let MouseEventOutcome::Dispatch(release) = release else {
            panic!("release should dispatch");
        };
        let release = ButtonWidget
            .handle_event(&node, release)
            .expect("release callback");
        assert_eq!(release.callback, Value::Symbol("release".to_string()));
        assert!(matches!(
            release.args.first(),
            Some(Value::Map(info))
                if matches!(
                    info.get("phase").map(|value| value.borrow().clone()),
                    Some(Value::String(phase)) if phase == "release"
                )
        ));
    }

    #[test]
    fn button_metal_centers_text_vertically() {
        let props = HashMap::from([
            ("text".to_string(), Value::String("Go".to_string())),
            ("variant".to_string(), Value::Keyword("primary".to_string())),
            ("font-size".to_string(), Value::Number(11.0)),
        ]);
        let node = test_button_node(props);
        let viewport = test_viewport();

        let prims = ButtonWidget.build_primitives("button", &node, viewport);
        let background_is_marked_background = prims.iter().any(|prim| {
            matches!(
                prim,
                GpuPrimitive::WidgetInstance {
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
                GpuPrimitive::ProportionalText(text) => Some(text),
                _ => None,
            })
            .expect("button should emit centered text");

        assert!((text.row - 3.0).abs() < 0.0001);
        assert!((text.col - node.rect.col).abs() < 0.0001);
        assert!((text.align_width - node.rect.width).abs() < 0.0001);
        assert!((text.h_align - 0.5).abs() < 0.0001);
    }

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
        let prims = ButtonWidget.build_primitives("button", &node, test_viewport());
        let instance = prims
            .iter()
            .find_map(|prim| match prim {
                GpuPrimitive::WidgetInstance {
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

    #[test]
    fn active_and_corner_radius_are_bindable_but_not_size_affecting() {
        assert_eq!(
            BUTTON_WIDGET.bindable_props(),
            &[
                "active",
                "corner-radius",
                "plock-active",
                "plock-color-r",
                "plock-color-g",
                "plock-color-b",
            ]
        );
        assert!(!BUTTON_WIDGET.size_affecting_props().contains(&"active"));
        assert!(!BUTTON_WIDGET.size_affecting_props().contains(&"corner-radius"));
    }

    #[test]
    fn source_list_icon_names_resolve_to_button_icons() {
        for icon in ["piano", "sliders", "note-arrow", "dial", "folder", "sine", "lfo"] {
            let props = HashMap::from([("icon".to_string(), Value::Keyword(icon.to_string()))]);
            assert!(icon_value(&props).is_some(), "{icon} should resolve");
        }
    }

    #[test]
    fn tab_shape_is_not_size_affecting_or_bindable() {
        assert!(!BUTTON_WIDGET.size_affecting_props().contains(&"shape"));
        assert!(!BUTTON_WIDGET.bindable_props().contains(&"shape"));
    }

    #[test]
    fn active_tab_button_sets_tab_shape_flag() {
        let node = test_button_node(HashMap::from([
            ("text".to_string(), Value::String("Mod 1".to_string())),
            ("shape".to_string(), Value::Keyword("tab".to_string())),
            ("active".to_string(), Value::Bool(true)),
        ]));

        let prims = ButtonWidget.build_primitives("button", &node, test_viewport());
        let instance = prims
            .iter()
            .find_map(|prim| match prim {
                GpuPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    is_background: true,
                } if widget_type == "button" => Some(instance),
                _ => None,
            })
            .expect("button background");

        assert_eq!(instance.uniform_a[0], 1.0);
    }

    #[test]
    fn inactive_tab_button_uses_normal_shape_flag() {
        let node = test_button_node(HashMap::from([
            ("text".to_string(), Value::String("Mod 1".to_string())),
            ("shape".to_string(), Value::Keyword("tab".to_string())),
            ("active".to_string(), Value::Bool(false)),
        ]));

        let prims = ButtonWidget.build_primitives("button", &node, test_viewport());
        let instance = prims
            .iter()
            .find_map(|prim| match prim {
                GpuPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    is_background: true,
                } if widget_type == "button" => Some(instance),
                _ => None,
            })
            .expect("button background");

        assert_eq!(instance.uniform_a[0], 0.0);
    }

    #[test]
    fn button_metal_left_aligns_icon_text_when_requested() {
        let node = test_button_node(HashMap::from([
            ("text".to_string(), Value::String("Samples".to_string())),
            ("icon".to_string(), Value::Keyword("waveform".to_string())),
            ("h-align".to_string(), Value::Keyword("left".to_string())),
            ("font-size".to_string(), Value::Number(11.0)),
        ]));

        let prims = ButtonWidget.build_primitives("button", &node, test_viewport());
        let text = prims
            .iter()
            .find_map(|prim| match prim {
                GpuPrimitive::ProportionalText(text) => Some(text),
                _ => None,
            })
            .expect("button should emit text");

        assert_eq!(text.h_align, 0.0);
        let icon_rect = button_icon_rect(node.rect);
        assert!(
            text.col >= icon_rect.col + icon_rect.width + 0.35,
            "icon text should start after the icon plus a visible gap: icon={icon_rect:?}, text_col={}",
            text.col
        );
    }

    #[test]
    fn button_metal_respects_custom_corner_radius_and_keeps_existing_default() {
        let background = (
            "background-color".to_string(),
            color_value(1.0, 1.0, 1.0, 0.07),
        );
        let custom_node = test_button_node(HashMap::from([
            ("text".to_string(), Value::String("Custom".to_string())),
            background.clone(),
            ("corner-radius".to_string(), Value::Number(4.0)),
        ]));
        let default_node = test_button_node(HashMap::from([
            ("text".to_string(), Value::String("Default".to_string())),
            background.clone(),
        ]));
        let pill_node = test_button_node(HashMap::from([
            ("text".to_string(), Value::String("Pill".to_string())),
            background.clone(),
            ("corner-radius".to_string(), Value::Number(15.0)),
        ]));
        let oversized_node = test_button_node(HashMap::from([
            ("text".to_string(), Value::String("Oversized".to_string())),
            background,
            ("corner-radius".to_string(), Value::Number(100.0)),
        ]));

        let corner_radius = |node: &LayoutNode| {
            ButtonWidget
                .build_primitives("button", node, test_viewport())
                .into_iter()
                .find_map(|prim| match prim {
                    GpuPrimitive::WidgetInstance {
                        widget_type,
                        instance,
                        is_background: true,
                    } if widget_type == "button" => Some(instance.corner_radius),
                    _ => None,
                })
                .expect("button background")
        };

        assert!((corner_radius(&custom_node) - (8.0 / 30.0)).abs() < 0.0001);
        assert!((corner_radius(&default_node) - 0.8).abs() < 0.0001);
        assert!((corner_radius(&pill_node) - 1.0).abs() < 0.0001);
        assert!((corner_radius(&oversized_node) - 1.0).abs() < 0.0001);
    }

    #[test]
    fn button_metal_resolves_reactive_corner_radius_at_draw_time() {
        let slots = crate::reactive::ReactiveBindingStore::default();
        let slot = slots.slot("BUTTON_TEST", "corner-radius");
        slots.write_float("BUTTON_TEST", "corner-radius", 4.0);
        let reactive_radius = Value::ReactiveRef {
            namespace: "BUTTON_TEST".to_string(),
            field: "corner-radius".to_string(),
            index: None,
            kind: crate::vm::BindingKind::Float,
            slot,
        };
        let widget = crate::widgets::build_widget(
            "button",
            vec![
                Value::String("Reactive".to_string()),
                Value::Keyword("corner-radius".to_string()),
                reactive_radius.clone(),
            ],
        );
        let Value::Map(widget) = widget else {
            panic!("expected button widget map");
        };
        assert!(!widget.contains_key("__widget-diagnostic"));

        let node = test_button_node(HashMap::from([
            ("text".to_string(), Value::String("Reactive".to_string())),
            ("corner-radius".to_string(), reactive_radius),
        ]));
        let render_radius = || {
            ButtonWidget
                .build_primitives("button", &node, test_viewport())
                .into_iter()
                .find_map(|prim| match prim {
                    GpuPrimitive::WidgetInstance {
                        widget_type,
                        instance,
                        is_background: true,
                    } if widget_type == "button" => Some(instance.corner_radius),
                    _ => None,
                })
                .expect("button background")
        };

        assert!((render_radius() - (8.0 / 30.0)).abs() < 0.0001);
        slots.write_float("BUTTON_TEST", "corner-radius", 2.0);
        assert!((render_radius() - (4.0 / 30.0)).abs() < 0.0001);
    }

    #[test]
    fn button_metal_resolves_reactive_active_colors_at_draw_time() {
        let slots = crate::reactive::ReactiveBindingStore::default();
        let slot = slots.slot("BUTTON_TEST", "active");
        slots.write_float("BUTTON_TEST", "active", 0.0);
        let node = test_button_node(HashMap::from([
            ("text".to_string(), Value::String("Loop".to_string())),
            (
                "active".to_string(),
                Value::ReactiveRef {
                    namespace: "BUTTON_TEST".to_string(),
                    field: "active".to_string(),
                    index: None,
                    kind: crate::vm::BindingKind::Float,
                    slot,
                },
            ),
            (
                "background-color".to_string(),
                color_value(0.1, 0.2, 0.3, 1.0),
            ),
            (
                "active-background-color".to_string(),
                color_value(0.0, 0.48, 0.95, 1.0),
            ),
            ("color".to_string(), Value::Keyword("dim".to_string())),
            (
                "active-color".to_string(),
                Value::Keyword("white".to_string()),
            ),
        ]));

        let inactive_prims = ButtonWidget.build_primitives("button", &node, test_viewport());
        let inactive_bg = inactive_prims
            .iter()
            .find_map(|prim| match prim {
                GpuPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    is_background: true,
                } if widget_type == "button" => Some(instance.color_a),
                _ => None,
            })
            .expect("button background");
        assert_eq!(inactive_bg, [0.1, 0.2, 0.3, 1.0]);

        slots.write_float("BUTTON_TEST", "active", 1.0);
        let active_prims = ButtonWidget.build_primitives("button", &node, test_viewport());
        let active_bg = active_prims
            .iter()
            .find_map(|prim| match prim {
                GpuPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    is_background: true,
                } if widget_type == "button" => Some(instance.color_a),
                _ => None,
            })
            .expect("button background");
        assert_eq!(active_bg, [0.0, 0.48, 0.95, 1.0]);
    }
}
