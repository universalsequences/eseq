use std::collections::HashMap;

use super::{CellBuffer, WidgetDefinition, get_f32_prop, resolve_named_color, styled_cell};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

use super::live_audio::{
    LiveAudioSourceSelector, TapPoint, prop_keyword, source_from_props, tap_point_from_props,
};
#[cfg(target_os = "macos")]
use super::{MetalLiveSpectrogramPrimitive, MetalPrimitive, MetalRectPrimitive, WidgetViewport};

pub struct SpectrogramWidget;

pub static SPECTROGRAM_WIDGET: SpectrogramWidget = SpectrogramWidget;

pub const DEFAULT_WIDTH: f32 = 32.0;
pub const DEFAULT_HEIGHT: f32 = 10.0;
pub const DEFAULT_FFT_SIZE: usize = 2048;
pub const DEFAULT_TIME_SLICES: usize = 256;
pub const DEFAULT_MIN_DB: f32 = -60.0;
pub const DEFAULT_MAX_DB: f32 = 0.0;
pub const DEFAULT_SMOOTHING: f32 = 0.65;

const MIN_FFT_SIZE: usize = 256;
const MAX_FFT_SIZE: usize = 8192;
const MIN_TIME_SLICES: usize = 8;
const MAX_TIME_SLICES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SpectrogramMode {
    Waterfall,
    Eq,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SpectrogramFreqScale {
    Log,
    Linear,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpectrogramRequest {
    pub data_key: String,
    pub source: LiveAudioSourceSelector,
    pub tap_point: TapPoint,
    pub fft_size: usize,
    pub time_slices: usize,
    pub min_db: f32,
    pub max_db: f32,
    pub smoothing: f32,
    pub required_ring_frames: usize,
}

impl SpectrogramMode {
    #[cfg(target_os = "macos")]
    fn metal_value(self) -> u32 {
        match self {
            SpectrogramMode::Waterfall => 0,
            SpectrogramMode::Eq => 1,
        }
    }
}

impl SpectrogramFreqScale {
    #[cfg(target_os = "macos")]
    fn metal_value(self) -> u32 {
        match self {
            SpectrogramFreqScale::Log => 0,
            SpectrogramFreqScale::Linear => 1,
        }
    }
}

impl WidgetDefinition for SpectrogramWidget {
    fn names(&self) -> &'static [&'static str] {
        &["spectrogram"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["fft-size", "time-slices", "min-db", "max-db", "smoothing"]
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
            .clamp(1.0, constraints.max_width.max(1.0));
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(DEFAULT_HEIGHT)
            .max(1.0);
        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let bg = background_color(props);
        let fg = resolve_named_color(props, "line-color", Color::rgba(0.72, 0.94, 1.0, 1.0));
        let rows = rect.height.round().max(0.0) as u16;
        let cols = rect.width.round().max(0.0) as u16;
        for row_offset in 0..rows {
            let row = rect.row.round() as u16 + row_offset;
            for col_offset in 0..cols {
                let col = rect.col.round() as u16 + col_offset;
                let border = row_offset == 0
                    || row_offset + 1 == rows
                    || col_offset == 0
                    || col_offset + 1 == cols;
                buf.set(
                    row,
                    col,
                    styled_cell(if border { '.' } else { ' ' }, fg, Some(bg)),
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        _viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let request = request_from_props(&node.props);
        let mode = mode_from_props(&node.props);
        let freq_scale = freq_scale_from_props(&node.props);
        vec![
            MetalPrimitive::Rect(MetalRectPrimitive {
                rect: node.rect,
                color: background_color(&node.props),
            }),
            MetalPrimitive::LiveSpectrogram(MetalLiveSpectrogramPrimitive {
                rect: node.rect,
                data_key: request.data_key,
                mode: mode.metal_value(),
                freq_scale: freq_scale.metal_value(),
                min_color: resolve_named_color(
                    &node.props,
                    "min-color",
                    Color::rgba(0.04, 0.04, 0.10, 1.0),
                ),
                mid_color: resolve_named_color(
                    &node.props,
                    "mid-color",
                    Color::rgba(0.12, 0.68, 0.86, 1.0),
                ),
                max_color: resolve_named_color(
                    &node.props,
                    "max-color",
                    Color::rgba(1.00, 0.74, 0.30, 1.0),
                ),
                eq_line_color: resolve_named_color(
                    &node.props,
                    "line-color",
                    Color::rgba(0.78, 0.98, 1.0, 1.0),
                ),
                eq_fill_color: resolve_named_color(
                    &node.props,
                    "fill-color",
                    Color::rgba(0.18, 0.78, 0.86, 0.26),
                ),
                background_color: background_color(&node.props),
            }),
        ]
    }
}

pub fn collect_spectrogram_requests(layout: &LayoutNode) -> Vec<SpectrogramRequest> {
    let mut requests = Vec::new();
    collect_spectrogram_requests_into(layout, &mut requests);
    requests
}

fn collect_spectrogram_requests_into(layout: &LayoutNode, requests: &mut Vec<SpectrogramRequest>) {
    if layout.widget_type == "spectrogram" && layout.rect.width > 0.0 && layout.rect.height > 0.0 {
        requests.push(request_from_props(&layout.props));
    }
    for child in &layout.children {
        collect_spectrogram_requests_into(child, requests);
    }
}

pub fn request_from_props(props: &HashMap<String, Value>) -> SpectrogramRequest {
    let source = source_from_props(props);
    let tap_point = tap_point_from_props(props);
    let fft_size = power_of_two_prop(
        props,
        "fft-size",
        DEFAULT_FFT_SIZE,
        MIN_FFT_SIZE,
        MAX_FFT_SIZE,
    );
    let time_slices = usize_prop(props, "time-slices", DEFAULT_TIME_SLICES)
        .clamp(MIN_TIME_SLICES, MAX_TIME_SLICES);
    let mut min_db = get_f32_prop(props, "min-db", DEFAULT_MIN_DB);
    let mut max_db = get_f32_prop(props, "max-db", DEFAULT_MAX_DB);
    if !min_db.is_finite() {
        min_db = DEFAULT_MIN_DB;
    }
    if !max_db.is_finite() {
        max_db = DEFAULT_MAX_DB;
    }
    if max_db <= min_db + 1.0 {
        max_db = min_db + 1.0;
    }
    let smoothing = get_f32_prop(props, "smoothing", DEFAULT_SMOOTHING).clamp(0.0, 0.98);
    let required_ring_frames = (fft_size * 4).next_power_of_two();
    let data_key = format!(
        "spectrogram:{}:{}:fft={}:slices={}:min={:08x}:max={:08x}:smooth={:08x}",
        source.key_fragment(),
        tap_point.key_fragment(),
        fft_size,
        time_slices,
        min_db.to_bits(),
        max_db.to_bits(),
        smoothing.to_bits()
    );
    SpectrogramRequest {
        data_key,
        source,
        tap_point,
        fft_size,
        time_slices,
        min_db,
        max_db,
        smoothing,
        required_ring_frames,
    }
}

fn mode_from_props(props: &HashMap<String, Value>) -> SpectrogramMode {
    match prop_keyword(props, "mode").as_deref() {
        Some("eq") => SpectrogramMode::Eq,
        _ => SpectrogramMode::Waterfall,
    }
}

fn freq_scale_from_props(props: &HashMap<String, Value>) -> SpectrogramFreqScale {
    match prop_keyword(props, "freq-scale").as_deref() {
        Some("linear") => SpectrogramFreqScale::Linear,
        _ => SpectrogramFreqScale::Log,
    }
}

fn usize_prop(props: &HashMap<String, Value>, key: &str, default: usize) -> usize {
    match props.get(key) {
        Some(Value::Number(value)) if value.is_finite() => (*value).round().max(0.0) as usize,
        Some(Value::ReactiveRef { slot, .. }) => {
            crate::reactive::read_float_slot(slot).round().max(0.0) as usize
        }
        _ => default,
    }
}

fn power_of_two_prop(
    props: &HashMap<String, Value>,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> usize {
    usize_prop(props, key, default)
        .clamp(min, max)
        .next_power_of_two()
        .clamp(min, max)
}

fn background_color(props: &HashMap<String, Value>) -> Color {
    resolve_named_color(props, "background-color", theme::BG())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

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

    fn layout_node(props: HashMap<String, Value>) -> LayoutNode {
        LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "spectrogram".to_string(),
            rect: Rect {
                row: 1.0,
                col: 2.0,
                width: 24.0,
                height: 8.0,
            },
            props,
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

    #[test]
    fn measures_to_finite_nonzero_rect() {
        let node = map(vec![
            ("width", Value::Number(22.0)),
            ("height", Value::Number(7.0)),
        ]);
        let mut measure_child = |_value: &Value, _constraints: Constraints| None;
        let size = SPECTROGRAM_WIDGET
            .measure(
                &node,
                &[],
                constraints(),
                &measure_ctx(),
                &mut measure_child,
            )
            .unwrap();
        assert_eq!(size.width, 22.0);
        assert_eq!(size.height, 7.0);
    }

    #[test]
    fn accepts_reactive_processor_props() {
        assert!(SPECTROGRAM_WIDGET.bindable_props().contains(&"fft-size"));
        assert!(SPECTROGRAM_WIDGET.bindable_props().contains(&"time-slices"));
        assert!(SPECTROGRAM_WIDGET.bindable_props().contains(&"min-db"));
        assert!(SPECTROGRAM_WIDGET.bindable_props().contains(&"max-db"));
        assert!(SPECTROGRAM_WIDGET.bindable_props().contains(&"smoothing"));
    }

    #[test]
    fn parses_track_source_and_normalizes_processing_props() {
        let props = HashMap::from([
            (
                "source".to_string(),
                map(vec![
                    ("kind", Value::Keyword("track".to_string())),
                    ("index", Value::Number(3.0)),
                ]),
            ),
            (
                "tap-point".to_string(),
                Value::Keyword("pre-fx".to_string()),
            ),
            ("fft-size".to_string(), Value::Number(3000.0)),
            ("time-slices".to_string(), Value::Number(12.0)),
            ("min-db".to_string(), Value::Number(-72.0)),
            ("max-db".to_string(), Value::Number(-6.0)),
            ("smoothing".to_string(), Value::Number(0.7)),
        ]);
        let request = request_from_props(&props);
        assert_eq!(request.source, LiveAudioSourceSelector::Track { index: 3 });
        assert_eq!(request.tap_point, TapPoint::PreFx);
        assert_eq!(request.fft_size, 4096);
        assert_eq!(request.time_slices, 12);
        assert_eq!(request.required_ring_frames, 16_384);
        assert!(request.data_key.contains("track:3"));
    }

    #[test]
    fn parses_bus_source_by_stable_id() {
        let props = HashMap::from([(
            "source".to_string(),
            map(vec![
                ("kind", Value::Keyword("bus".to_string())),
                ("id", Value::Number(42.0)),
                ("index", Value::Number(2.0)),
            ]),
        )]);
        let request = request_from_props(&props);
        assert_eq!(
            request.source,
            LiveAudioSourceSelector::Bus {
                id: Some(42),
                index: Some(2)
            }
        );
        assert!(request.data_key.contains("bus-id:42"));
    }

    #[test]
    fn reads_reactive_fft_size_for_request_key() {
        let slot = Arc::new(AtomicU64::new((1024.0f64).to_bits()));
        let props = HashMap::from([(
            "fft-size".to_string(),
            Value::ReactiveRef {
                namespace: "test".to_string(),
                field: "fft".to_string(),
                index: None,
                kind: crate::vm::BindingKind::Float,
                slot,
            },
        )]);
        let request = request_from_props(&props);
        assert_eq!(request.fft_size, 1024);
        assert!(request.data_key.contains("fft=1024"));
    }

    #[test]
    fn collect_requests_walks_child_layouts() {
        let child = layout_node(HashMap::new());
        let root = LayoutNode {
            widget_id: 9,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "box".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 40.0,
                height: 20.0,
            },
            props: HashMap::new(),
            children: vec![child],
            focusable: false,
        };
        assert_eq!(collect_spectrogram_requests(&root).len(), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn emits_live_spectrogram_metal_primitive() {
        let node = layout_node(HashMap::new());
        let viewport = WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 800.0,
            vp_h: 600.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 24.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        };
        let primitives = SPECTROGRAM_WIDGET.build_metal_primitives("spectrogram", &node, viewport);
        assert!(matches!(
            primitives.get(1),
            Some(MetalPrimitive::LiveSpectrogram(_))
        ));
    }
}
