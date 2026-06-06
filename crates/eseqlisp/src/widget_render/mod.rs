pub mod adsr_editor;
pub mod box_widget;
pub mod button;
pub mod cable;
pub mod dropdown;
pub mod grid;
pub mod hslider;
pub mod hstack;
pub mod image;
pub mod knob;
pub mod knob_number;
pub mod label;
pub mod matrix;
pub mod mixer_meter;
pub mod modulator_curve;
pub mod number_label;
pub mod number_picker;
pub mod patcher;
pub mod response_curve_editor;
pub mod scroll;
pub mod sdf_widget;
pub mod tabs;
pub mod text_input;
pub mod time_view;
pub mod timeline;
pub mod toggle;
pub mod transport_clock;
pub mod tree;
pub mod virtual_vstack;
pub mod vslider;
pub mod vstack;
pub mod waveform;
pub mod wrap;

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use crossterm::event::{KeyCode, KeyModifiers, MouseEventKind};

use crate::backend::{Cell, CellStyle, Color};
use crate::layout::{Constraints, LayoutCtx, LayoutNode, MeasureCtx, Rect, Size, get_map};
use crate::theme;
use crate::vm::Value;

#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
    NSHapticFeedbackPerformer,
};

// ── Widget state generation counter ─────────────────────────────────────────
// Bumped whenever a widget's internal state changes (scroll offset, tree
// expand/collapse, etc.) so that primitive caches can be invalidated.

static WIDGET_STATE_GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn bump_widget_state_generation() {
    WIDGET_STATE_GENERATION.fetch_add(1, Ordering::Relaxed);
}

pub fn widget_state_generation() -> u64 {
    WIDGET_STATE_GENERATION.load(Ordering::Relaxed)
}

fn haptic_quantum(min: f32, max: f32) -> f32 {
    let range = (max - min).abs();
    if range > 1000.0 {
        100.0
    } else if range > 40.0 {
        10.0
    } else {
        1.0
    }
}

fn haptic_bucket(value: f32, min: f32, quantum: f32) -> i64 {
    ((value - min) / quantum).floor() as i64
}

pub fn should_trigger_integer_haptic(
    widget_id: u64,
    previous: f32,
    new_value: f32,
    min: f32,
    max: f32,
) -> bool {
    let range = (max - min).abs();
    if range <= 1.0 || !previous.is_finite() || !new_value.is_finite() {
        return false;
    }
    let quantum = haptic_quantum(min, max);
    let previous_bucket = haptic_bucket(previous, min, quantum);
    let new_bucket = haptic_bucket(new_value, min, quantum);
    HAPTIC_BUCKETS.with(|buckets| {
        let mut buckets = buckets.borrow_mut();
        let last_bucket = buckets.entry(widget_id).or_insert(previous_bucket);
        if *last_bucket != new_bucket {
            *last_bucket = new_bucket;
            true
        } else {
            false
        }
    })
}

pub fn mapped_haptic_value(props: &HashMap<String, Value>, normalized: f32, fallback: f32) -> f32 {
    let Some(Value::Number(_)) = props.get("haptic-value") else {
        return fallback;
    };
    let min = get_f32_prop(props, "haptic-min", get_f32_prop(props, "min", 0.0));
    let max = get_f32_prop(props, "haptic-max", get_f32_prop(props, "max", 1.0));
    let pivot_pos = get_f32_prop(props, "haptic-pivot-position", 1.0).clamp(0.0, 1.0);
    let pivot_value = get_f32_prop(props, "haptic-pivot-value", max).clamp(min, max);
    let exponent = get_f32_prop(props, "haptic-exponent", 1.0).max(0.001);
    let t = normalized.clamp(0.0, 1.0);

    if pivot_pos <= 0.0 || pivot_pos >= 1.0 {
        return min + (max - min) * t;
    }

    if t <= pivot_pos {
        min + (pivot_value - min) * (t / pivot_pos)
    } else {
        let upper_t = (t - pivot_pos) / (1.0 - pivot_pos);
        pivot_value + (max - pivot_value) * upper_t.powf(exponent)
    }
}

#[cfg(target_os = "macos")]
pub fn trigger_level_change_haptic() {
    let performer = NSHapticFeedbackManager::defaultPerformer();
    performer.performFeedbackPattern_performanceTime(
        NSHapticFeedbackPattern::LevelChange,
        NSHapticFeedbackPerformanceTime::Now,
    );
}

#[cfg(not(target_os = "macos"))]
pub fn trigger_level_change_haptic() {}

#[cfg(target_os = "macos")]
pub fn trigger_alignment_haptic() {
    let performer = NSHapticFeedbackManager::defaultPerformer();
    performer.performFeedbackPattern_performanceTime(
        NSHapticFeedbackPattern::Alignment,
        NSHapticFeedbackPerformanceTime::Now,
    );
}

#[cfg(not(target_os = "macos"))]
pub fn trigger_alignment_haptic() {}

// ── Overlay system ───────────────────────────────────────────────────────────
// Only one overlay (dropdown menu, etc.) can be active at a time.

struct OverlayInfo {
    widget_id: u64,
    /// Hit-test bounds in layout space (screen-relative, post-scroll).
    rect: Rect,
}

thread_local! {
    static OVERLAY_INFO: RefCell<Option<OverlayInfo>> = RefCell::new(None);
    static HAPTIC_BUCKETS: RefCell<HashMap<u64, i64>> = RefCell::new(HashMap::new());
    static DROP_HOVER_TARGET: RefCell<Option<u64>> = const { RefCell::new(None) };
    #[cfg(target_os = "macos")]
    static OVERLAY_PRIMITIVES: RefCell<Vec<MetalPrimitive>> = RefCell::new(Vec::new());
    #[cfg(target_os = "macos")]
    static WIDGET_PRIMITIVE_CACHE: RefCell<HashMap<u64, Vec<MetalPrimitive>>> = RefCell::new(HashMap::new());
}

pub fn set_overlay(widget_id: u64, rect: Rect) {
    OVERLAY_INFO.with(|o| *o.borrow_mut() = Some(OverlayInfo { widget_id, rect }));
}

pub fn clear_overlay() {
    OVERLAY_INFO.with(|o| *o.borrow_mut() = None);
    #[cfg(target_os = "macos")]
    OVERLAY_PRIMITIVES.with(|o| o.borrow_mut().clear());
    bump_widget_state_generation();
}

pub fn overlay_widget_id() -> Option<u64> {
    OVERLAY_INFO.with(|o| o.borrow().as_ref().map(|s| s.widget_id))
}

pub fn get_overlay_rect() -> Option<Rect> {
    OVERLAY_INFO.with(|o| o.borrow().as_ref().map(|s| s.rect))
}

pub fn overlay_contains(local_col: f32, local_row: f32) -> bool {
    OVERLAY_INFO.with(|o| {
        if let Some(ref s) = *o.borrow() {
            local_row >= s.rect.row
                && local_row < s.rect.row + s.rect.height
                && local_col >= s.rect.col
                && local_col < s.rect.col + s.rect.width
        } else {
            false
        }
    })
}

pub fn set_drop_hover_target(widget_id: Option<u64>) {
    DROP_HOVER_TARGET.with(|target| {
        let mut target = target.borrow_mut();
        if *target != widget_id {
            *target = widget_id;
            bump_widget_state_generation();
        }
    });
}

pub fn active_drop_hover_target() -> Option<u64> {
    DROP_HOVER_TARGET.with(|target| *target.borrow())
}

pub fn drop_target_hovered(widget_id: u64) -> bool {
    active_drop_hover_target() == Some(widget_id)
}

#[cfg(target_os = "macos")]
pub fn push_overlay_primitive(prim: MetalPrimitive) {
    OVERLAY_PRIMITIVES.with(|o| o.borrow_mut().push(prim));
}

#[cfg(target_os = "macos")]
fn drain_overlay_primitives() -> Vec<MetalPrimitive> {
    OVERLAY_PRIMITIVES.with(|o| std::mem::take(&mut *o.borrow_mut()))
}

