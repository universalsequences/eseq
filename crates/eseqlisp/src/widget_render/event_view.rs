use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, MouseEventOutcome, WidgetDefinition, get_f32_prop, resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::vm::Value;

#[cfg(target_os = "macos")]
use super::{MetalPatchCablePrimitive, MetalPrimitive, MetalRectPrimitive, WidgetViewport};

pub struct EventViewWidget;

pub static EVENT_VIEW_WIDGET: EventViewWidget = EventViewWidget;

const DEFAULT_WIDTH: f32 = 26.0;
const DEFAULT_HEIGHT: f32 = 12.0;
const DEFAULT_BINS_X: usize = 24;
const DEFAULT_BINS_Y: usize = 8;
const DEFAULT_ROTATE_SENSITIVITY: f32 = 0.06;
const DEFAULT_AUTO_ROTATE_SPEED: f32 = 0.18;
const MAX_PITCH: f32 = 1.25;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct EventViewRotation {
    yaw: f32,
    pitch: f32,
}

thread_local! {
    static ROTATIONS: RefCell<HashMap<u64, EventViewRotation>> = RefCell::new(HashMap::new());
}

fn state_key(node: &LayoutNode) -> u64 {
    node.stable_widget_id.unwrap_or(node.widget_id)
}

fn get_rotation(node: &LayoutNode) -> EventViewRotation {
    let key = state_key(node);
    ROTATIONS.with(|rotations| rotations.borrow().get(&key).copied().unwrap_or_default())
}

fn set_rotation(node: &LayoutNode, rotation: EventViewRotation) {
    let key = state_key(node);
    ROTATIONS.with(|rotations| {
        let mut rotations = rotations.borrow_mut();
        if rotations.get(&key) == Some(&rotation) {
            return;
        }
        rotations.insert(key, rotation);
        super::bump_widget_state_generation();
    });
}

fn rotation_gesture_value(rotation: EventViewRotation) -> Value {
    Value::List(vec![
        Rc::new(RefCell::new(Value::Number(rotation.yaw as f64))),
        Rc::new(RefCell::new(Value::Number(rotation.pitch as f64))),
    ])
}

