use std::collections::HashMap;
use std::{cell::RefCell, rc::Rc};

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MetalPrimitive, MouseEventOutcome, WidgetCursor, WidgetDefinition,
    WidgetEvent, WidgetInstance, WidgetViewport, ndc_bounds, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct ResponseCurveEditorWidget;

pub static RESPONSE_CURVE_EDITOR_WIDGET: ResponseCurveEditorWidget = ResponseCurveEditorWidget;

const PAD_X: f32 = 0.0;
const PAD_Y: f32 = 0.0;

#[derive(Clone, Copy, Debug)]
struct LiveBandState {
    band_id: i32,
    freq: f32,
    gain: f32,
    q: f32,
}

thread_local! {
    static LIVE_BANDS: RefCell<HashMap<u64, LiveBandState>> = RefCell::new(HashMap::new());
}

fn set_live_band(widget_id: u64, state: LiveBandState) {
    LIVE_BANDS.with(|states| {
        states.borrow_mut().insert(widget_id, state);
    });
    super::bump_widget_state_generation();
}

fn clear_live_band(widget_id: u64) {
    let removed = LIVE_BANDS.with(|states| states.borrow_mut().remove(&widget_id).is_some());
    if removed {
        super::bump_widget_state_generation();
    }
}

fn live_band(widget_id: u64) -> Option<LiveBandState> {
    LIVE_BANDS.with(|states| states.borrow().get(&widget_id).copied())
}

#[derive(Clone, Debug)]
struct ResponseBand {
    id: i32,
    band_type: String,
    freq: f32,
    freq_min: f32,
    freq_max: f32,
    gain: f32,
    gain_min: f32,
    gain_max: f32,
    q: f32,
    q_min: f32,
    q_max: f32,
    enabled: bool,
    selected: bool,
}

fn value_string(value: &Value) -> Option<&str> {
    match value {
        Value::String(s) | Value::Keyword(s) => Some(s),
        _ => None,
    }
}

fn prop_string(props: &HashMap<String, Value>, key: &str, default: &str) -> String {
    props
        .get(key)
        .and_then(value_string)
        .unwrap_or(default)
        .to_string()
}

fn map_num(map: &HashMap<String, Value>, key: &str, default: f32) -> f32 {
    match map.get(key) {
        Some(Value::Number(n)) => *n as f32,
        Some(Value::ReactiveRef { slot, .. }) => crate::reactive::read_float_slot(slot) as f32,
        _ => default,
    }
}

fn map_bool(map: &HashMap<String, Value>, key: &str, default: bool) -> bool {
    match map.get(key) {
        Some(Value::Bool(v)) => *v,
        _ => default,
    }
}

fn map_string(map: &HashMap<String, Value>, key: &str, default: &str) -> String {
    map.get(key)
        .and_then(value_string)
        .unwrap_or(default)
        .to_string()
}

fn value_map(value: &Value) -> Option<HashMap<String, Value>> {
    match value {
        Value::Map(map) => Some(
            map.iter()
                .map(|(key, value)| (key.clone(), value.borrow().clone()))
                .collect(),
        ),
        _ => None,
    }
}

fn prop_bands(props: &HashMap<String, Value>) -> Vec<ResponseBand> {
    let freq_min = super::get_f32_prop(props, "freq-min", 20.0).max(1.0);
    let freq_max = super::get_f32_prop(props, "freq-max", 20_000.0).max(freq_min + 1.0);
    let gain_min = super::get_f32_prop(props, "gain-min", -12.0);
    let gain_max = super::get_f32_prop(props, "gain-max", 12.0).max(gain_min + 0.001);
    let q_min = super::get_f32_prop(props, "q-min", 0.5).max(0.001);
    let q_max = super::get_f32_prop(props, "q-max", 10.0).max(q_min + 0.001);

    let Some(Value::List(items)) = props.get("bands") else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            let map = value_map(&item.borrow())?;
            let band_freq_min = map_num(&map, "freq-min", freq_min).max(1.0);
            let band_freq_max = map_num(&map, "freq-max", freq_max).max(band_freq_min + 1.0);
            let band_gain_min = map_num(&map, "gain-min", gain_min);
            let band_gain_max = map_num(&map, "gain-max", gain_max).max(band_gain_min + 0.001);
            let band_q_min = map_num(&map, "q-min", q_min).max(0.001);
            let band_q_max = map_num(&map, "q-max", q_max).max(band_q_min + 0.001);
            Some(ResponseBand {
                id: map_num(&map, "id", 0.0).round() as i32,
                band_type: map_string(&map, "type", "bell"),
                freq: map_num(&map, "freq", 1_000.0).clamp(band_freq_min, band_freq_max),
                freq_min: band_freq_min,
                freq_max: band_freq_max,
                gain: map_num(&map, "gain", 0.0).clamp(band_gain_min, band_gain_max),
                gain_min: band_gain_min,
                gain_max: band_gain_max,
                q: map_num(&map, "q", 1.0).clamp(band_q_min, band_q_max),
                q_min: band_q_min,
                q_max: band_q_max,
                enabled: map_bool(&map, "enabled", true),
                selected: map_bool(&map, "selected", false),
            })
        })
        .collect()
}

