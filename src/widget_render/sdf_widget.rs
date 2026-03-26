use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

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
    pub state_uniforms: Vec<String>,
    pub region_count: usize,
    pub width: f32,
    pub height: f32,
    pub paint_margin: f32,
}

pub const MAX_SDF_STATE_UNIFORMS: usize = 8;

pub fn shader_state_prop_name(name: &str) -> String {
    format!("shader-state-{}", name)
}

/// Per-widget hit state tracked between frames.
#[derive(Clone)]
pub struct SdfHitState {
    pub hit_region: i32, // -1 = none
    pub hit_pressed: bool,
}

impl Default for SdfHitState {
    fn default() -> Self {
        Self {
            hit_region: -1,
            hit_pressed: false,
        }
    }
}

thread_local! {
    static SDF_WIDGETS: RefCell<HashMap<String, Rc<SdfWidgetDef>>> = RefCell::new(HashMap::new());
    /// Hit state keyed by widget_id (from LayoutNode).
    static SDF_HIT_STATES: RefCell<HashMap<u64, SdfHitState>> = RefCell::new(HashMap::new());
    static SDF_TIME_ORIGIN: Instant = Instant::now();
    static SDF_TIME_SECONDS: RefCell<f32> = const { RefCell::new(0.0) };
}

pub fn set_sdf_hit_state(widget_id: u64, state: SdfHitState) {
    SDF_HIT_STATES.with(|s| s.borrow_mut().insert(widget_id, state));
}

pub fn get_sdf_hit_state(widget_id: u64) -> SdfHitState {
    SDF_HIT_STATES.with(|s| s.borrow().get(&widget_id).cloned().unwrap_or_default())
}

pub fn set_sdf_time_seconds(time_seconds: f32) {
    SDF_TIME_SECONDS.with(|slot| {
        *slot.borrow_mut() = time_seconds.max(0.0);
    });
}

pub fn current_sdf_time_seconds() -> f32 {
    SDF_TIME_SECONDS.with(|slot| *slot.borrow())
}

pub fn current_sdf_time_fallback_seconds() -> f32 {
    SDF_TIME_ORIGIN.with(|origin| origin.elapsed().as_secs_f32())
}

/// Resolve a state prop value by name — tries direct name, then prefixed.
fn resolve_state_prop(props: &HashMap<String, Value>, name: &str) -> Option<f64> {
    let val = props
        .get(name)
        .or_else(|| props.get(&shader_state_prop_name(name)));
    match val {
        Some(Value::Number(n)) => Some(*n),
        Some(Value::Bool(true)) => Some(1.0),
        Some(Value::Bool(false)) => Some(0.0),
        _ => None,
    }
}

fn hit_test_uniform_vars(
    props: &HashMap<String, Value>,
    state_uniforms: &[String],
) -> HashMap<String, f64> {
    let mut vars = HashMap::new();
    let itime = current_sdf_time_seconds();
    vars.insert(
        "itime".to_string(),
        if itime > 0.0 {
            itime as f64
        } else {
            current_sdf_time_fallback_seconds() as f64
        },
    );
    for state_name in state_uniforms {
        if let Some(value) = resolve_state_prop(props, state_name) {
            vars.insert(state_name.clone(), value);
        }
    }
    vars
}

/// Perform hit testing on an SDF widget at widget-local layout coordinates.
pub fn sdf_widget_hit_test(
    node: &LayoutNode,
    local_col: f32,
    local_row: f32,
    pixel_aspect: f32,
) -> i32 {
    let def = match sdf_widget_def(&node.widget_type) {
        Some(d) => d,
        None => return -1,
    };
    let (x, y) = crate::lang::sdf_hit::layout_to_sdf_coords(
        local_col,
        local_row,
        node.rect.width,
        node.rect.height,
        pixel_aspect,
    );
    let mut vars = hit_test_uniform_vars(&node.props, &def.state_uniforms);
    let aspect = if node.rect.height > 0.0 {
        (node.rect.width / node.rect.height) as f64
    } else {
        1.0
    };
    vars.insert("aspect".to_string(), aspect);
    crate::lang::sdf_hit::sdf_hit_test_with_vars(&def.sdf_expr, x, y, &vars)
}