fn rotation_from_gesture(value: Option<&Value>) -> Option<EventViewRotation> {
    let Some(Value::List(items)) = value else {
        return None;
    };
    let yaw = items
        .first()
        .and_then(|value| number_value(&value.borrow()))?;
    let pitch = items
        .get(1)
        .and_then(|value| number_value(&value.borrow()))?;
    Some(EventViewRotation { yaw, pitch })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EventViewRenderer {
    Heatmap,
    Isometric,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct EventPoint {
    x: f32,
    y: f32,
    z: f32,
    brightness: f32,
    color_t: Option<f32>,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct HeatmapCell {
    brightness: f32,
    color_t: Option<f32>,
}

fn prop_symbol(props: &HashMap<String, Value>, key: &str, default: &'static str) -> String {
    match props.get(key) {
        Some(Value::Keyword(value)) | Some(Value::String(value)) => value.clone(),
        _ => default.to_string(),
    }
}

fn renderer(props: &HashMap<String, Value>) -> EventViewRenderer {
    match prop_symbol(props, "renderer", "heatmap").as_str() {
        "isometric" | "iso" => EventViewRenderer::Isometric,
        _ => EventViewRenderer::Heatmap,
    }
}

fn prop_bool(props: &HashMap<String, Value>, key: &str, default: bool) -> bool {
    match props.get(key) {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) if value.is_finite() => *value != 0.0,
        Some(Value::ReactiveRef { slot, .. }) => crate::reactive::read_float_slot(slot) != 0.0,
        Some(Value::Keyword(value)) | Some(Value::String(value)) => {
            matches!(value.as_str(), "true" | "yes" | "on")
        }
        _ => default,
    }
}

fn number_value(value: &Value) -> Option<f32> {
    match value {
        Value::Number(value) if value.is_finite() => Some(*value as f32),
        Value::ReactiveRef { slot, .. } => Some(crate::reactive::read_float_slot(slot) as f32),
        _ => None,
    }
}

fn map_number(
    map: &HashMap<String, std::rc::Rc<std::cell::RefCell<Value>>>,
    key: &str,
) -> Option<f32> {
    map.get(key).and_then(|value| number_value(&value.borrow()))
}

fn event_field(
    map: &HashMap<String, std::rc::Rc<std::cell::RefCell<Value>>>,
    field: &str,
    phase_beats: f32,
) -> Option<f32> {
    match field {
        "beat-phase" | "phase" => {
            let beat = map_number(map, "beat")?;
            if phase_beats > 0.0 && phase_beats.is_finite() {
                Some(beat.rem_euclid(phase_beats))
            } else {
                Some(beat)
            }
        }
        "node" | "track" | "sample" | "beat" | "transpose" | "velocity" => map_number(map, field),
        _ => map_number(map, field),
    }
}

fn normalize(value: f32, min: f32, max: f32) -> f32 {
    let range = max - min;
    if range.abs() <= f32::EPSILON || !range.is_finite() {
        0.0
    } else {
        ((value - min) / range).clamp(0.0, 1.0)
    }
}

fn parsed_events(props: &HashMap<String, Value>) -> Vec<EventPoint> {
    let Some(Value::List(items)) = props.get("events") else {
        return Vec::new();
    };

    let phase_beats = get_f32_prop(props, "phase-beats", 16.0);
    let window_beats = get_f32_prop(props, "window-beats", 16.0);
    let x_field = prop_symbol(props, "x", "transpose");
    let y_field = prop_symbol(props, "y", "node");
    let z_field = prop_symbol(props, "z", "beat-phase");
    let brightness_field = prop_symbol(props, "brightness", "velocity");
    let color_field = prop_symbol(props, "color-by", "");

    let x_min = get_f32_prop(props, "x-min", -24.0);
    let x_max = get_f32_prop(props, "x-max", 24.0);
    let y_min = get_f32_prop(props, "y-min", 0.0);
    let y_max = get_f32_prop(props, "y-max", 7.0);
    let z_min = get_f32_prop(props, "z-min", 0.0);
    let z_max = get_f32_prop(props, "z-max", phase_beats.max(1.0));
    let color_min = get_f32_prop(props, "color-min", x_min);
    let color_max = get_f32_prop(props, "color-max", x_max);

    let mut raw = Vec::with_capacity(items.len());
    let mut latest_beat = None::<f32>;
    for item in items {
        let Value::Map(map) = &*item.borrow() else {
            continue;
        };
        let Some(beat) = map_number(map, "beat") else {
            continue;
        };
        if beat.is_finite() {
            latest_beat = Some(latest_beat.map_or(beat, |latest| latest.max(beat)));
        }
        raw.push((beat, map.clone()));
    }

    let explicit_current_beat = props.get("current-beat").and_then(number_value);
    let reference_beat = explicit_current_beat.or(latest_beat);
    let min_beat = reference_beat
        .filter(|_| window_beats.is_finite() && window_beats > 0.0)
        .map(|latest| latest - window_beats);
    let max_beat = explicit_current_beat.map(|current| current + 0.0001);

    raw.into_iter()
        .filter_map(|(beat, map)| {
            if min_beat.is_some_and(|min| beat < min) {
                return None;
            }
            if max_beat.is_some_and(|max| beat > max) {
                return None;
            }
            let x = event_field(&map, &x_field, phase_beats)?;
            let y = event_field(&map, &y_field, phase_beats)?;
            let z = event_field(&map, &z_field, phase_beats)?;
            let brightness = event_field(&map, &brightness_field, phase_beats).unwrap_or(1.0);
            let color_t = if color_field.is_empty() {
                None
            } else {
                event_field(&map, &color_field, phase_beats)
                    .map(|value| normalize(value, color_min, color_max))
            };
            Some(EventPoint {
                x: normalize(x, x_min, x_max),
                y: normalize(y, y_min, y_max),
                z: normalize(z, z_min, z_max),
                brightness: brightness.clamp(0.0, 1.0),
                color_t,
            })
        })
        .collect()
}

fn mix_color(a: Color, b: Color, t: f32) -> Color {
    let [ar, ag, ab, aa] = a.to_rgba();
    let [br, bg, bb, ba] = b.to_rgba();
    let t = t.clamp(0.0, 1.0);
    Color::rgba(
        ar + (br - ar) * t,
        ag + (bg - ag) * t,
        ab + (bb - ab) * t,
        aa + (ba - aa) * t,
    )
}

fn categorical_color(index: usize) -> Color {
    const PALETTE: [Color; 16] = [
        Color::rgba(0.22, 0.72, 1.00, 1.0),
        Color::rgba(1.00, 0.42, 0.18, 1.0),
        Color::rgba(0.62, 0.42, 1.00, 1.0),
        Color::rgba(0.38, 0.92, 0.42, 1.0),
        Color::rgba(1.00, 0.28, 0.76, 1.0),
        Color::rgba(1.00, 0.86, 0.24, 1.0),
        Color::rgba(0.16, 0.44, 1.00, 1.0),
        Color::rgba(1.00, 0.22, 0.28, 1.0),
        Color::rgba(0.18, 0.88, 0.78, 1.0),
        Color::rgba(1.00, 0.62, 0.12, 1.0),
        Color::rgba(0.78, 0.72, 1.00, 1.0),
        Color::rgba(0.58, 0.82, 0.18, 1.0),
        Color::rgba(1.00, 0.52, 0.68, 1.0),
        Color::rgba(0.38, 0.82, 1.00, 1.0),
        Color::rgba(1.00, 0.68, 0.46, 1.0),
        Color::rgba(0.58, 1.00, 0.72, 1.0),
    ];
    PALETTE[index % PALETTE.len()]
}

fn color_palette(props: &HashMap<String, Value>) -> Vec<Color> {
    let Some(Value::List(items)) = props.get("color-palette") else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| crate::theme::parse_color_value(&item.borrow()))
        .collect()
}

fn event_color(props: &HashMap<String, Value>, brightness: f32, color_t: Option<f32>) -> Color {
    let base = if let Some(color_t) = color_t {
        match prop_symbol(props, "color-mode", "gradient").as_str() {
            "categorical" | "category" => {
                let palette = color_palette(props);
                let default_count = palette.len().max(16);
                let color_count = prop_usize(props, "color-count", default_count).max(1);
                let index = (color_t.clamp(0.0, 1.0) * (color_count - 1) as f32).round() as usize;
                palette
                    .get(index % palette.len().max(1))
                    .copied()
                    .unwrap_or_else(|| categorical_color(index))
            }
            _ => {
                let low =
                    resolve_named_color(props, "color-low", Color::rgba(0.15, 0.62, 1.0, 1.0));
                let high =
                    resolve_named_color(props, "color-high", Color::rgba(1.0, 0.45, 0.18, 1.0));
                mix_color(low, high, color_t)
            }
        }
    } else {
        resolve_named_color(props, "color", Color::rgba(0.24, 0.72, 1.0, 1.0))
    };
    let [r, g, b, a] = base.to_rgba();
    let t = brightness.clamp(0.0, 1.0);
    Color::rgba(
        (r * (0.25 + 0.75 * t)).clamp(0.0, 1.0),
        (g * (0.25 + 0.75 * t)).clamp(0.0, 1.0),
        (b * (0.25 + 0.75 * t)).clamp(0.0, 1.0),
        (a * (0.20 + 0.80 * t)).clamp(0.0, 1.0),
    )
}

fn prop_usize(props: &HashMap<String, Value>, key: &str, default: usize) -> usize {
    props
        .get(key)
        .and_then(number_value)
        .map(|value| value.round().max(1.0) as usize)
        .unwrap_or(default)
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug)]
struct IsoCube {
    rect: Rect,
    center_col: f32,
    center_row: f32,
    x_col: f32,
    x_row: f32,
    y_col: f32,
    y_row: f32,
    z_col: f32,
    z_row: f32,
}