fn band_type_code(band_type: &str) -> f32 {
    match band_type {
        "lowpass" | "lowcut" => 0.0,
        "highpass" | "highcut" => 1.0,
        "bandpass" | "notch" => 2.0,
        "passband" => 6.0,
        "lowshelf" => 3.0,
        "highshelf" => 4.0,
        _ => 5.0,
    }
}

#[cfg(target_os = "macos")]
fn normalized_corner_radius(rect: Rect, viewport: WidgetViewport, radius_px: f32) -> f32 {
    if radius_px <= 0.0 {
        return 0.0;
    }
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((radius_px * 2.0) / px_h).clamp(0.0, 0.5)
}

fn freq_to_t(freq: f32, min: f32, max: f32) -> f32 {
    let min = min.max(1.0);
    let max = max.max(min + 1.0);
    ((freq.max(min).ln() - min.ln()) / (max.ln() - min.ln())).clamp(0.0, 1.0)
}

fn freq_from_t(t: f32, min: f32, max: f32) -> f32 {
    let min = min.max(1.0);
    let max = max.max(min + 1.0);
    (min.ln() + t.clamp(0.0, 1.0) * (max.ln() - min.ln())).exp()
}

fn gain_to_t(gain: f32, min: f32, max: f32) -> f32 {
    ((gain - min) / (max - min).max(0.001)).clamp(0.0, 1.0)
}

fn q_to_t(q: f32, min: f32, max: f32) -> f32 {
    ((q - min) / (max - min).max(0.001)).clamp(0.0, 1.0)
}

fn q_from_t(t: f32, min: f32, max: f32) -> f32 {
    min + t.clamp(0.0, 1.0) * (max - min).max(0.001)
}

fn gain_from_t(t: f32, min: f32, max: f32) -> f32 {
    min + t.clamp(0.0, 1.0) * (max - min).max(0.001)
}

fn data_from_local(rect: Rect, col: f32, row: f32) -> (f32, f32) {
    let u = ((col - rect.col) / rect.width.max(0.0001)).clamp(0.0, 1.0);
    let v = ((row - rect.row) / rect.height.max(0.0001)).clamp(0.0, 1.0);
    (
        ((u - PAD_X) / (1.0 - PAD_X * 2.0)).clamp(0.0, 1.0),
        (1.0 - ((v - PAD_Y) / (1.0 - PAD_Y * 2.0))).clamp(0.0, 1.0),
    )
}

fn plot_point(rect: Rect, x_t: f32, y_t: f32) -> (f32, f32) {
    (
        rect.col + (PAD_X + x_t * (1.0 - PAD_X * 2.0)) * rect.width,
        rect.row + (PAD_Y + (1.0 - y_t) * (1.0 - PAD_Y * 2.0)) * rect.height,
    )
}

fn is_filter_mode(props: &HashMap<String, Value>) -> bool {
    matches!(prop_string(props, "mode", "eq").as_str(), "filter")
}

fn band_y_t(band: &ResponseBand, filter_mode: bool) -> f32 {
    if filter_mode {
        q_to_t(band.q, band.q_min, band.q_max)
    } else {
        gain_to_t(band.gain, band.gain_min, band.gain_max)
    }
}

