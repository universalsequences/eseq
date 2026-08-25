pub mod adsr_editor;
pub mod box_widget;
pub mod button;
pub mod cable;
pub mod compressor_display;
pub mod context_menu;
pub mod dropdown;
pub mod eq8_editor;
pub mod event_view;
pub mod focus_decoration;
pub mod gate_led;
pub mod grid;
pub mod hslider;
pub mod hstack;
pub mod image;
pub mod knob;
pub mod knob_number;
pub mod label;
pub mod lane_preview;
pub mod live_audio;
pub mod linegraph;
pub mod matrix;
pub(crate) mod menu_style;
pub mod mixer_meter;
pub mod modal;
pub mod modulator_curve;
pub mod multiband_meter;
pub mod number_label;
pub mod number_picker;
pub mod patcher;
pub mod phaser_notch;
pub mod piano_keyboard;
pub mod response_curve_editor;
pub mod roar_filter;
pub mod roar_shaper;
pub mod scope;
pub mod scroll;
pub mod sdf_widget;
pub mod sound_glyph;
pub mod spectrogram;
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
pub mod wavetable_viewer;
pub(crate) mod wgsl;
pub mod wrap;

pub use focus_decoration::{FocusCornerStyle, FocusDecoration};

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crossterm::event::{KeyCode, KeyModifiers, MouseEventKind};

use crate::backend::{Cell, CellStyle, Color};
use crate::layout::{
    Constraints, LayoutCtx, LayoutNode, MeasureCtx, Rect, Size, TextMeasurer, get_map,
};
use crate::theme;
use crate::vm::Value;

// ── Widget state generation counter ─────────────────────────────────────────
// Bumped whenever a widget's internal state changes (scroll offset, tree
// expand/collapse, etc.) so that primitive caches can be invalidated.

static WIDGET_STATE_GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static POINTER_HOVER_WIDGET_ID: RefCell<Option<u64>> = const { RefCell::new(None) };
    /// Text measurer available to render-pass code. Widget metrics that are
    /// normally cached as a side effect of measure() (e.g. text-input
    /// per-character widths) can be recomputed on a cache miss instead of
    /// falling back to approximations — measure() may be skipped entirely when
    /// subtree layout reuse re-renders a widget whose size did not change.
    static RENDER_TEXT_MEASURER: RefCell<Option<Rc<dyn TextMeasurer>>> = const { RefCell::new(None) };
}

pub fn bump_widget_state_generation() {
    WIDGET_STATE_GENERATION.fetch_add(1, Ordering::Relaxed);
}

pub fn widget_state_generation() -> u64 {
    WIDGET_STATE_GENERATION.load(Ordering::Relaxed)
}

pub fn set_render_text_measurer(measurer: Rc<dyn TextMeasurer>) {
    RENDER_TEXT_MEASURER.with(|m| *m.borrow_mut() = Some(measurer));
}

pub(crate) fn with_render_text_measurer<R>(f: impl FnOnce(&dyn TextMeasurer) -> R) -> Option<R> {
    RENDER_TEXT_MEASURER.with(|m| m.borrow().as_ref().map(|measurer| f(measurer.as_ref())))
}

/// Updates the widget under the pointer, returning whether the visual state changed.
pub fn set_pointer_hover_widget(widget_id: Option<u64>) -> bool {
    POINTER_HOVER_WIDGET_ID.with(|hovered| {
        let mut hovered = hovered.borrow_mut();
        if *hovered == widget_id {
            return false;
        }
        *hovered = widget_id;
        bump_widget_state_generation();
        true
    })
}

pub fn pointer_hovered(widget_id: u64) -> bool {
    POINTER_HOVER_WIDGET_ID.with(|hovered| *hovered.borrow() == Some(widget_id))
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

pub fn trigger_level_change_haptic() {
    crate::ui::platform::trigger_level_change_haptic();
}

pub fn trigger_alignment_haptic() {
    crate::ui::platform::trigger_alignment_haptic();
}

// ── Overlay system ───────────────────────────────────────────────────────────
// Overlays form a small kind-tagged stack (expected depth ≤ 2: a modal with a
// dropdown above it). One entry per kind at a time: registering a kind that is
// already on the stack replaces that entry in place. Input routes to the
// topmost entry; cache bypasses trigger while any entry is active.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayKind {
    Dropdown,
    Modal,
}

#[derive(Clone, Copy, Debug)]
pub struct OverlayEntry {
    pub widget_id: u64,
    /// Hit-test bounds in layout space (screen-relative, post-scroll).
    pub rect: Rect,
    pub kind: OverlayKind,
}

thread_local! {
    static OVERLAY_STACK: RefCell<Vec<OverlayEntry>> = const { RefCell::new(Vec::new()) };
    static HAPTIC_BUCKETS: RefCell<HashMap<u64, i64>> = RefCell::new(HashMap::new());
    static DROP_HOVER_TARGET: RefCell<Option<u64>> = const { RefCell::new(None) };
    static OVERLAY_PRIMITIVES: RefCell<Vec<GpuPrimitive>> = RefCell::new(Vec::new());
    static WIDGET_PRIMITIVE_CACHE: RefCell<HashMap<u64, Vec<GpuPrimitive>>> = RefCell::new(HashMap::new());
}

/// Register (or refresh) an overlay entry. Replaces the existing entry of the
/// same kind if one is on the stack, otherwise pushes on top.
pub fn push_overlay(entry: OverlayEntry) {
    OVERLAY_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if let Some(existing) = stack.iter_mut().find(|e| e.kind == entry.kind) {
            *existing = entry;
        } else {
            stack.push(entry);
        }
    });
}

/// Dropdown registration shim: dropdowns are the only widgets that used the
/// single-slot API this replaced.
pub fn set_overlay(widget_id: u64, rect: Rect) {
    push_overlay(OverlayEntry {
        widget_id,
        rect,
        kind: OverlayKind::Dropdown,
    });
}

/// Remove the entry owned by `widget_id`, if present. Used when an overlay
/// owner dismisses itself (dropdown close/select/Escape).
pub fn remove_overlay(widget_id: u64) {
    let removed = OVERLAY_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let before = stack.len();
        stack.retain(|entry| entry.widget_id != widget_id);
        before != stack.len()
    });
    if removed {
        if !any_overlay_active() {
            OVERLAY_PRIMITIVES.with(|o| o.borrow_mut().clear());
        }
        bump_widget_state_generation();
    }
}

/// Clear the whole overlay stack. Used when the overlay world is torn down
/// wholesale (buffer/tree switches, hot reload, tests).
pub fn clear_overlay() {
    OVERLAY_STACK.with(|stack| stack.borrow_mut().clear());
    OVERLAY_PRIMITIVES.with(|o| o.borrow_mut().clear());
    bump_widget_state_generation();
}

/// The topmost overlay entry — the input-routing target.
pub fn topmost_overlay() -> Option<OverlayEntry> {
    OVERLAY_STACK.with(|stack| stack.borrow().last().copied())
}

/// True while any overlay is active — the cache-bypass gate.
pub fn any_overlay_active() -> bool {
    OVERLAY_STACK.with(|stack| !stack.borrow().is_empty())
}

/// Widget id of the topmost overlay entry.
pub fn overlay_widget_id() -> Option<u64> {
    topmost_overlay().map(|entry| entry.widget_id)
}

/// Hit rect of the topmost overlay entry.
pub fn get_overlay_rect() -> Option<Rect> {
    topmost_overlay().map(|entry| entry.rect)
}

/// Hit rect of the entry owned by `widget_id`, wherever it sits on the stack.
pub fn overlay_rect_for_widget(widget_id: u64) -> Option<Rect> {
    OVERLAY_STACK.with(|stack| {
        stack
            .borrow()
            .iter()
            .find(|entry| entry.widget_id == widget_id)
            .map(|entry| entry.rect)
    })
}