/// Map mouse events to widget events for SDF widgets.
pub fn sdf_map_mouse_event(
    _node: &LayoutNode,
    mouse_kind: crossterm::event::MouseEventKind,
    local_col: f32,
    local_row: f32,
) -> super::MouseEventOutcome {
    use crossterm::event::{MouseButton, MouseEventKind};
    match mouse_kind {
        MouseEventKind::Down(MouseButton::Left) => super::MouseEventOutcome::Dispatch(
            super::WidgetEvent::PointerDown(super::PointerEvent {
                local_col,
                local_row,
            }),
        ),
        MouseEventKind::Drag(MouseButton::Left) => super::MouseEventOutcome::Dispatch(
            super::WidgetEvent::PointerDrag(super::PointerDragEvent {
                start_local_col: local_col,
                start_local_row: local_row,
                local_col,
                local_row,
            }),
        ),
        MouseEventKind::Up(MouseButton::Left) => {
            super::MouseEventOutcome::Dispatch(super::WidgetEvent::PointerUp(super::PointerEvent {
                local_col,
                local_row,
            }))
        }
        _ => super::MouseEventOutcome::Ignore,
    }
}

/// Dispatch SDF widget pointer events to Lisp callbacks (:on-click, :on-drag, :on-mouse-up).
/// Coordinates are normalized to [-1,1] matching the shader's coordinate space.
pub fn sdf_handle_event(
    node: &LayoutNode,
    event: &super::WidgetEvent,
) -> Option<super::EventOutput> {
    sdf_widget_def(&node.widget_type)?;

    let (local_col, local_row, prop_names): (f32, f32, &[&str]) = match event {
        super::WidgetEvent::PointerDown(pe) => {
            (pe.local_col, pe.local_row, &["on-mouse-down", "on-click"])
        }
        super::WidgetEvent::PointerDrag(pe) => (pe.local_col, pe.local_row, &["on-drag"]),
        super::WidgetEvent::PointerUp(pe) => (pe.local_col, pe.local_row, &["on-mouse-up"]),
        _ => return None,
    };

    let callback = prop_names
        .iter()
        .find_map(|name| node.props.get(*name))
        .filter(|cb| !matches!(cb, Value::Nil | Value::Bool(false)))?
        .clone();

    let wc = local_col - node.rect.col;
    let wr = local_row - node.rect.row;
    let sx = (wc / node.rect.width * 2.0 - 1.0) as f64;
    let sy = (wr / node.rect.height * 2.0 - 1.0) as f64;
    let pixel_aspect = if node.rect.height > 0.0 {
        node.rect.width / node.rect.height
    } else {
        1.0
    };
    let region = sdf_widget_hit_test(node, wc, wr, pixel_aspect);

    Some(super::EventOutput {
        callback,
        args: vec![
            Value::Number(sx),
            Value::Number(sy),
            Value::Number(region as f64),
        ],
    })
}

pub fn register_sdf_widget(def: SdfWidgetDef) {
    let name = def.name.clone();
    SDF_WIDGETS.with(|w| w.borrow_mut().insert(name, Rc::new(def)));
}