#[cfg(target_os = "macos")]
impl IsoCube {
    fn new(
        rect: Rect,
        padding: f32,
        cell_w: f32,
        cell_h: f32,
        rotation: EventViewRotation,
    ) -> Self {
        let cell_w = cell_w.max(1.0);
        let cell_h = cell_h.max(1.0);
        let max_padding = (rect.width * 0.5).min(rect.height * 0.5).max(0.0);
        let padding = padding.clamp(0.0, max_padding);
        let drawing_rect = Rect {
            row: rect.row + padding,
            col: rect.col + padding,
            width: (rect.width - padding * 2.0).max(1.0),
            height: (rect.height - padding * 2.0).max(1.0),
        };
        let stroke_safe_inset_px = 1.0_f32;
        let drawing_col_px = drawing_rect.col * cell_w + stroke_safe_inset_px;
        let drawing_row_px = drawing_rect.row * cell_h + stroke_safe_inset_px;
        let drawing_width_px = (drawing_rect.width * cell_w - stroke_safe_inset_px * 2.0).max(1.0);
        let drawing_height_px =
            (drawing_rect.height * cell_h - stroke_safe_inset_px * 2.0).max(1.0);

        let raw_x = [1.0_f32, 0.42_f32];
        let raw_y = [0.0_f32, -1.12_f32];
        let raw_z = [-0.82_f32, 0.62_f32];
        let project_axis = |point: [f32; 3]| {
            let rotated = rotate_point(point, rotation);
            [
                rotated[0] * raw_x[0] + rotated[1] * raw_y[0] + rotated[2] * raw_z[0],
                rotated[0] * raw_x[1] + rotated[1] * raw_y[1] + rotated[2] * raw_z[1],
            ]
        };
        let x_axis = project_axis([1.0, 0.0, 0.0]);
        let y_axis = project_axis([0.0, 1.0, 0.0]);
        let z_axis = project_axis([0.0, 0.0, 1.0]);
        let mut min_col = f32::INFINITY;
        let mut max_col = f32::NEG_INFINITY;
        let mut min_row = f32::INFINITY;
        let mut max_row = f32::NEG_INFINITY;
        for x in [-0.5_f32, 0.5] {
            for y in [-0.5_f32, 0.5] {
                for z in [-0.5_f32, 0.5] {
                    let col = x * x_axis[0] + y * y_axis[0] + z * z_axis[0];
                    let row = x * x_axis[1] + y * y_axis[1] + z * z_axis[1];
                    min_col = min_col.min(col);
                    max_col = max_col.max(col);
                    min_row = min_row.min(row);
                    max_row = max_row.max(row);
                }
            }
        }
        let raw_width = (max_col - min_col).max(0.001);
        let raw_height = (max_row - min_row).max(0.001);
        let scale = (drawing_width_px / raw_width)
            .min(drawing_height_px / raw_height)
            .max(0.001);
        let raw_center_col = (min_col + max_col) * 0.5;
        let raw_center_row = (min_row + max_row) * 0.5;
        let center_col_px = drawing_col_px + drawing_width_px * 0.5 - raw_center_col * scale;
        let center_row_px = drawing_row_px + drawing_height_px * 0.5 - raw_center_row * scale;

        Self {
            rect,
            center_col: center_col_px / cell_w,
            center_row: center_row_px / cell_h,
            x_col: x_axis[0] * scale / cell_w,
            x_row: x_axis[1] * scale / cell_h,
            y_col: y_axis[0] * scale / cell_w,
            y_row: y_axis[1] * scale / cell_h,
            z_col: z_axis[0] * scale / cell_w,
            z_row: z_axis[1] * scale / cell_h,
        }
    }