fn nearest_band(node: &LayoutNode, col: f32, row: f32) -> Option<i32> {
    let filter_mode = is_filter_mode(&node.props);
    let bands = prop_bands(&node.props);
    let mut best = None;
    let mut best_dist = f32::MAX;
    for band in bands.iter().filter(|band| band.enabled) {
        let x_t = freq_to_t(band.freq, band.freq_min, band.freq_max);
        let y_t = band_y_t(band, filter_mode);
        let (px, py) = plot_point(node.rect, x_t, y_t);
        let dist = ((px - col).powi(2) + (py - row).powi(2)).sqrt();
        if dist < best_dist {
            best_dist = dist;
            best = Some(band.id);
        }
    }
    if best_dist <= 1.25 { best } else { None }
}

fn selected_or_first_band(node: &LayoutNode, gesture: Option<&Value>) -> Option<ResponseBand> {
    let bands = prop_bands(&node.props);
    if let Some(Value::Number(id)) = gesture {
        let id = *id as i32;
        if let Some(band) = bands.iter().find(|band| band.id == id) {
            return Some(band.clone());
        }
    }
    bands
        .iter()
        .find(|band| band.selected)
        .cloned()
        .or_else(|| bands.first().cloned())
}

fn map_entry(key: &str, value: Value) -> (String, Rc<RefCell<Value>>) {
    (key.to_string(), Rc::new(RefCell::new(value)))
}

fn action_map(action_type: &str, band: &ResponseBand, freq: f32, gain: f32, q: f32) -> Value {
    Value::Map(HashMap::from([
        map_entry("type", Value::Keyword(action_type.to_string())),
        map_entry("id", Value::Number(band.id as f64)),
        map_entry("band-type", Value::String(band.band_type.clone())),
        map_entry("freq", Value::Number(freq as f64)),
        map_entry("gain", Value::Number(gain as f64)),
        map_entry("q", Value::Number(q as f64)),
    ]))
}

fn meaningful_change(band: &ResponseBand, freq: f32, gain: f32, q: f32, filter_mode: bool) -> bool {
    let freq_ratio = (freq.max(1.0) / band.freq.max(1.0)).max(band.freq.max(1.0) / freq.max(1.0));
    if freq_ratio > 1.004 {
        return true;
    }
    if filter_mode {
        (q - band.q).abs() >= 0.015
    } else {
        (gain - band.gain).abs() >= 0.02
    }
}

fn select_action(band: &ResponseBand) -> Value {
    action_map("select-band", band, band.freq, band.gain, band.q)
}

fn changed_band_values(
    node: &LayoutNode,
    band: &ResponseBand,
    col: f32,
    row: f32,
) -> (f32, f32, f32) {
    let (x_t, y_t) = data_from_local(node.rect, col, row);
    let freq = freq_from_t(x_t, band.freq_min, band.freq_max);
    let filter_mode = is_filter_mode(&node.props);
    let (gain, q) = if filter_mode {
        (
            band.gain,
            q_from_t(y_t, band.q_min, band.q_max).clamp(band.q_min, band.q_max),
        )
    } else {
        (
            gain_from_t(y_t, band.gain_min, band.gain_max).clamp(band.gain_min, band.gain_max),
            band.q,
        )
    };
    (freq, gain, q)
}

fn change_action(
    action_type: &str,
    node: &LayoutNode,
    band: &ResponseBand,
    col: f32,
    row: f32,
) -> Option<Value> {
    let (freq, gain, q) = changed_band_values(node, band, col, row);
    if !meaningful_change(band, freq, gain, q, is_filter_mode(&node.props)) {
        return None;
    }
    Some(action_map(action_type, band, freq, gain, q))
}

fn force_change_action(node: &LayoutNode, band: &ResponseBand, col: f32, row: f32) -> Value {
    let (freq, gain, q) = changed_band_values(node, band, col, row);
    action_map("change-band", band, freq, gain, q)
}

