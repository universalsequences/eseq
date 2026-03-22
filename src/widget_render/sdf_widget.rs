use std::cell::RefCell;
use std::collections::HashMap;

use super::{
    CellBuffer, Constraints, LayoutNode, MeasureCtx, MetalPrimitive, Rect, Size, WidgetViewport,
};
use crate::vm::Value;

/// A dynamically-defined SDF widget registered via `defwidget`.
#[derive(Clone)]
pub struct SdfWidgetDef {
    pub name: String,
    pub shader_source: String,
    pub region_count: usize,
    pub width: f32,
    pub height: f32,
}

thread_local! {
    static SDF_WIDGETS: RefCell<HashMap<String, SdfWidgetDef>> = RefCell::new(HashMap::new());
}

pub fn register_sdf_widget(def: SdfWidgetDef) {
    let name = def.name.clone();
    SDF_WIDGETS.with(|w| w.borrow_mut().insert(name, def));
}

pub fn sdf_widget_def(name: &str) -> Option<SdfWidgetDef> {
    SDF_WIDGETS.with(|w| w.borrow().get(name).cloned())
}

pub fn sdf_widget_names() -> Vec<String> {
    SDF_WIDGETS.with(|w| w.borrow().keys().cloned().collect())
}

/// Measure an SDF widget — returns the fixed size from defwidget :measure.
pub fn sdf_widget_measure(
    widget_type: &str,
    _node: &Value,
    _children: &[Value],
    _constraints: Constraints,
    _ctx: &MeasureCtx<'_>,
) -> Option<Size> {
    let def = sdf_widget_def(widget_type)?;
    Some(Size {
        width: def.width,
        height: def.height,
    })
}

/// TUI rendering for SDF widgets — placeholder.
pub fn sdf_widget_tui_render(
    _widget_type: &str,
    _props: &HashMap<String, Value>,
    _rect: Rect,
    _buf: &mut CellBuffer,
) {
    // Future: Milestone 6 TUI fallback rendering
}

/// Build Metal primitives for an SDF widget.
#[cfg(target_os = "macos")]
pub fn sdf_widget_metal_primitives(
    widget_type: &str,
    node: &LayoutNode,
    viewport: WidgetViewport,
) -> Vec<MetalPrimitive> {
    use super::{WidgetInstance, ndc_bounds};

    let _def = match sdf_widget_def(widget_type) {
        Some(d) => d,
        None => return Vec::new(),
    };

    let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
    let px_w = node.rect.width * viewport.cell_w;
    let px_h = node.rect.height * viewport.cell_h;

    let value_t = super::get_f32_prop(&node.props, "value", 0.0);
    let color_a = super::resolve_named_color(&node.props, "color", crate::theme::GREEN())
        .to_rgba();

    vec![MetalPrimitive::WidgetInstance {
        widget_type: widget_type.to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t,
            orientation: 0.0,
            color_a,
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: 0.0,
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
    }]
}

/// Collect shader sources for all registered SDF widgets.
#[cfg(target_os = "macos")]
pub fn sdf_widget_shader_sources() -> Vec<(String, String)> {
    SDF_WIDGETS.with(|w| {
        w.borrow()
            .iter()
            .map(|(name, def)| (name.clone(), def.shader_source.clone()))
            .collect()
    })
}
