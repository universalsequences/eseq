use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetCursor, WidgetDefinition, WidgetEvent,
    styled_cell,
};
#[cfg(target_os = "macos")]
use super::{
    MetalCirclePrimitive, MetalCircleVisibleHalf, MetalLiveSpectrogramPrimitive, MetalPrimitive,
    MetalRectPrimitive, WidgetViewport, resolve_named_color,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
#[cfg(target_os = "macos")]
use crate::theme;
use crate::vm::Value;

pub struct Eq8EditorWidget;

pub static EQ8_EDITOR_WIDGET: Eq8EditorWidget = Eq8EditorWidget;

const DEFAULT_WIDTH: f32 = 32.0;
const DEFAULT_HEIGHT: f32 = 8.0;
const DEFAULT_FREQ_MIN: f32 = 20.0;
const DEFAULT_FREQ_MAX: f32 = 20_000.0;
const DEFAULT_GAIN_MIN: f32 = -24.0;
const DEFAULT_GAIN_MAX: f32 = 24.0;
const DEFAULT_Q_MIN: f32 = 0.1;
const DEFAULT_Q_MAX: f32 = 18.0;
const DEFAULT_SAMPLE_RATE: f32 = 48_000.0;
const RESPONSE_POINTS: usize = 128;

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
struct Eq8Band {
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

#[derive(Clone, Copy, Debug)]
struct BiquadCoefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl BiquadCoefficients {
    const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn magnitude_at(self, frequency: f32, sample_rate: f32) -> f32 {
        let w = std::f32::consts::TAU * frequency / sample_rate.max(1.0);
        let cos_w = w.cos();
        let cos_2w = (2.0 * w).cos();
        let sin_w = w.sin();
        let sin_2w = (2.0 * w).sin();
        let num_real = self.b0 + self.b1 * cos_w + self.b2 * cos_2w;
        let num_imag = -(self.b1 * sin_w + self.b2 * sin_2w);
        let den_real = 1.0 + self.a1 * cos_w + self.a2 * cos_2w;
        let den_imag = -(self.a1 * sin_w + self.a2 * sin_2w);
        let num_mag = (num_real * num_real + num_imag * num_imag).sqrt();
        let den_mag = (den_real * den_real + den_imag * den_imag).sqrt();
        num_mag / den_mag.max(1.0e-10)
    }
}

fn value_string(value: &Value) -> Option<&str> {
    match value {
        Value::String(s) | Value::Keyword(s) => Some(s),
        _ => None,
    }
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
        Some(Value::Number(v)) => *v > 0.5,
        Some(Value::ReactiveRef { slot, .. }) => crate::reactive::read_float_slot(slot) > 0.5,
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

fn prop_bands(props: &HashMap<String, Value>) -> Vec<Eq8Band> {
    let freq_min = super::get_f32_prop(props, "freq-min", DEFAULT_FREQ_MIN).max(1.0);
    let freq_max = super::get_f32_prop(props, "freq-max", DEFAULT_FREQ_MAX).max(freq_min + 1.0);
    let gain_min = super::get_f32_prop(props, "gain-min", DEFAULT_GAIN_MIN);
    let gain_max = super::get_f32_prop(props, "gain-max", DEFAULT_GAIN_MAX).max(gain_min + 0.001);
    let q_min = super::get_f32_prop(props, "q-min", DEFAULT_Q_MIN).max(0.001);
    let q_max = super::get_f32_prop(props, "q-max", DEFAULT_Q_MAX).max(q_min + 0.001);
    let selected_band = super::get_f32_prop(props, "selected-band", -1.0).round() as i32;

    let Some(Value::List(items)) = props.get("bands") else {
        return Vec::new();
    };

    items
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| {
            let map = value_map(&item.borrow())?;
            let id = map_num(&map, "id", idx as f32).round() as i32;
            let band_type = map_string(&map, "type", "bell");
            let freq_min = map_num(&map, "freq-min", freq_min).max(1.0);
            let freq_max = map_num(&map, "freq-max", freq_max).max(freq_min + 1.0);
            let gain_min = map_num(&map, "gain-min", gain_min);
            let gain_max = map_num(&map, "gain-max", gain_max).max(gain_min + 0.001);
            let q_min = map_num(&map, "q-min", q_min).max(0.001);
            let q_max = map_num(&map, "q-max", q_max).max(q_min + 0.001);
            Some(Eq8Band {
                id,
                band_type,
                freq: map_num(&map, "freq", 1_000.0).clamp(freq_min, freq_max),
                freq_min,
                freq_max,
                gain: map_num(&map, "gain", 0.0).clamp(gain_min, gain_max),
                gain_min,
                gain_max,
                q: map_num(&map, "q", 1.0).clamp(q_min, q_max),
                q_min,
                q_max,
                enabled: map_bool(&map, "enabled", true),
                selected: map_bool(&map, "selected", id == selected_band),
            })
        })
        .take(8)
        .collect()
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

fn linear_to_t(value: f32, min: f32, max: f32) -> f32 {
    ((value - min) / (max - min).max(0.001)).clamp(0.0, 1.0)
}

fn linear_from_t(t: f32, min: f32, max: f32) -> f32 {
    min + t.clamp(0.0, 1.0) * (max - min).max(0.001)
}

fn data_from_local(rect: Rect, col: f32, row: f32) -> (f32, f32) {
    let x_t = ((col - rect.col) / rect.width.max(0.0001)).clamp(0.0, 1.0);
    let y_t = (1.0 - ((row - rect.row) / rect.height.max(0.0001))).clamp(0.0, 1.0);
    (x_t, y_t)
}

fn plot_point(rect: Rect, x_t: f32, y_t: f32) -> [f32; 2] {
    [
        rect.col + x_t.clamp(0.0, 1.0) * rect.width,
        rect.row + (1.0 - y_t.clamp(0.0, 1.0)) * rect.height,
    ]
}

fn band_y_t(band: &Eq8Band) -> f32 {
    linear_to_t(band.gain, band.gain_min, band.gain_max)
}

fn nearest_band(node: &LayoutNode, col: f32, row: f32) -> Option<i32> {
    let bands = prop_bands(&node.props);
    let mut best = None;
    let mut best_dist = f32::MAX;
    for band in bands.iter() {
        let x_t = freq_to_t(band.freq, band.freq_min, band.freq_max);
        let y_t = band_y_t(band);
        let [px, py] = plot_point(node.rect, x_t, y_t);
        let dist = ((px - col).powi(2) + (py - row).powi(2)).sqrt();
        if dist < best_dist {
            best_dist = dist;
            best = Some(band.id);
        }
    }
    if best_dist <= 1.35 { best } else { None }
}

fn selected_or_first_band(node: &LayoutNode, gesture: Option<&Value>) -> Option<Eq8Band> {
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

fn changed_band_values(
    node: &LayoutNode,
    band: &Eq8Band,
    col: f32,
    row: f32,
    modifiers: KeyModifiers,
) -> (f32, f32, f32) {
    let (x_t, y_t) = data_from_local(node.rect, col, row);
    let freq = freq_from_t(x_t, band.freq_min, band.freq_max);
    if modifiers.intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL | KeyModifiers::ALT) {
        let q = linear_from_t(y_t, band.q_min, band.q_max).clamp(band.q_min, band.q_max);
        (freq, band.gain, q)
    } else {
        let gain =
            linear_from_t(y_t, band.gain_min, band.gain_max).clamp(band.gain_min, band.gain_max);
        (freq, gain, band.q)
    }
}

fn map_entry(key: &str, value: Value) -> (String, Rc<RefCell<Value>>) {
    (key.to_string(), Rc::new(RefCell::new(value)))
}

fn action_map(action_type: &str, band: &Eq8Band, freq: f32, gain: f32, q: f32) -> Value {
    Value::Map(HashMap::from([
        map_entry("type", Value::Keyword(action_type.to_string())),
        map_entry("id", Value::Number(band.id as f64)),
        map_entry("band-type", Value::String(band.band_type.clone())),
        map_entry("enabled", Value::Bool(band.enabled)),
        map_entry("freq", Value::Number(freq as f64)),
        map_entry("gain", Value::Number(gain as f64)),
        map_entry("q", Value::Number(q as f64)),
    ]))
}

fn select_action(band: &Eq8Band) -> Value {
    action_map("select-band", band, band.freq, band.gain, band.q)
}

fn toggle_action(band: &Eq8Band) -> Value {
    let mut value = action_map("toggle-band", band, band.freq, band.gain, band.q);
    if let Value::Map(map) = &mut value {
        map.insert(
            "enabled".to_string(),
            Rc::new(RefCell::new(Value::Bool(!band.enabled))),
        );
    }
    value
}

fn meaningful_change(band: &Eq8Band, freq: f32, gain: f32, q: f32) -> bool {
    let freq_ratio = (freq.max(1.0) / band.freq.max(1.0)).max(band.freq.max(1.0) / freq.max(1.0));
    freq_ratio > 1.004 || (gain - band.gain).abs() >= 0.02 || (q - band.q).abs() >= 0.015
}

fn change_action(
    action_type: &str,
    node: &LayoutNode,
    band: &Eq8Band,
    col: f32,
    row: f32,
    modifiers: KeyModifiers,
) -> Option<Value> {
    let (freq, gain, q) = changed_band_values(node, band, col, row, modifiers);
    if !meaningful_change(band, freq, gain, q) {
        return None;
    }
    Some(action_map(action_type, band, freq, gain, q))
}

fn calculate_coefficients(band: &Eq8Band, sample_rate: f32) -> BiquadCoefficients {
    if !band.enabled || band.gain.abs() <= 0.000_001 {
        return BiquadCoefficients::IDENTITY;
    }
    let sr = sample_rate.max(1.0);
    let freq = band.freq.clamp(20.0, 20_000.0_f32.min(sr * 0.45));
    let gain = band.gain.clamp(-24.0, 24.0);
    let q = band.q.clamp(0.1, 18.0);
    let a = 10.0_f32.powf(gain / 40.0);
    let w0 = std::f32::consts::TAU * freq / sr;
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let alpha = sin_w0 / (2.0 * q);

    let (b0, b1, b2, a0, a1, a2) = match band.band_type.as_str() {
        "lowshelf" | "low-shelf" => {
            let sqrt_a = a.sqrt();
            (
                a * ((a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0),
                a * ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha),
                (a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0),
                (a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha,
            )
        }
        "highshelf" | "high-shelf" => {
            let sqrt_a = a.sqrt();
            (
                a * ((a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0),
                a * ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha),
                (a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha,
                2.0 * ((a - 1.0) - (a + 1.0) * cos_w0),
                (a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha,
            )
        }
        _ => (
            1.0 + alpha * a,
            -2.0 * cos_w0,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos_w0,
            1.0 - alpha / a,
        ),
    };
    if a0.abs() <= 1.0e-12 || !a0.is_finite() {
        return BiquadCoefficients::IDENTITY;
    }
    let coeffs = BiquadCoefficients {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    };
    if coeffs.b0.is_finite()
        && coeffs.b1.is_finite()
        && coeffs.b2.is_finite()
        && coeffs.a1.is_finite()
        && coeffs.a2.is_finite()
    {
        coeffs
    } else {
        BiquadCoefficients::IDENTITY
    }
}

fn combined_response_db(bands: &[Eq8Band], frequency: f32, sample_rate: f32) -> f32 {
    let magnitude = bands
        .iter()
        .map(|band| calculate_coefficients(band, sample_rate).magnitude_at(frequency, sample_rate))
        .product::<f32>()
        .max(1.0e-9);
    20.0 * magnitude.log10()
}

fn response_y_t(db: f32, min_db: f32, max_db: f32) -> f32 {
    ((db - min_db) / (max_db - min_db).max(0.001)).clamp(0.0, 1.0)
}

#[cfg(target_os = "macos")]
fn color_with_alpha(color: Color, alpha: f32) -> Color {
    let [r, g, b, a] = color.to_rgba();
    Color::rgba(r, g, b, (a * alpha).clamp(0.0, 1.0))
}

impl WidgetDefinition for Eq8EditorWidget {
    fn names(&self) -> &'static [&'static str] {
        &["eq8-editor"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &[
            "bands",
            "selected-band",
            "fft-size",
            "time-slices",
            "min-db",
            "max-db",
            "smoothing",
            "freq-min",
            "freq-max",
        ]
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
            .unwrap_or(DEFAULT_WIDTH)
            .clamp(8.0, constraints.max_width.max(8.0));
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(DEFAULT_HEIGHT)
            .clamp(3.0, constraints.max_height.max(3.0));
        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let label = prop_bands(props)
            .into_iter()
            .find(|band| band.selected)
            .map(|band| format!("EQ8 {} {:.0}Hz {:+.1}dB", band.id + 1, band.freq, band.gain))
            .unwrap_or_else(|| "EQ8".to_string());
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16;
        for (idx, ch) in label.chars().enumerate() {
            if idx >= rect.width.round() as usize {
                break;
            }
            buf.set(
                row,
                col_start + idx as u16,
                styled_cell(ch, crate::theme::FG_MUTED(), None),
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

    fn begin_gesture(&self, node: &LayoutNode, local_col: f32, local_row: f32) -> Option<Value> {
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
        modifiers: KeyModifiers,
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
            MouseEventKind::Down(MouseButton::Right) => {
                if let Some(id) = nearest_band(node, local_col, local_row) {
                    if let Some(band) = prop_bands(&node.props)
                        .into_iter()
                        .find(|band| band.id == id)
                    {
                        return MouseEventOutcome::Dispatch(WidgetEvent::Custom(toggle_action(
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
                let (freq, gain, q) =
                    changed_band_values(node, &band, local_col, local_row, modifiers);
                set_live_band(
                    node.widget_id,
                    LiveBandState {
                        band_id: band.id,
                        freq,
                        gain,
                        q,
                    },
                );
                change_action("change-band", node, &band, local_col, local_row, modifiers)
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
                            changed_band_values(node, &band, local_col, local_row, modifiers);
                        action_map("commit-band", &band, freq, gain, q)
                    });
                clear_live_band(node.widget_id);
                MouseEventOutcome::Dispatch(WidgetEvent::Custom(event))
            }
            _ => MouseEventOutcome::Consume,
        }
    }

    fn double_click_event(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
    ) -> Option<WidgetEvent> {
        let id = nearest_band(node, local_col, local_row)?;
        let band = prop_bands(&node.props)
            .into_iter()
            .find(|band| band.id == id)?;
        Some(WidgetEvent::Custom(toggle_action(&band)))
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
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        _viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let mut bands = prop_bands(&node.props);
        if let Some(live) = live_band(node.widget_id) {
            for band in &mut bands {
                if band.id == live.band_id {
                    band.freq = live.freq.clamp(band.freq_min, band.freq_max);
                    band.gain = live.gain.clamp(band.gain_min, band.gain_max);
                    band.q = live.q.clamp(band.q_min, band.q_max);
                }
            }
        }

        let background = resolve_named_color(
            &node.props,
            "background-color",
            Color::rgba(0.045, 0.048, 0.052, 1.0),
        );
        let spectrum_fill = resolve_named_color(
            &node.props,
            "spectrum-color",
            Color::rgba(0.12, 0.58, 0.62, 0.30),
        );
        let spectrum_peak = resolve_named_color(
            &node.props,
            "spectrum-peak-color",
            Color::rgba(0.42, 0.95, 0.88, 0.74),
        );
        let spectrum = super::spectrogram::request_from_props(&node.props);
        let (min_hz, max_hz) =
            super::spectrogram::display_hz_range(&node.props, DEFAULT_FREQ_MIN, DEFAULT_FREQ_MAX);
        let mut primitives = vec![
            MetalPrimitive::Rect(MetalRectPrimitive {
                rect: node.rect,
                color: background,
            }),
            MetalPrimitive::LiveSpectrogram(MetalLiveSpectrogramPrimitive {
                rect: node.rect,
                data_key: spectrum.data_key,
                mode: 1,
                freq_scale: 0,
                min_hz,
                max_hz,
                min_color: Color::rgba(0.03, 0.035, 0.04, 1.0),
                mid_color: spectrum_fill,
                max_color: spectrum_peak,
                eq_line_color: spectrum_peak,
                eq_fill_color: spectrum_fill,
                background_color: background,
            }),
        ];

        let curve_color = resolve_named_color(
            &node.props,
            "curve-color",
            Color::rgba(1.0, 0.58, 0.18, 1.0),
        );
        let inactive_color = resolve_named_color(
            &node.props,
            "inactive-band-color",
            Color::rgba(0.50, 0.52, 0.54, 0.72),
        );
        let selected_color = resolve_named_color(&node.props, "selected-color", theme::YELLOW());
        let grid_color = resolve_named_color(
            &node.props,
            "grid-color",
            Color::rgba(0.42, 0.43, 0.45, 0.34),
        );
        let sample_rate = super::get_f32_prop(&node.props, "sample-rate", DEFAULT_SAMPLE_RATE);
        let min_db = super::get_f32_prop(&node.props, "response-min-db", DEFAULT_GAIN_MIN);
        let max_db =
            super::get_f32_prop(&node.props, "response-max-db", DEFAULT_GAIN_MAX).max(min_db + 1.0);

        for db in [-12.0, 0.0, 12.0] {
            let y_t = response_y_t(db, min_db, max_db);
            for idx in 0..80 {
                let x_t = idx as f32 / 79.0;
                primitives.push(MetalPrimitive::Circle(MetalCirclePrimitive {
                    center: plot_point(node.rect, x_t, y_t),
                    radius_px: if db == 0.0 { 0.95 } else { 0.7 },
                    color: grid_color,
                    visible_half: MetalCircleVisibleHalf::Full,
                }));
            }
        }

        for freq in [100.0, 1_000.0, 10_000.0] {
            let x_t = freq_to_t(freq, DEFAULT_FREQ_MIN, DEFAULT_FREQ_MAX);
            for idx in 0..32 {
                let y_t = idx as f32 / 31.0;
                primitives.push(MetalPrimitive::Circle(MetalCirclePrimitive {
                    center: plot_point(node.rect, x_t, y_t),
                    radius_px: 0.7,
                    color: grid_color,
                    visible_half: MetalCircleVisibleHalf::Full,
                }));
            }
        }

        for idx in 0..RESPONSE_POINTS {
            let x_t = idx as f32 / (RESPONSE_POINTS - 1) as f32;
            let freq = freq_from_t(x_t, DEFAULT_FREQ_MIN, DEFAULT_FREQ_MAX);
            let db = combined_response_db(&bands, freq, sample_rate).clamp(min_db, max_db);
            let y_t = response_y_t(db, min_db, max_db);
            primitives.push(MetalPrimitive::Circle(MetalCirclePrimitive {
                center: plot_point(node.rect, x_t, y_t),
                radius_px: 1.45,
                color: curve_color,
                visible_half: MetalCircleVisibleHalf::Full,
            }));
        }

        for band in &bands {
            let x_t = freq_to_t(band.freq, band.freq_min, band.freq_max);
            let y_t = band_y_t(band);
            let center = plot_point(node.rect, x_t, y_t);
            let color = if !band.enabled {
                inactive_color
            } else if band.selected {
                selected_color
            } else {
                curve_color
            };
            primitives.push(MetalPrimitive::Circle(MetalCirclePrimitive {
                center,
                radius_px: if band.selected { 12.0 } else { 10.0 },
                color,
                visible_half: MetalCircleVisibleHalf::Full,
            }));
            if band.enabled && !band.selected {
                primitives.push(MetalPrimitive::Circle(MetalCirclePrimitive {
                    center,
                    radius_px: 5.4,
                    color: color_with_alpha(background, 0.95),
                    visible_half: MetalCircleVisibleHalf::Full,
                }));
            }
        }

        primitives
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rc(value: Value) -> Rc<RefCell<Value>> {
        Rc::new(RefCell::new(value))
    }

    fn map(entries: Vec<(&str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), rc(value)))
                .collect(),
        )
    }

    fn band(id: i32, selected: bool) -> Rc<RefCell<Value>> {
        rc(map(vec![
            ("id", Value::Number(id as f64)),
            ("type", Value::Keyword("bell".to_string())),
            ("freq", Value::Number(1_000.0)),
            ("gain", Value::Number(0.0)),
            ("q", Value::Number(1.0)),
            ("enabled", Value::Bool(true)),
            ("selected", Value::Bool(selected)),
        ]))
    }

    fn layout_node() -> LayoutNode {
        LayoutNode {
            widget_id: 42,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "eq8-editor".to_string(),
            rect: Rect {
                row: 1.0,
                col: 2.0,
                width: 30.0,
                height: 8.0,
            },
            props: HashMap::from([(
                "bands".to_string(),
                Value::List(vec![band(0, true), band(1, false)]),
            )]),
            children: Vec::new(),
            focusable: false,
        }
    }

    fn constraints() -> Constraints {
        Constraints {
            min_width: 0.0,
            max_width: 80.0,
            min_height: 0.0,
            max_height: 24.0,
            aspect: 1.0,
        }
    }

    fn measure_ctx() -> MeasureCtx<'static> {
        MeasureCtx {
            text_measurer: None,
            cell_w: 10.0,
            cell_h: 20.0,
            inherited_font_size: 12.0,
        }
    }

    fn event_field<'a>(value: &'a Value, field: &str) -> Option<Value> {
        let Value::Map(map) = value else {
            return None;
        };
        map.get(field).map(|value| value.borrow().clone())
    }

    #[test]
    fn measures_to_finite_nonzero_rect() {
        let node = map(vec![
            ("width", Value::Number(32.0)),
            ("height", Value::Number(8.0)),
        ]);
        let mut measure_child = |_value: &Value, _constraints: Constraints| None;
        let size = EQ8_EDITOR_WIDGET
            .measure(
                &node,
                &[],
                constraints(),
                &measure_ctx(),
                &mut measure_child,
            )
            .unwrap();
        assert_eq!(size.width, 32.0);
        assert_eq!(size.height, 8.0);
    }

    #[test]
    fn accepts_reactive_contract_props() {
        assert!(EQ8_EDITOR_WIDGET.bindable_props().contains(&"bands"));
        assert!(
            EQ8_EDITOR_WIDGET
                .bindable_props()
                .contains(&"selected-band")
        );
        assert!(EQ8_EDITOR_WIDGET.bindable_props().contains(&"fft-size"));
        assert!(EQ8_EDITOR_WIDGET.bindable_props().contains(&"smoothing"));
        assert!(EQ8_EDITOR_WIDGET.bindable_props().contains(&"freq-min"));
        assert!(EQ8_EDITOR_WIDGET.bindable_props().contains(&"freq-max"));
    }

    #[test]
    fn drag_emits_change_band_with_band_values() {
        let node = layout_node();
        let outcome = EQ8_EDITOR_WIDGET.mouse_event(
            &node,
            MouseEventKind::Drag(MouseButton::Left),
            node.rect.col + node.rect.width * 0.6,
            node.rect.row + node.rect.height * 0.25,
            None,
            Some(&Value::Number(0.0)),
            KeyModifiers::NONE,
            10.0,
            20.0,
        );
        let MouseEventOutcome::Dispatch(WidgetEvent::Custom(value)) = outcome else {
            panic!("expected change-band dispatch");
        };
        assert_eq!(
            event_field(&value, "type"),
            Some(Value::Keyword("change-band".to_string()))
        );
        assert_eq!(event_field(&value, "id"), Some(Value::Number(0.0)));
        let Some(Value::Number(freq)) = event_field(&value, "freq") else {
            panic!("missing freq");
        };
        assert!(freq > 1_000.0);
        let Some(Value::Number(gain)) = event_field(&value, "gain") else {
            panic!("missing gain");
        };
        assert!(gain > 0.0);
    }

    #[test]
    fn modifier_drag_emits_q_change() {
        let node = layout_node();
        let outcome = EQ8_EDITOR_WIDGET.mouse_event(
            &node,
            MouseEventKind::Drag(MouseButton::Left),
            node.rect.col + node.rect.width * 0.5,
            node.rect.row + node.rect.height * 0.1,
            None,
            Some(&Value::Number(0.0)),
            KeyModifiers::SUPER,
            10.0,
            20.0,
        );
        let MouseEventOutcome::Dispatch(WidgetEvent::Custom(value)) = outcome else {
            panic!("expected change-band dispatch");
        };
        let Some(Value::Number(q)) = event_field(&value, "q") else {
            panic!("missing q");
        };
        assert!(q > 1.0);
        assert_eq!(event_field(&value, "gain"), Some(Value::Number(0.0)));
    }

    #[test]
    fn double_click_toggles_nearest_band() {
        let node = layout_node();
        let x = node.rect.col
            + node.rect.width * freq_to_t(1_000.0, DEFAULT_FREQ_MIN, DEFAULT_FREQ_MAX);
        let y = node.rect.row + node.rect.height * 0.5;
        let Some(WidgetEvent::Custom(value)) = EQ8_EDITOR_WIDGET.double_click_event(&node, x, y)
        else {
            panic!("expected toggle event");
        };
        assert_eq!(
            event_field(&value, "type"),
            Some(Value::Keyword("toggle-band".to_string()))
        );
        assert_eq!(event_field(&value, "enabled"), Some(Value::Bool(false)));
    }

    #[test]
    fn combined_response_uses_enabled_bands() {
        let bands = prop_bands(&layout_node().props);
        let flat = combined_response_db(&bands, 1_000.0, DEFAULT_SAMPLE_RATE);
        assert!(flat.abs() < 0.001);
        let mut boosted = bands.clone();
        boosted[0].gain = 6.0;
        let db = combined_response_db(&boosted, 1_000.0, DEFAULT_SAMPLE_RATE);
        assert!(db > 5.5);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn emits_spectrum_and_response_primitives() {
        let viewport = WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 800.0,
            vp_h: 600.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 24.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        };
        let primitives =
            EQ8_EDITOR_WIDGET.build_metal_primitives("eq8-editor", &layout_node(), viewport);
        let spectrogram = primitives.iter().find_map(|primitive| match primitive {
            MetalPrimitive::LiveSpectrogram(spectrogram) => Some(spectrogram),
            _ => None,
        });
        let spectrogram = spectrogram.expect("eq8 editor should emit a live spectrum primitive");
        assert!(spectrogram.eq_line_color.a > 0.0);
        assert!(spectrogram.eq_fill_color.a > 0.0);
        assert!(
            primitives
                .iter()
                .filter(|primitive| matches!(primitive, MetalPrimitive::Circle(_)))
                .count()
                >= RESPONSE_POINTS
        );
    }
}
