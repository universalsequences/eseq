use std::cell::RefCell;
use std::rc::Rc;

use super::{Align, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, resolve_align};
#[cfg(target_os = "macos")]
use super::{
    MetalPrimitive, MetalRectPrimitive, WidgetInstance, WidgetViewport, ndc_bounds,
    resolve_named_color,
};
#[cfg(target_os = "macos")]
use crate::backend::Color;
use crate::layout::Rect;
use crate::layout::{
    Constraints, LayoutNode, MeasureCtx, Size, f64_to_f32, get_prop_num, prop_is_keyword,
    shrink_constraints_xy,
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

fn box_mouse_info(
    phase: &str,
    modifiers: KeyModifiers,
    node: &LayoutNode,
    local_col: f32,
    local_row: f32,
) -> Value {
    let mut info = std::collections::HashMap::new();
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
    info.insert(
        "sx".to_string(),
        Rc::new(RefCell::new(Value::Number(sx as f64))),
    );
    info.insert(
        "sy".to_string(),
        Rc::new(RefCell::new(Value::Number(sy as f64))),
    );
    info.insert(
        "shift".to_string(),
        Rc::new(RefCell::new(Value::Bool(
            modifiers.contains(KeyModifiers::SHIFT),
        ))),
    );
    info.insert(
        "ctrl".to_string(),
        Rc::new(RefCell::new(Value::Bool(
            modifiers.contains(KeyModifiers::CONTROL),
        ))),
    );
    info.insert(
        "alt".to_string(),
        Rc::new(RefCell::new(Value::Bool(
            modifiers.contains(KeyModifiers::ALT),
        ))),
    );
    let super_pressed = modifiers.contains(KeyModifiers::SUPER);
    info.insert(
        "super".to_string(),
        Rc::new(RefCell::new(Value::Bool(super_pressed))),
    );
    info.insert(
        "cmd".to_string(),
        Rc::new(RefCell::new(Value::Bool(super_pressed))),
    );
    info.insert(
        "meta".to_string(),
        Rc::new(RefCell::new(Value::Bool(super_pressed))),
    );
    Value::Map(info)
}

#[cfg(target_os = "macos")]
fn normalized_corner_radius(rect: Rect, viewport: WidgetViewport, radius_px: f32) -> f32 {
    if radius_px <= 0.0 {
        return 0.001;
    }
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((radius_px * 2.0) / px_h).clamp(0.001, 0.5)
}

#[cfg(target_os = "macos")]
fn push_rounded_rect(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    color: Color,
    viewport: WidgetViewport,
    radius_px: f32,
) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    prims.push(MetalPrimitive::WidgetInstance {
        widget_type: "box".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [0.0; 4],
            uniform_b: [0.0; 4],
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

#[cfg(target_os = "macos")]
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
        let child_constraints = if let Some(width) = fixed_width {
            let mut constrained = constraints;
            constrained.max_width = (width - padding * 2.0).max(0.0);
            constrained.min_width = constrained.min_width.min(constrained.max_width);
            shrink_constraints_xy(constrained, 0.0, pad_y)
        } else {
            shrink_constraints_xy(constraints, padding, pad_y)
        };
        let child_size = children
            .first()
            .and_then(|child| measure_child(child, child_constraints));
        let width = fixed_width.unwrap_or_else(|| {
            child_size
                .map(|size| size.width + padding * 2.0)
                .unwrap_or(padding * 2.0)
        });
        let height =
            if prop_is_keyword(node, "height", "fill") && constraints.max_height.is_finite() {
                constraints.max_height
            } else if let Some(height) = get_prop_num(node, "height").map(f64_to_f32) {
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
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect) -> LayoutNode,
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
                )
            })
            .into_iter()
            .collect()
    }

    fn begin_gesture(&self, node: &LayoutNode, _local_col: f32, _local_row: f32) -> Option<Value> {
        if node.props.contains_key("on-drag") || node.props.contains_key("on-mouse-up") {
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
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
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

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(super::ROUNDED_RECT_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let mut prims = Vec::new();
        let corner_radius_px = match node.props.get("corner-radius") {
            Some(Value::Number(n)) => (*n as f32).max(0.0),
            _ => 0.0,
        };
        let border_width_px = match node.props.get("border-width") {
            Some(Value::Number(n)) => (*n as f32).max(0.0),
            _ => 1.0,
        };
        let has_rounded_corners = corner_radius_px > 0.0;
        let background_color = if node.props.contains_key("background-color") {
            Some(resolve_named_color(
                &node.props,
                "background-color",
                Color::rgba(0.0, 0.0, 0.0, 0.0),
            ))
        } else {
            None
        };
        let border_color = if node.props.contains_key("border-color") {
            Some(resolve_named_color(
                &node.props,
                "border-color",
                Color::rgba(0.0, 0.0, 0.0, 0.0),
            ))
        } else {
            None
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
                        (corner_radius_px - border_width_px).max(0.0),
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
                        prims.push(MetalPrimitive::Rect(MetalRectPrimitive { rect, color }));
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

        prims
    }
}