    fn project(self, x: f32, y: f32, z: f32) -> [f32; 2] {
        let x = x.clamp(0.0, 1.0) - 0.5;
        let y = y.clamp(0.0, 1.0) - 0.5;
        let z = z.clamp(0.0, 1.0) - 0.5;
        [
            self.center_col + x * self.x_col + y * self.y_col + z * self.z_col,
            self.center_row + x * self.x_row + y * self.y_row + z * self.z_row,
        ]
    }

    fn contains(self, p: [f32; 2]) -> bool {
        p[0] >= self.rect.col
            && p[0] <= self.rect.col + self.rect.width
            && p[1] >= self.rect.row
            && p[1] <= self.rect.row + self.rect.height
    }
}

#[cfg(target_os = "macos")]
fn rotate_point([x, y, z]: [f32; 3], rotation: EventViewRotation) -> [f32; 3] {
    let (yaw_sin, yaw_cos) = rotation.yaw.sin_cos();
    let yaw_x = x * yaw_cos + z * yaw_sin;
    let yaw_z = -x * yaw_sin + z * yaw_cos;
    let (pitch_sin, pitch_cos) = rotation.pitch.sin_cos();
    [
        yaw_x,
        y * pitch_cos - yaw_z * pitch_sin,
        y * pitch_sin + yaw_z * pitch_cos,
    ]
}

#[cfg(target_os = "macos")]
fn color_with_alpha(color: Color, alpha: f32) -> Color {
    let [r, g, b, a] = color.to_rgba();
    Color::rgba(r, g, b, (a * alpha).clamp(0.0, 1.0))
}

#[cfg(target_os = "macos")]
fn push_sdf_segment(
    primitives: &mut Vec<MetalPrimitive>,
    start: [f32; 2],
    end: [f32; 2],
    radius_px: f32,
    color: Color,
) {
    let radius_px = radius_px.max(0.1);
    primitives.push(MetalPrimitive::PatchCable(MetalPatchCablePrimitive {
        start,
        control1: start,
        control2: end,
        end,
        radius_px,
        color,
        is_segmented: false,
        segment_row: 0.0,
        corner_radius_cells: 0.0,
    }));
}

#[cfg(target_os = "macos")]
fn push_sdf_dot(
    primitives: &mut Vec<MetalPrimitive>,
    center: [f32; 2],
    radius_px: f32,
    color: Color,
) {
    push_sdf_segment(primitives, center, center, radius_px, color);
}

#[cfg(target_os = "macos")]
fn push_iso_cube(
    primitives: &mut Vec<MetalPrimitive>,
    props: &HashMap<String, Value>,
    cube: IsoCube,
) {
    let edge = resolve_named_color(props, "cube-color", Color::rgba(0.20, 0.43, 0.58, 0.55));
    let grid = resolve_named_color(props, "grid-color", Color::rgba(0.13, 0.27, 0.36, 0.32));
    let edge_radius_px = get_f32_prop(props, "cube-line-radius-px", 0.72).max(0.1);
    let grid_radius_px = get_f32_prop(props, "grid-line-radius-px", 0.38).max(0.1);

    let p = |x, y, z| cube.project(x, y, z);
    for t in [0.25_f32, 0.5, 0.75] {
        push_sdf_segment(
            primitives,
            p(t, 0.0, 0.0),
            p(t, 0.0, 1.0),
            grid_radius_px,
            grid,
        );
        push_sdf_segment(
            primitives,
            p(0.0, 0.0, t),
            p(1.0, 0.0, t),
            grid_radius_px,
            grid,
        );
        push_sdf_segment(
            primitives,
            p(0.0, t, 0.0),
            p(1.0, t, 0.0),
            grid_radius_px,
            grid,
        );
        push_sdf_segment(
            primitives,
            p(0.0, t, 0.0),
            p(0.0, t, 1.0),
            grid_radius_px,
            grid,
        );
        push_sdf_segment(
            primitives,
            p(0.0, 0.0, t),
            p(0.0, 1.0, t),
            grid_radius_px,
            grid,
        );
        push_sdf_segment(
            primitives,
            p(t, 0.0, 0.0),
            p(t, 1.0, 0.0),
            grid_radius_px,
            grid,
        );
    }

    for (start, end) in [
        ((0.0, 0.0, 0.0), (1.0, 0.0, 0.0)),
        ((1.0, 0.0, 0.0), (1.0, 1.0, 0.0)),
        ((1.0, 1.0, 0.0), (0.0, 1.0, 0.0)),
        ((0.0, 1.0, 0.0), (0.0, 0.0, 0.0)),
        ((0.0, 0.0, 1.0), (1.0, 0.0, 1.0)),
        ((1.0, 0.0, 1.0), (1.0, 1.0, 1.0)),
        ((1.0, 1.0, 1.0), (0.0, 1.0, 1.0)),
        ((0.0, 1.0, 1.0), (0.0, 0.0, 1.0)),
        ((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)),
        ((1.0, 0.0, 0.0), (1.0, 0.0, 1.0)),
        ((1.0, 1.0, 0.0), (1.0, 1.0, 1.0)),
        ((0.0, 1.0, 0.0), (0.0, 1.0, 1.0)),
    ] {
        push_sdf_segment(
            primitives,
            p(start.0, start.1, start.2),
            p(end.0, end.1, end.2),
            edge_radius_px,
            edge,
        );
    }
}

