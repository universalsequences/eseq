use std::cell::RefCell;
use std::rc::Rc;

use super::{Align, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, resolve_align};
use super::{
    GpuCirclePrimitive, GpuCircleVisibleHalf, GpuPrimitive, GpuRectPrimitive,
    WidgetInstance, WidgetViewport, get_f32_prop, ndc_bounds, resolve_named_color,
};
use crate::backend::Color;
use crate::layout::{
    Constraints, LayoutCtx, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
    prop_is_keyword, shrink_constraints_xy,
};
use crate::vm::Value;
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

/// Compute the bounding rect that covers the box itself and all its children.
/// This ensures the SDF background covers the full scrollable content.
fn content_extent(node: &LayoutNode) -> Rect {
    let mut rect = node.rect;
    for child in &node.children {
        let cr = child_extent(child);
        // Use the child's actual laid out inset rather than raw padding.
        // This keeps the background extent aligned with aspect-corrected vertical padding.
        let inset_x = (child.rect.col - node.rect.col).max(0.0);
        let inset_y = (child.rect.row - node.rect.row).max(0.0);
        let right = (cr.col + cr.width + inset_x).max(rect.col + rect.width);
        let bottom = (cr.row + cr.height + inset_y).max(rect.row + rect.height);
        rect.width = right - rect.col;
        rect.height = bottom - rect.row;
    }
    rect
}

fn child_extent(node: &LayoutNode) -> Rect {
    let mut rect = node.rect;
    // Don't recurse into scroll containers — their children may be much
    // taller than the viewport, which would inflate the parent's extent.
    if node.widget_type == "scroll" {
        return rect;
    }
    for child in &node.children {
        let cr = child_extent(child);
        let right = (cr.col + cr.width).max(rect.col + rect.width);
        let bottom = (cr.row + cr.height).max(rect.row + rect.height);
        rect.width = right - rect.col;
        rect.height = bottom - rect.row;
    }
    rect
}

pub struct BoxWidget;

pub static BOX_WIDGET: BoxWidget = BoxWidget;

fn prop_truthy(props: &std::collections::HashMap<String, Value>, key: &str) -> bool {
    match props.get(key) {
        Some(Value::Bool(value)) => *value,
        Some(Value::Nil) | None => false,
        Some(_) => true,
    }
}

fn box_state_active(props: &std::collections::HashMap<String, Value>, key: &str) -> bool {
    match props.get(key) {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => *value != 0.0,
        Some(Value::ReactiveRef { .. }) => get_f32_prop(props, key, 0.0) != 0.0,
        _ => false,
    }
}

fn state_color_prop<'a>(
    props: &'a std::collections::HashMap<String, Value>,
    base_prop: &'a str,
    selected_prop: &'a str,
    muted_prop: &'a str,
) -> Option<&'a str> {
    if box_state_active(props, "selected") && props.contains_key(selected_prop) {
        Some(selected_prop)
    } else if box_state_active(props, "muted") && props.contains_key(muted_prop) {
        Some(muted_prop)
    } else if props.contains_key(base_prop) {
        Some(base_prop)
    } else {
        None
    }
}

fn box_mouse_info(
    phase: &str,
    modifiers: KeyModifiers,
    node: &LayoutNode,
    local_col: f32,
    local_row: f32,
) -> Value {
    let mut info = super::pointer_modifier_info(modifiers);
    info.insert(
        "phase".to_string(),
        Rc::new(RefCell::new(Value::String(phase.to_string()))),
    );
    let wc = local_col - node.rect.col;
    let wr = local_row - node.rect.row;
    let sx = if node.rect.width > 0.0 {
        wc / node.rect.width * 2.0 - 1.0
    } else {
        0.0
    };
    let sy = if node.rect.height > 0.0 {
        wr / node.rect.height * 2.0 - 1.0
    } else {
        0.0
    };
    info.insert(
        "x".to_string(),
        Rc::new(RefCell::new(Value::Number(wc as f64))),
    );
    info.insert(
        "y".to_string(),
        Rc::new(RefCell::new(Value::Number(wr as f64))),
    );
    // Absolute pointer position in tile-local layout CONTENT cells: the
    // tile's own scroll offsets are already folded in, so this matches the
    // space layout rects live in (the backend draws the layout, and the
    // overlay channel, translated by -scroll). It is therefore the space
    // `context-menu` :anchor-col/:anchor-row are expressed in — a
    // frame-anchored widget reconciles the two via `current_frame_viewport`,
    // which reports the frame in this same content space.
    //
    // Not folded in: the offset of an enclosing `scroll` WIDGET, which
    // children read separately via `scroll::current_event_scroll_offset`.
    info.insert(
        "col".to_string(),
        Rc::new(RefCell::new(Value::Number(local_col as f64))),
    );
    info.insert(
        "row".to_string(),
        Rc::new(RefCell::new(Value::Number(local_row as f64))),
    );
    info.insert(
        "sx".to_string(),
        Rc::new(RefCell::new(Value::Number(sx as f64))),
    );
    info.insert(
        "sy".to_string(),
        Rc::new(RefCell::new(Value::Number(sy as f64))),
    );
    Value::Map(info)
}