/// Register a compiled shader for use as a built-in widget's visual override.
/// Unlike full SdfWidgetDef, this doesn't need width/height/sdf_expr since the
/// built-in widget handles layout and hit testing.
pub fn register_inline_shader(
    name: String,
    shader_source: String,
    state_uniforms: Vec<String>,
    paint_margin: f32,
) {
    let def = SdfWidgetDef {
        name: name.clone(),
        shader_source,
        sdf_expr: crate::parser::Expression::Number(0.0), // placeholder — not used for hit testing
        state_uniforms,
        region_count: 0,
        width: 1.0,
        height: 1.0,
        paint_margin,
    };
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

pub fn sdf_widget_paint_rect(rect: Rect, paint_margin: f32) -> Rect {
    if paint_margin <= 0.0 {
        return rect;
    }
    Rect {
        row: rect.row - paint_margin,
        col: rect.col - paint_margin,
        width: rect.width + paint_margin * 2.0,
        height: rect.height + paint_margin * 2.0,
    }
}

pub fn sdf_widget_logical_uv_bounds(logical_rect: Rect, paint_rect: Rect) -> [f32; 4] {
    if paint_rect.width <= 0.0 || paint_rect.height <= 0.0 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let u0 = ((logical_rect.col - paint_rect.col) / paint_rect.width).clamp(0.0, 1.0);
    let v0 = ((logical_rect.row - paint_rect.row) / paint_rect.height).clamp(0.0, 1.0);
    let u1 = ((logical_rect.col + logical_rect.width - paint_rect.col) / paint_rect.width)
        .clamp(0.0, 1.0);
    let v1 = ((logical_rect.row + logical_rect.height - paint_rect.row) / paint_rect.height)
        .clamp(0.0, 1.0);
    [u0, v0, u1, v1]
}

fn numeric_literal(expr: &Expression) -> Option<f32> {
    match expr {
        Expression::Number(n) => Some(*n as f32),
        _ => None,
    }
}

fn prop_uniform_value(props: &HashMap<String, Value>, name: &str) -> f32 {
    resolve_state_prop(props, name).unwrap_or(0.0) as f32
}

fn vec2_literal(expr: &Expression) -> Option<(f32, f32)> {
    let Expression::List(items) = expr else {
        return None;
    };
    let [Expression::Symbol(head), x, y] = items.as_slice() else {
        return None;
    };
    if head != "vec2" {
        return None;
    }
    Some((numeric_literal(x)?, numeric_literal(y)?))
}

fn shadow_extent_norm(expr: &Expression) -> Option<f32> {
    let Expression::List(items) = expr else {
        return None;
    };
    let Some(Expression::Symbol(head)) = items.first() else {
        return None;
    };
    if head != "shadow" {
        return None;
    }

    let mut blur = 0.0;
    let mut spread = 0.0;
    let mut offset = (0.0, 0.0);
    let mut saw_blur = false;
    let mut i = 1;
    while i + 1 < items.len() {
        if let Expression::Keyword(key) = &items[i] {
            match key.as_str() {
                "blur" => {
                    blur = numeric_literal(&items[i + 1])?;
                    saw_blur = true;
                }
                "spread" => {
                    spread = numeric_literal(&items[i + 1])?;
                }
                "offset" => {
                    offset = vec2_literal(&items[i + 1])?;
                }
                _ => {}
            }
            i += 2;
        } else {
            i += 1;
        }
    }

    if !saw_blur {
        return None;
    }

    Some(offset.0.abs().max(offset.1.abs()) + blur + spread)
}

fn max_shadow_extent_norm(expr: &Expression) -> f32 {
    match expr {
        Expression::List(items) if !items.is_empty() => {
            let mut max_extent: f32 = 0.0;
            if let Some(Expression::Symbol(head)) = items.first()
                && head == "material"
            {
                let mut i = 1;
                while i + 1 < items.len() {
                    if let Expression::Keyword(key) = &items[i]
                        && key == "shadow"
                        && let Some(extent) = shadow_extent_norm(&items[i + 1])
                    {
                        max_extent = max_extent.max(extent);
                        i += 2;
                        continue;
                    }
                    i += 1;
                }
            }
            for item in items {
                max_extent = max_extent.max(max_shadow_extent_norm(item));
            }
            max_extent
        }
        _ => 0.0,
    }
}

pub fn estimate_shadow_paint_margin(expr: &Expression, width: f32, height: f32) -> f32 {
    let extent_norm = max_shadow_extent_norm(expr);
    if extent_norm <= 0.0 {
        return 0.0;
    }
    let cells = (extent_norm * (width.max(height) * 0.5)).ceil();
    cells.max(1.0)
}

/// Build Metal primitives for an SDF widget.
#[cfg(target_os = "macos")]
pub fn sdf_widget_metal_primitives(
    widget_type: &str,
    node: &LayoutNode,
    viewport: WidgetViewport,
) -> Vec<MetalPrimitive> {
    use super::{WidgetInstance, ndc_bounds};

    let def = match sdf_widget_def(widget_type) {
        Some(d) => d,
        None => return Vec::new(),
    };

    let paint_rect = sdf_widget_paint_rect(node.rect, def.paint_margin);
    let (ndc_min, ndc_max) = ndc_bounds(paint_rect, viewport);
    let logical_uv_bounds = sdf_widget_logical_uv_bounds(node.rect, paint_rect);
    let px_w = node.rect.width * viewport.cell_w;
    let px_h = node.rect.height * viewport.cell_h;

    let value_t = super::get_f32_prop(&node.props, "value", 0.0);
    let color_a = super::resolve_named_color(&node.props, "color", crate::theme::GREEN()).to_rgba();

    let pixel_aspect = if px_h > 0.0 { px_w / px_h } else { 1.0 };
    let hit = get_sdf_hit_state(node.widget_id);
    let mut uniform_a = [0.0; 4];
    let mut uniform_b = [0.0; 4];
    for (idx, name) in def
        .state_uniforms
        .iter()
        .take(MAX_SDF_STATE_UNIFORMS)
        .enumerate()
    {
        let value = prop_uniform_value(&node.props, name);
        if idx < 4 {
            uniform_a[idx] = value;
        } else {
            uniform_b[idx - 4] = value;
        }
    }

    vec![MetalPrimitive::WidgetInstance {
        widget_type: widget_type.to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a,
            uniform_b,
            color_a,
            color_b: [
                hit.hit_region as f32,
                if hit.hit_pressed { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ],
            color_c: logical_uv_bounds,
            color_d: [0.0; 4],
            corner_radius: 0.0,
            pixel_aspect,
        },
        is_background: false,
    }]
}

/// Build Metal primitives for an SDF widget used as a container background.
/// Uses the container's rect instead of the widget's own rect.
#[cfg(target_os = "macos")]
pub fn sdf_widget_background_primitives(
    widget_type: &str,
    rect: Rect,
    viewport: WidgetViewport,
    props: &HashMap<String, Value>,
) -> Vec<MetalPrimitive> {
    use super::{WidgetInstance, ndc_bounds};

    let def = match sdf_widget_def(widget_type) {
        Some(d) => d,
        None => return Vec::new(),
    };

    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    let pixel_aspect = if px_h > 0.0 { px_w / px_h } else { 1.0 };
    const NO_HIT_REGION: f32 = -1.0;
    const FULL_UV_BOUNDS: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

    // Resolve state uniforms from the box's props
    let mut uniform_a = [0.0; 4];
    let mut uniform_b = [0.0; 4];
    for (idx, name) in def
        .state_uniforms
        .iter()
        .take(MAX_SDF_STATE_UNIFORMS)
        .enumerate()
    {
        let value = prop_uniform_value(props, name);
        if idx < 4 {
            uniform_a[idx] = value;
        } else {
            uniform_b[idx - 4] = value;
        }
    }

    vec![MetalPrimitive::WidgetInstance {
        widget_type: widget_type.to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a,
            uniform_b,
            color_a: [0.0; 4],
            color_b: [NO_HIT_REGION, 0.0, 0.0, 0.0],
            color_c: FULL_UV_BOUNDS,
            color_d: [0.0; 4],
            corner_radius: 0.0,
            pixel_aspect,
        },
        is_background: true,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Expression;

    #[test]
    fn paint_rect_expands_uniformly() {
        let rect = Rect {
            row: 10.0,
            col: 20.0,
            width: 2.0,
            height: 3.0,
        };
        let expanded = sdf_widget_paint_rect(rect, 1.0);
        assert_eq!(
            expanded,
            Rect {
                row: 9.0,
                col: 19.0,
                width: 4.0,
                height: 5.0,
            }
        );
    }

    #[test]
    fn zero_paint_margin_keeps_rect() {
        let rect = Rect {
            row: 1.0,
            col: 2.0,
            width: 3.0,
            height: 4.0,
        };
        assert_eq!(sdf_widget_paint_rect(rect, 0.0), rect);
    }

    #[test]
    fn logical_uv_bounds_cover_center_subrect() {
        let logical = Rect {
            row: 10.0,
            col: 20.0,
            width: 2.0,
            height: 2.0,
        };
        let paint = sdf_widget_paint_rect(logical, 1.0);
        assert_eq!(
            sdf_widget_logical_uv_bounds(logical, paint),
            [0.25, 0.25, 0.75, 0.75]
        );
    }

    #[test]
    fn estimate_shadow_paint_margin_from_constant_shadow() {
        let expr = Expression::List(vec![
            Expression::Symbol("sdf/layer".into()),
            Expression::List(vec![
                Expression::Symbol("sdf/fill".into()),
                Expression::Symbol("shape".into()),
                Expression::List(vec![
                    Expression::Symbol("material".into()),
                    Expression::Keyword("color".into()),
                    Expression::Keyword("accent".into()),
                    Expression::Keyword("shadow".into()),
                    Expression::List(vec![
                        Expression::Symbol("shadow".into()),
                        Expression::Keyword("color".into()),
                        Expression::List(vec![
                            Expression::Symbol("rgba".into()),
                            Expression::Number(0.0),
                            Expression::Number(0.0),
                            Expression::Number(0.0),
                            Expression::Number(0.2),
                        ]),
                        Expression::Keyword("blur".into()),
                        Expression::Number(0.5),
                        Expression::Keyword("offset".into()),
                        Expression::List(vec![
                            Expression::Symbol("vec2".into()),
                            Expression::Number(0.0),
                            Expression::Number(0.5),
                        ]),
                    ]),
                ]),
            ]),
        ]);
        assert_eq!(estimate_shadow_paint_margin(&expr, 4.0, 4.0), 2.0);
    }

    #[test]
    fn estimate_shadow_paint_margin_is_zero_without_constant_shadow() {
        let expr = Expression::List(vec![
            Expression::Symbol("sdf/layer".into()),
            Expression::List(vec![
                Expression::Symbol("sdf/fill".into()),
                Expression::Symbol("shape".into()),
                Expression::List(vec![
                    Expression::Symbol("material".into()),
                    Expression::Keyword("color".into()),
                    Expression::Keyword("accent".into()),
                ]),
            ]),
        ]);
        assert_eq!(estimate_shadow_paint_margin(&expr, 4.0, 4.0), 0.0);
    }
}
