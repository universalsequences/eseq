pub mod box_widget;
pub mod grid;
pub mod hslider;
pub mod hstack;
pub mod knob;
pub mod label;
pub mod scroll;
pub mod sdf_widget;
pub mod tree;
pub mod tabs;
pub mod time_view;
pub mod timeline;
pub mod toggle;
pub mod vslider;
pub mod vstack;
pub mod waveform;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crossterm::event::{KeyCode, KeyModifiers, MouseEventKind};

use crate::backend::{Cell, CellStyle, Color};
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, get_map};
use crate::theme;
use crate::vm::Value;

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
    pub playhead_position: f32,
    pub show_playhead: bool,
    pub waveform_color: Color,
    pub selection_color: Color,
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub enum MetalPrimitive {
    Rect(MetalRectPrimitive),
    Quad(MetalQuadPrimitive),
    GlyphRun(MetalGlyphRunPrimitive),
    ProportionalText(MetalProportionalTextPrimitive),
    Waveform(MetalWaveformPrimitive),
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
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let _ = (node, area, children, aspect, measure_child, build_child);
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
    &toggle::TOGGLE_WIDGET,
    &knob::KNOB_WIDGET,
    &tabs::TABS_WIDGET,
    &timeline::TIMELINE_WIDGET,
    &waveform::WAVEFORM_WIDGET,
    &vstack::VSTACK_WIDGET,
    &hstack::HSTACK_WIDGET,
    &box_widget::BOX_WIDGET,
    &grid::GRID_WIDGET,
    &scroll::SCROLL_WIDGET,
    &tree::TREE_WIDGET,
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
    if let Some(definition) = widget_definition(&node.widget_type) {
        definition.build_metal_primitives(&node.widget_type, node, viewport)
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
    scroll_top: u16,
    max_rows: u16,
) -> Vec<MetalPrimitive> {
    let mut primitives = Vec::new();
    collect_metal_primitives_recursive(node, viewport, scroll_top, max_rows, &mut primitives);
    primitives
}

#[cfg(target_os = "macos")]
fn collect_metal_primitives_recursive(
    node: &LayoutNode,
    viewport: WidgetViewport,
    _scroll_top: u16,
    _max_rows: u16,
    primitives: &mut Vec<MetalPrimitive>,
) {
    let node_is_focused = node.focusable && viewport.focused_widget_id == Some(node.widget_id);
    let focused_branch = viewport.focused_branch || node_is_focused;

    // If a container node is focused, emit a background highlight rect.
    // This renders before children (correct z-order: highlight under content).
    if node_is_focused && is_layout_widget_type(&node.widget_type) {
        primitives.push(MetalPrimitive::Rect(MetalRectPrimitive {
            rect: node.rect,
            color: crate::theme::WIDGET_FOCUS_BG(),
        }));
    }

    let node_viewport = WidgetViewport {
        focused_branch,
        ..viewport
    };

    // Scroll container: clip children to viewport rect and offset by scroll amount
    if node.widget_type == "scroll" {
        let state = scroll::get_scroll_state(node.widget_id);
        let offset_y = state.offset_y;

        primitives.push(MetalPrimitive::PushClipRect(node.rect));

        for child in &node.children {
            let mut child_primitives = Vec::new();
            collect_metal_primitives_recursive(
                child,
                node_viewport,
                _scroll_top,
                _max_rows,
                &mut child_primitives,
            );
            // Offset child primitives by negative scroll offset (shift content up)
            for prim in child_primitives {
                primitives.push(offset_primitive_y(prim, -offset_y, node_viewport));
            }
        }

        primitives.push(MetalPrimitive::PopClipRect);

        // Scrollbar rendered AFTER children so it draws on top
        primitives.extend(widget_primitives_for_node(node, node_viewport));
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

/// Shift a metal primitive vertically by `dy` cells.
#[cfg(target_os = "macos")]
fn offset_primitive_y(prim: MetalPrimitive, dy: f32, viewport: WidgetViewport) -> MetalPrimitive {
    match prim {
        MetalPrimitive::Rect(mut r) => {
            r.rect.row += dy;
            MetalPrimitive::Rect(r)
        }
        MetalPrimitive::Quad(mut q) => {
            q.y += dy;
            MetalPrimitive::Quad(q)
        }
        MetalPrimitive::GlyphRun(mut g) => {
            g.row += dy;
            MetalPrimitive::GlyphRun(g)
        }
        MetalPrimitive::ProportionalText(mut t) => {
            t.row += dy;
            MetalPrimitive::ProportionalText(t)
        }
        MetalPrimitive::Waveform(w) => MetalPrimitive::Waveform(w),
        MetalPrimitive::WidgetInstance {
            widget_type,
            mut instance,
            is_background,
        } => {
            // Convert cell offset to NDC offset
            let ndc_dy = -(dy * viewport.cell_h / viewport.vp_h) * 2.0;
            instance.ndc_min[1] += ndc_dy;
            instance.ndc_max[1] += ndc_dy;
            MetalPrimitive::WidgetInstance {
                widget_type,
                instance,
                is_background,
            }
        }
        MetalPrimitive::PushClipRect(r) => MetalPrimitive::PushClipRect(r),
        MetalPrimitive::PopClipRect => MetalPrimitive::PopClipRect,
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
            definition.mouse_event(node, mouse_kind, local_col, local_row, drag_start, gesture, modifiers)
        })
        .unwrap_or(MouseEventOutcome::Ignore)
}

pub fn widget_captures_drag(widget_type: &str) -> bool {
    if sdf_widget::sdf_widget_def(widget_type).is_some() {
        return true; // SDF widgets capture drag for :on-drag support
    }
    widget_definition(widget_type)
        .map(WidgetDefinition::captures_drag)
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
    match props.get(key) {
        Some(Value::Number(n)) => *n as f32,
        _ => default,
    }
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
