use std::collections::HashMap;
use std::{cell::RefCell, rc::Rc};

use super::{
    CellBuffer, EventOutput, MetalPrimitive, MouseEventOutcome, WidgetDefinition, WidgetEvent,
    WidgetInstance, WidgetViewport, ndc_bounds, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

pub struct AdsrEditorWidget;

pub static ADSR_EDITOR_WIDGET: AdsrEditorWidget = AdsrEditorWidget;

fn clamp_measured_axis(requested: f32, min: f32, max: f32) -> f32 {
    if max.is_finite() {
        let upper = max.max(0.0);
        let lower = min.min(upper);
        requested.clamp(lower, upper)
    } else {
        requested.max(min)
    }
}

fn prop_ms(props: &HashMap<String, Value>, key: &str, default: f32) -> f32 {
    super::get_f32_prop(props, key, default)
        .clamp(0.0, 120_000.0)
        .max(0.0)
}

fn prop_unit(props: &HashMap<String, Value>, key: &str, default: f32) -> f32 {
    super::get_f32_prop(props, key, default).clamp(0.0, 1.0)
}

fn adsr_log_weight(ms: f32) -> f32 {
    (1.0 + (ms / 20.0)).ln().max(0.018)
}

fn adsr_ms_from_norm(norm: f32, max_ms: f32) -> f32 {
    let max_weight = adsr_log_weight(max_ms);
    20.0 * ((norm.clamp(0.0, 1.0) * max_weight).exp() - 1.0)
}

fn adsr_x_positions(props: &HashMap<String, Value>) -> (f32, f32, f32, f32) {
    let attack = prop_ms(props, "attack", 4.0);
    let decay = prop_ms(props, "decay", 400.0);
    let release = prop_ms(props, "release", 300.0);
    let release_start = 0.68;
    let attack_max = super::get_f32_prop(props, "attack-max", 5000.0).max(1.0);
    let decay_max = super::get_f32_prop(props, "decay-max", 5000.0).max(1.0);
    let release_max = super::get_f32_prop(props, "release-max", 10000.0).max(1.0);
    let attack_norm = (adsr_log_weight(attack) / adsr_log_weight(attack_max)).clamp(0.0, 1.0);
    let decay_norm = (adsr_log_weight(decay) / adsr_log_weight(decay_max)).clamp(0.0, 1.0);
    let release_norm = (adsr_log_weight(release) / adsr_log_weight(release_max)).clamp(0.0, 1.0);
    let x1 = release_start * 0.42 * attack_norm;
    let x2 = x1 + (release_start - x1) * decay_norm;
    let x3 = release_start;
    let x4 = x3 + (1.0 - x3) * release_norm;
    (x1, x2, x3, x4)
}

fn plot_point(data_x: f32, data_y: f32, rect: Rect) -> (f32, f32) {
    let pad_x = 0.055;
    let pad_y = 0.12;
    (
        rect.col + (pad_x + data_x * (1.0 - pad_x * 2.0)) * rect.width,
        rect.row + (pad_y + (1.0 - data_y) * (1.0 - pad_y * 2.0)) * rect.height,
    )
}

fn data_from_local(rect: Rect, col: f32, row: f32) -> (f32, f32) {
    let pad_x = 0.055;
    let pad_y = 0.12;
    let u = ((col - rect.col) / rect.width.max(0.0001)).clamp(0.0, 1.0);
    let v = ((row - rect.row) / rect.height.max(0.0001)).clamp(0.0, 1.0);
    (
        ((u - pad_x) / (1.0 - pad_x * 2.0)).clamp(0.0, 1.0),
        (1.0 - ((v - pad_y) / (1.0 - pad_y * 2.0))).clamp(0.0, 1.0),
    )
}

fn nearest_handle(node: &LayoutNode, col: f32, row: f32) -> i32 {
    let (x1, x2, x3, x4) = adsr_x_positions(&node.props);
    let sustain = prop_unit(&node.props, "sustain", 0.5);
    let points = [
        plot_point(0.0, 0.0, node.rect),
        plot_point(x1, 1.0, node.rect),
        plot_point(x2, sustain, node.rect),
        plot_point(x3, sustain, node.rect),
        plot_point(x4, 0.0, node.rect),
    ];
    let mut best_idx = 0;
    let mut best_dist = f32::MAX;
    for (idx, (px, py)) in points.iter().enumerate() {
        let dist = ((*px - col).powi(2) + (*py - row).powi(2)).sqrt();
        if dist < best_dist {
            best_dist = dist;
            best_idx = idx as i32;
        }
    }
    if best_dist <= 1.0 { best_idx } else { -1 }
}

fn env_map(attack: f32, decay: f32, sustain: f32, release: f32) -> Value {
    Value::Map(HashMap::from([
        (
            "attack".to_string(),
            Rc::new(RefCell::new(Value::Number(attack as f64))),
        ),
        (
            "decay".to_string(),
            Rc::new(RefCell::new(Value::Number(decay as f64))),
        ),
        (
            "sustain".to_string(),
            Rc::new(RefCell::new(Value::Number(sustain as f64))),
        ),
        (
            "release".to_string(),
            Rc::new(RefCell::new(Value::Number(release as f64))),
        ),
    ]))
}

fn event_for_drag(node: &LayoutNode, handle_idx: i32, col: f32, row: f32) -> Value {
    let attack_max = super::get_f32_prop(&node.props, "attack-max", 5000.0).max(1.0);
    let decay_max = super::get_f32_prop(&node.props, "decay-max", 5000.0).max(1.0);
    let release_max = super::get_f32_prop(&node.props, "release-max", 10000.0).max(1.0);
    let release_start = 0.68;
    let (data_x, data_y) = data_from_local(node.rect, col, row);
    let (x1, _x2, x3, _x4) = adsr_x_positions(&node.props);

    let mut attack = prop_ms(&node.props, "attack", 4.0);
    let mut decay = prop_ms(&node.props, "decay", 400.0);
    let mut sustain = prop_unit(&node.props, "sustain", 0.5);
    let mut release = prop_ms(&node.props, "release", 300.0);

    match handle_idx {
        1 => {
            let norm = data_x / (release_start * 0.42);
            attack = adsr_ms_from_norm(norm, attack_max);
        }
        2 => {
            let norm = (data_x - x1) / (release_start - x1).max(0.0001);
            decay = adsr_ms_from_norm(norm, decay_max);
            sustain = data_y;
        }
        3 => {
            sustain = data_y;
        }
        4 => {
            let norm = (data_x - x3) / (1.0 - x3);
            release = adsr_ms_from_norm(norm, release_max);
        }
        _ => {}
    }

    env_map(attack, decay, sustain, release)
}

impl WidgetDefinition for AdsrEditorWidget {
    fn names(&self) -> &'static [&'static str] {
        &["adsr-editor"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["attack", "decay", "sustain", "release"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let requested_width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or(constraints.max_width);
        let requested_height = get_prop_num(node, "height").map(f64_to_f32).unwrap_or(5.0);
        let width = clamp_measured_axis(requested_width, 4.0, constraints.max_width);
        let height = clamp_measured_axis(requested_height, 2.0, constraints.max_height);
        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let attack = prop_ms(props, "attack", 4.0);
        let decay = prop_ms(props, "decay", 400.0);
        let sustain = prop_unit(props, "sustain", 0.5);
        let release = prop_ms(props, "release", 300.0);
        let label = format!("A{attack:.0} D{decay:.0} S{sustain:.2} R{release:.0}");
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16;
        for (i, ch) in label.chars().enumerate() {
            if i >= rect.width.round() as usize {
                break;
            }
            buf.set(
                row,
                col_start + i as u16,
                styled_cell(ch, theme::FG_MUTED(), None),
            );
        }
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn begin_gesture(&self, node: &LayoutNode, local_col: f32, local_row: f32) -> Option<Value> {
        Some(Value::Number(
            nearest_handle(node, local_col, local_row) as f64
        ))
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
        let handle_idx = match gesture {
            Some(Value::Number(n)) => *n as i32,
            _ => nearest_handle(node, local_col, local_row),
        };
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => MouseEventOutcome::Consume,
            MouseEventKind::Drag(MouseButton::Left) if handle_idx > 0 => {
                MouseEventOutcome::Dispatch(WidgetEvent::Custom(event_for_drag(
                    node, handle_idx, local_col, local_row,
                )))
            }
            _ => MouseEventOutcome::Consume,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(value) = event else {
            return None;
        };
        let callback = node.props.get("on-change")?.clone();
        Some(EventOutput {
            callback,
            args: vec![value],
        })
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(ADSR_EDITOR_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let attack = prop_ms(&node.props, "attack", 4.0);
        let decay = prop_ms(&node.props, "decay", 400.0);
        let sustain = prop_unit(&node.props, "sustain", 0.5);
        let release = prop_ms(&node.props, "release", 300.0);
        let hold = super::get_f32_prop(&node.props, "hold", 0.35).clamp(0.05, 2.0);

        let curve_color =
            resolve_named_color(&node.props, "curve-color", theme::WIDGET_KNOB_FILLED());
        let bg_color = resolve_named_color(
            &node.props,
            "background-color",
            Color::rgba(0.07, 0.07, 0.07, 1.0),
        );
        let grid_color = resolve_named_color(
            &node.props,
            "grid-color",
            Color::rgba(0.30, 0.30, 0.30, 0.55),
        );
        let point_color = resolve_named_color(
            &node.props,
            "point-color",
            Color::rgba(1.0, 0.55, 0.16, 1.0),
        );

        let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
        let px_w = node.rect.width * viewport.cell_w;
        let px_h = node.rect.height * viewport.cell_h;
        vec![MetalPrimitive::WidgetInstance {
            widget_type: widget_type.to_string(),
            instance: WidgetInstance {
                ndc_min,
                ndc_max,
                value_t: 0.0,
                orientation: 0.0,
                itime: viewport.time_seconds,
                uniform_a: [attack, decay, sustain, release],
                uniform_b: [hold, 0.0, 0.0, 0.0],
                uniform_c: [0.0; 4],
                uniform_d: [0.0; 4],
                color_a: curve_color.to_rgba(),
                color_b: bg_color.to_rgba(),
                color_c: grid_color.to_rgba(),
                color_d: point_color.to_rgba(),
                corner_radius: 0.0,
                pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
            },
            is_background: false,
        }]
    }
}

#[cfg(target_os = "macos")]
const ADSR_EDITOR_SHADER: &str = r#"
float adsr_sdSegment(float2 p, float2 a, float2 b) {
    float2 pa = p - a;
    float2 ba = b - a;
    float h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

float adsr_logWeight(float ms) {
    return max(0.018, log(1.0 + (ms / 20.0)));
}

float2 adsr_toPlot(float2 data) {
    float2 pad = float2(0.055, 0.12);
    return float2(
        pad.x + data.x * (1.0 - pad.x * 2.0),
        pad.y + (1.0 - data.y) * (1.0 - pad.y * 2.0));
}

float2 adsr_point(int idx, float x1, float x2, float x3, float x4, float sustain) {
    if (idx == 0) return adsr_toPlot(float2(0.0, 0.0));
    if (idx == 1) return adsr_toPlot(float2(x1, 1.0));
    if (idx == 2) return adsr_toPlot(float2(x2, sustain));
    if (idx == 3) return adsr_toPlot(float2(x3, sustain));
    return adsr_toPlot(float2(x4, 0.0));
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float attack = in.uniform_a.x;
    float decay = in.uniform_a.y;
    float sustain = clamp(in.uniform_a.z, 0.0, 1.0);
    float release = in.uniform_a.w;
    float releaseStart = 0.68;
    float attackNorm = clamp(adsr_logWeight(attack) / adsr_logWeight(5000.0), 0.0, 1.0);
    float decayNorm = clamp(adsr_logWeight(decay) / adsr_logWeight(5000.0), 0.0, 1.0);
    float releaseNorm = clamp(adsr_logWeight(release) / adsr_logWeight(10000.0), 0.0, 1.0);
    float x1 = releaseStart * 0.42 * attackNorm;
    float x2 = x1 + (releaseStart - x1) * decayNorm;
    float x3 = releaseStart;
    float x4 = mix(x3, 1.0, releaseNorm);

    float2 uv = in.uv;
    float4 col = in.color_b;

    float2 pad = float2(0.055, 0.12);
    float plotLeft = pad.x;
    float plotRight = 1.0 - pad.x;
    float plotTop = pad.y;
    float plotBottom = 1.0 - pad.y;
    float insidePlot = step(plotLeft, uv.x) * step(uv.x, plotRight)
        * step(plotTop, uv.y) * step(uv.y, plotBottom);

    float gridW = max(fwidth(uv.y), 0.001);
    float gridMid = adsr_toPlot(float2(0.0, 0.5)).y;
    float gridZero = adsr_toPlot(float2(0.0, 0.0)).y;
    float gridOne = adsr_toPlot(float2(0.0, 1.0)).y;
    float grid = 0.0;
    grid = max(grid, 1.0 - smoothstep(0.0, gridW * 1.4, abs(uv.y - gridMid)));
    grid = max(grid, 1.0 - smoothstep(0.0, gridW * 1.4, abs(uv.y - gridZero)));
    grid = max(grid, 1.0 - smoothstep(0.0, gridW * 1.4, abs(uv.y - gridOne)));
    col.rgb = mix(col.rgb, in.color_c.rgb, grid * in.color_c.a * insidePlot);

    float2 aspectScale = float2(max(in.aspect, 1e-5), 1.0);
    float2 p = uv * aspectScale;
    float minDist = 1000.0;
    for (int i = 1; i < 5; ++i) {
        float2 a = adsr_point(i - 1, x1, x2, x3, x4, sustain) * aspectScale;
        float2 b = adsr_point(i, x1, x2, x3, x4, sustain) * aspectScale;
        minDist = min(minDist, adsr_sdSegment(p, a, b));
    }
    float pixel = max(fwidth(uv.y), 0.001);
    float pixelDist = max(length(float2(fwidth(uv.x) * aspectScale.x, fwidth(uv.y))), 0.001);
    float stroke = pixelDist * 1.35;
    float aa = pixelDist * 0.45;
    float curve = smoothstep(stroke + aa, stroke - aa, minDist);
    col.rgb = mix(col.rgb, in.color_a.rgb, curve * in.color_a.a);

    float handleHalfPx = 4.0;
    float handleStrokePx = 1.25;
    for (int i = 0; i < 5; ++i) {
        float2 h = adsr_point(i, x1, x2, x3, x4, sustain);
        float2 pxDelta = float2((uv.x - h.x) / max(fwidth(uv.x), 0.001),
                                (uv.y - h.y) / pixel);
        float2 d = abs(pxDelta);
        float2 outerQ = d - float2(handleHalfPx);
        float outerDist = length(max(outerQ, float2(0.0))) + min(max(outerQ.x, outerQ.y), 0.0);
        float2 innerQ = d - float2(max(handleHalfPx - handleStrokePx, 0.0));
        float innerDist = length(max(innerQ, float2(0.0))) + min(max(innerQ.x, innerQ.y), 0.0);
        float outer = 1.0 - smoothstep(0.0, 0.75, outerDist);
        float inner = 1.0 - smoothstep(0.0, 0.75, innerDist);
        float square = max(outer - inner, 0.0);
        col.rgb = mix(col.rgb, in.color_d.rgb, square * in.color_d.a);
    }

    float border = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
    float borderMask = 1.0 - smoothstep(0.0, max(fwidth(border) * 1.5, 0.002), border);
    col.rgb = mix(col.rgb, in.color_c.rgb, borderMask * in.color_c.a * 0.55);

    return col;
}
"#;
