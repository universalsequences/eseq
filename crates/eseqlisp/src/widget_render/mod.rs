pub mod adsr_editor;
pub mod box_widget;
pub mod button;
pub mod dropdown;
pub mod grid;
pub mod hslider;
pub mod hstack;
pub mod image;
pub mod knob;
pub mod knob_number;
pub mod label;
pub mod mixer_meter;
pub mod modulator_curve;
pub mod number_label;
pub mod number_picker;
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

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
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
pub enum MetalPrimitive {
    Rect(MetalRectPrimitive),
    Quad(MetalQuadPrimitive),
    GlyphRun(MetalGlyphRunPrimitive),
    ProportionalText(MetalProportionalTextPrimitive),
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
    &knob::KNOB_WIDGET,
    &knob_number::KNOB_NUMBER_WIDGET,
    &mixer_meter::MIXER_METER_WIDGET,
    &modulator_curve::MODULATOR_CURVE_WIDGET,
    &number_label::NUMBER_LABEL_WIDGET,
    &adsr_editor::ADSR_EDITOR_WIDGET,
    &tabs::TABS_WIDGET,
    &timeline::TIMELINE_WIDGET,
    &transport_clock::TRANSPORT_CLOCK_WIDGET,
    &waveform::WAVEFORM_WIDGET,
    &vstack::VSTACK_WIDGET,
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

/// Shift a metal primitive vertically by `dy` cells (in-place).
#[cfg(target_os = "macos")]
fn offset_primitive_y_mut(prim: &mut MetalPrimitive, dy: f32, viewport: WidgetViewport) {
    match prim {
        MetalPrimitive::Rect(r) => r.rect.row += dy,
        MetalPrimitive::Quad(q) => q.y += dy,
        MetalPrimitive::GlyphRun(g) => g.row += dy,
        MetalPrimitive::ProportionalText(t) => t.row += dy,
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
) -> MouseEventOutcome {
    // SDF widgets handle their own mouse events
    if sdf_widget::sdf_widget_def(&node.widget_type).is_some() {
        return sdf_widget::sdf_map_mouse_event(node, mouse_kind, local_col, local_row);
    }
    widget_definition(&node.widget_type)
        .map(|definition| {
            definition.mouse_event(
                node, mouse_kind, local_col, local_row, drag_start, gesture, modifiers,
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