// ── Flex-style alignment enums ──────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Align {
    Start,
    Center,
    End,
    Stretch,
    /// Align children by their text baseline. Non-text widgets align to the
    /// bottom of the baseline row.
    Baseline,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Justify {
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

pub fn resolve_align(node: &Value, key: &str, default: Align) -> Align {
    let map = match get_map(node) {
        Some(m) => m,
        None => return default,
    };
    match map.get(key) {
        Some(Value::Keyword(s)) | Some(Value::String(s)) => match s.as_str() {
            "start" => Align::Start,
            "center" => Align::Center,
            "end" => Align::End,
            "stretch" => Align::Stretch,
            "baseline" => Align::Baseline,
            _ => default,
        },
        _ => default,
    }
}

/// Computes (start_offset, effective_gap) for main-axis justify distribution.
/// `remaining` is the leftover space after content + explicit gaps.
/// `count` is the number of children. `gap` is the explicit gap between items.
pub fn distribute_justify(justify: Justify, remaining: f32, count: usize, gap: f32) -> (f32, f32) {
    match justify {
        Justify::Start => (0.0, gap),
        Justify::Center => (remaining / 2.0, gap),
        Justify::End => (remaining, gap),
        Justify::SpaceBetween => {
            if count <= 1 {
                (0.0, gap)
            } else {
                (0.0, gap + remaining / (count as f32 - 1.0))
            }
        }
        Justify::SpaceAround => {
            let space = remaining / count as f32;
            (space / 2.0, gap + space)
        }
        Justify::SpaceEvenly => {
            let space = remaining / (count as f32 + 1.0);
            (space, gap + space)
        }
    }
}

pub fn resolve_justify(node: &Value, key: &str, default: Justify) -> Justify {
    let map = match get_map(node) {
        Some(m) => m,
        None => return default,
    };
    match map.get(key) {
        Some(Value::Keyword(s)) | Some(Value::String(s)) => match s.as_str() {
            "start" => Justify::Start,
            "center" => Justify::Center,
            "end" => Justify::End,
            "space-between" => Justify::SpaceBetween,
            "space-around" => Justify::SpaceAround,
            "space-evenly" => Justify::SpaceEvenly,
            _ => default,
        },
        _ => default,
    }
}

// ── Semantic events (backend-agnostic) ───────────────────────────────────────

#[allow(dead_code)]
pub enum WidgetEvent {
    /// Drag/click: normalized 0..1 position within widget
    SetNormalized(f32),
    /// Scroll/arrow: delta in value-space units
    Nudge(f32),
    /// Click/confirm (carries modifier keys from the triggering mouse event)
    Activate(KeyModifiers),
    PointerDown(PointerEvent),
    PointerDrag(PointerDragEvent),
    PointerUp(PointerEvent),
    Key(WidgetKeyEvent),
    Custom(Value),
}

#[derive(Clone, Copy, Debug)]
pub struct PointerEvent {
    pub local_col: f32,
    pub local_row: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct PointerDragEvent {
    pub start_local_col: f32,
    pub start_local_row: f32,
    pub local_col: f32,
    pub local_row: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct WidgetKeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

pub struct EventOutput {
    pub callback: Value,
    pub args: Vec<Value>,
}

pub enum MouseEventOutcome {
    Ignore,
    Consume,
    Dispatch(WidgetEvent),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetCursor {
    Default,
    EwResize,
    DragCopy,
    DragNotAllowed,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WidgetInstance {
    pub ndc_min: [f32; 2],
    pub ndc_max: [f32; 2],
    pub value_t: f32,
    pub orientation: f32,
    pub itime: f32,
    pub uniform_a: [f32; 4],
    pub uniform_b: [f32; 4],
    pub color_a: [f32; 4],
    pub color_b: [f32; 4],
    pub color_c: [f32; 4],
    pub color_d: [f32; 4],
    pub corner_radius: f32,
    pub pixel_aspect: f32,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
pub struct WidgetViewport {
    pub cell_w: f32,
    pub cell_h: f32,
    pub vp_w: f32,
    pub vp_h: f32,
    pub time_seconds: f32,
    pub focused_widget_id: Option<u64>,
    pub focused_branch: bool,
    /// Tile content area height in rows (excludes status bar).
    /// Used by overlays (dropdowns) to clamp to the visible tile region.
    pub tile_content_rows: f32,
    /// Total vertical scroll already applied before tile-position offset.
    /// This includes tile-level widget scroll and any ancestor scroll widgets.
    pub scroll_top: f32,
    /// Total horizontal scroll already applied before tile-position offset.
    pub scroll_left: f32,
    /// True when a parent container is currently hovered.
    pub inherited_hover: bool,
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub struct MetalRectPrimitive {
    pub rect: Rect,
    pub color: Color,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
pub struct MetalQuadPrimitive {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: Color,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
pub struct MetalTrianglePrimitive {
    pub points: [[f32; 2]; 3],
    pub color: Color,
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub struct MetalGlyphRunPrimitive {
    pub row: f32,
    pub col: i32,
    pub text: String,
    pub fg: Color,
    pub bg: Color,
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub struct MetalProportionalTextPrimitive {
    /// Position in cell-space (fractional allowed).
    pub row: f32,
    pub col: f32,
    /// Optional horizontal alignment box in cell-space. When positive,
    /// `h_align` controls where text sits inside this width: 0=start,
    /// 0.5=center, 1=end.
    pub align_width: f32,
    pub h_align: f32,
    pub text: String,
    /// Font size in points.
    pub font_size: f32,
    /// Additional geometry scale applied to glyph quads after rasterization.
    pub scale: f32,
    pub fg: Color,
    pub bg: Color,
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub struct MetalWaveformPrimitive {
    pub rect: Rect,
    pub sample_key: String,
    pub sample_start: f32,
    pub sample_end: f32,
    pub samples_per_bucket: u32,
    pub bucket_count: u32,
    pub selection_start: f32,
    pub selection_end: f32,
    pub show_selection_start: bool,
    pub show_selection_end: bool,
    pub playhead_position: f32,
    pub show_playhead: bool,
    pub waveform_color: Color,
    pub inactive_waveform_color: Color,
    pub marker_color: Color,
    pub active_marker_color: Color,
    pub active_selection_start: bool,
    pub active_selection_end: bool,
    pub selection_color: Color,
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub struct MetalImagePrimitive {
    pub widget_id: u64,
    pub rect: Rect,
    pub src: String,
    pub fit: ImageFit,
    pub radius_px: f32,
    pub opacity: f32,
    pub rotation: f32,
    pub rotation_speed: f32,
    pub clip_circle: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFit {
    Cover,
    Contain,
    Stretch,
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub struct MetalPatchCablePrimitive {
    pub start: [f32; 2],
    pub control1: [f32; 2],
    pub control2: [f32; 2],
    pub end: [f32; 2],
    pub radius_px: f32,
    pub color: Color,
    pub is_segmented: bool,
    pub segment_row: f32,
    pub corner_radius_cells: f32,
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub struct MetalCirclePrimitive {
    pub center: [f32; 2],
    pub radius_px: f32,
    pub color: Color,
    pub visible_half: MetalCircleVisibleHalf,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetalCircleVisibleHalf {
    Full,
    Top,
    Bottom,
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub enum MetalPrimitive {
    ZLayer {
        z_index: i32,
        primitive: Box<MetalPrimitive>,
    },
    Rect(MetalRectPrimitive),
    /// Rectangles that must render above foreground widget instances but below
    /// proportional text. This is for widget-local editing overlays such as
    /// text selections and cursors on widgets whose chassis intentionally masks
    /// cables or other canvas geometry.
    ForegroundRect(MetalRectPrimitive),
    Quad(MetalQuadPrimitive),
    Triangle(MetalTrianglePrimitive),
    GlyphRun(MetalGlyphRunPrimitive),
    ProportionalText(MetalProportionalTextPrimitive),
    PatchCable(MetalPatchCablePrimitive),
    Circle(MetalCirclePrimitive),
    Waveform(MetalWaveformPrimitive),
    Image(MetalImagePrimitive),
    WidgetInstance {
        widget_type: String,
        instance: WidgetInstance,
        is_background: bool,
    },
    /// Narrow the scissor rect to the intersection of the current scissor and this rect.
    PushClipRect(Rect),
    /// Restore the previous scissor rect.
    PopClipRect,
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub struct MetalPrimitiveRun {
    pub widget_id: u64,
    pub ordinal: u16,
    pub widget_type: String,
    pub ancestor_widget_ids: Vec<u64>,
    pub primitives: Vec<MetalPrimitive>,
    pub reused_from_previous: bool,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MetalPrimitiveRunKey {
    pub widget_id: u64,
    pub ordinal: u16,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RetainedMetalPrimitiveRunStats {
    pub reused_runs: usize,
    pub rebuilt_runs: usize,
    pub missing_previous_runs: usize,
    pub invalid_previous_runs: usize,
}

#[cfg(target_os = "macos")]
pub fn z_layer(z_index: i32, primitive: MetalPrimitive) -> MetalPrimitive {
    MetalPrimitive::ZLayer {
        z_index,
        primitive: Box::new(primitive),
    }
}

#[cfg(target_os = "macos")]
pub fn effective_z_index(primitive: &MetalPrimitive) -> i32 {
    match primitive {
        MetalPrimitive::ZLayer { z_index, .. } => *z_index,
        _ => 0,
    }
}

#[cfg(target_os = "macos")]
pub fn innermost_primitive(primitive: &MetalPrimitive) -> &MetalPrimitive {
    match primitive {
        MetalPrimitive::ZLayer { primitive, .. } => innermost_primitive(primitive),
        primitive => primitive,
    }
}

#[cfg(target_os = "macos")]
pub fn metal_widget_instance(widget_type: &str, instance: WidgetInstance) -> Vec<MetalPrimitive> {
    vec![MetalPrimitive::WidgetInstance {
        widget_type: widget_type.to_string(),
        instance,
        is_background: false,
    }]
}

// ── CellBuffer ───────────────────────────────────────────────────────────────

/// A 2D grid of optional cells for widget rendering.
/// `None` cells are transparent — they don't overwrite whatever is behind them.
pub struct CellBuffer {
    pub cells: Vec<Vec<Option<Cell>>>,
    pub width: u16,
    pub height: u16,
}

impl CellBuffer {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            cells: vec![vec![None; width as usize]; height as usize],
            width,
            height,
        }
    }

    pub fn set(&mut self, row: u16, col: u16, cell: Cell) {
        if (row as usize) < self.cells.len() {
            let row_cells = &mut self.cells[row as usize];
            if (col as usize) < row_cells.len() {
                row_cells[col as usize] = Some(cell);
            }
        }
    }

    #[allow(dead_code)]
    pub fn get(&self, row: u16, col: u16) -> Option<&Cell> {
        self.cells
            .get(row as usize)
            .and_then(|r| r.get(col as usize))
            .and_then(|c| c.as_ref())
    }
}

pub trait WidgetDefinition: Sync {
    fn names(&self) -> &'static [&'static str];
    fn is_container(&self) -> bool {
        false
    }
    fn size_affecting_props(&self) -> &'static [&'static str] {
        &[]
    }
    fn bindable_props(&self) -> &'static [&'static str] {
        &[]
    }
    fn measure(
        &self,
        node: &Value,
        children: &[Value],
        constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size>;
    fn layout_children(
        &self,
        node: &Value,
        area: Rect,
        children: &[Value],
        aspect: f32,
        layout_ctx: LayoutCtx,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect, LayoutCtx) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let _ = (
            node,
            area,
            children,
            aspect,
            layout_ctx,
            measure_child,
            build_child,
        );
        vec![]
    }
    fn tui_render(&self, _props: &HashMap<String, Value>, _rect: Rect, _buf: &mut CellBuffer) {}
    fn begin_gesture(&self, _node: &LayoutNode, _local_col: f32, _local_row: f32) -> Option<Value> {
        None
    }
    fn mouse_event(
        &self,
        _node: &LayoutNode,
        _mouse_kind: MouseEventKind,
        _local_col: f32,
        _local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        MouseEventOutcome::Ignore
    }

    fn cursor(&self, _node: &LayoutNode, _local_col: f32, _local_row: f32) -> WidgetCursor {
        WidgetCursor::Default
    }
    fn double_click_event(
        &self,
        _node: &LayoutNode,
        _local_col: f32,
        _local_row: f32,
    ) -> Option<WidgetEvent> {
        None
    }
    fn key_event(&self, _node: &LayoutNode, _key: WidgetKeyEvent) -> Option<WidgetEvent> {
        None
    }
    fn magnify_event(
        &self,
        _node: &LayoutNode,
        _local_col: f32,
        _local_row: f32,
        _delta: f64,
    ) -> Option<WidgetEvent> {
        None
    }
    fn scroll_gesture_event(
        &self,
        _node: &LayoutNode,
        _local_col: f32,
        _local_row: f32,
        _delta_x: f32,
        _delta_y: f32,
    ) -> Option<WidgetEvent> {
        None
    }
    fn captures_scroll_gesture(&self) -> bool {
        false
    }
    fn captures_drag(&self) -> bool {
        false
    }
    /// When true, drag is NOT clamped to widget bounds — the raw mouse position
    /// is passed through.
    fn unclamped_drag(&self) -> bool {
        false
    }
    /// When true, the default container focus highlight is suppressed — the
    /// widget renders its own focus styling (e.g. focus ring).
    fn renders_own_focus(&self) -> bool {
        false
    }
    fn handle_event(&self, _node: &LayoutNode, _event: WidgetEvent) -> Option<EventOutput> {
        None
    }
    fn wants_animation_frames(&self, _node: &LayoutNode) -> bool {
        false
    }
    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        None
    }
    #[cfg(target_os = "macos")]
    fn metal_vertex_shader(&self, _widget_type: &str) -> Option<&'static str> {
        None
    }
    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        _node: &LayoutNode,
        _viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        Vec::new()
    }
}

static WIDGET_DEFINITIONS: &[&dyn WidgetDefinition] = &[
    &label::LABEL_WIDGET,
    &hslider::HSLIDER_WIDGET,
    &vslider::VSLIDER_WIDGET,
    &button::BUTTON_WIDGET,
    &toggle::TOGGLE_WIDGET,
    &matrix::MATRIX_WIDGET,
    &knob::KNOB_WIDGET,
    &knob_number::KNOB_NUMBER_WIDGET,
    &mixer_meter::MIXER_METER_WIDGET,
    &modulator_curve::MODULATOR_CURVE_WIDGET,
    &number_label::NUMBER_LABEL_WIDGET,
    &patcher::PATCHER_WIDGET,
    &adsr_editor::ADSR_EDITOR_WIDGET,
    &tabs::TABS_WIDGET,
    &timeline::TIMELINE_WIDGET,
    &transport_clock::TRANSPORT_CLOCK_WIDGET,
    &waveform::WAVEFORM_WIDGET,
    &vstack::VSTACK_WIDGET,
    &wrap::WRAP_WIDGET,
    &hstack::HSTACK_WIDGET,
    &box_widget::BOX_WIDGET,
    &grid::GRID_WIDGET,
    &grid::RESPONSIVE_GRID_WIDGET,
    &virtual_vstack::VIRTUAL_VSTACK_WIDGET,
    &image::IMAGE_WIDGET,
    &dropdown::DROPDOWN_WIDGET,
    &number_picker::NUMBER_PICKER_WIDGET,
    &response_curve_editor::RESPONSE_CURVE_EDITOR_WIDGET,
    &scroll::SCROLL_WIDGET,
    &text_input::TEXT_INPUT_WIDGET,
    &text_input::TEXTBOX_WIDGET,
    &tree::TREE_WIDGET,
    &tree::TREE_ROW_BG_WIDGET,
];

pub fn widget_definition(widget_type: &str) -> Option<&'static dyn WidgetDefinition> {
    WIDGET_DEFINITIONS
        .iter()
        .copied()
        .find(|definition| definition.names().contains(&widget_type))
}

pub fn is_layout_widget_type(widget_type: &str) -> bool {
    widget_definition(widget_type)
        .map(WidgetDefinition::is_container)
        .unwrap_or(false)
}

pub fn node_handles_pointer_events(node: &LayoutNode) -> bool {
    let has_pointer_callback = node.props.contains_key("on-click")
        || node.props.contains_key("on-drag")
        || node.props.contains_key("on-drop")
        || node.props.contains_key("on-change")
        || node.props.contains_key("on-mouse-down")
        || node.props.contains_key("on-mouse-up")
        || node.props.contains_key("on-double-click")
        || node.props.contains_key("bind");
    if has_pointer_callback {
        return true;
    }

    if sdf_widget::sdf_widget_def(&node.widget_type).is_some() {
        return false;
    }

    widget_definition(&node.widget_type)
        .map(WidgetDefinition::captures_drag)
        .unwrap_or(false)
}

pub fn render_widget_tree(node: &LayoutNode, buf: &mut CellBuffer) {
    // Always call tui_render (no-op for most containers)
    if let Some(definition) = widget_definition(&node.widget_type) {
        definition.tui_render(&node.props, node.rect, buf);
    }

    // Containers also recurse into children
    if is_layout_widget_type(&node.widget_type) {
        for child in &node.children {
            render_widget_tree(child, buf);
        }
    }
}

pub fn layout_wants_animation_frames(node: &LayoutNode) -> bool {
    widget_definition(&node.widget_type)
        .is_some_and(|definition| definition.wants_animation_frames(node))
        || sdf_widget::sdf_widget_def(&node.widget_type)
            .is_some_and(|definition| definition.animates)
        || node_uses_animated_sdf_material(node)
        || node.children.iter().any(layout_wants_animation_frames)
}

fn node_uses_animated_sdf_material(node: &LayoutNode) -> bool {
    let Some(Value::String(shader_type)) = node.props.get(sdf_widget::SHADER_TYPE_PROP) else {
        return false;
    };
    sdf_widget::sdf_widget_def(shader_type).is_some_and(|definition| definition.animates)
}

#[cfg(target_os = "macos")]
fn cacheable_widget_primitives(widget_type: &str) -> bool {
    matches!(
        widget_type,
        "label"
            | "button"
            | "badge"
            | "slider"
            | "hslider"
            | "vslider"
            | "toggle"
            | "knob"
            | "tabs"
            | "box"
    )
}

#[cfg(target_os = "macos")]
fn hash_value(value: &Value, hasher: &mut DefaultHasher) {
    std::mem::discriminant(value).hash(hasher);
    match value {
        Value::Number(n) => n.to_bits().hash(hasher),
        Value::Bool(b) => b.hash(hasher),
        Value::Nil => {}
        Value::String(s) | Value::Symbol(s) | Value::Keyword(s) => s.hash(hasher),
        Value::List(items) => {
            items.len().hash(hasher);
            for item in items {
                hash_value(&item.borrow(), hasher);
            }
        }
        Value::Map(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            keys.len().hash(hasher);
            for key in keys {
                key.hash(hasher);
                if let Some(value) = map.get(key) {
                    hash_value(&value.borrow(), hasher);
                }
            }
        }
        Value::Closure(idx, _) | Value::Function(idx) => idx.hash(hasher),
        Value::NodeRef(id) => id.hash(hasher),
        Value::ReactiveRef {
            namespace,
            field,
            index,
            kind,
            ..
        } => {
            namespace.hash(hasher);
            field.hash(hasher);
            index.hash(hasher);
            kind.hash(hasher);
        }
        Value::NativeFunction(_) => {}
    }
}

#[cfg(target_os = "macos")]
fn widget_primitive_cache_key(node: &LayoutNode, viewport: WidgetViewport) -> Option<u64> {
    if overlay_widget_id().is_some() || !cacheable_widget_primitives(&node.widget_type) {
        return None;
    }
    if props_contain_reactive_ref(&node.props) {
        return None;
    }

    let mut hasher = DefaultHasher::new();
    node.widget_id.hash(&mut hasher);
    node.widget_type.hash(&mut hasher);
    node.rect.row.to_bits().hash(&mut hasher);
    node.rect.col.to_bits().hash(&mut hasher);
    node.rect.width.to_bits().hash(&mut hasher);
    node.rect.height.to_bits().hash(&mut hasher);
    widget_state_generation().hash(&mut hasher);
    theme::generation().hash(&mut hasher);
    viewport.cell_w.to_bits().hash(&mut hasher);
    viewport.cell_h.to_bits().hash(&mut hasher);
    viewport.vp_w.to_bits().hash(&mut hasher);
    viewport.vp_h.to_bits().hash(&mut hasher);
    viewport.focused_widget_id.hash(&mut hasher);
    viewport.focused_branch.hash(&mut hasher);
    viewport.tile_content_rows.to_bits().hash(&mut hasher);
    viewport.scroll_top.to_bits().hash(&mut hasher);
    viewport.scroll_left.to_bits().hash(&mut hasher);
    hash_props(&node.props, &mut hasher);
    Some(hasher.finish())
}

#[cfg(target_os = "macos")]
fn props_contain_reactive_ref(props: &HashMap<String, Value>) -> bool {
    props.values().any(value_contains_reactive_ref)
}

#[cfg(target_os = "macos")]
fn value_contains_reactive_ref(value: &Value) -> bool {
    match value {
        Value::ReactiveRef { .. } => true,
        Value::List(items) => items
            .iter()
            .any(|item| value_contains_reactive_ref(&item.borrow())),
        Value::Map(map) => map
            .values()
            .any(|item| value_contains_reactive_ref(&item.borrow())),
        _ => false,
    }
}

#[cfg(target_os = "macos")]
fn hash_props(props: &HashMap<String, Value>, hasher: &mut DefaultHasher) {
    let mut keys = props.keys().collect::<Vec<_>>();
    keys.sort();
    keys.len().hash(hasher);
    for key in keys {
        key.hash(hasher);
        if let Some(value) = props.get(key) {
            hash_value(value, hasher);
        }
    }
}

#[cfg(target_os = "macos")]
pub fn widget_shader_sources() -> Vec<(&'static str, Option<&'static str>, &'static str)> {
    let mut shaders = Vec::new();
    shaders.push(("tile-tab", None, TILE_TAB_SHADER));
    shaders.push(("patcher-node", None, PATCHER_NODE_SHADER));
    shaders.push(("patcher-port", None, PATCHER_PORT_SHADER));
    shaders.push(("patcher-back-chevron", None, PATCHER_BACK_CHEVRON_SHADER));
    for definition in WIDGET_DEFINITIONS {
        for &name in definition.names() {
            if let Some(fragment_shader) = definition.metal_fragment_shader(name) {
                shaders.push((name, definition.metal_vertex_shader(name), fragment_shader));
            }
        }
    }
    shaders
}

#[cfg(target_os = "macos")]
pub const TILE_TAB_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 p = float2((in.uv.x - 0.5) * 2.0 * in.aspect, (in.uv.y - 0.5) * 2.0);

    float r = in.corner_radius > 0.0 ? in.corner_radius : 0.75;
    r = min(r, min(in.aspect, 1.0));
    float2 half_size = float2(in.aspect - r, 1.0 - r);
    float2 q = abs(p) - half_size;
    float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;

    float edge = fwidth(d) * 1.2;
    float mask = smoothstep(edge, -edge, d);
    if (mask < 0.002) { discard_fragment(); }

    float border_px = in.value_t > 0.5 ? 1.25 : 0.0;
    float inner_d = d + border_px * max(edge, 0.001);
    float inner_mask = smoothstep(edge, -edge, inner_d);
    float border_mask = clamp(mask - inner_mask, 0.0, 1.0);

    float top_light = smoothstep(1.0, 0.10, in.uv.y);
    float bottom_shadow = smoothstep(0.35, 1.0, in.uv.y);
    float3 fill_lit = in.color_a.rgb;
    fill_lit = mix(fill_lit, in.color_c.rgb, in.color_c.a * top_light * 0.05);
    fill_lit = mix(fill_lit, in.color_d.rgb, in.color_d.a * bottom_shadow * 0.05);
    float3 border_lit = in.color_b.rgb;
    border_lit = mix(border_lit, in.color_c.rgb, in.color_c.a * top_light);
    border_lit = mix(border_lit, in.color_d.rgb, in.color_d.a * bottom_shadow);

    float4 fill = float4(fill_lit, in.color_a.a * inner_mask);
    float4 border = float4(border_lit, in.color_b.a * border_mask);
    float out_alpha = fill.a + border.a * (1.0 - fill.a);
    if (out_alpha <= 0.002) { discard_fragment(); }
    float3 out_rgb = (fill.rgb * fill.a + border.rgb * border.a * (1.0 - fill.a)) / out_alpha;
    return float4(out_rgb, out_alpha);
}
"#;

#[cfg(target_os = "macos")]
pub fn widget_primitives_for_node(
    node: &LayoutNode,
    viewport: WidgetViewport,
) -> Vec<MetalPrimitive> {
    let cache_key = widget_primitive_cache_key(node, viewport);
    if let Some(cache_key) = cache_key
        && let Some(cached) =
            WIDGET_PRIMITIVE_CACHE.with(|cache| cache.borrow().get(&cache_key).cloned())
    {
        return cached;
    }

    if let Some(definition) = widget_definition(&node.widget_type) {
        let primitives = definition.build_metal_primitives(&node.widget_type, node, viewport);
        if let Some(cache_key) = cache_key {
            WIDGET_PRIMITIVE_CACHE.with(|cache| {
                let mut cache = cache.borrow_mut();
                if cache.len() >= 4096 {
                    cache.clear();
                }
                cache.insert(cache_key, primitives.clone());
            });
        }
        primitives
    } else if sdf_widget::sdf_widget_def(&node.widget_type).is_some() {
        sdf_widget::sdf_widget_metal_primitives(&node.widget_type, node, viewport)
    } else {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
pub fn collect_metal_primitives(
    node: &LayoutNode,
    viewport: WidgetViewport,
    scroll_top: f32,
    max_rows: u16,
) -> (Vec<MetalPrimitive>, Vec<MetalPrimitive>) {
    let mut primitives = Vec::new();
    collect_metal_primitives_recursive(node, viewport, scroll_top, max_rows, &mut primitives);
    // Overlay content (dropdown menus, etc.) returned separately so the
    // renderer can draw it in its own pass on top of everything.
    let overlay = drain_overlay_primitives();
    (primitives, overlay)
}

#[cfg(target_os = "macos")]
pub fn collect_metal_primitive_runs(
    node: &LayoutNode,
    viewport: WidgetViewport,
    scroll_top: f32,
    max_rows: u16,
) -> (Vec<MetalPrimitiveRun>, Vec<MetalPrimitive>) {
    let mut runs = Vec::new();
    let mut run_ordinals = HashMap::new();
    collect_metal_primitive_runs_recursive(
        node,
        viewport,
        scroll_top,
        max_rows,
        &[],
        &mut run_ordinals,
        &mut runs,
    );
    let overlay = drain_overlay_primitives();
    (runs, overlay)
}

#[cfg(target_os = "macos")]
pub fn collect_metal_primitive_runs_retained(
    node: &LayoutNode,
    viewport: WidgetViewport,
    scroll_top: f32,
    max_rows: u16,
    previous_runs: &[MetalPrimitiveRun],
    dirty_widget_ids: &[u64],
) -> (
    Vec<MetalPrimitiveRun>,
    Vec<MetalPrimitive>,
    RetainedMetalPrimitiveRunStats,
) {
    let previous_by_key: HashMap<MetalPrimitiveRunKey, &MetalPrimitiveRun> = previous_runs
        .iter()
        .map(|run| {
            (
                MetalPrimitiveRunKey {
                    widget_id: run.widget_id,
                    ordinal: run.ordinal,
                },
                run,
            )
        })
        .collect();
    let dirty_widget_ids: HashSet<u64> = dirty_widget_ids.iter().copied().collect();
    let mut stats = RetainedMetalPrimitiveRunStats::default();
    let mut runs = Vec::new();
    let mut run_ordinals = HashMap::new();
    collect_metal_primitive_runs_retained_recursive(
        node,
        viewport,
        scroll_top,
        max_rows,
        &[],
        false,
        &previous_by_key,
        &dirty_widget_ids,
        &mut run_ordinals,
        &mut stats,
        &mut runs,
    );
    let overlay = drain_overlay_primitives();
    (runs, overlay, stats)
}

#[cfg(target_os = "macos")]
pub fn flatten_metal_primitive_runs(runs: &[MetalPrimitiveRun]) -> Vec<MetalPrimitive> {
    runs.iter()
        .flat_map(|run| run.primitives.iter().cloned())
        .collect()
}

#[cfg(target_os = "macos")]
pub fn build_metal_primitive_run_index(
    runs: &[MetalPrimitiveRun],
) -> HashMap<MetalPrimitiveRunKey, usize> {
    runs.iter()
        .enumerate()
        .map(|(index, run)| {
            (
                MetalPrimitiveRunKey {
                    widget_id: run.widget_id,
                    ordinal: run.ordinal,
                },
                index,
            )
        })
        .collect()
}

#[cfg(target_os = "macos")]
pub fn refresh_metal_primitive_runs_retained_in_place(
    node: &LayoutNode,
    viewport: WidgetViewport,
    scroll_top: f32,
    max_rows: u16,
    runs: &mut [MetalPrimitiveRun],
    run_indices: &HashMap<MetalPrimitiveRunKey, usize>,
    dirty_widget_ids: &[u64],
) -> (Vec<MetalPrimitive>, RetainedMetalPrimitiveRunStats) {
    let mut stats = RetainedMetalPrimitiveRunStats::default();
    let mut run_ordinals = HashMap::new();
    let mut rebuilt_indices = Vec::new();
    for run in runs.iter_mut() {
        run.reused_from_previous = true;
    }
    refresh_metal_primitive_runs_retained_in_place_recursive(
        node,
        viewport,
        scroll_top,
        max_rows,
        &[],
        false,
        runs,
        run_indices,
        dirty_widget_ids,
        &mut run_ordinals,
        &mut rebuilt_indices,
        &mut stats,
    );
    let overlay = drain_overlay_primitives();
    (overlay, stats)
}

#[cfg(target_os = "macos")]
fn next_primitive_run_key(
    run_ordinals: &mut HashMap<u64, u16>,
    widget_id: u64,
) -> MetalPrimitiveRunKey {
    let ordinal = run_ordinals.entry(widget_id).or_insert(0);
    let key = MetalPrimitiveRunKey {
        widget_id,
        ordinal: *ordinal,
    };
    *ordinal = ordinal.saturating_add(1);
    key
}

#[cfg(target_os = "macos")]
fn push_primitive_run(
    runs: &mut Vec<MetalPrimitiveRun>,
    run_ordinals: &mut HashMap<u64, u16>,
    widget_id: u64,
    widget_type: &str,
    ancestor_widget_ids: &[u64],
    primitives: Vec<MetalPrimitive>,
) {
    let key = next_primitive_run_key(run_ordinals, widget_id);
    if primitives.is_empty() {
        return;
    }
    runs.push(MetalPrimitiveRun {
        widget_id,
        ordinal: key.ordinal,
        widget_type: widget_type.to_string(),
        ancestor_widget_ids: ancestor_widget_ids.to_vec(),
        primitives,
        reused_from_previous: false,
    });
}

#[cfg(target_os = "macos")]
fn push_retained_primitive_run(
    runs: &mut Vec<MetalPrimitiveRun>,
    run_ordinals: &mut HashMap<u64, u16>,
    previous_by_key: &HashMap<MetalPrimitiveRunKey, &MetalPrimitiveRun>,
    stats: &mut RetainedMetalPrimitiveRunStats,
    dirty_ancestor: bool,
    widget_id: u64,
    widget_type: &str,
    ancestor_widget_ids: &[u64],
    build_primitives: impl FnOnce() -> Vec<MetalPrimitive>,
) {
    let key = next_primitive_run_key(run_ordinals, widget_id);
    if !dirty_ancestor {
        if let Some(previous) = previous_by_key.get(&key) {
            if previous.widget_type == widget_type
                && previous.ancestor_widget_ids.as_slice() == ancestor_widget_ids
            {
                stats.reused_runs += 1;
                let mut reused = (**previous).clone();
                reused.reused_from_previous = true;
                runs.push(reused);
                return;
            }
            stats.invalid_previous_runs += 1;
        } else {
            stats.missing_previous_runs += 1;
        }
    }

    let primitives = build_primitives();
    if primitives.is_empty() {
        return;
    }
    stats.rebuilt_runs += 1;
    runs.push(MetalPrimitiveRun {
        widget_id,
        ordinal: key.ordinal,
        widget_type: widget_type.to_string(),
        ancestor_widget_ids: ancestor_widget_ids.to_vec(),
        primitives,
        reused_from_previous: false,
    });
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn refresh_retained_primitive_run_in_place(
    runs: &mut [MetalPrimitiveRun],
    run_indices: &HashMap<MetalPrimitiveRunKey, usize>,
    run_ordinals: &mut HashMap<u64, u16>,
    rebuilt_indices: &mut Vec<usize>,
    stats: &mut RetainedMetalPrimitiveRunStats,
    dirty_ancestor: bool,
    widget_id: u64,
    widget_type: &str,
    ancestor_widget_ids: &[u64],
    build_primitives: impl FnOnce() -> Vec<MetalPrimitive>,
) {
    let key = next_primitive_run_key(run_ordinals, widget_id);
    let Some(index) = run_indices.get(&key).copied() else {
        if dirty_ancestor {
            let primitives = build_primitives();
            if !primitives.is_empty() {
                stats.missing_previous_runs += 1;
            }
        } else {
            stats.reused_runs += 1;
        }
        return;
    };

    let run = &mut runs[index];
    if run.widget_type != widget_type || run.ancestor_widget_ids.as_slice() != ancestor_widget_ids {
        stats.invalid_previous_runs += 1;
        return;
    }

    if !dirty_ancestor {
        stats.reused_runs += 1;
        run.reused_from_previous = true;
        return;
    }

    run.primitives = build_primitives();
    run.reused_from_previous = false;
    stats.rebuilt_runs += 1;
    rebuilt_indices.push(index);
}

#[cfg(target_os = "macos")]
fn collect_metal_primitives_recursive(
    node: &LayoutNode,
    viewport: WidgetViewport,
    _scroll_top: f32,
    _max_rows: u16,
    primitives: &mut Vec<MetalPrimitive>,
) {
    let node_is_focused = node.focusable && viewport.focused_widget_id == Some(node.widget_id);
    let focused_branch = viewport.focused_branch || node_is_focused;

    // If a container node is focused, emit a background highlight rect.
    // This renders before children (correct z-order: highlight under content).
    // Skip for widgets that render their own focus styling (e.g. text-input).
    let renders_own_focus = widget_definition(&node.widget_type)
        .map(WidgetDefinition::renders_own_focus)
        .unwrap_or(false);
    if node_is_focused && is_layout_widget_type(&node.widget_type) && !renders_own_focus {
        primitives.push(MetalPrimitive::Rect(MetalRectPrimitive {
            rect: node.rect,
            color: crate::theme::WIDGET_FOCUS_BG(),
        }));
    }

    let node_viewport = WidgetViewport {
        scroll_top: _scroll_top,
        focused_branch,
        ..viewport
    };

    // Scroll container: clip children to viewport rect and offset by scroll amount
    if node.widget_type == "scroll" {
        let state = scroll::sync_node_state(node);
        let offset_y = state.offset_y;

        primitives.push(MetalPrimitive::PushClipRect(node.rect));

        for child in &node.children {
            let start = primitives.len();
            collect_metal_primitives_recursive(
                child,
                node_viewport,
                _scroll_top,
                _max_rows,
                primitives,
            );
            // Offset child primitives in-place by negative scroll offset
            for prim in &mut primitives[start..] {
                offset_primitive_y_mut(prim, -offset_y, node_viewport);
            }
        }

        primitives.push(MetalPrimitive::PopClipRect);

        // Scrollbar rendered AFTER children so it draws on top
        primitives.extend(widget_primitives_for_node(node, node_viewport));
        return;
    }

    if node.widget_type == "box" && node.props.contains_key("background") {
        let child_hover =
            crate::widget_render::sdf_widget::get_sdf_hit_state(node.widget_id).hit_region >= 0;
        let child_viewport = WidgetViewport {
            inherited_hover: node_viewport.inherited_hover || child_hover,
            ..node_viewport
        };
        primitives.extend(widget_primitives_for_node(node, node_viewport));
        primitives.push(MetalPrimitive::PushClipRect(node.rect));
        for child in &node.children {
            collect_metal_primitives_recursive(
                child,
                child_viewport,
                _scroll_top,
                _max_rows,
                primitives,
            );
        }
        primitives.push(MetalPrimitive::PopClipRect);
        return;
    }

    primitives.extend(widget_primitives_for_node(node, node_viewport));

    for child in &node.children {
        collect_metal_primitives_recursive(
            child,
            node_viewport,
            _scroll_top,
            _max_rows,
            primitives,
        );
    }
}

#[cfg(target_os = "macos")]
fn collect_metal_primitive_runs_recursive(
    node: &LayoutNode,
    viewport: WidgetViewport,
    _scroll_top: f32,
    _max_rows: u16,
    ancestor_widget_ids: &[u64],
    run_ordinals: &mut HashMap<u64, u16>,
    runs: &mut Vec<MetalPrimitiveRun>,
) {
    let node_is_focused = node.focusable && viewport.focused_widget_id == Some(node.widget_id);
    let focused_branch = viewport.focused_branch || node_is_focused;
    let renders_own_focus = widget_definition(&node.widget_type)
        .map(WidgetDefinition::renders_own_focus)
        .unwrap_or(false);

    let node_viewport = WidgetViewport {
        scroll_top: _scroll_top,
        focused_branch,
        ..viewport
    };

    if node.widget_type == "scroll" {
        let state = scroll::sync_node_state(node);
        let offset_y = state.offset_y;

        let mut own = Vec::new();
        if node_is_focused && is_layout_widget_type(&node.widget_type) && !renders_own_focus {
            own.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: node.rect,
                color: crate::theme::WIDGET_FOCUS_BG(),
            }));
        }
        own.push(MetalPrimitive::PushClipRect(node.rect));
        push_primitive_run(
            runs,
            run_ordinals,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            own,
        );

        let mut child_ancestor_widget_ids = ancestor_widget_ids.to_vec();
        child_ancestor_widget_ids.push(node.widget_id);
        for child in &node.children {
            let start = runs.len();
            collect_metal_primitive_runs_recursive(
                child,
                node_viewport,
                _scroll_top,
                _max_rows,
                &child_ancestor_widget_ids,
                run_ordinals,
                runs,
            );
            for run in &mut runs[start..] {
                if !run.reused_from_previous {
                    for prim in &mut run.primitives {
                        offset_primitive_y_mut(prim, -offset_y, node_viewport);
                    }
                }
            }
        }

        let mut tail = vec![MetalPrimitive::PopClipRect];
        tail.extend(widget_primitives_for_node(node, node_viewport));
        push_primitive_run(
            runs,
            run_ordinals,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            tail,
        );
        return;
    }

    if node.widget_type == "box" && node.props.contains_key("background") {
        let child_hover =
            crate::widget_render::sdf_widget::get_sdf_hit_state(node.widget_id).hit_region >= 0;
        let child_viewport = WidgetViewport {
            inherited_hover: node_viewport.inherited_hover || child_hover,
            ..node_viewport
        };
        let mut own = Vec::new();
        if node_is_focused && is_layout_widget_type(&node.widget_type) && !renders_own_focus {
            own.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: node.rect,
                color: crate::theme::WIDGET_FOCUS_BG(),
            }));
        }
        own.extend(widget_primitives_for_node(node, node_viewport));
        own.push(MetalPrimitive::PushClipRect(node.rect));
        push_primitive_run(
            runs,
            run_ordinals,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            own,
        );

        let mut child_ancestor_widget_ids = ancestor_widget_ids.to_vec();
        child_ancestor_widget_ids.push(node.widget_id);
        for child in &node.children {
            collect_metal_primitive_runs_recursive(
                child,
                child_viewport,
                _scroll_top,
                _max_rows,
                &child_ancestor_widget_ids,
                run_ordinals,
                runs,
            );
        }

        push_primitive_run(
            runs,
            run_ordinals,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            vec![MetalPrimitive::PopClipRect],
        );
        return;
    }

    let mut own = Vec::new();
    if node_is_focused && is_layout_widget_type(&node.widget_type) && !renders_own_focus {
        own.push(MetalPrimitive::Rect(MetalRectPrimitive {
            rect: node.rect,
            color: crate::theme::WIDGET_FOCUS_BG(),
        }));
    }
    own.extend(widget_primitives_for_node(node, node_viewport));
    push_primitive_run(
        runs,
        run_ordinals,
        node.widget_id,
        &node.widget_type,
        ancestor_widget_ids,
        own,
    );

    let mut child_ancestor_widget_ids = ancestor_widget_ids.to_vec();
    child_ancestor_widget_ids.push(node.widget_id);
    for child in &node.children {
        collect_metal_primitive_runs_recursive(
            child,
            node_viewport,
            _scroll_top,
            _max_rows,
            &child_ancestor_widget_ids,
            run_ordinals,
            runs,
        );
    }
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn collect_metal_primitive_runs_retained_recursive(
    node: &LayoutNode,
    viewport: WidgetViewport,
    _scroll_top: f32,
    _max_rows: u16,
    ancestor_widget_ids: &[u64],
    dirty_ancestor: bool,
    previous_by_key: &HashMap<MetalPrimitiveRunKey, &MetalPrimitiveRun>,
    dirty_widget_ids: &HashSet<u64>,
    run_ordinals: &mut HashMap<u64, u16>,
    stats: &mut RetainedMetalPrimitiveRunStats,
    runs: &mut Vec<MetalPrimitiveRun>,
) {
    let node_is_dirty = dirty_widget_ids.contains(&node.widget_id);
    let subtree_dirty = dirty_ancestor || node_is_dirty;
    let node_is_focused = node.focusable && viewport.focused_widget_id == Some(node.widget_id);
    let focused_branch = viewport.focused_branch || node_is_focused;
    let renders_own_focus = widget_definition(&node.widget_type)
        .map(WidgetDefinition::renders_own_focus)
        .unwrap_or(false);

    let node_viewport = WidgetViewport {
        scroll_top: _scroll_top,
        focused_branch,
        ..viewport
    };

    if node.widget_type == "scroll" {
        let state = scroll::sync_node_state(node);
        let offset_y = state.offset_y;

        push_retained_primitive_run(
            runs,
            run_ordinals,
            previous_by_key,
            stats,
            subtree_dirty,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            || {
                let mut own = Vec::new();
                if node_is_focused && is_layout_widget_type(&node.widget_type) && !renders_own_focus
                {
                    own.push(MetalPrimitive::Rect(MetalRectPrimitive {
                        rect: node.rect,
                        color: crate::theme::WIDGET_FOCUS_BG(),
                    }));
                }
                own.push(MetalPrimitive::PushClipRect(node.rect));
                own
            },
        );

        let mut child_ancestor_widget_ids = ancestor_widget_ids.to_vec();
        child_ancestor_widget_ids.push(node.widget_id);
        for child in &node.children {
            let start = runs.len();
            collect_metal_primitive_runs_retained_recursive(
                child,
                node_viewport,
                _scroll_top,
                _max_rows,
                &child_ancestor_widget_ids,
                subtree_dirty,
                previous_by_key,
                dirty_widget_ids,
                run_ordinals,
                stats,
                runs,
            );
            for run in &mut runs[start..] {
                if !run.reused_from_previous {
                    for prim in &mut run.primitives {
                        offset_primitive_y_mut(prim, -offset_y, node_viewport);
                    }
                }
            }
        }

        push_retained_primitive_run(
            runs,
            run_ordinals,
            previous_by_key,
            stats,
            subtree_dirty,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            || {
                let mut tail = vec![MetalPrimitive::PopClipRect];
                tail.extend(widget_primitives_for_node(node, node_viewport));
                tail
            },
        );
        return;
    }

    if node.widget_type == "box" && node.props.contains_key("background") {
        let child_hover =
            crate::widget_render::sdf_widget::get_sdf_hit_state(node.widget_id).hit_region >= 0;
        let child_viewport = WidgetViewport {
            inherited_hover: node_viewport.inherited_hover || child_hover,
            ..node_viewport
        };

        push_retained_primitive_run(
            runs,
            run_ordinals,
            previous_by_key,
            stats,
            subtree_dirty,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            || {
                let mut own = Vec::new();
                if node_is_focused && is_layout_widget_type(&node.widget_type) && !renders_own_focus
                {
                    own.push(MetalPrimitive::Rect(MetalRectPrimitive {
                        rect: node.rect,
                        color: crate::theme::WIDGET_FOCUS_BG(),
                    }));
                }
                own.extend(widget_primitives_for_node(node, node_viewport));
                own.push(MetalPrimitive::PushClipRect(node.rect));
                own
            },
        );

        let mut child_ancestor_widget_ids = ancestor_widget_ids.to_vec();
        child_ancestor_widget_ids.push(node.widget_id);
        for child in &node.children {
            collect_metal_primitive_runs_retained_recursive(
                child,
                child_viewport,
                _scroll_top,
                _max_rows,
                &child_ancestor_widget_ids,
                subtree_dirty,
                previous_by_key,
                dirty_widget_ids,
                run_ordinals,
                stats,
                runs,
            );
        }

        push_retained_primitive_run(
            runs,
            run_ordinals,
            previous_by_key,
            stats,
            subtree_dirty,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            || vec![MetalPrimitive::PopClipRect],
        );
        return;
    }

    push_retained_primitive_run(
        runs,
        run_ordinals,
        previous_by_key,
        stats,
        subtree_dirty,
        node.widget_id,
        &node.widget_type,
        ancestor_widget_ids,
        || {
            let mut own = Vec::new();
            if node_is_focused && is_layout_widget_type(&node.widget_type) && !renders_own_focus {
                own.push(MetalPrimitive::Rect(MetalRectPrimitive {
                    rect: node.rect,
                    color: crate::theme::WIDGET_FOCUS_BG(),
                }));
            }
            own.extend(widget_primitives_for_node(node, node_viewport));
            own
        },
    );

    let mut child_ancestor_widget_ids = ancestor_widget_ids.to_vec();
    child_ancestor_widget_ids.push(node.widget_id);
    for child in &node.children {
        collect_metal_primitive_runs_retained_recursive(
            child,
            node_viewport,
            _scroll_top,
            _max_rows,
            &child_ancestor_widget_ids,
            subtree_dirty,
            previous_by_key,
            dirty_widget_ids,
            run_ordinals,
            stats,
            runs,
        );
    }
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn refresh_metal_primitive_runs_retained_in_place_recursive(
    node: &LayoutNode,
    viewport: WidgetViewport,
    _scroll_top: f32,
    _max_rows: u16,
    ancestor_widget_ids: &[u64],
    dirty_ancestor: bool,
    runs: &mut [MetalPrimitiveRun],
    run_indices: &HashMap<MetalPrimitiveRunKey, usize>,
    dirty_widget_ids: &[u64],
    run_ordinals: &mut HashMap<u64, u16>,
    rebuilt_indices: &mut Vec<usize>,
    stats: &mut RetainedMetalPrimitiveRunStats,
) {
    let node_is_dirty = dirty_widget_ids.contains(&node.widget_id);
    let subtree_dirty = dirty_ancestor || node_is_dirty;
    let node_is_focused = node.focusable && viewport.focused_widget_id == Some(node.widget_id);
    let focused_branch = viewport.focused_branch || node_is_focused;
    let renders_own_focus = widget_definition(&node.widget_type)
        .map(WidgetDefinition::renders_own_focus)
        .unwrap_or(false);

    let node_viewport = WidgetViewport {
        scroll_top: _scroll_top,
        focused_branch,
        ..viewport
    };

    if node.widget_type == "scroll" {
        let state = scroll::sync_node_state(node);
        let offset_y = state.offset_y;

        refresh_retained_primitive_run_in_place(
            runs,
            run_indices,
            run_ordinals,
            rebuilt_indices,
            stats,
            subtree_dirty,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            || {
                let mut own = Vec::new();
                if node_is_focused && is_layout_widget_type(&node.widget_type) && !renders_own_focus
                {
                    own.push(MetalPrimitive::Rect(MetalRectPrimitive {
                        rect: node.rect,
                        color: crate::theme::WIDGET_FOCUS_BG(),
                    }));
                }
                own.push(MetalPrimitive::PushClipRect(node.rect));
                own
            },
        );

        let mut child_ancestor_widget_ids = ancestor_widget_ids.to_vec();
        child_ancestor_widget_ids.push(node.widget_id);
        for child in &node.children {
            let rebuilt_start = rebuilt_indices.len();
            refresh_metal_primitive_runs_retained_in_place_recursive(
                child,
                node_viewport,
                _scroll_top,
                _max_rows,
                &child_ancestor_widget_ids,
                subtree_dirty,
                runs,
                run_indices,
                dirty_widget_ids,
                run_ordinals,
                rebuilt_indices,
                stats,
            );
            for index in &rebuilt_indices[rebuilt_start..] {
                if let Some(run) = runs.get_mut(*index) {
                    for prim in &mut run.primitives {
                        offset_primitive_y_mut(prim, -offset_y, node_viewport);
                    }
                }
            }
        }

        refresh_retained_primitive_run_in_place(
            runs,
            run_indices,
            run_ordinals,
            rebuilt_indices,
            stats,
            subtree_dirty,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            || {
                let mut tail = vec![MetalPrimitive::PopClipRect];
                tail.extend(widget_primitives_for_node(node, node_viewport));
                tail
            },
        );
        return;
    }

    if node.widget_type == "box" && node.props.contains_key("background") {
        let child_hover =
            crate::widget_render::sdf_widget::get_sdf_hit_state(node.widget_id).hit_region >= 0;
        let child_viewport = WidgetViewport {
            inherited_hover: node_viewport.inherited_hover || child_hover,
            ..node_viewport
        };

        refresh_retained_primitive_run_in_place(
            runs,
            run_indices,
            run_ordinals,
            rebuilt_indices,
            stats,
            subtree_dirty,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            || {
                let mut own = Vec::new();
                if node_is_focused && is_layout_widget_type(&node.widget_type) && !renders_own_focus
                {
                    own.push(MetalPrimitive::Rect(MetalRectPrimitive {
                        rect: node.rect,
                        color: crate::theme::WIDGET_FOCUS_BG(),
                    }));
                }
                own.extend(widget_primitives_for_node(node, node_viewport));
                own.push(MetalPrimitive::PushClipRect(node.rect));
                own
            },
        );

        let mut child_ancestor_widget_ids = ancestor_widget_ids.to_vec();
        child_ancestor_widget_ids.push(node.widget_id);
        for child in &node.children {
            refresh_metal_primitive_runs_retained_in_place_recursive(
                child,
                child_viewport,
                _scroll_top,
                _max_rows,
                &child_ancestor_widget_ids,
                subtree_dirty,
                runs,
                run_indices,
                dirty_widget_ids,
                run_ordinals,
                rebuilt_indices,
                stats,
            );
        }

        refresh_retained_primitive_run_in_place(
            runs,
            run_indices,
            run_ordinals,
            rebuilt_indices,
            stats,
            subtree_dirty,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            || vec![MetalPrimitive::PopClipRect],
        );
        return;
    }

    refresh_retained_primitive_run_in_place(
        runs,
        run_indices,
        run_ordinals,
        rebuilt_indices,
        stats,
        subtree_dirty,
        node.widget_id,
        &node.widget_type,
        ancestor_widget_ids,
        || {
            let mut own = Vec::new();
            if node_is_focused && is_layout_widget_type(&node.widget_type) && !renders_own_focus {
                own.push(MetalPrimitive::Rect(MetalRectPrimitive {
                    rect: node.rect,
                    color: crate::theme::WIDGET_FOCUS_BG(),
                }));
            }
            own.extend(widget_primitives_for_node(node, node_viewport));
            own
        },
    );

    let mut child_ancestor_widget_ids = ancestor_widget_ids.to_vec();
    child_ancestor_widget_ids.push(node.widget_id);
    for child in &node.children {
        refresh_metal_primitive_runs_retained_in_place_recursive(
            child,
            node_viewport,
            _scroll_top,
            _max_rows,
            &child_ancestor_widget_ids,
            subtree_dirty,
            runs,
            run_indices,
            dirty_widget_ids,
            run_ordinals,
            rebuilt_indices,
            stats,
        );
    }
}

/// Shift a metal primitive vertically by `dy` cells (in-place).
#[cfg(target_os = "macos")]
fn offset_primitive_y_mut(prim: &mut MetalPrimitive, dy: f32, viewport: WidgetViewport) {
    match prim {
        MetalPrimitive::ZLayer { primitive, .. } => offset_primitive_y_mut(primitive, dy, viewport),
        MetalPrimitive::Rect(r) => r.rect.row += dy,
        MetalPrimitive::ForegroundRect(r) => r.rect.row += dy,
        MetalPrimitive::Quad(q) => q.y += dy,
        MetalPrimitive::Triangle(t) => {
            for point in &mut t.points {
                point[1] += dy;
            }
        }
        MetalPrimitive::GlyphRun(g) => g.row += dy,
        MetalPrimitive::ProportionalText(t) => t.row += dy,
        MetalPrimitive::PatchCable(c) => {
            c.start[1] += dy;
            c.control1[1] += dy;
            c.control2[1] += dy;
            c.end[1] += dy;
            c.segment_row += dy;
        }
        MetalPrimitive::Circle(c) => c.center[1] += dy,
        MetalPrimitive::Waveform(w) => w.rect.row += dy,
        MetalPrimitive::Image(i) => i.rect.row += dy,
        MetalPrimitive::WidgetInstance { instance, .. } => {
            let ndc_dy = -(dy * viewport.cell_h / viewport.vp_h) * 2.0;
            instance.ndc_min[1] += ndc_dy;
            instance.ndc_max[1] += ndc_dy;
        }
        MetalPrimitive::PushClipRect(r) => r.row += dy,
        MetalPrimitive::PopClipRect => {}
    }
}

pub fn handle_event(node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
    if let Some(output) = sdf_widget::sdf_handle_event(node, &event) {
        return Some(output);
    }
    widget_definition(&node.widget_type)?.handle_event(node, event)
}

pub fn map_mouse_event(
    node: &LayoutNode,
    mouse_kind: MouseEventKind,
    local_col: f32,
    local_row: f32,
    drag_start: Option<(f32, f32)>,
    gesture: Option<&Value>,
    modifiers: KeyModifiers,
    cell_w: f32,
    cell_h: f32,
) -> MouseEventOutcome {
    // SDF widgets handle their own mouse events
    if sdf_widget::sdf_widget_def(&node.widget_type).is_some() {
        return sdf_widget::sdf_map_mouse_event(node, mouse_kind, local_col, local_row);
    }
    widget_definition(&node.widget_type)
        .map(|definition| {
            definition.mouse_event(
                node, mouse_kind, local_col, local_row, drag_start, gesture, modifiers, cell_w,
                cell_h,
            )
        })
        .unwrap_or(MouseEventOutcome::Ignore)
}

pub fn cursor_for_node(node: &LayoutNode, local_col: f32, local_row: f32) -> WidgetCursor {
    widget_definition(&node.widget_type)
        .map(|definition| definition.cursor(node, local_col, local_row))
        .unwrap_or(WidgetCursor::Default)
}

pub fn widget_captures_drag(widget_type: &str) -> bool {
    if sdf_widget::sdf_widget_def(widget_type).is_some() {
        return true; // SDF widgets capture drag for :on-drag support
    }
    widget_definition(widget_type)
        .map(WidgetDefinition::captures_drag)
        .unwrap_or(false)
}

pub fn widget_unclamped_drag(widget_type: &str) -> bool {
    widget_definition(widget_type)
        .map(WidgetDefinition::unclamped_drag)
        .unwrap_or(false)
}

pub fn begin_widget_gesture(node: &LayoutNode, local_col: f32, local_row: f32) -> Option<Value> {
    widget_definition(&node.widget_type)?.begin_gesture(node, local_col, local_row)
}

pub fn map_key_event(node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
    widget_definition(&node.widget_type)?.key_event(node, key)
}

pub fn map_double_click_event(
    node: &LayoutNode,
    local_col: f32,
    local_row: f32,
) -> Option<WidgetEvent> {
    widget_definition(&node.widget_type)?.double_click_event(node, local_col, local_row)
}

pub fn map_magnify_event(
    node: &LayoutNode,
    local_col: f32,
    local_row: f32,
    delta: f64,
) -> Option<WidgetEvent> {
    widget_definition(&node.widget_type)?.magnify_event(node, local_col, local_row, delta)
}

pub fn map_scroll_gesture_event(
    node: &LayoutNode,
    local_col: f32,
    local_row: f32,
    delta_x: f32,
    delta_y: f32,
) -> Option<WidgetEvent> {
    widget_definition(&node.widget_type)?
        .scroll_gesture_event(node, local_col, local_row, delta_x, delta_y)
}

pub fn captures_scroll_gesture(node: &LayoutNode) -> bool {
    widget_definition(&node.widget_type)
        .map(WidgetDefinition::captures_scroll_gesture)
        .unwrap_or(false)
}

// ── Shared helpers ───────────────────────────────────────────────────────────

pub fn get_f32_prop(props: &HashMap<String, Value>, key: &str, default: f32) -> f32 {
    let value = match props.get(key) {
        Some(Value::Number(n)) => *n as f32,
        Some(Value::ReactiveRef { slot, .. }) => crate::reactive::read_float_slot(slot) as f32,
        _ => return default,
    };
    if value.is_finite() { value } else { default }
}

pub fn get_bool_prop(props: &HashMap<String, Value>, key: &str, default: bool) -> bool {
    match props.get(key) {
        Some(Value::Bool(b)) => *b,
        _ => default,
    }
}

pub fn resolve_named_color(props: &HashMap<String, Value>, key: &str, default: Color) -> Color {
    match props.get(key) {
        Some(value) => theme::parse_color_value(value).unwrap_or(default),
        _ => default,
    }
}

/// Helper to create a styled cell.
pub fn styled_cell(ch: char, fg: Color, bg: Option<Color>) -> Cell {
    Cell {
        ch,
        style: CellStyle {
            fg,
            bg,
            bold: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clear_haptic_buckets() {
        HAPTIC_BUCKETS.with(|buckets| buckets.borrow_mut().clear());
    }

    #[test]
    fn integer_haptic_only_fires_once_for_stale_previous_bucket() {
        clear_haptic_buckets();
        assert!(should_trigger_integer_haptic(10, 1.0, 2.1, 0.0, 10.0));
        assert!(!should_trigger_integer_haptic(10, 1.0, 2.2, 0.0, 10.0));
        assert!(!should_trigger_integer_haptic(10, 1.0, 2.9, 0.0, 10.0));
        assert!(should_trigger_integer_haptic(10, 1.0, 3.0, 0.0, 10.0));
    }

    #[test]
    fn integer_haptic_buckets_are_relative_to_min() {
        clear_haptic_buckets();
        assert!(!should_trigger_integer_haptic(11, 5.2, 5.9, 5.0, 12.0));
        assert!(should_trigger_integer_haptic(11, 5.2, 6.1, 5.0, 12.0));
    }

    #[test]
    fn mapped_haptic_value_supports_piecewise_exponential_slider() {
        let mut props = HashMap::new();
        props.insert("haptic-value".to_string(), Value::Number(2.0));
        props.insert("haptic-min".to_string(), Value::Number(0.0));
        props.insert("haptic-max".to_string(), Value::Number(32.0));
        props.insert("haptic-pivot-position".to_string(), Value::Number(0.5));
        props.insert("haptic-pivot-value".to_string(), Value::Number(2.0));
        props.insert("haptic-exponent".to_string(), Value::Number(4.0));

        assert!((mapped_haptic_value(&props, 0.25, 0.0) - 1.0).abs() < 0.0001);
        assert!((mapped_haptic_value(&props, 0.5, 0.0) - 2.0).abs() < 0.0001);
        assert!((mapped_haptic_value(&props, 0.75, 0.0) - 3.875).abs() < 0.0001);
        assert!((mapped_haptic_value(&props, 1.0, 0.0) - 32.0).abs() < 0.0001);
    }

    #[cfg(target_os = "macos")]
    fn primitive_token(primitive: &MetalPrimitive) -> String {
        fn color_token(color: Color) -> String {
            format!(
                "{:08x}:{:08x}:{:08x}:{:08x}",
                color.r.to_bits(),
                color.g.to_bits(),
                color.b.to_bits(),
                color.a.to_bits()
            )
        }

        fn rect_token(rect: Rect) -> String {
            format!(
                "{:08x}:{:08x}:{:08x}:{:08x}",
                rect.row.to_bits(),
                rect.col.to_bits(),
                rect.width.to_bits(),
                rect.height.to_bits()
            )
        }

        fn f32s<const N: usize>(values: [f32; N]) -> String {
            values
                .into_iter()
                .map(|value| format!("{:08x}", value.to_bits()))
                .collect::<Vec<_>>()
                .join(":")
        }

        match primitive {
            MetalPrimitive::ZLayer { z_index, primitive } => {
                format!("z:{z_index}:{}", primitive_token(primitive))
            }
            MetalPrimitive::Rect(rect) => {
                format!("rect:{}:{}", rect_token(rect.rect), color_token(rect.color))
            }
            MetalPrimitive::ForegroundRect(rect) => {
                format!(
                    "fg-rect:{}:{}",
                    rect_token(rect.rect),
                    color_token(rect.color)
                )
            }
            MetalPrimitive::Quad(quad) => format!(
                "quad:{:08x}:{:08x}:{:08x}:{:08x}:{}",
                quad.x.to_bits(),
                quad.y.to_bits(),
                quad.width.to_bits(),
                quad.height.to_bits(),
                color_token(quad.color)
            ),
            MetalPrimitive::Triangle(triangle) => format!(
                "tri:{}:{}",
                triangle
                    .points
                    .iter()
                    .flat_map(|point| point.iter())
                    .map(|value| format!("{:08x}", value.to_bits()))
                    .collect::<Vec<_>>()
                    .join(":"),
                color_token(triangle.color)
            ),
            MetalPrimitive::GlyphRun(run) => format!(
                "glyph:{:08x}:{}:{}:{}:{}",
                run.row.to_bits(),
                run.col,
                run.text,
                color_token(run.fg),
                color_token(run.bg)
            ),
            MetalPrimitive::ProportionalText(run) => format!(
                "prop:{:08x}:{:08x}:{:08x}:{:08x}:{}:{:08x}:{:08x}:{}:{}",
                run.row.to_bits(),
                run.col.to_bits(),
                run.align_width.to_bits(),
                run.h_align.to_bits(),
                run.text,
                run.font_size.to_bits(),
                run.scale.to_bits(),
                color_token(run.fg),
                color_token(run.bg)
            ),
            MetalPrimitive::PatchCable(cable) => format!(
                "cable:{}:{}:{}:{}:{:08x}:{}:{}:{:08x}:{:08x}",
                f32s(cable.start),
                f32s(cable.control1),
                f32s(cable.control2),
                f32s(cable.end),
                cable.radius_px.to_bits(),
                color_token(cable.color),
                cable.is_segmented,
                cable.segment_row.to_bits(),
                cable.corner_radius_cells.to_bits()
            ),
            MetalPrimitive::Circle(circle) => format!(
                "circle:{}:{:08x}:{}:{:?}",
                f32s(circle.center),
                circle.radius_px.to_bits(),
                color_token(circle.color),
                circle.visible_half
            ),
            MetalPrimitive::Waveform(waveform) => {
                format!(
                    "waveform:{}:{}",
                    rect_token(waveform.rect),
                    waveform.sample_key
                )
            }
            MetalPrimitive::Image(image) => {
                format!(
                    "image:{}:{}:{}",
                    image.widget_id,
                    rect_token(image.rect),
                    image.src
                )
            }
            MetalPrimitive::WidgetInstance {
                widget_type,
                instance,
                is_background,
            } => format!(
                "widget:{widget_type}:{is_background}:{}:{}:{:08x}:{:08x}:{}:{}:{}:{}:{:08x}:{:08x}",
                f32s(instance.ndc_min),
                f32s(instance.ndc_max),
                instance.value_t.to_bits(),
                instance.orientation.to_bits(),
                f32s(instance.uniform_a),
                f32s(instance.uniform_b),
                f32s(instance.color_a),
                f32s(instance.color_b),
                instance.corner_radius.to_bits(),
                instance.pixel_aspect.to_bits()
            ),
            MetalPrimitive::PushClipRect(rect) => format!("push:{}", rect_token(*rect)),
            MetalPrimitive::PopClipRect => "pop".to_string(),
        }
    }

    #[cfg(target_os = "macos")]
    fn primitive_tokens(primitives: &[MetalPrimitive]) -> Vec<String> {
        primitives.iter().map(primitive_token).collect()
    }

    #[cfg(target_os = "macos")]
    fn test_viewport() -> WidgetViewport {
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 640.0,
            vp_h: 480.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            tile_content_rows: 24.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        }
    }

    #[cfg(target_os = "macos")]
    fn test_node(
        widget_id: u64,
        widget_type: &str,
        rect: Rect,
        props: HashMap<String, Value>,
        children: Vec<LayoutNode>,
    ) -> LayoutNode {
        LayoutNode {
            widget_id,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: widget_type.to_string(),
            rect,
            props,
            children,
            focusable: false,
        }
    }

    #[cfg(target_os = "macos")]
    fn assert_tagged_collection_matches_flat_collection(
        layout: &LayoutNode,
        viewport: WidgetViewport,
    ) {
        let (flat, _) = collect_metal_primitives(layout, viewport, 0.0, 24);
        let (runs, _) = collect_metal_primitive_runs(layout, viewport, 0.0, 24);
        let flattened = flatten_metal_primitive_runs(&runs);
        assert_eq!(primitive_tokens(&flattened), primitive_tokens(&flat));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tagged_metal_collection_matches_flat_collection_for_simple_widgets() {
        let label = test_node(
            2,
            "label",
            Rect {
                row: 1.0,
                col: 1.0,
                width: 8.0,
                height: 1.0,
            },
            HashMap::from([("text".to_string(), Value::String("Rate".to_string()))]),
            Vec::new(),
        );
        let button = test_node(
            3,
            "button",
            Rect {
                row: 3.0,
                col: 1.0,
                width: 6.0,
                height: 1.5,
            },
            HashMap::from([
                ("text".to_string(), Value::String("Apply".to_string())),
                ("font-size".to_string(), Value::Number(12.0)),
            ]),
            Vec::new(),
        );
        let root = test_node(
            1,
            "box",
            Rect {
                row: 0.0,
                col: 0.0,
                width: 12.0,
                height: 8.0,
            },
            HashMap::new(),
            vec![label, button],
        );

        assert_tagged_collection_matches_flat_collection(&root, test_viewport());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tagged_metal_collection_records_ancestor_widget_ids() {
        let label = test_node(
            3,
            "label",
            Rect {
                row: 1.0,
                col: 1.0,
                width: 8.0,
                height: 1.0,
            },
            HashMap::from([("text".to_string(), Value::String("Child".to_string()))]),
            Vec::new(),
        );
        let child_box = test_node(
            2,
            "box",
            Rect {
                row: 0.0,
                col: 0.0,
                width: 10.0,
                height: 4.0,
            },
            HashMap::new(),
            vec![label],
        );
        let root = test_node(
            1,
            "box",
            Rect {
                row: 0.0,
                col: 0.0,
                width: 10.0,
                height: 4.0,
            },
            HashMap::new(),
            vec![child_box],
        );

        let (runs, _) = collect_metal_primitive_runs(&root, test_viewport(), 0.0, 24);
        let label_run = runs
            .iter()
            .find(|run| run.widget_id == 3)
            .expect("label run should be collected");
        assert_eq!(label_run.ancestor_widget_ids, vec![1, 2]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tagged_metal_collection_matches_flat_collection_for_clipped_box() {
        let label = test_node(
            2,
            "label",
            Rect {
                row: 1.0,
                col: 1.0,
                width: 8.0,
                height: 1.0,
            },
            HashMap::from([("text".to_string(), Value::String("Inside".to_string()))]),
            Vec::new(),
        );
        let root = test_node(
            1,
            "box",
            Rect {
                row: 0.0,
                col: 0.0,
                width: 10.0,
                height: 4.0,
            },
            HashMap::from([("background".to_string(), Value::String("panel".to_string()))]),
            vec![label],
        );

        assert_tagged_collection_matches_flat_collection(&root, test_viewport());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tagged_metal_collection_matches_flat_collection_for_scroll() {
        let label = test_node(
            2,
            "label",
            Rect {
                row: 2.0,
                col: 1.0,
                width: 8.0,
                height: 1.0,
            },
            HashMap::from([("text".to_string(), Value::String("Scrolled".to_string()))]),
            Vec::new(),
        );
        let root = test_node(
            1,
            "scroll",
            Rect {
                row: 0.0,
                col: 0.0,
                width: 10.0,
                height: 4.0,
            },
            HashMap::new(),
            vec![label],
        );

        assert_tagged_collection_matches_flat_collection(&root, test_viewport());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn retained_metal_collection_reuses_clean_runs_and_rebuilds_dirty_runs() {
        let make_layout = |label_text: &str| {
            let label = test_node(
                2,
                "label",
                Rect {
                    row: 1.0,
                    col: 1.0,
                    width: 8.0,
                    height: 1.0,
                },
                HashMap::from([("text".to_string(), Value::String(label_text.to_string()))]),
                Vec::new(),
            );
            let button = test_node(
                3,
                "button",
                Rect {
                    row: 3.0,
                    col: 1.0,
                    width: 6.0,
                    height: 1.5,
                },
                HashMap::from([("text".to_string(), Value::String("Apply".to_string()))]),
                Vec::new(),
            );
            test_node(
                1,
                "box",
                Rect {
                    row: 0.0,
                    col: 0.0,
                    width: 12.0,
                    height: 8.0,
                },
                HashMap::new(),
                vec![label, button],
            )
        };
        let viewport = test_viewport();
        let before = make_layout("1");
        let after = make_layout("17");
        let (previous_runs, _) = collect_metal_primitive_runs(&before, viewport, 0.0, 24);
        let (retained_runs, _, stats) =
            collect_metal_primitive_runs_retained(&after, viewport, 0.0, 24, &previous_runs, &[2]);
        let (full_runs, _) = collect_metal_primitive_runs(&after, viewport, 0.0, 24);

        assert!(stats.reused_runs > 0);
        assert!(stats.rebuilt_runs > 0);
        assert_eq!(
            primitive_tokens(&flatten_metal_primitive_runs(&retained_runs)),
            primitive_tokens(&flatten_metal_primitive_runs(&full_runs))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn retained_metal_collection_rebuilds_descendants_for_dirty_ancestor() {
        let label = test_node(
            2,
            "label",
            Rect {
                row: 1.0,
                col: 1.0,
                width: 8.0,
                height: 1.0,
            },
            HashMap::from([("text".to_string(), Value::String("Inside".to_string()))]),
            Vec::new(),
        );
        let root = test_node(
            1,
            "box",
            Rect {
                row: 0.0,
                col: 0.0,
                width: 10.0,
                height: 4.0,
            },
            HashMap::from([("background".to_string(), Value::String("panel".to_string()))]),
            vec![label],
        );
        let viewport = test_viewport();
        let (previous_runs, _) = collect_metal_primitive_runs(&root, viewport, 0.0, 24);
        let (retained_runs, _, stats) =
            collect_metal_primitive_runs_retained(&root, viewport, 0.0, 24, &previous_runs, &[1]);
        let (full_runs, _) = collect_metal_primitive_runs(&root, viewport, 0.0, 24);

        assert_eq!(stats.reused_runs, 0);
        assert!(stats.rebuilt_runs >= 2);
        assert_eq!(
            primitive_tokens(&flatten_metal_primitive_runs(&retained_runs)),
            primitive_tokens(&flatten_metal_primitive_runs(&full_runs))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn retained_metal_collection_reuses_clean_scrolled_runs_without_double_offset() {
        let label = test_node(
            2,
            "label",
            Rect {
                row: 2.0,
                col: 1.0,
                width: 8.0,
                height: 1.0,
            },
            HashMap::from([("text".to_string(), Value::String("Clean".to_string()))]),
            Vec::new(),
        );
        let button = test_node(
            3,
            "button",
            Rect {
                row: 4.0,
                col: 1.0,
                width: 6.0,
                height: 1.5,
            },
            HashMap::from([("text".to_string(), Value::String("Dirty".to_string()))]),
            Vec::new(),
        );
        let root = test_node(
            1,
            "scroll",
            Rect {
                row: 0.0,
                col: 0.0,
                width: 10.0,
                height: 4.0,
            },
            HashMap::new(),
            vec![label, button],
        );
        let viewport = test_viewport();
        let (previous_runs, _) = collect_metal_primitive_runs(&root, viewport, 0.0, 24);
        let (retained_runs, _, stats) =
            collect_metal_primitive_runs_retained(&root, viewport, 0.0, 24, &previous_runs, &[3]);
        let (full_runs, _) = collect_metal_primitive_runs(&root, viewport, 0.0, 24);

        assert!(stats.reused_runs > 0);
        assert!(stats.rebuilt_runs > 0);
        assert_eq!(
            primitive_tokens(&flatten_metal_primitive_runs(&retained_runs)),
            primitive_tokens(&flatten_metal_primitive_runs(&full_runs))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn retained_metal_collection_refreshes_cached_runs_in_place() {
        let make_layout = |label_text: &str| {
            let label = test_node(
                2,
                "label",
                Rect {
                    row: 1.0,
                    col: 1.0,
                    width: 8.0,
                    height: 1.0,
                },
                HashMap::from([("text".to_string(), Value::String(label_text.to_string()))]),
                Vec::new(),
            );
            let button = test_node(
                3,
                "button",
                Rect {
                    row: 3.0,
                    col: 1.0,
                    width: 6.0,
                    height: 1.5,
                },
                HashMap::from([("text".to_string(), Value::String("Apply".to_string()))]),
                Vec::new(),
            );
            test_node(
                1,
                "box",
                Rect {
                    row: 0.0,
                    col: 0.0,
                    width: 12.0,
                    height: 8.0,
                },
                HashMap::new(),
                vec![label, button],
            )
        };
        let viewport = test_viewport();
        let before = make_layout("1");
        let after = make_layout("17");
        let (mut cached_runs, _) = collect_metal_primitive_runs(&before, viewport, 0.0, 24);
        let run_indices = build_metal_primitive_run_index(&cached_runs);
        let (_overlay, stats) = refresh_metal_primitive_runs_retained_in_place(
            &after,
            viewport,
            0.0,
            24,
            &mut cached_runs,
            &run_indices,
            &[2],
        );
        let (full_runs, _) = collect_metal_primitive_runs(&after, viewport, 0.0, 24);

        assert!(stats.reused_runs > 0);
        assert!(stats.rebuilt_runs > 0);
        assert_eq!(stats.missing_previous_runs, 0);
        assert_eq!(stats.invalid_previous_runs, 0);
        assert_eq!(
            primitive_tokens(&flatten_metal_primitive_runs(&cached_runs)),
            primitive_tokens(&flatten_metal_primitive_runs(&full_runs))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn retained_metal_collection_refreshes_scrolled_cached_runs_without_double_offset() {
        let label = test_node(
            2,
            "label",
            Rect {
                row: 2.0,
                col: 1.0,
                width: 8.0,
                height: 1.0,
            },
            HashMap::from([("text".to_string(), Value::String("Clean".to_string()))]),
            Vec::new(),
        );
        let button = test_node(
            3,
            "button",
            Rect {
                row: 4.0,
                col: 1.0,
                width: 6.0,
                height: 1.5,
            },
            HashMap::from([("text".to_string(), Value::String("Dirty".to_string()))]),
            Vec::new(),
        );
        let root = test_node(
            1,
            "scroll",
            Rect {
                row: 0.0,
                col: 0.0,
                width: 10.0,
                height: 4.0,
            },
            HashMap::new(),
            vec![label, button],
        );
        let viewport = test_viewport();
        let (mut cached_runs, _) = collect_metal_primitive_runs(&root, viewport, 0.0, 24);
        let run_indices = build_metal_primitive_run_index(&cached_runs);
        let (_overlay, stats) = refresh_metal_primitive_runs_retained_in_place(
            &root,
            viewport,
            0.0,
            24,
            &mut cached_runs,
            &run_indices,
            &[3],
        );
        let (full_runs, _) = collect_metal_primitive_runs(&root, viewport, 0.0, 24);

        assert!(stats.reused_runs > 0);
        assert!(stats.rebuilt_runs > 0);
        assert_eq!(
            primitive_tokens(&flatten_metal_primitive_runs(&cached_runs)),
            primitive_tokens(&flatten_metal_primitive_runs(&full_runs))
        );
    }

    #[test]
    fn sdf_defwidget_can_request_animation_frames() {
        sdf_widget::register_sdf_widget(sdf_widget::SdfWidgetDef {
            name: "test-animated-sdf".to_string(),
            shader_source: String::new(),
            sdf_expr: crate::parser::Expression::Number(0.0),
            state_uniforms: Vec::new(),
            bindable_props: Vec::new(),
            region_count: 0,
            width: 1.0,
            height: 1.0,
            paint_margin: 0.0,
            animates: true,
        });
        let node = LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "test-animated-sdf".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 1.0,
                height: 1.0,
            },
            props: HashMap::new(),
            children: Vec::new(),
            focusable: false,
        };

        assert!(layout_wants_animation_frames(&node));
    }

    #[test]
    fn animated_sdf_material_requests_animation_frames() {
        sdf_widget::register_sdf_widget(sdf_widget::SdfWidgetDef {
            name: "test-animated-material".to_string(),
            shader_source: String::new(),
            sdf_expr: crate::parser::Expression::Number(0.0),
            state_uniforms: Vec::new(),
            bindable_props: Vec::new(),
            region_count: 0,
            width: 1.0,
            height: 1.0,
            paint_margin: 0.0,
            animates: true,
        });
        let node = LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "hslider".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 8.0,
                height: 1.0,
            },
            props: HashMap::from([(
                sdf_widget::SHADER_TYPE_PROP.to_string(),
                Value::String("test-animated-material".to_string()),
            )]),
            children: Vec::new(),
            focusable: false,
        };

        assert!(layout_wants_animation_frames(&node));
    }
}

/// Shared rounded-rect SDF shader used by tree-row, text-input, number-picker, dropdown.
/// When `corner_radius > 0`, uses that as the radius (in normalized space).
/// Otherwise defaults to 0.75 (pill-like for small widgets).
#[cfg(target_os = "macos")]
pub const ROUNDED_RECT_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;
    float4 col = in.color_a;

    float2 p = float2((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);

    float r = in.corner_radius > 0.0 ? in.corner_radius : 0.75;
    r = min(r, min(aspect, 1.0));
    float2 half_size = float2(aspect - r, 1.0 - r);
    float2 q = abs(p) - half_size;
    float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;

    float edge = fwidth(d) * 1.2;
    float mask = smoothstep(edge, -edge, d);

    if (mask < 0.002) { discard_fragment(); }
    return float4(col.rgb, col.a * mask);
}
"#;

#[cfg(target_os = "macos")]
pub const PATCHER_PORT_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 p = (in.uv - float2(0.5)) * 2.0;

    if ((in.value_t > 0.0 && p.y < 0.0) || (in.value_t < 0.0 && p.y > 0.0)) {
        discard_fragment();
    }

    float d = length(p);
    float aa = max(fwidth(d), 0.001);
    float outerMask = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, d);
    if (outerMask < 0.002) {
        discard_fragment();
    }

    float innerRadius = clamp(in.uniform_a.x, 0.0, 0.98);
    float innerMask = 1.0 - smoothstep(innerRadius - aa, innerRadius + aa, d);
    float4 col = mix(in.color_a, in.color_b, innerMask);
    return float4(col.rgb, col.a * outerMask);
}
"#;

#[cfg(target_os = "macos")]
pub const PATCHER_BACK_CHEVRON_SHADER: &str = r#"
float patcher_chevron_segment_distance(float2 p, float2 a, float2 b)
{
    float2 pa = p - a;
    float2 ba = b - a;
    float h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0);
    return length(pa - ba * h);
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float aspect = max(in.aspect, 0.001);
    float2 p = float2(in.uv.x * aspect, in.uv.y);
    float scale = min(aspect, 1.0);
    float center_x = aspect * 0.5;
    float2 tip = float2(center_x - 0.18 * scale, 0.50);
    float2 upper = float2(center_x + 0.18 * scale, 0.25);
    float2 lower = float2(center_x + 0.18 * scale, 0.75);

    float d = min(
        patcher_chevron_segment_distance(p, upper, tip),
        patcher_chevron_segment_distance(p, tip, lower));
    float thickness = 0.055 * scale;
    float aa = max(fwidth(d), 0.001);
    float mask = smoothstep(thickness + aa, thickness - aa, d);
    if (mask < 0.002) {
        discard_fragment();
    }

    return float4(in.color_a.rgb, in.color_a.a * mask);
}
"#;

#[cfg(target_os = "macos")]
pub const PATCHER_NODE_SHADER: &str = r#"
float patcher_node_smooth_rounded_rect(float2 pos, float2 size, float radius, float smin, float smax)
{
    return smoothstep(smin, smax, sdf_rounded_rect(pos, size, radius));
}

float3 patcher_node_normal(float2 pos, float2 size, float radius, float eps, float ratio)
{
    float smin = -0.1 * ratio;
    float smax = 1.118;
    float right = patcher_node_smooth_rounded_rect(pos + float2(eps, 0.0), size, radius, smin, smax);
    float left = patcher_node_smooth_rounded_rect(pos - float2(eps, 0.0), size, radius, smin, smax);
    float up = patcher_node_smooth_rounded_rect(pos + float2(0.0, eps), size, radius, smin, smax);
    float down = patcher_node_smooth_rounded_rect(pos - float2(0.0, eps), size, radius, smin, smax);
    return normalize(float3((right - left) / (2.0 * eps), (up - down) / (2.0 * eps), 1.0));
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float aspect = max(in.aspect, 0.001);
    float2 localPos = float2((in.uv.x - 0.5) * 2.0 * aspect, (in.uv.y - 0.5) * 2.0);
    float2 sdfSize = float2(aspect, 1.0);
    float cornerRadius = min(in.corner_radius * 1.5, min(aspect, 1.0));

    float nodeDist = sdf_rounded_rect(localPos, sdfSize, cornerRadius);
    float nodeDerivative = max(fwidth(nodeDist), 0.001);
    float outerAlpha = smoothstep(nodeDerivative, -nodeDerivative, nodeDist);
    if (outerAlpha <= 0.001) {
        discard_fragment();
    }

    float borderThickness = max(in.uniform_a.x, 0.0) * nodeDerivative;
    float2 innerSize = max(sdfSize - float2(borderThickness), float2(0.001));
    float innerDist = sdf_rounded_rect(localPos, innerSize, max(cornerRadius - borderThickness, 0.0));
    float innerDerivative = max(fwidth(innerDist), 0.001);
    float innerAlpha = smoothstep(innerDerivative, -innerDerivative, innerDist);
    float borderMask = outerAlpha * (1.0 - innerAlpha);

    float3 normal = patcher_node_normal(
        localPos,
        sdfSize,
        cornerRadius,
        0.01,
        0.83 / max(min(aspect, 1.0), 0.001));
    float3 viewDir = float3(0.0, 0.0, 1.0);
    float3 lightDir = normalize(float3(-0.9, -0.9, 1.3));
    float diffuse = max(0.0, dot(normal, lightDir));
    float3 halfVector = normalize(lightDir + viewDir);
    float specularRaw = pow(max(0.0, dot(normal, halfVector)), 48.0);
    float specularFadeDistance = clamp(nodeDerivative * 2.5, 0.01, 0.06);
    float specular = specularRaw * smoothstep(0.0, -specularFadeDistance, nodeDist);

    float3 bg = in.color_b.rgb;
    float3 border = in.color_a.rgb;
    float3 litBg = bg * (0.82 + 0.18 * diffuse) + float3(0.20) * specular;
    float3 litBorder = border * (0.76 + 0.24 * diffuse) + float3(0.55) * specular;

    float edgeShade = smoothstep(0.18, 0.98, localPos.y * 0.5 + 0.5);
    litBg *= mix(0.94, 1.04, edgeShade);
    litBorder *= mix(0.88, 1.12, edgeShade);

    float3 color = mix(litBg, litBorder, borderMask);
    return float4(color, outerAlpha * max(in.color_a.a, in.color_b.a));
}
"#;

#[cfg(target_os = "macos")]
pub fn ndc_bounds(rect: Rect, viewport: WidgetViewport) -> ([f32; 2], [f32; 2]) {
    let ndc_x = |px: f32| px / viewport.vp_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / viewport.vp_h * 2.0;
    (
        [
            ndc_x(rect.col * viewport.cell_w),
            ndc_y(rect.row * viewport.cell_h),
        ],
        [
            ndc_x((rect.col + rect.width) * viewport.cell_w),
            ndc_y((rect.row + rect.height) * viewport.cell_h),
        ],
    )
}