pub(crate) fn box_drop_info(
    node: &LayoutNode,
    drag_type: &str,
    payload: Value,
    local_col: f32,
    local_row: f32,
) -> Value {
    let mut info = match box_mouse_info("drop", KeyModifiers::empty(), node, local_col, local_row) {
        Value::Map(map) => map,
        _ => std::collections::HashMap::new(),
    };
    info.insert(
        "drag-type".to_string(),
        Rc::new(RefCell::new(Value::String(drag_type.to_string()))),
    );
    info.insert("payload".to_string(), Rc::new(RefCell::new(payload)));
    info.insert(
        "target".to_string(),
        Rc::new(RefCell::new(
            node.props.get("drop-meta").cloned().unwrap_or(Value::Nil),
        )),
    );
    Value::Map(info)
}

fn box_drag_value(node: &LayoutNode, drag_type: &str) -> Value {
    let mut map = std::collections::HashMap::new();
    map.insert(
        "kind".to_string(),
        Rc::new(RefCell::new(Value::String("widget-drag".to_string()))),
    );
    map.insert(
        "drag-type".to_string(),
        Rc::new(RefCell::new(Value::String(drag_type.to_string()))),
    );
    map.insert(
        "payload".to_string(),
        Rc::new(RefCell::new(
            node.props
                .get("drag-payload")
                .cloned()
                .unwrap_or(Value::Nil),
        )),
    );
    Value::Map(map)
}

fn normalized_corner_radius(rect: Rect, viewport: WidgetViewport, radius_px: f32) -> f32 {
    if radius_px <= 0.0 {
        return 0.001;
    }
    let radius_px = super::ui_design_px(radius_px);
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((radius_px * 2.0) / px_h).clamp(0.001, 0.5)
}

fn push_rounded_rect(
    prims: &mut Vec<GpuPrimitive>,
    rect: Rect,
    color: Color,
    viewport: WidgetViewport,
    radius_px: f32,
) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    prims.push(GpuPrimitive::WidgetInstance {
        widget_type: "box".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [0.0; 4],
            uniform_b: [0.0; 4],
            uniform_c: [0.0; 4],
            uniform_d: [0.0; 4],
            color_a: color.to_rgba(),
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: normalized_corner_radius(rect, viewport, radius_px),
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
        is_background: true,
    });
}

fn inset_rect(rect: Rect, inset_x: f32, inset_y: f32) -> Rect {
    Rect {
        row: rect.row + inset_y,
        col: rect.col + inset_x,
        width: (rect.width - inset_x * 2.0).max(0.0),
        height: (rect.height - inset_y * 2.0).max(0.0),
    }
}

