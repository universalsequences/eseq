pub mod hslider;
pub mod label;
pub mod toggle;
pub mod vslider;

use std::collections::HashMap;

use crate::backend::{Cell, CellStyle, Color};
use crate::layout::{LayoutNode, Rect};
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

// ── Type-erased TUI registry ─────────────────────────────────────────────────

struct TuiEntry {
    render: Box<dyn Fn(&HashMap<String, Value>, Rect, &mut CellBuffer)>,
}

#[cfg(target_os = "macos")]
struct MetalEntry {
    #[allow(dead_code)]
    shader_source: &'static str,
    gpu_data_size: usize,
    write_gpu_data: Box<dyn Fn(&HashMap<String, Value>, Rect, (f32, f32), &mut Vec<u8>)>,
}

pub struct WidgetRenderRegistry {
    tui: HashMap<String, TuiEntry>,
    #[cfg(target_os = "macos")]
    metal: HashMap<String, MetalEntry>,
}

impl WidgetRenderRegistry {
    fn new() -> Self {
        Self {
            tui: HashMap::new(),
            #[cfg(target_os = "macos")]
            metal: HashMap::new(),
        }
    }
}

fn register_tui<F>(name: &str, registry: &mut WidgetRenderRegistry, render_fn: F)
where
    F: Fn(&HashMap<String, Value>, Rect, &mut CellBuffer) + 'static,
{
    registry.tui.insert(
        name.to_string(),
        TuiEntry {
            render: Box::new(render_fn),
        },
    );
}

#[cfg(target_os = "macos")]
fn register_metal<F>(
    name: &str,
    registry: &mut WidgetRenderRegistry,
    shader_source: &'static str,
    gpu_data_size: usize,
    write_fn: F,
) where
    F: Fn(&HashMap<String, Value>, Rect, (f32, f32), &mut Vec<u8>) + 'static,
{
    registry.metal.insert(
        name.to_string(),
        MetalEntry {
            shader_source,
            gpu_data_size,
            write_gpu_data: Box::new(write_fn),
        },
    );
}

/// Build the full registry with all widget renderers.
pub fn build_registry() -> WidgetRenderRegistry {
    let mut registry = WidgetRenderRegistry::new();

    // TUI renderers
    register_tui("label", &mut registry, label::tui_render);
    register_tui("slider", &mut registry, hslider::tui_render);
    register_tui("hslider", &mut registry, hslider::tui_render);
    register_tui("vslider", &mut registry, vslider::tui_render);
    register_tui("toggle", &mut registry, toggle::tui_render);

    // Metal renderers (labels use glyph atlas path, not custom shader)
    #[cfg(target_os = "macos")]
    {
        register_metal(
            "slider",
            &mut registry,
            hslider::SLIDER_SHADER,
            std::mem::size_of::<hslider::SliderGpuData>(),
            hslider::write_gpu_data,
        );
        register_metal(
            "hslider",
            &mut registry,
            hslider::SLIDER_SHADER,
            std::mem::size_of::<hslider::SliderGpuData>(),
            hslider::write_gpu_data,
        );
        register_metal(
            "vslider",
            &mut registry,
            vslider::VSLIDER_SHADER,
            std::mem::size_of::<vslider::VSliderGpuData>(),
            vslider::write_gpu_data,
        );
        register_metal(
            "toggle",
            &mut registry,
            toggle::TOGGLE_SHADER,
            std::mem::size_of::<toggle::ToggleGpuData>(),
            toggle::write_gpu_data,
        );
    }

    registry
}

// ── Tree walker ──────────────────────────────────────────────────────────────

/// Recursively walk the layout tree and render leaf widgets into the CellBuffer.
/// Containers (v-stack, h-stack, box, grid) are invisible — only recurse children.
pub fn render_widget_tree(
    node: &LayoutNode,
    registry: &WidgetRenderRegistry,
    buf: &mut CellBuffer,
) {
    match node.widget_type.as_str() {
        "v-stack" | "h-stack" | "box" | "grid" => {
            for child in &node.children {
                render_widget_tree(child, registry, buf);
            }
        }
        widget_type => {
            if let Some(entry) = registry.tui.get(widget_type) {
                (entry.render)(&node.props, node.rect, buf);
            }
        }
    }
}

// ── Metal tree walker ────────────────────────────────────────────────────────

/// Collect GPU instance data for a specific widget type from the layout tree.
#[cfg(target_os = "macos")]
pub fn collect_widget_instances(
    node: &LayoutNode,
    widget_type: &str,
    registry: &WidgetRenderRegistry,
    viewport: (f32, f32),
) -> Vec<u8> {
    let mut data = Vec::new();
    collect_instances_recursive(node, widget_type, registry, viewport, &mut data);
    data
}

#[cfg(target_os = "macos")]
fn collect_instances_recursive(
    node: &LayoutNode,
    widget_type: &str,
    registry: &WidgetRenderRegistry,
    viewport: (f32, f32),
    data: &mut Vec<u8>,
) {
    if node.widget_type == widget_type {
        if let Some(entry) = registry.metal.get(widget_type) {
            (entry.write_gpu_data)(&node.props, node.rect, viewport, data);
        }
    }
    for child in &node.children {
        collect_instances_recursive(child, widget_type, registry, viewport, data);
    }
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

pub fn handle_event(node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
    match node.widget_type.as_str() {
        "slider" | "hslider" => slider_event(node, event),
        "vslider" => vslider_event(node, event),
        "toggle" => toggle_event(node, event),
        _ => None,
    }
}

fn slider_event(node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
    let WidgetEvent::SetNormalized(t) = event else {
        return None;
    };
    let callback = node.props.get("on-change")?.clone();
    let min = get_f32_prop(&node.props, "min", 0.0);
    let max = get_f32_prop(&node.props, "max", 1.0);
    let value = min + (max - min) * t.clamp(0.0, 1.0);
    Some(EventOutput {
        callback,
        value: Value::Number(value as f64),
    })
}

fn vslider_event(node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
    let WidgetEvent::SetNormalized(t) = event else {
        return None;
    };
    let callback = node.props.get("on-change")?.clone();
    let min = get_f32_prop(&node.props, "min", 0.0);
    let max = get_f32_prop(&node.props, "max", 1.0);
    let value = min + (max - min) * t.clamp(0.0, 1.0);
    Some(EventOutput {
        callback,
        value: Value::Number(value as f64),
    })
}

fn toggle_event(node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
    let WidgetEvent::Activate = event else {
        return None;
    };
    let callback = node.props.get("on-change")?.clone();
    let current = get_bool_prop(&node.props, "value", false);
    Some(EventOutput {
        callback,
        value: Value::Bool(!current),
    })
}