impl WidgetDefinition for EventViewWidget {
    fn names(&self) -> &'static [&'static str] {
        &["event-view"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "renderer", "x-bins", "y-bins"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["events", "current-beat", "auto-rotate", "auto-rotate-speed"]
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
            width: get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or(DEFAULT_WIDTH)
                .max(1.0),
            height: get_prop_num(node, "height")
                .map(f64_to_f32)
                .unwrap_or(DEFAULT_HEIGHT)
                .max(1.0),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let points = parsed_events(props);
        let color = resolve_named_color(props, "color", Color::rgba(0.24, 0.72, 1.0, 1.0));
        for point in points {
            let col = (rect.col + point.x * (rect.width - 1.0).max(0.0)).round() as u16;
            let row = (rect.row + (1.0 - point.y) * (rect.height - 1.0).max(0.0)).round() as u16;
            buf.set(row, col, styled_cell('*', color, None));
        }
    }

    fn begin_gesture(
        &self,
        node: &LayoutNode,
        _local_col: f32,
        _local_row: f32,
        _modifiers: KeyModifiers,
    ) -> Option<Value> {
        Some(rotation_gesture_value(get_rotation(node)))
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        drag_start: Option<(f32, f32)>,
        gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => MouseEventOutcome::Consume,
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some((start_col, start_row)) = drag_start else {
                    return MouseEventOutcome::Consume;
                };
                let start_rotation =
                    rotation_from_gesture(gesture).unwrap_or_else(|| get_rotation(node));
                let sensitivity = get_f32_prop(
                    &node.props,
                    "rotation-sensitivity",
                    DEFAULT_ROTATE_SENSITIVITY,
                )
                .max(0.0);
                set_rotation(
                    node,
                    EventViewRotation {
                        yaw: start_rotation.yaw + (local_col - start_col) * sensitivity,
                        pitch: (start_rotation.pitch + (local_row - start_row) * sensitivity)
                            .clamp(-MAX_PITCH, MAX_PITCH),
                    },
                );
                MouseEventOutcome::Consume
            }
            MouseEventKind::Up(MouseButton::Left) => MouseEventOutcome::Consume,
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn unclamped_drag(&self) -> bool {
        true
    }

    fn wants_animation_frames(&self, node: &LayoutNode) -> bool {
        renderer(&node.props) == EventViewRenderer::Isometric
            && prop_bool(&node.props, "auto-rotate", false)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let points = parsed_events(&node.props);
        let bg = resolve_named_color(
            &node.props,
            "background",
            Color::rgba(0.02, 0.025, 0.03, 1.0),
        );
        let mut primitives = vec![MetalPrimitive::Rect(MetalRectPrimitive {
            rect: node.rect,
            color: bg,
        })];

        match renderer(&node.props) {
            EventViewRenderer::Heatmap => {
                let bins_x = prop_usize(&node.props, "x-bins", DEFAULT_BINS_X);
                let bins_y = prop_usize(&node.props, "y-bins", DEFAULT_BINS_Y);
                let mut cells = vec![HeatmapCell::default(); bins_x * bins_y];
                for point in points {
                    let x = (point.x * bins_x as f32)
                        .floor()
                        .clamp(0.0, bins_x as f32 - 1.0) as usize;
                    let y = ((1.0 - point.y) * bins_y as f32)
                        .floor()
                        .clamp(0.0, bins_y as f32 - 1.0) as usize;
                    let idx = y * bins_x + x;
                    if point.brightness >= cells[idx].brightness {
                        cells[idx] = HeatmapCell {
                            brightness: point.brightness,
                            color_t: point.color_t,
                        };
                    }
                }
                let cell_w = node.rect.width / bins_x as f32;
                let cell_h = node.rect.height / bins_y as f32;
                for y in 0..bins_y {
                    for x in 0..bins_x {
                        let cell = cells[y * bins_x + x];
                        if cell.brightness <= 0.0 {
                            continue;
                        }
                        primitives.push(MetalPrimitive::Rect(MetalRectPrimitive {
                            rect: Rect {
                                row: node.rect.row + y as f32 * cell_h,
                                col: node.rect.col + x as f32 * cell_w,
                                width: cell_w,
                                height: cell_h,
                            },
                            color: event_color(&node.props, cell.brightness, cell.color_t),
                        }));
                    }
                }
            }
            EventViewRenderer::Isometric => {
                let radius = get_f32_prop(&node.props, "point-radius-px", 3.2).max(0.5);
                let cube_padding = get_f32_prop(&node.props, "cube-padding", 0.0).max(0.0);
                let mut rotation = get_rotation(node);
                if prop_bool(&node.props, "auto-rotate", false) {
                    let speed =
                        get_f32_prop(&node.props, "auto-rotate-speed", DEFAULT_AUTO_ROTATE_SPEED);
                    rotation.yaw += viewport.time_seconds * speed;
                }
                let cube = IsoCube::new(
                    node.rect,
                    cube_padding,
                    viewport.cell_w,
                    viewport.cell_h,
                    rotation,
                );
                push_iso_cube(&mut primitives, &node.props, cube);
                for point in points {
                    let center = cube.project(point.x, point.y, point.z);
                    if !cube.contains(center) {
                        continue;
                    }
                    let color = event_color(&node.props, point.brightness, point.color_t);
                    let event_radius = radius * (0.65 + 0.70 * point.brightness);
                    push_sdf_dot(
                        &mut primitives,
                        center,
                        event_radius * 2.2,
                        color_with_alpha(color, 0.22),
                    );
                    push_sdf_dot(&mut primitives, center, event_radius, color);
                }
            }
        }

        primitives
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn value_cell(value: Value) -> Rc<RefCell<Value>> {
        Rc::new(RefCell::new(value))
    }

    fn event(node: f64, beat: f64, transpose: f64, velocity: f64) -> Value {
        let mut map = HashMap::new();
        map.insert("node".to_string(), value_cell(Value::Number(node)));
        map.insert("beat".to_string(), value_cell(Value::Number(beat)));
        map.insert(
            "transpose".to_string(),
            value_cell(Value::Number(transpose)),
        );
        map.insert("velocity".to_string(), value_cell(Value::Number(velocity)));
        Value::Map(map)
    }

    fn props_with_events(events: Vec<Value>) -> HashMap<String, Value> {
        let mut props = HashMap::new();
        props.insert(
            "events".to_string(),
            Value::List(events.into_iter().map(value_cell).collect()),
        );
        props
    }

    fn widget_value(props: HashMap<String, Value>) -> Value {
        Value::Map(
            props
                .into_iter()
                .map(|(key, value)| (key, value_cell(value)))
                .collect(),
        )
    }

    fn test_constraints() -> Constraints {
        Constraints {
            min_width: 0.0,
            max_width: 100.0,
            min_height: 0.0,
            max_height: 100.0,
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
    fn categorical_color_mode_separates_adjacent_low_channel_values() {
        let mut props = HashMap::new();
        props.insert(
            "color-mode".to_string(),
            Value::Keyword("categorical".to_string()),
        );
        props.insert("color-count".to_string(), Value::Number(16.0));

        let track_zero = event_color(&props, 1.0, Some(0.0)).to_rgba();
        let track_one = event_color(&props, 1.0, Some(1.0 / 15.0)).to_rgba();

        assert_ne!(track_zero, track_one);
        assert!(
            (track_zero[0] - track_one[0]).abs()
                + (track_zero[1] - track_one[1]).abs()
                + (track_zero[2] - track_one[2]).abs()
                > 0.5
        );
    }

    #[test]
    fn categorical_color_mode_uses_supplied_color_palette() {
        let mut props = HashMap::new();
        props.insert(
            "color-mode".to_string(),
            Value::Keyword("categorical".to_string()),
        );
        props.insert("color-count".to_string(), Value::Number(2.0));
        props.insert(
            "color-palette".to_string(),
            Value::List(vec![
                value_cell(Value::List(vec![
                    value_cell(Value::Number(0.96)),
                    value_cell(Value::Number(0.28)),
                    value_cell(Value::Number(0.52)),
                ])),
                value_cell(Value::List(vec![
                    value_cell(Value::Number(0.22)),
                    value_cell(Value::Number(0.78)),
                    value_cell(Value::Number(0.36)),
                ])),
            ]),
        );

        assert_eq!(
            event_color(&props, 1.0, Some(0.0)).to_rgba(),
            [0.96, 0.28, 0.52, 1.0]
        );
        assert_eq!(
            event_color(&props, 1.0, Some(1.0)).to_rgba(),
            [0.22, 0.78, 0.36, 1.0]
        );
    }

    #[test]
    fn filters_by_window_and_computes_beat_phase() {
        let mut props = props_with_events(vec![
            event(0.0, 1.0, -12.0, 0.5),
            event(1.0, 18.0, 12.0, 0.75),
        ]);
        props.insert("window-beats".to_string(), Value::Number(4.0));
        props.insert("phase-beats".to_string(), Value::Number(16.0));
        let points = parsed_events(&props);
        assert_eq!(points.len(), 1);
        assert!((points[0].z - 0.125).abs() < 1e-6);
    }

    #[test]
    fn current_beat_advances_window_without_new_events() {
        let mut props = props_with_events(vec![
            event(0.0, 1.0, -12.0, 0.5),
            event(1.0, 18.0, 12.0, 0.75),
        ]);
        props.insert("current-beat".to_string(), Value::Number(40.0));
        props.insert("window-beats".to_string(), Value::Number(4.0));

        assert!(parsed_events(&props).is_empty());
    }

    #[test]
    fn current_beat_reset_hides_future_history() {
        let mut props = props_with_events(vec![
            event(0.0, 25.0, -12.0, 0.5),
            event(1.0, 29.0, 12.0, 0.75),
        ]);
        props.insert("current-beat".to_string(), Value::Number(0.0));
        props.insert("window-beats".to_string(), Value::Number(16.0));

        assert!(parsed_events(&props).is_empty());
    }

    #[test]
    fn malformed_events_are_ignored() {
        let props = props_with_events(vec![Value::String("bad".to_string())]);
        assert!(parsed_events(&props).is_empty());
    }

    #[test]
    fn events_prop_is_bindable() {
        assert!(EVENT_VIEW_WIDGET.bindable_props().contains(&"events"));
        assert!(EVENT_VIEW_WIDGET.bindable_props().contains(&"current-beat"));
    }

    #[test]
    fn measures_to_finite_nonzero_rect() {
        let mut props = HashMap::new();
        props.insert("width".to_string(), Value::Number(26.0));
        props.insert("height".to_string(), Value::Number(12.0));
        let size = EVENT_VIEW_WIDGET
            .measure(
                &widget_value(props),
                &[],
                test_constraints(),
                &measure_ctx(),
                &mut |_, _| None,
            )
            .expect("event-view should measure");
        assert!(size.width.is_finite() && size.width > 0.0);
        assert!(size.height.is_finite() && size.height > 0.0);
    }

    #[cfg(target_os = "macos")]
    fn layout_node(props: HashMap<String, Value>) -> LayoutNode {
        LayoutNode {
            widget_id: 99,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "event-view".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 26.0,
                height: 12.0,
            },
            props,
            children: Vec::new(),
            focusable: false,
        }
    }

    #[cfg(target_os = "macos")]
    fn viewport() -> WidgetViewport {
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 640.0,
            vp_h: 360.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 18.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        }
    }

    #[cfg(target_os = "macos")]
    fn viewport_at(time_seconds: f32) -> WidgetViewport {
        WidgetViewport {
            time_seconds,
            ..viewport()
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn heatmap_renderer_produces_event_primitives() {
        let mut props = props_with_events(vec![event(1.0, 1.0, 0.0, 0.75)]);
        props.insert(
            "renderer".to_string(),
            Value::Keyword("heatmap".to_string()),
        );
        let prims =
            EVENT_VIEW_WIDGET.build_metal_primitives("event-view", &layout_node(props), viewport());
        assert!(
            prims
                .iter()
                .any(|prim| matches!(prim, MetalPrimitive::Rect(_)))
        );
        assert!(prims.len() > 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn heatmap_renderer_can_color_events_by_numeric_channel() {
        let mut props = props_with_events(vec![event(1.0, 1.0, 24.0, 1.0)]);
        props.insert(
            "renderer".to_string(),
            Value::Keyword("heatmap".to_string()),
        );
        props.insert(
            "color-by".to_string(),
            Value::Keyword("transpose".to_string()),
        );
        props.insert("color-min".to_string(), Value::Number(-24.0));
        props.insert("color-max".to_string(), Value::Number(24.0));
        props.insert(
            "color-low".to_string(),
            Value::List(vec![
                value_cell(Value::Number(0.0)),
                value_cell(Value::Number(0.0)),
                value_cell(Value::Number(1.0)),
                value_cell(Value::Number(1.0)),
            ]),
        );
        props.insert(
            "color-high".to_string(),
            Value::List(vec![
                value_cell(Value::Number(1.0)),
                value_cell(Value::Number(0.0)),
                value_cell(Value::Number(0.0)),
                value_cell(Value::Number(1.0)),
            ]),
        );

        let prims =
            EVENT_VIEW_WIDGET.build_metal_primitives("event-view", &layout_node(props), viewport());
        let event_rect = prims.iter().skip(1).find_map(|prim| match prim {
            MetalPrimitive::Rect(rect) => Some(rect),
            _ => None,
        });
        let event_color = event_rect.expect("event heatmap rect").color.to_rgba();

        assert!(event_color[0] > event_color[2]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn isometric_renderer_produces_event_primitives() {
        let mut props = props_with_events(vec![event(1.0, 1.0, 0.0, 0.75)]);
        props.insert(
            "renderer".to_string(),
            Value::Keyword("isometric".to_string()),
        );
        let prims =
            EVENT_VIEW_WIDGET.build_metal_primitives("event-view", &layout_node(props), viewport());
        assert!(
            prims
                .iter()
                .any(|prim| matches!(prim, MetalPrimitive::PatchCable(_)))
        );
        assert!(
            prims
                .iter()
                .filter(|prim| matches!(prim, MetalPrimitive::PatchCable(_)))
                .count()
                > 12
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cube_padding_controls_projection_extent() {
        fn bounds(cube: IsoCube) -> (f32, f32, f32, f32) {
            let mut min_col = f32::INFINITY;
            let mut max_col = f32::NEG_INFINITY;
            let mut min_row = f32::INFINITY;
            let mut max_row = f32::NEG_INFINITY;
            for x in [0.0_f32, 1.0] {
                for y in [0.0_f32, 1.0] {
                    for z in [0.0_f32, 1.0] {
                        let [col, row] = cube.project(x, y, z);
                        min_col = min_col.min(col);
                        max_col = max_col.max(col);
                        min_row = min_row.min(row);
                        max_row = max_row.max(row);
                    }
                }
            }
            (min_col, max_col, min_row, max_row)
        }

        let rect = Rect {
            row: 0.0,
            col: 0.0,
            width: 26.0,
            height: 12.0,
        };
        let tight = IsoCube::new(rect, 0.0, 10.0, 20.0, EventViewRotation::default());
        let padded = IsoCube::new(rect, 2.0, 10.0, 20.0, EventViewRotation::default());

        let (tight_min_x, tight_max_x, tight_top, tight_bottom) = bounds(tight);
        let (padded_min_x, padded_max_x, padded_top, padded_bottom) = bounds(padded);

        assert!(padded_min_x > tight_min_x);
        assert!(padded_max_x < tight_max_x);
        assert!(tight_top >= rect.row - 1e-5);
        assert!(tight_bottom <= rect.row + rect.height + 1e-5);
        assert!(padded_top > tight_top);
        assert!(padded_bottom < tight_bottom);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn drag_rotation_changes_isometric_projection() {
        let mut props = props_with_events(vec![event(1.0, 1.0, 0.0, 0.75)]);
        props.insert(
            "renderer".to_string(),
            Value::Keyword("isometric".to_string()),
        );
        let node = layout_node(props);
        let before = EVENT_VIEW_WIDGET.build_metal_primitives("event-view", &node, viewport());

        let gesture = EVENT_VIEW_WIDGET
            .begin_gesture(&node, 4.0, 4.0, KeyModifiers::empty())
            .expect("rotation gesture");
        assert!(matches!(
            EVENT_VIEW_WIDGET.mouse_event(
                &node,
                MouseEventKind::Drag(MouseButton::Left),
                10.0,
                6.0,
                Some((4.0, 4.0)),
                Some(&gesture),
                KeyModifiers::empty(),
                10.0,
                20.0,
            ),
            MouseEventOutcome::Consume
        ));

        let after = EVENT_VIEW_WIDGET.build_metal_primitives("event-view", &node, viewport());
        let first_cable_start = |prims: &[MetalPrimitive]| {
            prims.iter().find_map(|prim| match prim {
                MetalPrimitive::PatchCable(cable) => Some(cable.start),
                _ => None,
            })
        };
        assert_ne!(first_cable_start(&before), first_cable_start(&after));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn auto_rotate_requests_animation_frames_and_changes_with_time() {
        let mut props = props_with_events(vec![event(1.0, 1.0, 0.0, 0.75)]);
        props.insert(
            "renderer".to_string(),
            Value::Keyword("isometric".to_string()),
        );
        props.insert("auto-rotate".to_string(), Value::Bool(true));
        props.insert("auto-rotate-speed".to_string(), Value::Number(1.0));
        let node = layout_node(props);

        assert!(EVENT_VIEW_WIDGET.wants_animation_frames(&node));
        let at_zero =
            EVENT_VIEW_WIDGET.build_metal_primitives("event-view", &node, viewport_at(0.0));
        let at_one =
            EVENT_VIEW_WIDGET.build_metal_primitives("event-view", &node, viewport_at(1.0));
        let first_cable_start = |prims: &[MetalPrimitive]| {
            prims.iter().find_map(|prim| match prim {
                MetalPrimitive::PatchCable(cable) => Some(cable.start),
                _ => None,
            })
        };
        assert_ne!(first_cable_start(&at_zero), first_cable_start(&at_one));
    }
}