impl WidgetDefinition for BoxWidget {
    fn names(&self) -> &'static [&'static str] {
        &["box"]
    }

    fn is_container(&self) -> bool {
        true
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &[
            "padding", "width", "height", "aspect", "align", "h-align", "v-align",
        ]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["selected", "muted", "macro-owned"]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "background", "background-color", "border-color", "border-width", "corner-radius",
            "padding", "width", "height", "aspect", "align", "h-align", "v-align", "flex",
            "selected", "selected-background-color", "selected-border-color", "muted",
            "muted-background-color", "muted-border-color", "drop-hover-background-color",
            "drop-hover-border-color", "drag-type", "drag-payload", "drag-modifier",
            "capture-pointer", "focusable", "key", "on-click", "on-right-click",
            "on-double-click", "on-drag", "on-drop", "on-mouse-down", "on-mouse-up",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        children: &[Value],
        constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let padding = get_prop_num(node, "padding").map(f64_to_f32).unwrap_or(0.0);
        let pad_y = padding / constraints.aspect;
        let fixed_width =
            if prop_is_keyword(node, "width", "fill") && constraints.max_width.is_finite() {
                Some(constraints.max_width)
            } else {
                get_prop_num(node, "width").map(f64_to_f32)
            };
        let fixed_height =
            if prop_is_keyword(node, "height", "fill") && constraints.max_height.is_finite() {
                Some(constraints.max_height)
            } else {
                get_prop_num(node, "height").map(f64_to_f32)
            };
        let aspect_height = fixed_width.and_then(|width| {
            get_prop_num(node, "aspect")
                .map(f64_to_f32)
                .filter(|value| *value > 0.0)
                .map(|pixel_aspect| {
                    let cell_pixel_aspect = if ctx.cell_h > 0.0 {
                        ctx.cell_w / ctx.cell_h
                    } else {
                        1.0
                    };
                    width / pixel_aspect * cell_pixel_aspect
                })
        });
        let child_constraints = if let Some(width) = fixed_width {
            let mut constrained = constraints;
            constrained.max_width = (width - padding * 2.0).max(0.0);
            constrained.min_width = constrained.min_width.min(constrained.max_width);
            shrink_constraints_xy(constrained, 0.0, pad_y)
        } else {
            shrink_constraints_xy(constraints, padding, pad_y)
        };
        let child_size =
            if fixed_width.is_some() && (fixed_height.is_some() || aspect_height.is_some()) {
                None
            } else {
                children
                    .first()
                    .and_then(|child| measure_child(child, child_constraints))
            };
        let width = fixed_width.unwrap_or_else(|| {
            child_size
                .map(|size| size.width + padding * 2.0)
                .unwrap_or(padding * 2.0)
        });
        let height = if let Some(height) = fixed_height {
            height
        } else if let Some(height) = aspect_height {
            height
        } else if let Some(pixel_aspect) = get_prop_num(node, "aspect")
            .map(f64_to_f32)
            .filter(|value| *value > 0.0)
        {
            let cell_pixel_aspect = if ctx.cell_h > 0.0 {
                ctx.cell_w / ctx.cell_h
            } else {
                1.0
            };
            width / pixel_aspect * cell_pixel_aspect
        } else {
            child_size
                .map(|size| size.height + pad_y * 2.0)
                .unwrap_or(pad_y * 2.0)
        };
        Some(Size { width, height })
    }

    fn layout_children(
        &self,
        node: &Value,
        area: Rect,
        children: &[Value],
        aspect: f32,
        _measure_ctx: &MeasureCtx<'_>,
        _layout_ctx: LayoutCtx,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect, LayoutCtx) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let padding = get_prop_num(node, "padding").map(f64_to_f32).unwrap_or(0.0);
        let pad_y = padding / aspect;
        let fallback = resolve_align(node, "align", Align::Stretch);
        let h_align = resolve_align(node, "h-align", fallback);
        let v_align = resolve_align(node, "v-align", fallback);

        let inner_width = (area.width - padding * 2.0).max(0.0);
        let inner_height = (area.height - pad_y * 2.0).max(0.0);

        children
            .first()
            .map(|child| {
                // If both axes stretch, skip measurement (current behavior)
                if h_align == Align::Stretch && v_align == Align::Stretch {
                    return build_child(
                        child,
                        Rect {
                            row: area.row + pad_y,
                            col: area.col + padding,
                            width: inner_width,
                            height: inner_height,
                        },
                        LayoutCtx::default(),
                    );
                }

                let inner_constraints = Constraints {
                    min_width: 0.0,
                    max_width: inner_width,
                    min_height: 0.0,
                    max_height: inner_height,
                    aspect: 1.0,
                };
                let size = measure_child(child, inner_constraints).unwrap_or(Size {
                    width: inner_width,
                    height: inner_height,
                });

                let child_width =
                    if h_align == Align::Stretch || prop_is_keyword(child, "width", "fill") {
                        inner_width
                    } else {
                        size.width
                    };
                let child_height = if v_align == Align::Stretch {
                    inner_height
                } else {
                    size.height
                };

                let col = match h_align {
                    Align::Start | Align::Stretch | Align::Baseline => area.col + padding,
                    Align::Center => area.col + padding + (inner_width - child_width) / 2.0,
                    Align::End => area.col + padding + inner_width - child_width,
                };
                let row = match v_align {
                    Align::Start | Align::Stretch | Align::Baseline => area.row + pad_y,
                    Align::Center => area.row + pad_y + (inner_height - child_height) / 2.0,
                    Align::End => area.row + pad_y + inner_height - child_height,
                };

                build_child(
                    child,
                    Rect {
                        row,
                        col,
                        width: child_width,
                        height: child_height,
                    },
                    LayoutCtx::default(),
                )
            })
            .into_iter()
            .collect()
    }

    fn begin_gesture(
        &self,
        node: &LayoutNode,
        _local_col: f32,
        _local_row: f32,
        modifiers: KeyModifiers,
    ) -> Option<Value> {
        let drag_modifier_matches = match node.props.get("drag-modifier") {
            None => true,
            Some(Value::String(value) | Value::Keyword(value)) if value == "none" => {
                modifiers.is_empty()
            }
            Some(Value::String(value) | Value::Keyword(value)) if value == "shift" => {
                modifiers.contains(KeyModifiers::SHIFT)
            }
            Some(Value::String(value) | Value::Keyword(value)) if value == "ctrl" => {
                modifiers.contains(KeyModifiers::CONTROL)
            }
            Some(Value::String(value) | Value::Keyword(value)) if value == "alt" => {
                modifiers.contains(KeyModifiers::ALT)
            }
            Some(Value::String(value) | Value::Keyword(value))
                if matches!(value.as_str(), "super" | "cmd" | "meta") =>
            {
                modifiers.contains(KeyModifiers::SUPER)
            }
            Some(_) => false,
        };
        if drag_modifier_matches
            && let Some(Value::String(drag_type) | Value::Keyword(drag_type)) =
                node.props.get("drag-type")
        {
            Some(box_drag_value(node, drag_type))
        } else if prop_truthy(&node.props, "capture-pointer")
            && (node.props.contains_key("on-click")
                || node.props.contains_key("on-drag")
                || node.props.contains_key("on-mouse-up"))
        {
            Some(Value::String("capture-pointer".to_string()))
        } else if node.props.contains_key("on-drag") || node.props.contains_key("on-mouse-up") {
            Some(Value::String("box-pointer".to_string()))
        } else {
            None
        }
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Right) => {
                if node.props.contains_key("on-right-click") {
                    return MouseEventOutcome::Dispatch(WidgetEvent::Custom(box_mouse_info(
                        "right-click",
                        modifiers,
                        node,
                        local_col,
                        local_row,
                    )));
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // macOS convention: ctrl+click is a right-click synonym (some
                // terminals swallow the right button). Only when the widget
                // opts in via :on-right-click.
                if modifiers.contains(KeyModifiers::CONTROL)
                    && node.props.contains_key("on-right-click")
                {
                    return MouseEventOutcome::Dispatch(WidgetEvent::Custom(box_mouse_info(
                        "right-click",
                        modifiers,
                        node,
                        local_col,
                        local_row,
                    )));
                }
                if node.props.contains_key("on-mouse-down") {
                    return MouseEventOutcome::Dispatch(WidgetEvent::Custom(box_mouse_info(
                        "down", modifiers, node, local_col, local_row,
                    )));
                }
                if node.props.contains_key("on-click") {
                    return MouseEventOutcome::Dispatch(WidgetEvent::Custom(box_mouse_info(
                        "click", modifiers, node, local_col, local_row,
                    )));
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if node.props.contains_key("on-drag") => {
                return MouseEventOutcome::Dispatch(WidgetEvent::Custom(box_mouse_info(
                    "drag", modifiers, node, local_col, local_row,
                )));
            }
            MouseEventKind::Up(MouseButton::Left) if node.props.contains_key("on-mouse-up") => {
                return MouseEventOutcome::Dispatch(WidgetEvent::Custom(box_mouse_info(
                    "up", modifiers, node, local_col, local_row,
                )));
            }
            _ => {}
        }
        MouseEventOutcome::Ignore
    }

    fn double_click_event(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
    ) -> Option<WidgetEvent> {
        if node.props.contains_key("on-double-click") {
            Some(WidgetEvent::Custom(box_mouse_info(
                "double-click",
                KeyModifiers::empty(),
                node,
                local_col,
                local_row,
            )))
        } else {
            None
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let (callback_name, arg) = match event {
            WidgetEvent::Activate(modifiers) => (
                "on-click",
                box_mouse_info("click", modifiers, node, node.rect.col, node.rect.row),
            ),
            WidgetEvent::Custom(value) => {
                let phase = match &value {
                    Value::Map(map) => map.get("phase").and_then(|v| match &*v.borrow() {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    }),
                    _ => None,
                }?;
                let callback_name = match phase.as_str() {
                    "click" => "on-click",
                    "right-click" => "on-right-click",
                    "double-click" => "on-double-click",
                    "down" => "on-mouse-down",
                    "drag" => "on-drag",
                    "up" => "on-mouse-up",
                    _ => return None,
                };
                (callback_name, value)
            }
            _ => return None,
        };
        let callback = node.props.get(callback_name)?.clone();
        Some(EventOutput {
            callback,
            args: vec![arg],
        })
    }

    fn fragment_shader(
        &self,
        _widget_type: &str,
        backend: super::ShaderBackend,
    ) -> Option<&'static str> {
        super::ROUNDED_RECT_SHADER.source(backend)
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let mut prims = Vec::new();
        let corner_radius_px = match node.props.get("corner-radius") {
            Some(Value::Number(n)) => (*n as f32).max(0.0),
            _ => 0.0,
        };
        let border_width_design_px = match node.props.get("border-width") {
            Some(Value::Number(n)) => (*n as f32).max(0.0),
            _ => 1.0,
        };
        let border_width_px = super::ui_design_px(border_width_design_px);
        let has_rounded_corners = corner_radius_px > 0.0;
        let hover_drop = super::drop_target_hovered(node.widget_id);
        let background_color =
            if hover_drop && node.props.contains_key("drop-hover-background-color") {
                Some(resolve_named_color(
                    &node.props,
                    "drop-hover-background-color",
                    Color::rgba(0.0, 0.0, 0.0, 0.0),
                ))
            } else {
                state_color_prop(
                    &node.props,
                    "background-color",
                    "selected-background-color",
                    "muted-background-color",
                )
                .map(|prop| resolve_named_color(&node.props, prop, Color::rgba(0.0, 0.0, 0.0, 0.0)))
            };
        let border_color = if hover_drop && node.props.contains_key("drop-hover-border-color") {
            Some(resolve_named_color(
                &node.props,
                "drop-hover-border-color",
                Color::rgba(0.0, 0.0, 0.0, 0.0),
            ))
        } else {
            state_color_prop(
                &node.props,
                "border-color",
                "selected-border-color",
                "muted-border-color",
            )
            .map(|prop| resolve_named_color(&node.props, prop, Color::rgba(0.0, 0.0, 0.0, 0.0)))
        };

        if has_rounded_corners {
            if let Some(color) = border_color {
                if color.a > 0.0 && border_width_px > 0.0 {
                    push_rounded_rect(&mut prims, node.rect, color, viewport, corner_radius_px);
                }
            }
            if let Some(color) = background_color {
                if color.a > 0.0 {
                    let inset_x = if viewport.cell_w > 0.0 {
                        border_width_px / viewport.cell_w
                    } else {
                        0.0
                    };
                    let inset_y = if viewport.cell_h > 0.0 {
                        border_width_px / viewport.cell_h
                    } else {
                        0.0
                    };
                    push_rounded_rect(
                        &mut prims,
                        inset_rect(node.rect, inset_x, inset_y),
                        color,
                        viewport,
                        (corner_radius_px - border_width_design_px).max(0.0),
                    );
                }
            }
        } else if let Some(color) = background_color {
            if color.a > 0.0 {
                push_rounded_rect(&mut prims, node.rect, color, viewport, 0.0);
            }
        }

        if !has_rounded_corners {
            let Some(color) = border_color else {
                if let Some(Value::String(bg_type)) = node.props.get("background") {
                    let has_scroll_child = node.children.iter().any(|c| c.widget_type == "scroll");
                    let bg_rect = if has_scroll_child {
                        node.rect
                    } else {
                        content_extent(node)
                    };
                    prims.extend(super::sdf_widget::sdf_widget_background_primitives(
                        bg_type,
                        node.widget_id,
                        bg_rect,
                        viewport,
                        &node.props,
                    ));
                }
                return prims;
            };
            if color.a > 0.0 {
                let px = border_width_px;
                let bw_x = if viewport.cell_w > 0.0 {
                    px / viewport.cell_w
                } else {
                    0.0
                };
                let bw_y = if viewport.cell_h > 0.0 {
                    px / viewport.cell_h
                } else {
                    0.0
                };
                if bw_x > 0.0 && bw_y > 0.0 {
                    let top = Rect {
                        height: bw_y.min(node.rect.height),
                        ..node.rect
                    };
                    let bottom = Rect {
                        row: node.rect.row + (node.rect.height - bw_y).max(0.0),
                        height: bw_y.min(node.rect.height),
                        ..node.rect
                    };
                    let left = Rect {
                        width: bw_x.min(node.rect.width),
                        ..node.rect
                    };
                    let right = Rect {
                        col: node.rect.col + (node.rect.width - bw_x).max(0.0),
                        width: bw_x.min(node.rect.width),
                        ..node.rect
                    };
                    for rect in [top, bottom, left, right] {
                        prims.push(GpuPrimitive::Rect(GpuRectPrimitive { rect, color }));
                    }
                }
            }
        }

        if let Some(Value::String(bg_type)) = node.props.get("background") {
            // Scroll-containing boxes use their own layout rect: content_extent
            // would include scroll children, but node.rect is the viewport.
            let has_scroll_child = node.children.iter().any(|c| c.widget_type == "scroll");
            let bg_rect = if has_scroll_child {
                node.rect
            } else {
                content_extent(node)
            };
            prims.extend(super::sdf_widget::sdf_widget_background_primitives(
                bg_type,
                node.widget_id,
                bg_rect,
                viewport,
                &node.props,
            ));
        }

        if box_state_active(&node.props, "macro-owned") {
            let outer_px = super::ui_design_px(12.0);
            let inner_px = super::ui_design_px(8.0);
            let margin_px = super::ui_design_px(2.5);
            let center = [
                node.rect.col + (margin_px + outer_px * 0.5) / viewport.cell_w.max(1.0),
                node.rect.row + (margin_px + outer_px * 0.5) / viewport.cell_h.max(1.0),
            ];
            for (radius_px, color) in [
                (outer_px * 0.5, Color::rgba(0.02, 0.04, 0.025, 1.0)),
                (inner_px * 0.5, Color::rgba(0.12, 0.95, 0.38, 1.0)),
            ] {
                prims.push(GpuPrimitive::Circle(GpuCirclePrimitive {
                    center,
                    radius_px,
                    color,
                    visible_half: GpuCircleVisibleHalf::Full,
                }));
            }
        }

        prims
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::theme;

    #[test]
    fn selected_state_color_takes_precedence_over_muted_and_default() {
        let props = std::collections::HashMap::from([
            (
                "background-color".to_string(),
                Value::Keyword("black".to_string()),
            ),
            (
                "muted-background-color".to_string(),
                Value::Keyword("gray".to_string()),
            ),
            (
                "selected-background-color".to_string(),
                Value::Keyword("blue".to_string()),
            ),
            ("selected".to_string(), Value::Number(1.0)),
            ("muted".to_string(), Value::Bool(true)),
        ]);

        let prop = state_color_prop(
            &props,
            "background-color",
            "selected-background-color",
            "muted-background-color",
        );

        assert_eq!(prop, Some("selected-background-color"));
        assert_eq!(
            resolve_named_color(&props, prop.unwrap(), theme::BLACK()),
            theme::BLUE()
        );
    }
}