/// True if the point lies inside the topmost overlay entry's rect.
pub fn overlay_contains(local_col: f32, local_row: f32) -> bool {
    topmost_overlay().is_some_and(|entry| {
        local_row >= entry.rect.row
            && local_row < entry.rect.row + entry.rect.height
            && local_col >= entry.rect.col
            && local_col < entry.rect.col + entry.rect.width
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

pub fn push_overlay_primitive(prim: GpuPrimitive) {
    OVERLAY_PRIMITIVES.with(|o| o.borrow_mut().push(prim));
}

fn drain_overlay_primitives() -> Vec<GpuPrimitive> {
    OVERLAY_PRIMITIVES.with(|o| std::mem::take(&mut *o.borrow_mut()))
}

/// Current length of the overlay-primitive channel. Paired with
/// `split_off_overlay_primitives` to capture primitives that nested widgets
/// (e.g. a dropdown inside a modal) push during a subtree recursion, so the
/// enclosing overlay can re-order them on top of its own content.
fn overlay_primitives_mark() -> usize {
    OVERLAY_PRIMITIVES.with(|o| o.borrow().len())
}

fn split_off_overlay_primitives(mark: usize) -> Vec<GpuPrimitive> {
    OVERLAY_PRIMITIVES.with(|o| {
        let mut prims = o.borrow_mut();
        if mark >= prims.len() {
            Vec::new()
        } else {
            prims.split_off(mark)
        }
    })
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
    NsResize,
    /// Open-hand grab affordance (clip title bars,
    /// docs/arrangement-region-editing-spec.md 3.1 — spec'd as `Move`, drawn
    /// as the grab hand).
    Grab,
    DragCopy,
    DragNotAllowed,
}

#[repr(C)]
#[derive(Clone, Copy)]
#[cfg_attr(feature = "wgpu", derive(bytemuck::Pod, bytemuck::Zeroable))]
pub struct WidgetInstance {
    pub ndc_min: [f32; 2],
    pub ndc_max: [f32; 2],
    pub value_t: f32,
    pub orientation: f32,
    pub itime: f32,
    pub uniform_a: [f32; 4],
    pub uniform_b: [f32; 4],
    pub uniform_c: [f32; 4],
    pub uniform_d: [f32; 4],
    pub color_a: [f32; 4],
    pub color_b: [f32; 4],
    pub color_c: [f32; 4],
    pub color_d: [f32; 4],
    pub corner_radius: f32,
    pub pixel_aspect: f32,
}

/// Raw-pixel UI constants (tile corner radii, the ADSR editor's handle boxes,
/// autocomplete panel radii, …) were authored against macOS's 2x Retina
/// backing store, so a window at scale factor 2.0 renders them 1:1. Any other
/// scale factor multiplies those design pixels by `ui_px_scale()` to keep the
/// same on-screen proportions relative to the cell grid.
pub const UI_DESIGN_REFERENCE_SCALE: f32 = 2.0;

// Bits of 2.0f32: the macOS reference, so backends that never report a scale
// (tests, capture harnesses, the Metal path today) render unchanged.
static UI_WINDOW_SCALE_FACTOR_BITS: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0x4000_0000);

/// Record the window's scale factor. Called by the active backend at window
/// setup and again whenever the compositor reports a new factor.
pub fn set_ui_scale_factor(scale_factor: f32) {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        UI_WINDOW_SCALE_FACTOR_BITS.store(
            scale_factor.to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}

/// Multiplier converting design pixels (authored at the 2x reference scale)
/// into framebuffer pixels for the current window.
pub fn ui_px_scale() -> f32 {
    f32::from_bits(UI_WINDOW_SCALE_FACTOR_BITS.load(std::sync::atomic::Ordering::Relaxed))
        / UI_DESIGN_REFERENCE_SCALE
}

#[derive(Clone, Copy)]
pub struct WidgetViewport {
    pub cell_w: f32,
    pub cell_h: f32,
    pub vp_w: f32,
    pub vp_h: f32,
    pub time_seconds: f32,
    pub focused_widget_id: Option<u64>,
    pub focused_branch: bool,
    /// Bottom edge of the frame-level overlay viewport, expressed in the
    /// widget tree's post-scroll, tile-local row coordinates. The matching
    /// top edge is derived from the full frame height.
    ///
    /// Keeping this coordinate in the viewport makes overlay geometry
    /// independent of the tile's clip rect while ordinary widget primitives
    /// remain clipped to their tile.
    pub overlay_viewport_bottom: f32,
    /// Total vertical scroll already applied before tile-position offset.
    /// This includes tile-level widget scroll and any ancestor scroll widgets.
    pub scroll_top: f32,
    /// Total horizontal scroll already applied before tile-position offset.
    pub scroll_left: f32,
    /// True when a parent container is currently hovered.
    pub inherited_hover: bool,
}

#[derive(Clone)]
pub struct GpuRectPrimitive {
    pub rect: Rect,
    pub color: Color,
}

#[derive(Clone, Copy)]
pub struct GpuQuadPrimitive {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: Color,
}

#[derive(Clone, Copy)]
pub struct GpuTrianglePrimitive {
    pub points: [[f32; 2]; 3],
    pub color: Color,
}

#[derive(Clone)]
pub struct GpuGlyphRunPrimitive {
    pub row: f32,
    pub col: i32,
    pub text: String,
    pub fg: Color,
    pub bg: Color,
}

#[derive(Clone)]
pub struct GpuProportionalTextPrimitive {
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

#[derive(Clone)]
pub struct GpuWaveformPrimitive {
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

#[derive(Clone)]
pub struct GpuWavetablePrimitive {
    pub rect: Rect,
    /// Cache key for the GPU buffer (the bank file path).
    pub bank_key: String,
    /// Full bank data, wave-major; uploaded once per bank_key.
    pub data: std::sync::Arc<Vec<f32>>,
    pub data_revision: u64,
    pub frame_len: u32,
    pub set_base: u32,
    pub waves_in_set: u32,
    pub wave_pos: f32,
    pub warp: f32,
    pub fold: f32,
    /// 0 = bipolar time-domain waveform, 1 = non-negative magnitudes.
    pub domain: u32,
    pub selected_color: Color,
    pub inactive_color: Color,
    pub bg_color: Color,
}

#[derive(Clone)]
pub struct GpuLiveSpectrogramPrimitive {
    pub rect: Rect,
    pub data_key: String,
    pub mode: u32,
    pub freq_scale: u32,
    pub min_hz: f32,
    pub max_hz: f32,
    pub min_color: Color,
    pub mid_color: Color,
    pub max_color: Color,
    pub eq_line_color: Color,
    pub eq_fill_color: Color,
    pub background_color: Color,
}

#[derive(Clone)]
pub struct GpuImagePrimitive {
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

#[derive(Clone)]
pub struct GpuPatchCablePrimitive {
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

#[derive(Clone)]
pub struct GpuCirclePrimitive {
    pub center: [f32; 2],
    pub radius_px: f32,
    pub color: Color,
    pub visible_half: GpuCircleVisibleHalf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuCircleVisibleHalf {
    Full,
    Top,
    Bottom,
}

#[derive(Clone)]
pub enum GpuPrimitive {
    ZLayer {
        z_index: i32,
        primitive: Box<GpuPrimitive>,
    },
    Rect(GpuRectPrimitive),
    /// Rectangles that must render above foreground widget instances but below
    /// proportional text. This is for widget-local editing overlays such as
    /// text selections and cursors on widgets whose chassis intentionally masks
    /// cables or other canvas geometry.
    ForegroundRect(GpuRectPrimitive),
    Quad(GpuQuadPrimitive),
    Triangle(GpuTrianglePrimitive),
    GlyphRun(GpuGlyphRunPrimitive),
    ProportionalText(GpuProportionalTextPrimitive),
    PatchCable(GpuPatchCablePrimitive),
    Circle(GpuCirclePrimitive),
    Waveform(GpuWaveformPrimitive),
    Wavetable(GpuWavetablePrimitive),
    LiveSpectrogram(GpuLiveSpectrogramPrimitive),
    Image(GpuImagePrimitive),
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

#[derive(Clone)]
pub struct GpuPrimitiveRun {
    pub widget_id: u64,
    pub ordinal: u16,
    pub widget_type: String,
    pub ancestor_widget_ids: Vec<u64>,
    pub primitives: Vec<GpuPrimitive>,
    pub reused_from_previous: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GpuPrimitiveRunKey {
    pub widget_id: u64,
    pub ordinal: u16,
}

#[derive(Clone, Default)]
pub struct GpuPrimitiveRunIndex {
    by_key: HashMap<GpuPrimitiveRunKey, usize>,
    ancestor_widget_ids: HashMap<u64, Vec<u64>>,
    subtree_run_indices: HashMap<u64, Vec<usize>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RetainedGpuPrimitiveRunStats {
    pub reused_runs: usize,
    pub rebuilt_runs: usize,
    pub missing_previous_runs: usize,
    pub invalid_previous_runs: usize,
}

pub fn z_layer(z_index: i32, primitive: GpuPrimitive) -> GpuPrimitive {
    GpuPrimitive::ZLayer {
        z_index,
        primitive: Box::new(primitive),
    }
}

pub fn effective_z_index(primitive: &GpuPrimitive) -> i32 {
    match primitive {
        GpuPrimitive::ZLayer { z_index, .. } => *z_index,
        _ => 0,
    }
}

pub fn innermost_primitive(primitive: &GpuPrimitive) -> &GpuPrimitive {
    match primitive {
        GpuPrimitive::ZLayer { primitive, .. } => innermost_primitive(primitive),
        primitive => primitive,
    }
}

pub fn gpu_widget_instance(widget_type: &str, instance: WidgetInstance) -> Vec<GpuPrimitive> {
    vec![GpuPrimitive::WidgetInstance {
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

/// Shader language requested by a graphics backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderBackend {
    Msl,
    Wgsl,
}

/// Backend-tagged source bodies for one shader stage.
///
/// A stage may support either backend independently while shader ports are in
/// progress, or carry both bodies once the port is complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShaderSources {
    msl: Option<&'static str>,
    wgsl: Option<&'static str>,
}

impl ShaderSources {
    pub const fn new(msl: Option<&'static str>, wgsl: Option<&'static str>) -> Self {
        Self { msl, wgsl }
    }

    pub const fn msl(source: &'static str) -> Self {
        Self::new(Some(source), None)
    }

    pub const fn both(msl: &'static str, wgsl: &'static str) -> Self {
        Self::new(Some(msl), Some(wgsl))
    }

    pub const fn source(self, backend: ShaderBackend) -> Option<&'static str> {
        match backend {
            ShaderBackend::Msl => self.msl,
            ShaderBackend::Wgsl => self.wgsl,
        }
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
    /// Public Lisp keyword arguments understood by this widget constructor.
    /// This drives context-aware text completion and should include layout,
    /// styling, interaction, and reactive properties accepted by the widget.
    fn completion_props(&self) -> &'static [&'static str] {
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
    fn begin_gesture(
        &self,
        _node: &LayoutNode,
        _local_col: f32,
        _local_row: f32,
        _modifiers: KeyModifiers,
    ) -> Option<Value> {
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
    /// Whether a declined scroll gesture (a `None` from
    /// `scroll_gesture_event`) should still be swallowed instead of bubbling
    /// to an enclosing scroll container. Node-aware so widgets can opt
    /// specific instances into pass-through (e.g. a timeline lane inside a
    /// scroll container that delegates vertical scrolling to it).
    fn captures_scroll_gesture(&self, _node: &LayoutNode) -> bool {
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
    /// Describes whether `wants_animation_frames` is fixed by immutable
    /// layout props or must be re-evaluated against runtime interaction state.
    fn animation_frame_policy(&self) -> AnimationFramePolicy {
        AnimationFramePolicy::Never
    }
    /// Whether this widget's fragment shader reads `WidgetInstance::itime`.
    /// Time-dependent instances must include `itime` in primitive cache keys.
    fn shader_uses_time(&self) -> bool {
        false
    }
    fn fragment_shader(
        &self,
        _widget_type: &str,
        _backend: ShaderBackend,
    ) -> Option<&'static str> {
        None
    }
    fn vertex_shader(
        &self,
        _widget_type: &str,
        _backend: ShaderBackend,
    ) -> Option<&'static str> {
        None
    }
    /// Optional framework-rendered focus decoration. The decoration is added
    /// after this widget's own primitives and is bounded by its measured rect.
    fn focus_decoration(&self, _node: &LayoutNode) -> FocusDecoration {
        FocusDecoration::None
    }
    fn build_primitives(
        &self,
        _widget_type: &str,
        _node: &LayoutNode,
        _viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationFramePolicy {
    Never,
    LayoutStatic,
    RuntimeState,
}

static WIDGET_DEFINITIONS: &[&dyn WidgetDefinition] = &[
    &label::LABEL_WIDGET,
    &lane_preview::LANE_PREVIEW_WIDGET,
    &hslider::HSLIDER_WIDGET,
    &vslider::VSLIDER_WIDGET,
    &button::BUTTON_WIDGET,
    &toggle::TOGGLE_WIDGET,
    &event_view::EVENT_VIEW_WIDGET,
    &piano_keyboard::PIANO_KEYBOARD_WIDGET,
    &linegraph::LINEGRAPH_WIDGET,
    &matrix::MATRIX_WIDGET,
    &knob::KNOB_WIDGET,
    &knob_number::KNOB_NUMBER_WIDGET,
    &mixer_meter::MIXER_METER_WIDGET,
    &modal::MODAL_WIDGET,
    &context_menu::CONTEXT_MENU_WIDGET,
    &context_menu::MENU_ITEM_WIDGET,
    &context_menu::MENU_SEPARATOR_WIDGET,
    &modulator_curve::MODULATOR_CURVE_WIDGET,
    &number_label::NUMBER_LABEL_WIDGET,
    &patcher::PATCHER_WIDGET,
    &adsr_editor::ADSR_EDITOR_WIDGET,
    &tabs::TABS_WIDGET,
    &timeline::TIMELINE_WIDGET,
    &timeline::TIMELINE_CURSOR_MARKER_WIDGET,
    &transport_clock::TRANSPORT_CLOCK_WIDGET,
    &waveform::WAVEFORM_WIDGET,
    &wavetable_viewer::WAVETABLE_VIEWER_WIDGET,
    &sound_glyph::SOUND_GLYPH_WIDGET,
    &spectrogram::SPECTROGRAM_WIDGET,
    &eq8_editor::EQ8_EDITOR_WIDGET,
    &phaser_notch::PHASER_NOTCH_WIDGET,
    &multiband_meter::MULTIBAND_METER_WIDGET,
    &compressor_display::COMPRESSOR_DISPLAY_WIDGET,
    &roar_shaper::ROAR_SHAPER_WIDGET,
    &roar_filter::ROAR_FILTER_WIDGET,
    &gate_led::GATE_LED_WIDGET,
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
    &scope::SCOPE_WIDGET,
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

/// Widgets that render as frame-anchored overlay panels (subtree diverted to
/// the overlay pass, focus/dismissal handled by the modal-family intercepts).
pub fn is_overlay_panel_widget(widget_type: &str) -> bool {
    matches!(widget_type, "modal" | "context-menu")
}

pub fn node_handles_pointer_events(node: &LayoutNode) -> bool {
    let has_pointer_callback = node.props.contains_key("on-click")
        || node.props.contains_key("on-right-click")
        || node.props.contains_key("on-select")
        || node.props.contains_key("on-press")
        || node.props.contains_key("on-release")
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

fn node_wants_animation_frames(node: &LayoutNode) -> bool {
    widget_definition(&node.widget_type)
        .is_some_and(|definition| definition.wants_animation_frames(node))
        || sdf_widget::sdf_widget_def(&node.widget_type)
            .is_some_and(|definition| definition.animates)
        || node_uses_animated_sdf_material(node)
}

pub(crate) fn cache_layout_animation_hints(node: &mut LayoutNode) {
    for child in &mut node.children {
        if !child.animation.initialized {
            cache_layout_animation_hints(child);
        }
    }

    let policy = widget_definition(&node.widget_type)
        .map(WidgetDefinition::animation_frame_policy)
        .unwrap_or(AnimationFramePolicy::Never);
    let sdf_static = sdf_widget::sdf_widget_def(&node.widget_type)
        .is_some_and(|definition| definition.animates)
        || node_uses_animated_sdf_material(node);
    let self_static = sdf_static
        || (policy == AnimationFramePolicy::LayoutStatic
            && widget_definition(&node.widget_type)
                .is_some_and(|definition| definition.wants_animation_frames(node)));
    let self_dynamic = policy == AnimationFramePolicy::RuntimeState;
    node.animation = crate::layout::LayoutAnimationHints {
        initialized: true,
        self_static,
        subtree_static: self_static
            || node
                .children
                .iter()
                .any(|child| child.animation.subtree_static),
        self_dynamic,
        subtree_dynamic: self_dynamic
            || node
                .children
                .iter()
                .any(|child| child.animation.subtree_dynamic),
    };
}

fn layout_wants_animation_frames_uncached(node: &LayoutNode) -> bool {
    node_wants_animation_frames(node)
        || node
            .children
            .iter()
            .any(layout_wants_animation_frames_uncached)
}

pub fn layout_wants_animation_frames(node: &LayoutNode) -> bool {
    if !node.animation.initialized {
        return layout_wants_animation_frames_uncached(node);
    }
    if node.animation.subtree_static {
        return true;
    }
    if !node.animation.subtree_dynamic {
        return false;
    }
    (node.animation.self_dynamic && node_wants_animation_frames(node))
        || node
            .children
            .iter()
            .filter(|child| child.animation.subtree_dynamic)
            .any(layout_wants_animation_frames)
}

pub fn active_animation_widgets(node: &LayoutNode) -> Vec<(u64, &str)> {
    fn collect<'a>(node: &'a LayoutNode, result: &mut Vec<(u64, &'a str)>) {
        if !node.animation.initialized {
            if node_wants_animation_frames(node) {
                result.push((node.widget_id, &node.widget_type));
            }
            for child in &node.children {
                collect(child, result);
            }
            return;
        }
        if !node.animation.subtree_static && !node.animation.subtree_dynamic {
            return;
        }
        if node.animation.self_static
            || (node.animation.self_dynamic && node_wants_animation_frames(node))
        {
            result.push((node.widget_id, &node.widget_type));
        }
        for child in &node.children {
            collect(child, result);
        }
    }

    let mut result = Vec::new();
    collect(node, &mut result);
    result
}

pub fn active_animation_widget_ids(node: &LayoutNode) -> Vec<u64> {
    active_animation_widgets(node)
        .into_iter()
        .map(|(widget_id, _)| widget_id)
        .collect()
}

fn node_uses_animated_sdf_material(node: &LayoutNode) -> bool {
    [sdf_widget::SHADER_TYPE_PROP, "background"]
        .into_iter()
        .filter_map(|prop| match node.props.get(prop) {
            Some(Value::String(shader_type)) => Some(shader_type),
            _ => None,
        })
        .any(|shader_type| {
            sdf_widget::sdf_widget_def(shader_type).is_some_and(|definition| definition.animates)
        })
}

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
            | "linegraph"
            | "tabs"
            | "box"
    )
}

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
        Value::NativeFunction(_) | Value::HostHandle { .. } => {}
        Value::OverrideDispatcher(name) | Value::OverrideOriginal(name) => name.hash(hasher),
    }
}

fn widget_primitive_cache_key(node: &LayoutNode, viewport: WidgetViewport) -> Option<u64> {
    if any_overlay_active() || !cacheable_widget_primitives(&node.widget_type) {
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
    viewport.overlay_viewport_bottom.to_bits().hash(&mut hasher);
    viewport.scroll_top.to_bits().hash(&mut hasher);
    viewport.scroll_left.to_bits().hash(&mut hasher);
    hash_props(&node.props, &mut hasher);
    Some(hasher.finish())
}

fn props_contain_reactive_ref(props: &HashMap<String, Value>) -> bool {
    props.values().any(value_contains_reactive_ref)
}

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

fn hash_props(props: &HashMap<String, Value>, hasher: &mut DefaultHasher) {
    let mut keys = props
        .keys()
        .filter(|key| !is_internal_source_prop(key))
        .collect::<Vec<_>>();
    keys.sort();
    keys.len().hash(hasher);
    for key in keys {
        key.hash(hasher);
        if let Some(value) = props.get(key) {
            hash_value(value, hasher);
        }
    }
}

fn is_internal_source_prop(key: &str) -> bool {
    matches!(
        key,
        crate::vm::SOURCE_BUFFER_ID_PROP
            | crate::vm::SOURCE_MODULE_PATH_PROP
            | crate::vm::SOURCE_SYMBOL_PROP
            | crate::vm::SOURCE_START_BYTE_PROP
            | crate::vm::SOURCE_END_BYTE_PROP
            | crate::vm::SOURCE_REVISION_PROP
    )
}

pub fn widget_shader_sources(
    backend: ShaderBackend,
) -> Vec<(&'static str, Option<&'static str>, &'static str)> {
    let mut shaders = Vec::new();
    for (name, sources) in [
        ("tile-chrome", TILE_CHROME_SHADER),
        ("tile-tab", TILE_TAB_SHADER),
        ("patcher-node", PATCHER_NODE_SHADER),
        ("patcher-panel", PATCHER_PANEL_SHADER),
        ("patcher-port", PATCHER_PORT_SHADER),
        ("patcher-back-chevron", PATCHER_BACK_CHEVRON_SHADER),
    ] {
        if let Some(fragment_shader) = sources.source(backend) {
            shaders.push((name, None, fragment_shader));
        }
    }
    for definition in WIDGET_DEFINITIONS {
        for &name in definition.names() {
            if let Some(fragment_shader) = definition.fragment_shader(name, backend) {
                shaders.push((name, definition.vertex_shader(name, backend), fragment_shader));
            }
        }
    }
    shaders
}

pub const TILE_CHROME_SHADER: ShaderSources = ShaderSources::both(r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float aspect = max(in.aspect, 0.0001);
    float2 p = float2((in.uv.x - 0.5) * 2.0 * aspect, (in.uv.y - 0.5) * 2.0);

    float radius = clamp(in.corner_radius, 0.0, min(aspect, 1.0));
    float2 half_size = float2(aspect, 1.0);
    float d = sdf_rounded_rect(p, half_size, radius);
    float aa = max(fwidth(d), 0.001);
    float outer_mask = smoothstep(aa, -aa, d);

    float border_px = max(in.uniform_a.x, 0.0);
    float border_mask = 0.0;
    float fill_mask = outer_mask;
    if (border_px > 0.0) {
        float border_thickness = border_px * aa;
        float2 inner_size = max(half_size - float2(border_thickness), float2(0.001));
        float inner_radius = max(radius - border_thickness, 0.0);
        float inner_d = sdf_rounded_rect(p, inner_size, inner_radius);
        float inner_aa = max(fwidth(inner_d), 0.001);
        float inner_mask = smoothstep(inner_aa, -inner_aa, inner_d);
        border_mask = clamp(outer_mask - inner_mask, 0.0, 1.0);
        fill_mask = inner_mask;
    }

    float4 fill = float4(in.color_a.rgb, in.color_a.a * fill_mask);
    float4 border = float4(in.color_b.rgb, in.color_b.a * border_mask);
    float out_alpha = fill.a + border.a * (1.0 - fill.a);
    if (out_alpha <= 0.002) {
        discard_fragment();
    }
    float3 out_rgb = (fill.rgb * fill.a + border.rgb * border.a * (1.0 - fill.a)) / out_alpha;
    return float4(out_rgb, out_alpha);
}
"#, wgsl::TILE_CHROME_SHADER);

pub const TILE_TAB_SHADER: ShaderSources = ShaderSources::both(r#"
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
"#, wgsl::TILE_TAB_SHADER);

pub fn widget_primitives_for_node(
    node: &LayoutNode,
    viewport: WidgetViewport,
) -> Vec<GpuPrimitive> {
    let cache_key = widget_primitive_cache_key(node, viewport);
    if let Some(cache_key) = cache_key
        && let Some(cached) =
            WIDGET_PRIMITIVE_CACHE.with(|cache| cache.borrow().get(&cache_key).cloned())
    {
        return cached;
    }

    if let Some(definition) = widget_definition(&node.widget_type) {
        let mut primitives = definition.build_primitives(&node.widget_type, node, viewport);
        if node.focusable && viewport.focused_widget_id == Some(node.widget_id) {
            primitives.extend(
                definition
                    .focus_decoration(node)
                    .primitives(node.rect, viewport),
            );
        }
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
        sdf_widget::sdf_widget_primitives(&node.widget_type, node, viewport)
    } else {
        Vec::new()
    }
}

pub fn collect_gpu_primitives(
    node: &LayoutNode,
    viewport: WidgetViewport,
    scroll_top: f32,
    max_rows: u16,
) -> (Vec<GpuPrimitive>, Vec<GpuPrimitive>) {
    let mut primitives = Vec::new();
    collect_gpu_primitives_recursive(node, viewport, scroll_top, max_rows, &mut primitives);
    // Overlay content (dropdown menus, etc.) returned separately so the
    // renderer can draw it in its own pass on top of everything.
    let overlay = drain_overlay_primitives();
    (primitives, overlay)
}

/// Vertical band (top, bottom in layout rows, in the coordinate space of the
/// nodes being visited) that can be visible inside the innermost enclosing
/// scroll clip. `None` means unclipped. Nodes fully outside the band are
/// culled: everything they would emit is discarded by the scroll clip anyway.
type ScrollCullBand = Option<(f32, f32)>;

fn scroll_cull_band_excludes(node: &LayoutNode, band: ScrollCullBand) -> bool {
    let Some((top, bottom)) = band else {
        return false;
    };
    let node_top = node.rect.row;
    let node_bottom = node.rect.row + node.rect.height;
    if node_bottom > top && node_top < bottom {
        return false;
    }
    // Modal subtrees must be visited every collection — visiting is what
    // registers the overlay, independent of where the anchor scrolled to.
    !subtree_contains_modal(node)
}

fn subtree_contains_modal(node: &LayoutNode) -> bool {
    is_overlay_panel_widget(&node.widget_type) || node.children.iter().any(subtree_contains_modal)
}

/// The cull band for children of a scroll container: the intersection of the
/// incoming band with the scroll clip rect, translated into content
/// coordinates (children are laid out unshifted; the offset is applied to
/// their primitives after collection).
fn scroll_child_cull_band(
    node: &LayoutNode,
    offset_y: f32,
    band: ScrollCullBand,
) -> ScrollCullBand {
    let (band_top, band_bottom) = band.unwrap_or((f32::NEG_INFINITY, f32::INFINITY));
    let top = node.rect.row.max(band_top) + offset_y;
    let bottom = (node.rect.row + node.rect.height).min(band_bottom) + offset_y;
    Some((top, bottom))
}

pub fn collect_gpu_primitive_runs(
    node: &LayoutNode,
    viewport: WidgetViewport,
    scroll_top: f32,
    max_rows: u16,
) -> (Vec<GpuPrimitiveRun>, Vec<GpuPrimitive>) {
    let mut runs = Vec::new();
    let mut run_ordinals = HashMap::new();
    collect_gpu_primitive_runs_recursive(
        node,
        viewport,
        scroll_top,
        max_rows,
        &[],
        None,
        &mut run_ordinals,
        &mut runs,
    );
    let overlay = drain_overlay_primitives();
    (runs, overlay)
}

pub fn collect_gpu_primitive_runs_retained(
    node: &LayoutNode,
    viewport: WidgetViewport,
    scroll_top: f32,
    max_rows: u16,
    previous_runs: &[GpuPrimitiveRun],
    dirty_widget_ids: &[u64],
) -> (
    Vec<GpuPrimitiveRun>,
    Vec<GpuPrimitive>,
    RetainedGpuPrimitiveRunStats,
) {
    let previous_by_key: HashMap<GpuPrimitiveRunKey, &GpuPrimitiveRun> = previous_runs
        .iter()
        .map(|run| {
            (
                GpuPrimitiveRunKey {
                    widget_id: run.widget_id,
                    ordinal: run.ordinal,
                },
                run,
            )
        })
        .collect();
    let dirty_widget_ids: HashSet<u64> = dirty_widget_ids.iter().copied().collect();
    let mut stats = RetainedGpuPrimitiveRunStats::default();
    let mut runs = Vec::new();
    let mut run_ordinals = HashMap::new();
    collect_gpu_primitive_runs_retained_recursive(
        node,
        viewport,
        scroll_top,
        max_rows,
        &[],
        None,
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

pub fn flatten_gpu_primitive_runs(runs: &[GpuPrimitiveRun]) -> Vec<GpuPrimitive> {
    runs.iter()
        .flat_map(|run| run.primitives.iter().cloned())
        .collect()
}

pub fn build_gpu_primitive_run_index(runs: &[GpuPrimitiveRun]) -> GpuPrimitiveRunIndex {
    let mut result = GpuPrimitiveRunIndex::default();
    for (index, run) in runs.iter().enumerate() {
        result.by_key.insert(
            GpuPrimitiveRunKey {
                widget_id: run.widget_id,
                ordinal: run.ordinal,
            },
            index,
        );

        for (depth, widget_id) in run.ancestor_widget_ids.iter().copied().enumerate() {
            result
                .ancestor_widget_ids
                .entry(widget_id)
                .or_insert_with(|| run.ancestor_widget_ids[..depth].to_vec());
            result
                .subtree_run_indices
                .entry(widget_id)
                .or_default()
                .push(index);
        }
        result
            .ancestor_widget_ids
            .entry(run.widget_id)
            .or_insert_with(|| run.ancestor_widget_ids.clone());
        result
            .subtree_run_indices
            .entry(run.widget_id)
            .or_default()
            .push(index);
    }
    result
}

pub fn refresh_gpu_primitive_runs_retained_in_place(
    node: &LayoutNode,
    viewport: WidgetViewport,
    scroll_top: f32,
    max_rows: u16,
    runs: &mut [GpuPrimitiveRun],
    run_indices: &GpuPrimitiveRunIndex,
    dirty_widget_ids: &[u64],
) -> (Vec<GpuPrimitive>, RetainedGpuPrimitiveRunStats) {
    let mut stats = RetainedGpuPrimitiveRunStats::default();
    let mut run_ordinals = HashMap::new();
    let mut rebuilt_indices = Vec::new();
    let mut visited_indices = Vec::new();
    let dirty_widget_ids: HashSet<u64> = dirty_widget_ids.iter().copied().collect();
    let mut relevant_widget_ids = HashSet::new();
    let mut expected_dirty_indices = Vec::new();

    // A paint-only update cannot affect a clean sibling. Use the retained scene
    // index to restrict the layout walk to dirty subtrees and their ancestors.
    // Cached runs inside a dirty subtree are tracked so structural changes still
    // invalidate the retained scene.
    if !dirty_widget_ids.is_empty() {
        for widget_id in &dirty_widget_ids {
            relevant_widget_ids.insert(*widget_id);
            if let Some(ancestors) = run_indices.ancestor_widget_ids.get(widget_id) {
                relevant_widget_ids.extend(ancestors.iter().copied());
            }
            if let Some(indices) = run_indices.subtree_run_indices.get(widget_id) {
                expected_dirty_indices.extend(indices.iter().copied());
            }
        }
        expected_dirty_indices.sort_unstable();
        expected_dirty_indices.dedup();
    }
    let relevant_widget_ids = (!dirty_widget_ids.is_empty()).then_some(&relevant_widget_ids);

    for run in runs.iter_mut() {
        run.reused_from_previous = true;
    }
    refresh_gpu_primitive_runs_retained_in_place_recursive(
        node,
        viewport,
        scroll_top,
        max_rows,
        &[],
        None,
        false,
        runs,
        run_indices,
        &dirty_widget_ids,
        relevant_widget_ids,
        &mut run_ordinals,
        &mut rebuilt_indices,
        &mut visited_indices,
        &mut stats,
    );
    visited_indices.sort_unstable();
    visited_indices.dedup();
    if dirty_widget_ids.is_empty() {
        if visited_indices.len() < runs.len() {
            stats.invalid_previous_runs += runs.len() - visited_indices.len();
        }
    } else {
        stats.invalid_previous_runs += expected_dirty_indices
            .iter()
            .filter(|index| visited_indices.binary_search(index).is_err())
            .count();
    }
    let overlay = drain_overlay_primitives();
    (overlay, stats)
}

fn next_primitive_run_key(
    run_ordinals: &mut HashMap<u64, u16>,
    widget_id: u64,
) -> GpuPrimitiveRunKey {
    let ordinal = run_ordinals.entry(widget_id).or_insert(0);
    let key = GpuPrimitiveRunKey {
        widget_id,
        ordinal: *ordinal,
    };
    *ordinal = ordinal.saturating_add(1);
    key
}

fn push_primitive_run(
    runs: &mut Vec<GpuPrimitiveRun>,
    run_ordinals: &mut HashMap<u64, u16>,
    widget_id: u64,
    widget_type: &str,
    ancestor_widget_ids: &[u64],
    primitives: Vec<GpuPrimitive>,
) {
    let key = next_primitive_run_key(run_ordinals, widget_id);
    if primitives.is_empty() {
        return;
    }
    runs.push(GpuPrimitiveRun {
        widget_id,
        ordinal: key.ordinal,
        widget_type: widget_type.to_string(),
        ancestor_widget_ids: ancestor_widget_ids.to_vec(),
        primitives,
        reused_from_previous: false,
    });
}

fn push_retained_primitive_run(
    runs: &mut Vec<GpuPrimitiveRun>,
    run_ordinals: &mut HashMap<u64, u16>,
    previous_by_key: &HashMap<GpuPrimitiveRunKey, &GpuPrimitiveRun>,
    stats: &mut RetainedGpuPrimitiveRunStats,
    dirty_ancestor: bool,
    widget_id: u64,
    widget_type: &str,
    ancestor_widget_ids: &[u64],
    build_primitives: impl FnOnce() -> Vec<GpuPrimitive>,
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
    runs.push(GpuPrimitiveRun {
        widget_id,
        ordinal: key.ordinal,
        widget_type: widget_type.to_string(),
        ancestor_widget_ids: ancestor_widget_ids.to_vec(),
        primitives,
        reused_from_previous: false,
    });
}

#[allow(clippy::too_many_arguments)]
fn refresh_retained_primitive_run_in_place(
    runs: &mut [GpuPrimitiveRun],
    run_indices: &GpuPrimitiveRunIndex,
    run_ordinals: &mut HashMap<u64, u16>,
    rebuilt_indices: &mut Vec<usize>,
    visited_indices: &mut Vec<usize>,
    stats: &mut RetainedGpuPrimitiveRunStats,
    dirty_ancestor: bool,
    widget_id: u64,
    widget_type: &str,
    ancestor_widget_ids: &[u64],
    build_primitives: impl FnOnce() -> Vec<GpuPrimitive>,
) {
    let key = next_primitive_run_key(run_ordinals, widget_id);
    let Some(index) = run_indices.by_key.get(&key).copied() else {
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

    visited_indices.push(index);
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

fn suppresses_default_focus(node: &LayoutNode) -> bool {
    widget_definition(&node.widget_type)
        .map(|definition| {
            definition.renders_own_focus()
                || !matches!(
                    definition.focus_decoration(node),
                    FocusDecoration::None
                )
        })
        .unwrap_or(false)
}

/// Divert an (open) modal node's entire subtree into the overlay channel:
/// full-frame scrim, panel chrome, then the clipped subtree primitives. The
/// modal contributes nothing to the tile scene. Primitives that nested
/// overlay widgets (a dropdown inside the modal) push during the subtree
/// recursion are captured and re-appended after the modal's own content so
/// they stay on top.
///
/// Shared by all three collectors: the modal subtree is never retained as
/// runs — overlay content is excluded from scene caching, and caches are
/// bypassed while any overlay is active.
fn collect_modal_overlay(
    node: &LayoutNode,
    viewport: WidgetViewport,
    scroll_top: f32,
    max_rows: u16,
) {
    if node.children.is_empty() {
        // Closed this frame: drop any stale overlay entry we own.
        remove_overlay(node.widget_id);
        return;
    }
    let Some((frame_rect, modal_rect)) = modal::overlay_rects_from_props(&node.props) else {
        return;
    };
    // Layout coords -> post-scroll tile-local coords (the overlay channel's
    // convention; the backend offsets overlays by tile origin only). This only
    // stays coherent with the frame-anchored panel because tile scroll is
    // trapped while a modal is open (handle_widget_scroll consumes it).
    let dx = -viewport.scroll_left;
    let dy = -viewport.scroll_top;
    let shift = |rect: Rect| Rect {
        row: rect.row + dy,
        col: rect.col + dx,
        ..rect
    };
    let screen_frame = shift(frame_rect);
    let screen_modal = shift(modal_rect);

    let mark = overlay_primitives_mark();
    let mut subtree = Vec::new();
    for child in &node.children {
        collect_gpu_primitives_recursive(child, viewport, scroll_top, max_rows, &mut subtree);
    }
    let nested_overlay = split_off_overlay_primitives(mark);

    let is_context_menu = node.widget_type == "context-menu";
    if is_context_menu {
        // Context menus draw no scrim: the page beneath stays visible, and a
        // click outside the panel dismisses via the same modal-family
        // intercepts.
        context_menu::emit_menu_chrome(&node.props, screen_modal, viewport);
        push_overlay_primitive(GpuPrimitive::PushClipRect(screen_modal));
    } else {
        // The scrim gets its own clip segment: overlay drawing batches primitive
        // classes within a segment, so an unsegmented scrim rect would paint over
        // the panel background instance.
        push_overlay_primitive(GpuPrimitive::PushClipRect(screen_frame));
        modal::emit_modal_scrim(&node.props, screen_frame, viewport);
        push_overlay_primitive(GpuPrimitive::PopClipRect);
        modal::emit_modal_panel_chrome(&node.props, screen_modal, viewport);
        push_overlay_primitive(GpuPrimitive::PushClipRect(screen_modal));
        modal::emit_modal_title(&node.props, screen_modal, viewport);
    }
    for mut prim in subtree {
        offset_primitive_x_mut(&mut prim, dx, viewport);
        offset_primitive_y_mut(&mut prim, dy, viewport);
        push_overlay_primitive(prim);
    }
    push_overlay_primitive(GpuPrimitive::PopClipRect);
    for prim in nested_overlay {
        push_overlay_primitive(prim);
    }

    push_overlay(OverlayEntry {
        widget_id: node.widget_id,
        rect: screen_modal,
        kind: OverlayKind::Modal,
    });
}

fn collect_gpu_primitives_recursive(
    node: &LayoutNode,
    viewport: WidgetViewport,
    _scroll_top: f32,
    _max_rows: u16,
    primitives: &mut Vec<GpuPrimitive>,
) {
    let node_is_focused = node.focusable && viewport.focused_widget_id == Some(node.widget_id);
    let focused_branch = viewport.focused_branch || node_is_focused;

    // If a container node is focused, emit a background highlight rect.
    // This renders before children (correct z-order: highlight under content).
    // Skip for widgets that render or opt into their own focus styling.
    let suppresses_default_focus = suppresses_default_focus(node);
    if node_is_focused && is_layout_widget_type(&node.widget_type) && !suppresses_default_focus {
        primitives.push(GpuPrimitive::Rect(GpuRectPrimitive {
            rect: node.rect,
            color: crate::theme::WIDGET_FOCUS_BG(),
        }));
    }

    let node_viewport = WidgetViewport {
        scroll_top: _scroll_top,
        focused_branch,
        ..viewport
    };

    // Modal: the whole subtree renders in the overlay pass, nothing in the
    // tile scene.
    if is_overlay_panel_widget(&node.widget_type) {
        collect_modal_overlay(node, node_viewport, _scroll_top, _max_rows);
        return;
    }

    // Scroll container: clip children to viewport rect and offset by scroll amount
    if node.widget_type == "scroll" {
        let state = scroll::sync_node_state(node);
        let offset_y = state.offset_y;

        primitives.push(GpuPrimitive::PushClipRect(node.rect));

        for child in &node.children {
            let start = primitives.len();
            collect_gpu_primitives_recursive(
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

        primitives.push(GpuPrimitive::PopClipRect);

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
        primitives.push(GpuPrimitive::PushClipRect(node.rect));
        for child in &node.children {
            collect_gpu_primitives_recursive(
                child,
                child_viewport,
                _scroll_top,
                _max_rows,
                primitives,
            );
        }
        primitives.push(GpuPrimitive::PopClipRect);
        return;
    }

    primitives.extend(widget_primitives_for_node(node, node_viewport));

    for child in &node.children {
        collect_gpu_primitives_recursive(
            child,
            node_viewport,
            _scroll_top,
            _max_rows,
            primitives,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_gpu_primitive_runs_recursive(
    node: &LayoutNode,
    viewport: WidgetViewport,
    _scroll_top: f32,
    _max_rows: u16,
    ancestor_widget_ids: &[u64],
    cull_band: ScrollCullBand,
    run_ordinals: &mut HashMap<u64, u16>,
    runs: &mut Vec<GpuPrimitiveRun>,
) {
    if scroll_cull_band_excludes(node, cull_band) {
        return;
    }
    let node_is_focused = node.focusable && viewport.focused_widget_id == Some(node.widget_id);
    let focused_branch = viewport.focused_branch || node_is_focused;
    let suppresses_default_focus = suppresses_default_focus(node);

    let node_viewport = WidgetViewport {
        scroll_top: _scroll_top,
        focused_branch,
        ..viewport
    };

    // Modal: subtree renders in the overlay pass only — no retained runs.
    if is_overlay_panel_widget(&node.widget_type) {
        collect_modal_overlay(node, node_viewport, _scroll_top, _max_rows);
        return;
    }

    if node.widget_type == "scroll" {
        let state = scroll::sync_node_state(node);
        let offset_y = state.offset_y;

        let mut own = Vec::new();
        if node_is_focused && is_layout_widget_type(&node.widget_type) && !suppresses_default_focus
        {
            own.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: node.rect,
                color: crate::theme::WIDGET_FOCUS_BG(),
            }));
        }
        own.push(GpuPrimitive::PushClipRect(node.rect));
        push_primitive_run(
            runs,
            run_ordinals,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            own,
        );

        let child_cull_band = scroll_child_cull_band(node, offset_y, cull_band);
        let mut child_ancestor_widget_ids = ancestor_widget_ids.to_vec();
        child_ancestor_widget_ids.push(node.widget_id);
        for child in &node.children {
            let start = runs.len();
            collect_gpu_primitive_runs_recursive(
                child,
                node_viewport,
                _scroll_top,
                _max_rows,
                &child_ancestor_widget_ids,
                child_cull_band,
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

        let mut tail = vec![GpuPrimitive::PopClipRect];
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
        if node_is_focused && is_layout_widget_type(&node.widget_type) && !suppresses_default_focus
        {
            own.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: node.rect,
                color: crate::theme::WIDGET_FOCUS_BG(),
            }));
        }
        own.extend(widget_primitives_for_node(node, node_viewport));
        own.push(GpuPrimitive::PushClipRect(node.rect));
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
            collect_gpu_primitive_runs_recursive(
                child,
                child_viewport,
                _scroll_top,
                _max_rows,
                &child_ancestor_widget_ids,
                cull_band,
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
            vec![GpuPrimitive::PopClipRect],
        );
        return;
    }

    let mut own = Vec::new();
    if node_is_focused && is_layout_widget_type(&node.widget_type) && !suppresses_default_focus {
        own.push(GpuPrimitive::Rect(GpuRectPrimitive {
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
        collect_gpu_primitive_runs_recursive(
            child,
            node_viewport,
            _scroll_top,
            _max_rows,
            &child_ancestor_widget_ids,
            cull_band,
            run_ordinals,
            runs,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_gpu_primitive_runs_retained_recursive(
    node: &LayoutNode,
    viewport: WidgetViewport,
    _scroll_top: f32,
    _max_rows: u16,
    ancestor_widget_ids: &[u64],
    cull_band: ScrollCullBand,
    dirty_ancestor: bool,
    previous_by_key: &HashMap<GpuPrimitiveRunKey, &GpuPrimitiveRun>,
    dirty_widget_ids: &HashSet<u64>,
    run_ordinals: &mut HashMap<u64, u16>,
    stats: &mut RetainedGpuPrimitiveRunStats,
    runs: &mut Vec<GpuPrimitiveRun>,
) {
    if scroll_cull_band_excludes(node, cull_band) {
        return;
    }
    let node_is_dirty = dirty_widget_ids.contains(&node.widget_id);
    let subtree_dirty = dirty_ancestor || node_is_dirty;
    let node_is_focused = node.focusable && viewport.focused_widget_id == Some(node.widget_id);
    let focused_branch = viewport.focused_branch || node_is_focused;
    let suppresses_default_focus = suppresses_default_focus(node);

    let node_viewport = WidgetViewport {
        scroll_top: _scroll_top,
        focused_branch,
        ..viewport
    };

    // Modal: subtree renders in the overlay pass only; overlay content is
    // rebuilt every collection, never retained.
    if is_overlay_panel_widget(&node.widget_type) {
        collect_modal_overlay(node, node_viewport, _scroll_top, _max_rows);
        return;
    }

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
                if node_is_focused
                    && is_layout_widget_type(&node.widget_type)
                    && !suppresses_default_focus
                {
                    own.push(GpuPrimitive::Rect(GpuRectPrimitive {
                        rect: node.rect,
                        color: crate::theme::WIDGET_FOCUS_BG(),
                    }));
                }
                own.push(GpuPrimitive::PushClipRect(node.rect));
                own
            },
        );

        let child_cull_band = scroll_child_cull_band(node, offset_y, cull_band);
        let mut child_ancestor_widget_ids = ancestor_widget_ids.to_vec();
        child_ancestor_widget_ids.push(node.widget_id);
        for child in &node.children {
            let start = runs.len();
            collect_gpu_primitive_runs_retained_recursive(
                child,
                node_viewport,
                _scroll_top,
                _max_rows,
                &child_ancestor_widget_ids,
                child_cull_band,
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
                let mut tail = vec![GpuPrimitive::PopClipRect];
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
                if node_is_focused
                    && is_layout_widget_type(&node.widget_type)
                    && !suppresses_default_focus
                {
                    own.push(GpuPrimitive::Rect(GpuRectPrimitive {
                        rect: node.rect,
                        color: crate::theme::WIDGET_FOCUS_BG(),
                    }));
                }
                own.extend(widget_primitives_for_node(node, node_viewport));
                own.push(GpuPrimitive::PushClipRect(node.rect));
                own
            },
        );

        let mut child_ancestor_widget_ids = ancestor_widget_ids.to_vec();
        child_ancestor_widget_ids.push(node.widget_id);
        for child in &node.children {
            collect_gpu_primitive_runs_retained_recursive(
                child,
                child_viewport,
                _scroll_top,
                _max_rows,
                &child_ancestor_widget_ids,
                cull_band,
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
            || vec![GpuPrimitive::PopClipRect],
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
            if node_is_focused
                && is_layout_widget_type(&node.widget_type)
                && !suppresses_default_focus
            {
                own.push(GpuPrimitive::Rect(GpuRectPrimitive {
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
        collect_gpu_primitive_runs_retained_recursive(
            child,
            node_viewport,
            _scroll_top,
            _max_rows,
            &child_ancestor_widget_ids,
            cull_band,
            subtree_dirty,
            previous_by_key,
            dirty_widget_ids,
            run_ordinals,
            stats,
            runs,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn refresh_gpu_primitive_runs_retained_in_place_recursive(
    node: &LayoutNode,
    viewport: WidgetViewport,
    _scroll_top: f32,
    _max_rows: u16,
    ancestor_widget_ids: &[u64],
    cull_band: ScrollCullBand,
    dirty_ancestor: bool,
    runs: &mut [GpuPrimitiveRun],
    run_indices: &GpuPrimitiveRunIndex,
    dirty_widget_ids: &HashSet<u64>,
    relevant_widget_ids: Option<&HashSet<u64>>,
    run_ordinals: &mut HashMap<u64, u16>,
    rebuilt_indices: &mut Vec<usize>,
    visited_indices: &mut Vec<usize>,
    stats: &mut RetainedGpuPrimitiveRunStats,
) {
    if scroll_cull_band_excludes(node, cull_band) {
        return;
    }
    if !dirty_ancestor
        && relevant_widget_ids.is_some_and(|relevant| !relevant.contains(&node.widget_id))
    {
        return;
    }
    let node_is_dirty = dirty_widget_ids.contains(&node.widget_id);
    let subtree_dirty = dirty_ancestor || node_is_dirty;
    let node_is_focused = node.focusable && viewport.focused_widget_id == Some(node.widget_id);
    let focused_branch = viewport.focused_branch || node_is_focused;
    let suppresses_default_focus = suppresses_default_focus(node);

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
            visited_indices,
            stats,
            subtree_dirty,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            || {
                let mut own = Vec::new();
                if node_is_focused
                    && is_layout_widget_type(&node.widget_type)
                    && !suppresses_default_focus
                {
                    own.push(GpuPrimitive::Rect(GpuRectPrimitive {
                        rect: node.rect,
                        color: crate::theme::WIDGET_FOCUS_BG(),
                    }));
                }
                own.push(GpuPrimitive::PushClipRect(node.rect));
                own
            },
        );

        let child_cull_band = scroll_child_cull_band(node, offset_y, cull_band);
        let mut child_ancestor_widget_ids = ancestor_widget_ids.to_vec();
        child_ancestor_widget_ids.push(node.widget_id);
        for child in &node.children {
            let rebuilt_start = rebuilt_indices.len();
            refresh_gpu_primitive_runs_retained_in_place_recursive(
                child,
                node_viewport,
                _scroll_top,
                _max_rows,
                &child_ancestor_widget_ids,
                child_cull_band,
                subtree_dirty,
                runs,
                run_indices,
                dirty_widget_ids,
                relevant_widget_ids,
                run_ordinals,
                rebuilt_indices,
                visited_indices,
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
            visited_indices,
            stats,
            subtree_dirty,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            || {
                let mut tail = vec![GpuPrimitive::PopClipRect];
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
            visited_indices,
            stats,
            subtree_dirty,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            || {
                let mut own = Vec::new();
                if node_is_focused
                    && is_layout_widget_type(&node.widget_type)
                    && !suppresses_default_focus
                {
                    own.push(GpuPrimitive::Rect(GpuRectPrimitive {
                        rect: node.rect,
                        color: crate::theme::WIDGET_FOCUS_BG(),
                    }));
                }
                own.extend(widget_primitives_for_node(node, node_viewport));
                own.push(GpuPrimitive::PushClipRect(node.rect));
                own
            },
        );

        let mut child_ancestor_widget_ids = ancestor_widget_ids.to_vec();
        child_ancestor_widget_ids.push(node.widget_id);
        for child in &node.children {
            refresh_gpu_primitive_runs_retained_in_place_recursive(
                child,
                child_viewport,
                _scroll_top,
                _max_rows,
                &child_ancestor_widget_ids,
                cull_band,
                subtree_dirty,
                runs,
                run_indices,
                dirty_widget_ids,
                relevant_widget_ids,
                run_ordinals,
                rebuilt_indices,
                visited_indices,
                stats,
            );
        }

        refresh_retained_primitive_run_in_place(
            runs,
            run_indices,
            run_ordinals,
            rebuilt_indices,
            visited_indices,
            stats,
            subtree_dirty,
            node.widget_id,
            &node.widget_type,
            ancestor_widget_ids,
            || vec![GpuPrimitive::PopClipRect],
        );
        return;
    }

    refresh_retained_primitive_run_in_place(
        runs,
        run_indices,
        run_ordinals,
        rebuilt_indices,
        visited_indices,
        stats,
        subtree_dirty,
        node.widget_id,
        &node.widget_type,
        ancestor_widget_ids,
        || {
            let mut own = Vec::new();
            if node_is_focused
                && is_layout_widget_type(&node.widget_type)
                && !suppresses_default_focus
            {
                own.push(GpuPrimitive::Rect(GpuRectPrimitive {
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
        refresh_gpu_primitive_runs_retained_in_place_recursive(
            child,
            node_viewport,
            _scroll_top,
            _max_rows,
            &child_ancestor_widget_ids,
            cull_band,
            subtree_dirty,
            runs,
            run_indices,
            dirty_widget_ids,
            relevant_widget_ids,
            run_ordinals,
            rebuilt_indices,
            visited_indices,
            stats,
        );
    }
}

/// Shift a metal primitive horizontally by `dx` cells (in-place).
fn offset_primitive_x_mut(prim: &mut GpuPrimitive, dx: f32, viewport: WidgetViewport) {
    match prim {
        GpuPrimitive::ZLayer { primitive, .. } => offset_primitive_x_mut(primitive, dx, viewport),
        GpuPrimitive::Rect(r) => r.rect.col += dx,
        GpuPrimitive::ForegroundRect(r) => r.rect.col += dx,
        GpuPrimitive::Quad(q) => q.x += dx,
        GpuPrimitive::Triangle(t) => {
            for point in &mut t.points {
                point[0] += dx;
            }
        }
        GpuPrimitive::GlyphRun(g) => g.col += dx.round() as i32,
        GpuPrimitive::ProportionalText(t) => t.col += dx,
        GpuPrimitive::PatchCable(c) => {
            c.start[0] += dx;
            c.control1[0] += dx;
            c.control2[0] += dx;
            c.end[0] += dx;
        }
        GpuPrimitive::Circle(c) => c.center[0] += dx,
        GpuPrimitive::Waveform(w) => w.rect.col += dx,
        GpuPrimitive::Wavetable(w) => w.rect.col += dx,
        GpuPrimitive::LiveSpectrogram(s) => s.rect.col += dx,
        GpuPrimitive::Image(i) => i.rect.col += dx,
        GpuPrimitive::WidgetInstance { instance, .. } => {
            let ndc_dx = (dx * viewport.cell_w / viewport.vp_w) * 2.0;
            instance.ndc_min[0] += ndc_dx;
            instance.ndc_max[0] += ndc_dx;
        }
        GpuPrimitive::PushClipRect(r) => r.col += dx,
        GpuPrimitive::PopClipRect => {}
    }
}

/// Shift a metal primitive vertically by `dy` cells (in-place).
fn offset_primitive_y_mut(prim: &mut GpuPrimitive, dy: f32, viewport: WidgetViewport) {
    match prim {
        GpuPrimitive::ZLayer { primitive, .. } => offset_primitive_y_mut(primitive, dy, viewport),
        GpuPrimitive::Rect(r) => r.rect.row += dy,
        GpuPrimitive::ForegroundRect(r) => r.rect.row += dy,
        GpuPrimitive::Quad(q) => q.y += dy,
        GpuPrimitive::Triangle(t) => {
            for point in &mut t.points {
                point[1] += dy;
            }
        }
        GpuPrimitive::GlyphRun(g) => g.row += dy,
        GpuPrimitive::ProportionalText(t) => t.row += dy,
        GpuPrimitive::PatchCable(c) => {
            c.start[1] += dy;
            c.control1[1] += dy;
            c.control2[1] += dy;
            c.end[1] += dy;
            c.segment_row += dy;
        }
        GpuPrimitive::Circle(c) => c.center[1] += dy,
        GpuPrimitive::Waveform(w) => w.rect.row += dy,
        GpuPrimitive::Wavetable(w) => w.rect.row += dy,
        GpuPrimitive::LiveSpectrogram(s) => s.rect.row += dy,
        GpuPrimitive::Image(i) => i.rect.row += dy,
        GpuPrimitive::WidgetInstance { instance, .. } => {
            let ndc_dy = -(dy * viewport.cell_h / viewport.vp_h) * 2.0;
            instance.ndc_min[1] += ndc_dy;
            instance.ndc_max[1] += ndc_dy;
        }
        GpuPrimitive::PushClipRect(r) => r.row += dy,
        GpuPrimitive::PopClipRect => {}
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

pub fn begin_widget_gesture(
    node: &LayoutNode,
    local_col: f32,
    local_row: f32,
    modifiers: KeyModifiers,
) -> Option<Value> {
    widget_definition(&node.widget_type)?.begin_gesture(node, local_col, local_row, modifiers)
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
        .map(|definition| definition.captures_scroll_gesture(node))
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
        Some(Value::Number(n)) => *n > 0.5,
        Some(Value::ReactiveRef { slot, .. }) => crate::reactive::read_float_slot(slot) > 0.5,
        _ => default,
    }
}

pub fn plock_active(props: &HashMap<String, Value>) -> bool {
    get_f32_prop(props, "plock-active", 0.0) > 0.5
}

pub fn plock_color(props: &HashMap<String, Value>) -> Color {
    Color {
        r: get_f32_prop(props, "plock-color-r", 0.270_588_25),
        g: get_f32_prop(props, "plock-color-g", 0.784_313_74),
        b: get_f32_prop(props, "plock-color-b", 0.862_745_1),
        a: 1.0,
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
    fn shader_sources_select_the_requested_backend() {
        let sources = ShaderSources::new(Some("msl source"), Some("wgsl source"));

        assert_eq!(sources.source(ShaderBackend::Msl), Some("msl source"));
        assert_eq!(sources.source(ShaderBackend::Wgsl), Some("wgsl source"));
    }

    /// Every widget can be asked for either backend's source without panicking
    /// or falling back to the other language. Definitions without a custom
    /// shader legitimately answer `None`; the validation below proves that
    /// every MSL body has a corresponding WGSL body.
    #[test]
    fn every_widget_answers_both_backends() {
        for definition in WIDGET_DEFINITIONS {
            for &name in definition.names() {
                for backend in [ShaderBackend::Msl, ShaderBackend::Wgsl] {
                    if let Some(fragment) = definition.fragment_shader(name, backend) {
                        assert!(
                            !fragment.is_empty(),
                            "{name} returned an empty {backend:?} fragment shader"
                        );
                    }
                    if let Some(vertex) = definition.vertex_shader(name, backend) {
                        assert!(
                            !vertex.is_empty(),
                            "{name} returned an empty {backend:?} vertex shader"
                        );
                    }
                }
            }
        }
    }

    /// Every hand-written widget shader has a validated source body for both
    /// graphics backends. Parsing the fully assembled module catches accidental
    /// dependencies on MSL syntax as well as WGSL interface mismatches.
    #[test]
    fn every_widget_wgsl_shader_is_valid() {
        let msl = widget_shader_sources(ShaderBackend::Msl);
        let wgsl = widget_shader_sources(ShaderBackend::Wgsl);
        assert_eq!(wgsl.len(), msl.len(), "some MSL widget shaders lack WGSL ports");

        for (name, vertex, fragment) in wgsl {
            let source = crate::ui::wgsl_shaders::widget_shader_module(vertex, fragment);
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|error| panic!("{name} WGSL parse failed: {error}\n\n{source}"));
            let mut validator = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            );
            validator
                .validate(&module)
                .unwrap_or_else(|error| panic!("{name} WGSL validation failed: {error:#?}\n\n{source}"));
        }
    }

    /// The MSL enumeration must stay non-empty and must never leak a body that
    /// was registered for the other backend.
    #[test]
    fn widget_shader_sources_are_scoped_to_the_requested_backend() {
        let msl = widget_shader_sources(ShaderBackend::Msl);
        assert!(!msl.is_empty(), "MSL enumeration lost every widget shader");

        for (name, vertex, fragment) in &msl {
            assert!(
                fragment.contains("fragment") || fragment.contains("float4"),
                "{name} MSL fragment shader does not look like MSL"
            );
            assert!(
                !fragment.contains("@fragment"),
                "{name} MSL slot is serving WGSL"
            );
            if let Some(vertex) = vertex {
                assert!(!vertex.is_empty(), "{name} has an empty MSL vertex shader");
            }
        }

        for (name, _, fragment) in widget_shader_sources(ShaderBackend::Wgsl) {
            assert!(
                !fragment.contains("[[stage_in]]"),
                "{name} WGSL slot is serving MSL"
            );
        }
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

    fn primitive_token(primitive: &GpuPrimitive) -> String {
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
            GpuPrimitive::ZLayer { z_index, primitive } => {
                format!("z:{z_index}:{}", primitive_token(primitive))
            }
            GpuPrimitive::Rect(rect) => {
                format!("rect:{}:{}", rect_token(rect.rect), color_token(rect.color))
            }
            GpuPrimitive::ForegroundRect(rect) => {
                format!(
                    "fg-rect:{}:{}",
                    rect_token(rect.rect),
                    color_token(rect.color)
                )
            }
            GpuPrimitive::Quad(quad) => format!(
                "quad:{:08x}:{:08x}:{:08x}:{:08x}:{}",
                quad.x.to_bits(),
                quad.y.to_bits(),
                quad.width.to_bits(),
                quad.height.to_bits(),
                color_token(quad.color)
            ),
            GpuPrimitive::Triangle(triangle) => format!(
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
            GpuPrimitive::GlyphRun(run) => format!(
                "glyph:{:08x}:{}:{}:{}:{}",
                run.row.to_bits(),
                run.col,
                run.text,
                color_token(run.fg),
                color_token(run.bg)
            ),
            GpuPrimitive::ProportionalText(run) => format!(
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
            GpuPrimitive::PatchCable(cable) => format!(
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
            GpuPrimitive::Circle(circle) => format!(
                "circle:{}:{:08x}:{}:{:?}",
                f32s(circle.center),
                circle.radius_px.to_bits(),
                color_token(circle.color),
                circle.visible_half
            ),
            GpuPrimitive::Waveform(waveform) => {
                format!(
                    "waveform:{}:{}",
                    rect_token(waveform.rect),
                    waveform.sample_key
                )
            }
            GpuPrimitive::Wavetable(wavetable) => {
                format!(
                    "wavetable:{}:{}:{}:{}:{:08x}:{:08x}:{:08x}:{}:{}:{}",
                    rect_token(wavetable.rect),
                    wavetable.bank_key,
                    wavetable.set_base,
                    wavetable.waves_in_set,
                    wavetable.wave_pos.to_bits(),
                    wavetable.warp.to_bits(),
                    wavetable.fold.to_bits(),
                    color_token(wavetable.selected_color),
                    color_token(wavetable.inactive_color),
                    color_token(wavetable.bg_color)
                )
            }
            GpuPrimitive::LiveSpectrogram(spectrogram) => {
                format!(
                    "live-spectrogram:{}:{}:{}:{}:{:.3}:{:.3}:{}:{}:{}:{}:{}:{}",
                    rect_token(spectrogram.rect),
                    spectrogram.data_key,
                    spectrogram.mode,
                    spectrogram.freq_scale,
                    spectrogram.min_hz,
                    spectrogram.max_hz,
                    color_token(spectrogram.min_color),
                    color_token(spectrogram.mid_color),
                    color_token(spectrogram.max_color),
                    color_token(spectrogram.eq_line_color),
                    color_token(spectrogram.eq_fill_color),
                    color_token(spectrogram.background_color)
                )
            }
            GpuPrimitive::Image(image) => {
                format!(
                    "image:{}:{}:{}",
                    image.widget_id,
                    rect_token(image.rect),
                    image.src
                )
            }
            GpuPrimitive::WidgetInstance {
                widget_type,
                instance,
                is_background,
            } => format!(
                "widget:{widget_type}:{is_background}:{}:{}:{:08x}:{:08x}:{}:{}:{}:{}:{}:{}:{}:{}:{:08x}:{:08x}",
                f32s(instance.ndc_min),
                f32s(instance.ndc_max),
                instance.value_t.to_bits(),
                instance.orientation.to_bits(),
                f32s(instance.uniform_a),
                f32s(instance.uniform_b),
                f32s(instance.uniform_c),
                f32s(instance.uniform_d),
                f32s(instance.color_a),
                f32s(instance.color_b),
                f32s(instance.color_c),
                f32s(instance.color_d),
                instance.corner_radius.to_bits(),
                instance.pixel_aspect.to_bits()
            ),
            GpuPrimitive::PushClipRect(rect) => format!("push:{}", rect_token(*rect)),
            GpuPrimitive::PopClipRect => "pop".to_string(),
        }
    }

    fn primitive_tokens(primitives: &[GpuPrimitive]) -> Vec<String> {
        primitives.iter().map(primitive_token).collect()
    }

    fn test_viewport() -> WidgetViewport {
        WidgetViewport {
            cell_w: 10.0,
            cell_h: 20.0,
            vp_w: 640.0,
            vp_h: 480.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 24.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        }
    }

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
            animation: Default::default(),
        }
    }

    fn assert_tagged_collection_matches_flat_collection(
        layout: &LayoutNode,
        viewport: WidgetViewport,
    ) {
        let (flat, _) = collect_gpu_primitives(layout, viewport, 0.0, 24);
        let (runs, _) = collect_gpu_primitive_runs(layout, viewport, 0.0, 24);
        let flattened = flatten_gpu_primitive_runs(&runs);
        assert_eq!(primitive_tokens(&flattened), primitive_tokens(&flat));
    }

    #[test]
    fn knob_number_focus_corners_are_added_by_the_shared_primitive_pipeline() {
        let rect = Rect {
            row: 2.0,
            col: 3.0,
            width: 5.0,
            height: 3.0,
        };
        let mut knob = test_node(
            42,
            "knob-number",
            rect,
            HashMap::from([
                ("label".to_string(), Value::String("Pan".to_string())),
                ("value".to_string(), Value::Number(0.0)),
                ("min".to_string(), Value::Number(-1.0)),
                ("max".to_string(), Value::Number(1.0)),
            ]),
            Vec::new(),
        );
        knob.focusable = true;

        let unfocused = widget_primitives_for_node(&knob, test_viewport());
        assert_eq!(
            unfocused
                .iter()
                .filter(|primitive| matches!(primitive, GpuPrimitive::ForegroundRect(_)))
                .count(),
            0
        );

        let focused = widget_primitives_for_node(
            &knob,
            WidgetViewport {
                focused_widget_id: Some(knob.widget_id),
                ..test_viewport()
            },
        );
        let corner_rects: Vec<Rect> = focused
            .iter()
            .filter_map(|primitive| match primitive {
                GpuPrimitive::ForegroundRect(corner) => Some(corner.rect),
                _ => None,
            })
            .collect();
        assert_eq!(corner_rects.len(), 8);
        for corner in corner_rects {
            assert!(corner.col >= rect.col);
            assert!(corner.row >= rect.row);
            assert!(corner.col + corner.width <= rect.col + rect.width);
            assert!(corner.row + corner.height <= rect.row + rect.height);
        }
    }

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

        let (runs, _) = collect_gpu_primitive_runs(&root, test_viewport(), 0.0, 24);
        let label_run = runs
            .iter()
            .find(|run| run.widget_id == 3)
            .expect("label run should be collected");
        assert_eq!(label_run.ancestor_widget_ids, vec![1, 2]);
    }

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
        let (previous_runs, _) = collect_gpu_primitive_runs(&before, viewport, 0.0, 24);
        let (retained_runs, _, stats) =
            collect_gpu_primitive_runs_retained(&after, viewport, 0.0, 24, &previous_runs, &[2]);
        let (full_runs, _) = collect_gpu_primitive_runs(&after, viewport, 0.0, 24);

        assert!(stats.reused_runs > 0);
        assert!(stats.rebuilt_runs > 0);
        assert_eq!(
            primitive_tokens(&flatten_gpu_primitive_runs(&retained_runs)),
            primitive_tokens(&flatten_gpu_primitive_runs(&full_runs))
        );
    }

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
        let (previous_runs, _) = collect_gpu_primitive_runs(&root, viewport, 0.0, 24);
        let (retained_runs, _, stats) =
            collect_gpu_primitive_runs_retained(&root, viewport, 0.0, 24, &previous_runs, &[1]);
        let (full_runs, _) = collect_gpu_primitive_runs(&root, viewport, 0.0, 24);

        assert_eq!(stats.reused_runs, 0);
        assert!(stats.rebuilt_runs >= 2);
        assert_eq!(
            primitive_tokens(&flatten_gpu_primitive_runs(&retained_runs)),
            primitive_tokens(&flatten_gpu_primitive_runs(&full_runs))
        );
    }

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
                height: 6.0,
            },
            HashMap::new(),
            vec![label, button],
        );
        let viewport = test_viewport();
        let (previous_runs, _) = collect_gpu_primitive_runs(&root, viewport, 0.0, 24);
        let (retained_runs, _, stats) =
            collect_gpu_primitive_runs_retained(&root, viewport, 0.0, 24, &previous_runs, &[3]);
        let (full_runs, _) = collect_gpu_primitive_runs(&root, viewport, 0.0, 24);

        assert!(stats.reused_runs > 0);
        assert!(stats.rebuilt_runs > 0);
        assert_eq!(
            primitive_tokens(&flatten_gpu_primitive_runs(&retained_runs)),
            primitive_tokens(&flatten_gpu_primitive_runs(&full_runs))
        );
    }

    #[test]
    fn scrolled_collection_culls_fully_offscreen_children() {
        let visible = test_node(
            2,
            "label",
            Rect {
                row: 1.0,
                col: 1.0,
                width: 8.0,
                height: 1.0,
            },
            HashMap::from([("text".to_string(), Value::String("Visible".to_string()))]),
            Vec::new(),
        );
        let offscreen = test_node(
            3,
            "label",
            Rect {
                row: 10.0,
                col: 1.0,
                width: 8.0,
                height: 1.0,
            },
            HashMap::from([("text".to_string(), Value::String("Offscreen".to_string()))]),
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
            HashMap::from([("_content_height".to_string(), Value::Number(20.0))]),
            vec![visible, offscreen],
        );
        let viewport = test_viewport();

        let (runs, _) = collect_gpu_primitive_runs(&root, viewport, 0.0, 24);
        assert!(runs.iter().any(|run| run.widget_id == 2));
        assert!(!runs.iter().any(|run| run.widget_id == 3));

        // Scroll down: the culled child enters the viewport, the first leaves.
        scroll::set_scroll_state(
            1,
            scroll::ScrollState {
                offset_y: 9.0,
                content_height: 20.0,
                viewport_height: 4.0,
                ..Default::default()
            },
        );
        let (runs, _) = collect_gpu_primitive_runs(&root, viewport, 0.0, 24);
        assert!(!runs.iter().any(|run| run.widget_id == 2));
        assert!(runs.iter().any(|run| run.widget_id == 3));
    }

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
        let (mut cached_runs, _) = collect_gpu_primitive_runs(&before, viewport, 0.0, 24);
        let run_indices = build_gpu_primitive_run_index(&cached_runs);
        let (_overlay, stats) = refresh_gpu_primitive_runs_retained_in_place(
            &after,
            viewport,
            0.0,
            24,
            &mut cached_runs,
            &run_indices,
            &[2],
        );
        let (full_runs, _) = collect_gpu_primitive_runs(&after, viewport, 0.0, 24);

        assert!(stats.reused_runs > 0);
        assert!(stats.rebuilt_runs > 0);
        assert_eq!(stats.missing_previous_runs, 0);
        assert_eq!(stats.invalid_previous_runs, 0);
        assert_eq!(
            primitive_tokens(&flatten_gpu_primitive_runs(&cached_runs)),
            primitive_tokens(&flatten_gpu_primitive_runs(&full_runs))
        );
    }

    #[test]
    fn retained_metal_in_place_refresh_reports_removed_previous_runs() {
        let make_layout = |include_button: bool| {
            let label = test_node(
                2,
                "label",
                Rect {
                    row: 1.0,
                    col: 1.0,
                    width: 8.0,
                    height: 1.0,
                },
                HashMap::from([("text".to_string(), Value::String("Keep".to_string()))]),
                Vec::new(),
            );
            let mut children = vec![label];
            if include_button {
                children.push(test_node(
                    3,
                    "button",
                    Rect {
                        row: 3.0,
                        col: 1.0,
                        width: 6.0,
                        height: 1.5,
                    },
                    HashMap::from([("text".to_string(), Value::String("Remove".to_string()))]),
                    Vec::new(),
                ));
            }
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
                children,
            )
        };

        let viewport = test_viewport();
        let before = make_layout(true);
        let after = make_layout(false);
        let (mut cached_runs, _) = collect_gpu_primitive_runs(&before, viewport, 0.0, 24);
        let run_indices = build_gpu_primitive_run_index(&cached_runs);
        let (_overlay, stats) = refresh_gpu_primitive_runs_retained_in_place(
            &after,
            viewport,
            0.0,
            24,
            &mut cached_runs,
            &run_indices,
            &[1],
        );

        assert!(
            stats.invalid_previous_runs > 0,
            "removed cached runs must force a full retained-run rebuild"
        );
    }

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
                height: 6.0,
            },
            HashMap::new(),
            vec![label, button],
        );
        let viewport = test_viewport();
        let (mut cached_runs, _) = collect_gpu_primitive_runs(&root, viewport, 0.0, 24);
        let run_indices = build_gpu_primitive_run_index(&cached_runs);
        let (_overlay, stats) = refresh_gpu_primitive_runs_retained_in_place(
            &root,
            viewport,
            0.0,
            24,
            &mut cached_runs,
            &run_indices,
            &[3],
        );
        let (full_runs, _) = collect_gpu_primitive_runs(&root, viewport, 0.0, 24);

        assert!(stats.reused_runs > 0);
        assert!(stats.rebuilt_runs > 0);
        assert_eq!(
            primitive_tokens(&flatten_gpu_primitive_runs(&cached_runs)),
            primitive_tokens(&flatten_gpu_primitive_runs(&full_runs))
        );
    }

    #[test]
    fn animated_subtree_refresh_reuses_static_sibling_runs() {
        sdf_widget::register_sdf_widget(sdf_widget::SdfWidgetDef {
            name: "test-retained-animated-sdf".to_string(),
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
        let animated = test_node(
            2,
            "test-retained-animated-sdf",
            Rect {
                row: 0.0,
                col: 0.0,
                width: 2.0,
                height: 2.0,
            },
            HashMap::new(),
            Vec::new(),
        );
        let label = test_node(
            3,
            "label",
            Rect {
                row: 2.0,
                col: 0.0,
                width: 8.0,
                height: 1.0,
            },
            HashMap::from([("text".to_string(), Value::String("Static".to_string()))]),
            Vec::new(),
        );
        let mut root = test_node(
            1,
            "vstack",
            Rect {
                row: 0.0,
                col: 0.0,
                width: 8.0,
                height: 3.0,
            },
            HashMap::new(),
            vec![animated, label],
        );
        cache_layout_animation_hints(&mut root);
        let active = active_animation_widget_ids(&root);
        assert_eq!(active, vec![2]);

        let viewport = test_viewport();
        let (mut runs, _) = collect_gpu_primitive_runs(&root, viewport, 0.0, 24);
        let run_index = build_gpu_primitive_run_index(&runs);
        let (_overlay, stats) = refresh_gpu_primitive_runs_retained_in_place(
            &root, viewport, 0.0, 24, &mut runs, &run_index, &active,
        );
        assert!(stats.rebuilt_runs > 0);
        assert!(stats.reused_runs > 0);
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
        let mut node = LayoutNode {
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
            animation: Default::default(),
        };
        cache_layout_animation_hints(&mut node);

        assert!(node.animation.initialized);
        assert!(node.animation.self_static);
        assert!(node.animation.subtree_static);
        assert!(layout_wants_animation_frames(&node));
        assert_eq!(
            active_animation_widgets(&node),
            vec![(1, "test-animated-sdf")]
        );
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
            animation: Default::default(),
        };

        assert!(layout_wants_animation_frames(&node));
    }

    #[test]
    fn animated_sdf_box_background_requests_animation_frames() {
        sdf_widget::register_sdf_widget(sdf_widget::SdfWidgetDef {
            name: "test-animated-background".to_string(),
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
            widget_type: "box".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 8.0,
                height: 1.0,
            },
            props: HashMap::from([(
                "background".to_string(),
                Value::String("test-animated-background".to_string()),
            )]),
            children: Vec::new(),
            focusable: false,
            animation: Default::default(),
        };

        assert!(layout_wants_animation_frames(&node));
    }
}

/// Shared rounded-rect SDF shader used by tree-row, text-input, number-picker, dropdown.
/// When `corner_radius > 0`, uses that as the radius (in normalized space).
/// Otherwise defaults to 0.75 (pill-like for small widgets).
pub const ROUNDED_RECT_SHADER: ShaderSources = ShaderSources::both(r#"
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
"#, wgsl::ROUNDED_RECT_SHADER);

/// Flat panel chrome: solid fill, crisp uniform border, rounded corners.
///
/// `color_a` = border color, `color_b` = fill color, `uniform_a.x` = border
/// width in device pixels, `corner_radius` = normalized radius (see
/// `normalized_corner_radius`). Deliberately unlit — no gradient, no specular —
/// so completion menus and tooltips read as flat surfaces instead of nodes.
pub const PATCHER_PANEL_SHADER: ShaderSources = ShaderSources::both(r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float aspect = max(in.aspect, 0.001);
    float2 localPos = float2((in.uv.x - 0.5) * 2.0 * aspect, (in.uv.y - 0.5) * 2.0);
    float2 sdfSize = float2(aspect, 1.0);
    float cornerRadius = min(in.corner_radius, min(aspect, 1.0));

    float dist = sdf_rounded_rect(localPos, sdfSize, cornerRadius);
    float derivative = max(fwidth(dist), 0.001);
    float outerAlpha = smoothstep(derivative, -derivative, dist);
    if (outerAlpha <= 0.001) {
        discard_fragment();
    }

    float borderThickness = max(in.uniform_a.x, 0.0) * derivative;
    float2 innerSize = max(sdfSize - float2(borderThickness), float2(0.001));
    float innerDist = sdf_rounded_rect(localPos, innerSize, max(cornerRadius - borderThickness, 0.0));
    float innerDerivative = max(fwidth(innerDist), 0.001);
    float innerAlpha = smoothstep(innerDerivative, -innerDerivative, innerDist);
    float borderMask = outerAlpha * (1.0 - innerAlpha);

    float3 color = mix(in.color_b.rgb, in.color_a.rgb, borderMask);
    float alpha = mix(in.color_b.a, in.color_a.a, borderMask);
    return float4(color, alpha * outerAlpha);
}
"#, wgsl::PATCHER_PANEL_SHADER);

pub const PATCHER_PORT_SHADER: ShaderSources = ShaderSources::both(r#"
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
"#, wgsl::PATCHER_PORT_SHADER);

pub const PATCHER_BACK_CHEVRON_SHADER: ShaderSources = ShaderSources::both(r#"
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
"#, wgsl::PATCHER_BACK_CHEVRON_SHADER);

pub const PATCHER_NODE_SHADER: ShaderSources = ShaderSources::both(r#"
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
    // Isotropic pixel size in local units. fwidth(nodeDist) would grow with the
    // gradient direction (up to ~1.41x on the 45 degree stretch of a corner),
    // which fattens the stroke around the curves; fwidth(localPos) does not.
    float pixel = max(max(fwidth(localPos.x), fwidth(localPos.y)), 0.0001);
    float outerAlpha = smoothstep(pixel, -pixel, nodeDist);
    if (outerAlpha <= 0.001) {
        discard_fragment();
    }

    // sdf_rounded_rect is a true euclidean distance, so the inner contour is
    // just the outer one offset inward - uniform thickness by construction.
    float borderThickness = max(in.uniform_a.x, 0.0) * pixel;
    float innerDist = nodeDist + borderThickness;
    float innerAlpha = smoothstep(pixel, -pixel, innerDist);
    float borderMask = clamp(outerAlpha - innerAlpha, 0.0, 1.0);

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
    float specularFadeDistance = clamp(pixel * 2.5, 0.01, 0.06);
    float specular = specularRaw * smoothstep(0.0, -specularFadeDistance, nodeDist);

    float3 bg = in.color_b.rgb;
    float3 border = in.color_a.rgb;
    float3 litBg = bg * (0.82 + 0.18 * diffuse) + float3(0.20) * specular;
    float3 litBorder = border * (0.76 + 0.24 * diffuse) + float3(0.55) * specular;

    float edgeShade = smoothstep(0.18, 0.98, localPos.y * 0.5 + 0.5);
    litBg *= mix(0.94, 1.04, edgeShade);
    litBorder *= mix(0.88, 1.12, edgeShade);

    // Flatness dials the whole fake-3d treatment out: the bevel's diffuse, the
    // specular, and the vertical edge shade all land in litBg/litBorder, so
    // mixing back to the raw colours at 1.0 leaves a flat card with a clean SDF
    // edge. A node wants the shading - at pill size it is most of what gives
    // the node its physicality - but on a surface as large as an agentic
    // bubble the same treatment reads as a smudge near the border.
    float flatness = clamp(in.uniform_a.y, 0.0, 1.0);
    litBg = mix(litBg, bg, flatness);
    litBorder = mix(litBorder, border, flatness);

    float3 color = mix(litBg, litBorder, borderMask);
    return float4(color, outerAlpha * max(in.color_a.a, in.color_b.a));
}
"#, wgsl::PATCHER_NODE_SHADER);

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
