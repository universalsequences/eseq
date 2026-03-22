use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::{
    CellBuffer, Constraints, LayoutNode, MeasureCtx, MetalPrimitive, Rect, Size, WidgetViewport,
};
use crate::parser::Expression;
use crate::vm::Value;

/// A dynamically-defined SDF widget registered via `defwidget`.
#[derive(Clone)]
pub struct SdfWidgetDef {
    pub name: String,
    pub shader_source: String,
    pub sdf_expr: Expression, // macro-expanded SDF expression for CPU hit testing
    pub region_count: usize,
    pub width: f32,
    pub height: f32,
}

/// Per-widget hit state tracked between frames.
#[derive(Clone)]
pub struct SdfHitState {
    pub hit_region: i32,  // -1 = none
    pub hit_pressed: bool,
}

impl Default for SdfHitState {
    fn default() -> Self {
        Self { hit_region: -1, hit_pressed: false }
    }
}

thread_local! {
    static SDF_WIDGETS: RefCell<HashMap<String, Rc<SdfWidgetDef>>> = RefCell::new(HashMap::new());
    /// Hit state keyed by widget_id (from LayoutNode).
    static SDF_HIT_STATES: RefCell<HashMap<u64, SdfHitState>> = RefCell::new(HashMap::new());
}

pub fn set_sdf_hit_state(widget_id: u64, state: SdfHitState) {
    SDF_HIT_STATES.with(|s| s.borrow_mut().insert(widget_id, state));
}

pub fn get_sdf_hit_state(widget_id: u64) -> SdfHitState {
    SDF_HIT_STATES.with(|s| s.borrow().get(&widget_id).cloned().unwrap_or_default())
}

/// Perform hit testing on an SDF widget at widget-local layout coordinates.
pub fn sdf_widget_hit_test(
    widget_type: &str,
    local_col: f32,
    local_row: f32,
    rect: Rect,
    pixel_aspect: f32,
) -> i32 {
    let def = match sdf_widget_def(widget_type) {
        Some(d) => d,
        None => return -1,
    };
    let (x, y) = crate::lang::sdf_hit::layout_to_sdf_coords(
        local_col, local_row, rect.width, rect.height, pixel_aspect,
    );
    crate::lang::sdf_hit::sdf_hit_test(&def.sdf_expr, x, y)
}

pub fn register_sdf_widget(def: SdfWidgetDef) {
    let name = def.name.clone();
    SDF_WIDGETS.with(|w| w.borrow_mut().insert(name, Rc::new(def)));
}

pub fn sdf_widget_def(name: &str) -> Option<Rc<SdfWidgetDef>> {
    SDF_WIDGETS.with(|w| w.borrow().get(name).cloned())
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
    let color_a = super::resolve_named_color(&node.props, "color", crate::theme::GREEN()).to_rgba();

    let pixel_aspect = if px_h > 0.0 { px_w / px_h } else { 1.0 };
    let hit = get_sdf_hit_state(node.widget_id);

    vec![MetalPrimitive::WidgetInstance {
        widget_type: widget_type.to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t,
            orientation: 0.0,
            color_a,
            color_b: [
                hit.hit_region as f32,
                if hit.hit_pressed { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: 0.0,
            pixel_aspect,
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
