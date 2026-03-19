pub mod box_widget;
pub mod grid;
pub mod hslider;
pub mod hstack;
pub mod knob;
pub mod label;
pub mod toggle;
pub mod vslider;
pub mod vstack;

use std::collections::HashMap;

use crossterm::event::MouseEventKind;

use crate::backend::{Cell, CellStyle, Color};
use crate::layout::{Constraints, LayoutNode, Rect, Size};
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
}

pub struct EventOutput {
    pub callback: Value,
    pub value: Value,
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
    fn mouse_event(
        &self,
        _node: &LayoutNode,
        _mouse_kind: MouseEventKind,
        _local_col: f32,
        _local_row: f32,
    ) -> MouseEventOutcome {
        MouseEventOutcome::Ignore
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
}

static WIDGET_DEFINITIONS: &[&dyn WidgetDefinition] = &[
    &label::LABEL_WIDGET,
    &hslider::HSLIDER_WIDGET,
    &vslider::VSLIDER_WIDGET,
    &toggle::TOGGLE_WIDGET,
    &knob::KNOB_WIDGET,
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
pub fn widget_instance_for_node(
    node: &LayoutNode,
    viewport: WidgetViewport,
) -> Option<WidgetInstance> {
    widget_definition(&node.widget_type)?.build_metal_instance(&node.widget_type, node, viewport)
}

/// Collect all label nodes from the layout tree (labels use glyph atlas, not custom shaders).
#[cfg(target_os = "macos")]
pub fn collect_label_nodes(node: &LayoutNode) -> Vec<(Rect, String)> {
    let mut labels = Vec::new();
    collect_labels_recursive(node, &mut labels);
    labels
}

#[cfg(target_os = "macos")]
fn collect_labels_recursive(node: &LayoutNode, labels: &mut Vec<(Rect, String)>) {
    if node.widget_type == "label" {
        if let Some(Value::String(text)) = node.props.get("text") {
            labels.push((node.rect, text.clone()));
        }
    }
    for child in &node.children {
        collect_labels_recursive(child, labels);
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
) -> MouseEventOutcome {
    widget_definition(&node.widget_type)
        .map(|definition| definition.mouse_event(node, mouse_kind, local_col, local_row))
        .unwrap_or(MouseEventOutcome::Ignore)
}

pub fn widget_captures_drag(widget_type: &str) -> bool {
    widget_definition(widget_type)
        .map(WidgetDefinition::captures_drag)
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