impl WidgetDefinition for ResponseCurveEditorWidget {
    fn names(&self) -> &'static [&'static str] {
        &["response-curve-editor"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["bands"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or(constraints.max_width)
            .clamp(8.0, constraints.max_width.max(8.0));
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(5.0)
            .max(2.5);
        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let bands = prop_bands(props);
        let label = if let Some(band) = bands
            .iter()
            .find(|band| band.selected)
            .or_else(|| bands.first())
        {
            format!("{} {:.0}Hz Q{:.2}", band.band_type, band.freq, band.q)
        } else {
            "response".to_string()
        };
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

    fn cursor(&self, node: &LayoutNode, local_col: f32, local_row: f32) -> WidgetCursor {
        if nearest_band(node, local_col, local_row).is_some() {
            WidgetCursor::EwResize
        } else {
            WidgetCursor::Default
        }
    }

    fn begin_gesture(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
        _modifiers: KeyModifiers,
    ) -> Option<Value> {
        let band_id = nearest_band(node, local_col, local_row)
            .or_else(|| selected_or_first_band(node, None).map(|band| band.id))?;
        Some(Value::Number(band_id as f64))
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
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(id) = nearest_band(node, local_col, local_row) {
                    if let Some(band) = prop_bands(&node.props)
                        .into_iter()
                        .find(|band| band.id == id)
                    {
                        return MouseEventOutcome::Dispatch(WidgetEvent::Custom(select_action(
                            &band,
                        )));
                    }
                }
                MouseEventOutcome::Consume
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(band) = selected_or_first_band(node, gesture) else {
                    return MouseEventOutcome::Consume;
                };
                let (freq, gain, q) = changed_band_values(node, &band, local_col, local_row);
                set_live_band(
                    node.widget_id,
                    LiveBandState {
                        band_id: band.id,
                        freq,
                        gain,
                        q,
                    },
                );
                change_action("change-band", node, &band, local_col, local_row)
                    .map(|event| MouseEventOutcome::Dispatch(WidgetEvent::Custom(event)))
                    .unwrap_or(MouseEventOutcome::Consume)
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(band) = selected_or_first_band(node, gesture) else {
                    clear_live_band(node.widget_id);
                    return MouseEventOutcome::Consume;
                };
                let event = live_band(node.widget_id)
                    .filter(|state| state.band_id == band.id)
                    .map(|state| action_map("commit-band", &band, state.freq, state.gain, state.q))
                    .unwrap_or_else(|| {
                        let (freq, gain, q) =
                            changed_band_values(node, &band, local_col, local_row);
                        action_map("commit-band", &band, freq, gain, q)
                    });
                clear_live_band(node.widget_id);
                MouseEventOutcome::Dispatch(WidgetEvent::Custom(event))
            }
            _ => MouseEventOutcome::Consume,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(value) = event else {
            return None;
        };
        let callback = node.props.get("on-action")?.clone();
        Some(EventOutput {
            callback,
            args: vec![value],
        })
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(RESPONSE_CURVE_EDITOR_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let live = live_band(node.widget_id);
        let bands: Vec<ResponseBand> = prop_bands(&node.props)
            .into_iter()
            .map(|mut band| {
                if let Some(state) = live {
                    if state.band_id == band.id {
                        band.freq = state.freq.clamp(band.freq_min, band.freq_max);
                        band.gain = state.gain.clamp(band.gain_min, band.gain_max);
                        band.q = state.q.clamp(band.q_min, band.q_max);
                    }
                }
                band
            })
            .collect();
        let filter_mode = if is_filter_mode(&node.props) {
            1.0
        } else {
            0.0
        };
        let curve_color = if node.props.contains_key("stroke-color") {
            resolve_named_color(&node.props, "stroke-color", theme::BLUE())
        } else {
            resolve_named_color(&node.props, "curve-color", theme::BLUE())
        };
        let bg_color = resolve_named_color(
            &node.props,
            "background-color",
            Color::rgba(0.055, 0.058, 0.06, 1.0),
        );
        let grid_color = resolve_named_color(
            &node.props,
            "grid-color",
            Color::rgba(0.36, 0.36, 0.38, 0.55),
        );
        let point_color = resolve_named_color(
            &node.props,
            "point-color",
            Color::rgba(1.0, 0.62, 0.25, 1.0),
        );

        let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
        let px_w = node.rect.width * viewport.cell_w;
        let px_h = node.rect.height * viewport.cell_h;
        let pixel_aspect = if px_h > 0.0 { px_w / px_h } else { 1.0 };
        let corner_radius = normalized_corner_radius(
            node.rect,
            viewport,
            super::get_f32_prop(&node.props, "corner-radius", 0.0),
        );
        let total = bands.len().max(1) as f32;
        let mut primitives = Vec::new();

        for (idx, band) in bands.iter().enumerate() {
            if !band.enabled {
                continue;
            }
            primitives.push(MetalPrimitive::WidgetInstance {
                widget_type: widget_type.to_string(),
                instance: WidgetInstance {
                    ndc_min,
                    ndc_max,
                    value_t: if band.selected { 1.0 } else { 0.0 },
                    orientation: filter_mode,
                    itime: viewport.time_seconds,
                    uniform_a: [
                        band_type_code(&band.band_type),
                        freq_to_t(band.freq, band.freq_min, band.freq_max),
                        band_y_t(band, filter_mode > 0.5),
                        q_to_t(band.q, band.q_min, band.q_max),
                    ],
                    uniform_b: [idx as f32, total, band.gain, filter_mode],
                    uniform_c: [0.0; 4],
                    uniform_d: [0.0; 4],
                    color_a: curve_color.to_rgba(),
                    color_b: bg_color.to_rgba(),
                    color_c: grid_color.to_rgba(),
                    color_d: point_color.to_rgba(),
                    corner_radius,
                    pixel_aspect,
                },
                is_background: false,
            });
        }

        if primitives.is_empty() {
            primitives.push(MetalPrimitive::WidgetInstance {
                widget_type: widget_type.to_string(),
                instance: WidgetInstance {
                    ndc_min,
                    ndc_max,
                    value_t: 0.0,
                    orientation: filter_mode,
                    itime: viewport.time_seconds,
                    uniform_a: [5.0, 0.5, 0.5, 0.0],
                    uniform_b: [0.0, 1.0, 0.0, filter_mode],
                    uniform_c: [0.0; 4],
                    uniform_d: [0.0; 4],
                    color_a: curve_color.to_rgba(),
                    color_b: bg_color.to_rgba(),
                    color_c: grid_color.to_rgba(),
                    color_d: point_color.to_rgba(),
                    corner_radius,
                    pixel_aspect,
                },
                is_background: false,
            });
        }

        primitives
    }
}

#[cfg(target_os = "macos")]
const RESPONSE_CURVE_EDITOR_SHADER: &str = r#"
float rce_sdSegment(float2 p, float2 a, float2 b) {
    float2 pa = p - a;
    float2 ba = b - a;
    float h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

float2 rce_plot(float2 data) {
    float2 pad = float2(0.0, 0.0);
    return float2(
        pad.x + data.x * (1.0 - pad.x * 2.0),
        pad.y + (1.0 - data.y) * (1.0 - pad.y * 2.0));
}

float rce_curveY(float bandType, float x, float freqT, float yT, float qT, float isFilter) {
    float dist = x - freqT;
    float q = mix(0.22, 0.045, qT);
    if (isFilter > 0.5) {
        float resonance = qT * 0.92;
        if (bandType > 5.5 && bandType < 6.5) {
            float octaveSpan = 9.97;
            float widthOctaves = mix(0.25, 6.0, qT);
            float halfWidth = (widthOctaves * 0.5) / octaveSpan;
            float lowEdge = freqT - halfWidth;
            float highEdge = freqT + halfWidth;
            float hpOctaves = (x - lowEdge) * octaveSpan;
            float lpOctaves = (highEdge - x) * octaveSpan;
            float hp = 1.0 / sqrt(1.0 + pow(2.0, -hpOctaves * 3.6));
            float lp = 1.0 / sqrt(1.0 + pow(2.0, -lpOctaves * 3.6));
            float pass = clamp(hp * lp * 1.14, 0.0, 1.0);
            float eased = smoothstep(0.0, 1.0, pass);
            return -0.95 + eased * 1.45;
        }
        if (bandType < 0.5) {
            float slopeStart = freqT - q * mix(1.10, 0.0, qT);
            float slopeEnd = freqT + q * mix(2.85, 1.42, qT);
            float slope = smoothstep(slopeStart, slopeEnd, x);
            float peakCenter = freqT;
            float peakDist = x - peakCenter;
            float peakWidth = max(q * q * mix(0.65, 0.16, qT), 0.00004);
            float bump = resonance * exp(-peakDist * peakDist / peakWidth);
            return 0.50 + bump - slope * 1.45;
        }
        if (bandType < 1.5) {
            float slopeStart = freqT - q * mix(2.85, 1.42, qT);
            float slopeEnd = freqT + q * mix(1.10, 0.0, qT);
            float slope = smoothstep(slopeStart, slopeEnd, x);
            float peakCenter = freqT;
            float peakDist = x - peakCenter;
            float peakWidth = max(q * q * mix(0.65, 0.16, qT), 0.00004);
            float bump = resonance * exp(-peakDist * peakDist / peakWidth);
            return -0.95 + slope * 1.45 + bump;
        }
        float peak = exp(-dist * dist / max(q * q, 0.0001));
        return -0.42 + peak * (0.86 + qT * 0.52);
    }

    if (bandType > 2.5 && bandType < 3.5) {
        float shelf = 1.0 - smoothstep(freqT - q, freqT + q, x);
        return mix(0.5, yT, shelf);
    }
    if (bandType > 3.5 && bandType < 4.5) {
        float shelf = smoothstep(freqT - q, freqT + q, x);
        return mix(0.5, yT, shelf);
    }
    float peak = exp(-dist * dist / max(q * q, 0.0001));
    return mix(0.5, yT, peak);
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = max(in.aspect, 0.0001);
    float bandType = in.uniform_a.x;
    float freqT = clamp(in.uniform_a.y, 0.0, 1.0);
    float yT = clamp(in.uniform_a.z, 0.0, 1.0);
    float qT = clamp(in.uniform_a.w, 0.0, 1.0);
    float bandIndex = in.uniform_b.x;
    float isFilter = in.uniform_b.w;

    float4 col = float4(0.0);
    float clipMask = 1.0;
    if (in.corner_radius > 0.0) {
        float r = min(in.corner_radius, min(aspect, 1.0));
        float2 p = float2((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);
        float2 halfSize = float2(aspect - r, 1.0 - r);
        float2 qr = abs(p) - halfSize;
        float d = length(max(qr, 0.0)) + min(max(qr.x, qr.y), 0.0) - r;
        float edge = max(fwidth(d) * 1.2, 0.001);
        clipMask = smoothstep(edge, -edge, d);
        if (clipMask < 0.002) { discard_fragment(); }
    }

    if (bandIndex < 0.5) {
        col = in.color_b;
        float grid = 0.0;
        float majorXs[4] = {0.285, 0.50, 0.715, 0.93};
        for (int i = 0; i < 4; i++) {
            float d = abs(uv.x - majorXs[i]);
            grid = max(grid, 1.0 - smoothstep(0.0015, 0.004, d));
        }
        for (int i = 1; i < 4; i++) {
            float y = float(i) / 4.0;
            float d = abs(uv.y - y);
            grid = max(grid, 0.75 * (1.0 - smoothstep(0.0015, 0.004, d)));
        }
        col.rgb = mix(col.rgb, in.color_c.rgb, grid * in.color_c.a);
    }

    float lineMask = 0.0;
    const int steps = 96;
    float prevX = 0.0;
    float prevY = rce_curveY(bandType, 0.0, freqT, yT, qT, isFilter);
    for (int i = 1; i <= steps; i++) {
        float x = float(i) / float(steps);
        float y = rce_curveY(bandType, x, freqT, yT, qT, isFilter);
        float2 a = rce_plot(float2(prevX, prevY));
        float2 b = rce_plot(float2(x, y));
        float d = rce_sdSegment(float2(uv.x * aspect, uv.y), float2(a.x * aspect, a.y), float2(b.x * aspect, b.y));
        float aa = max(fwidth(d), 0.001);
        lineMask = max(lineMask, smoothstep(0.006 + aa, 0.002, d));
        prevX = x;
        prevY = y;
    }

    float2 handle = rce_plot(float2(freqT, yT));
    float hd = length(float2((uv.x - handle.x) * aspect, uv.y - handle.y));
    float selected = in.value_t;
    float handleOuter = smoothstep(0.068, 0.052, hd);
    float handleInner = smoothstep(0.048, 0.032, hd);

    col.rgb = mix(col.rgb, in.color_a.rgb, lineMask * in.color_a.a);
    col.a = max(col.a, lineMask * in.color_a.a);
    col.rgb = mix(col.rgb, in.color_d.rgb, handleOuter * in.color_d.a);
    if (selected < 0.5) {
        col.rgb = mix(col.rgb, in.color_b.rgb, handleInner);
    }
    col.a = max(col.a, handleOuter);
    col.a *= clipMask;
    return col;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn band_value(id: i32, freq: f32, gain: f32, q: f32, selected: bool) -> Value {
        Value::Map(HashMap::from([
            map_entry("id", Value::Number(id as f64)),
            map_entry("type", Value::String("bell".to_string())),
            map_entry("freq", Value::Number(freq as f64)),
            map_entry("gain", Value::Number(gain as f64)),
            map_entry("q", Value::Number(q as f64)),
            map_entry("selected", Value::Bool(selected)),
        ]))
    }

    fn test_node(mode: &str) -> LayoutNode {
        LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "response-curve-editor".to_string(),
            rect: Rect {
                col: 0.0,
                row: 0.0,
                width: 40.0,
                height: 8.0,
            },
            props: HashMap::from([
                ("mode".to_string(), Value::Keyword(mode.to_string())),
                ("freq-min".to_string(), Value::Number(20.0)),
                ("freq-max".to_string(), Value::Number(20_000.0)),
                ("gain-min".to_string(), Value::Number(-12.0)),
                ("gain-max".to_string(), Value::Number(12.0)),
                ("q-min".to_string(), Value::Number(0.5)),
                ("q-max".to_string(), Value::Number(10.0)),
                (
                    "bands".to_string(),
                    Value::List(vec![Rc::new(RefCell::new(band_value(
                        2, 1_000.0, 0.0, 1.0, true,
                    )))]),
                ),
                (
                    "on-action".to_string(),
                    Value::Symbol("handler".to_string()),
                ),
            ]),
            children: Vec::new(),
            focusable: false,
        }
    }

    fn map_number(value: &Value, key: &str) -> f32 {
        let Value::Map(map) = value else {
            panic!("expected map");
        };
        match &*map.get(key).expect("missing key").borrow() {
            Value::Number(n) => *n as f32,
            other => panic!("expected number for {key}, got {other:?}"),
        }
    }

    fn map_keyword(value: &Value, key: &str) -> String {
        let Value::Map(map) = value else {
            panic!("expected map");
        };
        match &*map.get(key).expect("missing key").borrow() {
            Value::Keyword(s) => s.clone(),
            other => panic!("expected keyword for {key}, got {other:?}"),
        }
    }

    #[test]
    fn frequency_mapping_round_trips_log_space() {
        for freq in [20.0, 100.0, 1_000.0, 20_000.0] {
            let t = freq_to_t(freq, 20.0, 20_000.0);
            let roundtrip = freq_from_t(t, 20.0, 20_000.0);
            let rel = (roundtrip - freq).abs() / freq;
            assert!(rel < 0.0001, "freq={freq} roundtrip={roundtrip}");
        }
    }

    #[test]
    fn passband_has_dedicated_shader_code() {
        assert_eq!(band_type_code("passband"), 6.0);
    }

    #[test]
    fn eq_drag_emits_change_band_with_gain() {
        let node = test_node("eq");
        let event = force_change_action(&node, &prop_bands(&node.props)[0], 20.0, 2.0);
        assert_eq!(map_keyword(&event, "type"), "change-band");
        assert_eq!(map_number(&event, "id"), 2.0);
        assert!(map_number(&event, "freq") > 500.0);
        assert!(map_number(&event, "gain") > 0.0);
        assert_eq!(map_number(&event, "q"), 1.0);
    }

    #[test]
    fn filter_drag_emits_change_band_with_q() {
        let node = test_node("filter");
        let event = force_change_action(&node, &prop_bands(&node.props)[0], 20.0, 2.0);
        assert_eq!(map_keyword(&event, "type"), "change-band");
        assert_eq!(map_number(&event, "gain"), 0.0);
        assert!(map_number(&event, "q") > 1.0);
    }
}
