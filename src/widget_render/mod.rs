pub mod box_widget;
pub mod grid;
pub mod hslider;
pub mod hstack;
pub mod knob;
pub mod label;
pub mod timeline;
pub mod toggle;
pub mod vslider;
pub mod vstack;

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers, MouseEventKind};

use crate::backend::{Cell, CellStyle, Color};
use crate::layout::{Constraints, LayoutNode, Rect, Size};
use crate::theme;
use crate::vm::Value;

// ── Semantic events (backend-agnostic) ───────────────────────────────────────

#[allow(dead_code)]
pub enum WidgetEvent {
    /// Drag/click: normalized 0..1 position within widget
    SetNormalized(f32),
    /// Scroll/arrow: delta in value-space units
    Nudge(f32),
    /// Click/confirm
    Activate,
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
    pub color_a: [f32; 4],
    pub color_b: [f32; 4],
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
    pub focused_widget_id: Option<u64>,
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
    pub row: u16,
    pub col: u16,
    pub text: String,
    pub fg: Color,
    pub bg: Color,
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub enum MetalPrimitive {
    Rect(MetalRectPrimitive),
    Quad(MetalQuadPrimitive),
    GlyphRun(MetalGlyphRunPrimitive),
    WidgetInstance {
        widget_type: String,
        instance: WidgetInstance,
    },
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
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size>;
    fn layout_children(
        &self,
        node: &Value,
        area: Rect,
        children: &[Value],
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let _ = (node, area, children, measure_child, build_child);
        vec![]
    }
    fn tui_render(&self, _props: &HashMap<String, Value>, _rect: Rect, _buf: &mut CellBuffer) {}
    fn begin_gesture(
        &self,
        _node: &LayoutNode,
        _local_col: f32,
        _local_row: f32,
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
    fn build_metal_instance(
        &self,
        _widget_type: &str,
        _node: &LayoutNode,
        _viewport: WidgetViewport,
    ) -> Option<WidgetInstance> {
        None
    }
    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        self.build_metal_instance(widget_type, node, viewport)
            .map(|instance| {
                vec![MetalPrimitive::WidgetInstance {
                    widget_type: widget_type.to_string(),
                    instance,
                }]
            })
            .unwrap_or_default()
    }
}

static WIDGET_DEFINITIONS: &[&dyn WidgetDefinition] = &[
    &label::LABEL_WIDGET,
    &hslider::HSLIDER_WIDGET,
    &vslider::VSLIDER_WIDGET,
    &toggle::TOGGLE_WIDGET,
    &knob::KNOB_WIDGET,
    &timeline::TIMELINE_WIDGET,
    &vstack::VSTACK_WIDGET,
    &hstack::HSTACK_WIDGET,
    &box_widget::BOX_WIDGET,
    &grid::GRID_WIDGET,
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
    if is_layout_widget_type(&node.widget_type) {
        for child in &node.children {
            render_widget_tree(child, buf);
        }
        return;
    }

    if let Some(definition) = widget_definition(&node.widget_type) {
        definition.tui_render(&node.props, node.rect, buf);
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
    widget_definition(&node.widget_type)
        .map(|definition| definition.build_metal_primitives(&node.widget_type, node, viewport))
        .unwrap_or_default()
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
    scroll_top: u16,
    max_rows: u16,
    primitives: &mut Vec<MetalPrimitive>,
) {
    let vis_row = node.rect.row as i32 - scroll_top as i32;
    let vis_end = vis_row + node.rect.height as i32;
    if vis_end <= 0 || vis_row >= max_rows as i32 {
        return;
    }

    let mut scrolled_node = node.clone();
    scrolled_node.rect.row = vis_row.max(0) as u16;
    let clipped_end = vis_end.min(max_rows as i32).max(scrolled_node.rect.row as i32) as u16;
    scrolled_node.rect.height = clipped_end
        .saturating_sub(scrolled_node.rect.row)
        .max(1);

    primitives.extend(widget_primitives_for_node(&scrolled_node, viewport));

    for child in &node.children {
        collect_metal_primitives_recursive(child, viewport, scroll_top, max_rows, primitives);
    }
}

pub fn handle_event(node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
    widget_definition(&node.widget_type)?.handle_event(node, event)
}

pub fn map_mouse_event(
    node: &LayoutNode,
    mouse_kind: MouseEventKind,
    local_col: f32,
    local_row: f32,
    drag_start: Option<(f32, f32)>,
    gesture: Option<&Value>,
) -> MouseEventOutcome {
    widget_definition(&node.widget_type)
        .map(|definition| definition.mouse_event(node, mouse_kind, local_col, local_row, drag_start, gesture))
        .unwrap_or(MouseEventOutcome::Ignore)
}

pub fn widget_captures_drag(widget_type: &str) -> bool {
    widget_definition(widget_type)
        .map(WidgetDefinition::captures_drag)
        .unwrap_or(false)
}

pub fn begin_widget_gesture(
    node: &LayoutNode,
    local_col: f32,
    local_row: f32,
) -> Option<Value> {
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
        Some(Value::String(name)) | Some(Value::Keyword(name)) => match name.as_str() {
            "red" => theme::RED,
            "green" | "cyan" => theme::GREEN,
            "yellow" | "orange" => theme::YELLOW,
            "blue" | "purple" => theme::BLUE,
            "magenta" => theme::MAGENTA,
            "white" => theme::WHITE,
            "gray" | "grey" | "dim" => theme::BRIGHT_BLACK,
            _ => default,
        },
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
            ndc_x(rect.col as f32 * viewport.cell_w),
            ndc_y(rect.row as f32 * viewport.cell_h),
        ],
        [
            ndc_x((rect.col + rect.width) as f32 * viewport.cell_w),
            ndc_y((rect.row + rect.height) as f32 * viewport.cell_h),
        ],
    )
}
