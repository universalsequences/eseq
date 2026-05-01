use std::cell::RefCell;
use std::rc::Rc;

use super::{Align, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, resolve_align};
#[cfg(target_os = "macos")]
use super::{MetalPrimitive, WidgetViewport};
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

fn box_mouse_info(phase: &str, modifiers: KeyModifiers) -> Value {
    let mut info = std::collections::HashMap::new();
    info.insert(
        "phase".to_string(),
        Rc::new(RefCell::new(Value::String(phase.to_string()))),
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

impl WidgetDefinition for BoxWidget {
    fn names(&self) -> &'static [&'static str] {
        &["box"]
    }

    fn is_container(&self) -> bool {
        true
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["padding", "width", "height", "align", "h-align", "v-align"]
    }

    fn measure(
        &self,
        node: &Value,
        children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let padding = get_prop_num(node, "padding").map(f64_to_f32).unwrap_or(0.0);
        let pad_y = padding / constraints.aspect;
        let inner = shrink_constraints_xy(constraints, padding, pad_y);
        let child_size = children
            .first()
            .and_then(|child| measure_child(child, inner));
        Some(Size {
            width: get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or_else(|| {
                    child_size
                        .map(|size| size.width + padding * 2.0)
                        .unwrap_or(padding * 2.0)
                }),
            height: get_prop_num(node, "height")
                .map(f64_to_f32)
                .unwrap_or_else(|| {
                    child_size
                        .map(|size| size.height + pad_y * 2.0)
                        .unwrap_or(pad_y * 2.0)
                }),
        })
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
        _local_col: f32,
        _local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        modifiers: KeyModifiers,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if node.props.contains_key("on-mouse-down") {
                    return MouseEventOutcome::Dispatch(WidgetEvent::Custom(box_mouse_info(
                        "down", modifiers,
                    )));
                }
                if node.props.contains_key("on-click") {
                    return MouseEventOutcome::Dispatch(WidgetEvent::Activate(modifiers));
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if node.props.contains_key("on-drag") => {
                return MouseEventOutcome::Dispatch(WidgetEvent::Custom(box_mouse_info(
                    "drag", modifiers,
                )));
            }
            MouseEventKind::Up(MouseButton::Left) if node.props.contains_key("on-mouse-up") => {
                return MouseEventOutcome::Dispatch(WidgetEvent::Custom(box_mouse_info(
                    "up", modifiers,
                )));
            }
            _ => {}
        }
        MouseEventOutcome::Ignore
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let (callback_name, arg) = match event {
            WidgetEvent::Activate(modifiers) => ("on-click", box_mouse_info("click", modifiers)),
            WidgetEvent::Custom(value) => {
                let phase = match &value {
                    Value::Map(map) => map.get("phase").and_then(|v| match &*v.borrow() {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    }),
                    _ => None,
                }?;
                let callback_name = match phase.as_str() {
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
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        if let Some(Value::String(bg_type)) = node.props.get("background") {
            // Scroll-containing boxes use their own layout rect: content_extent
            // would include scroll children, but node.rect is the viewport.
            let has_scroll_child = node.children.iter().any(|c| c.widget_type == "scroll");
            let bg_rect = if has_scroll_child {
                node.rect
            } else {
                content_extent(node)
            };
            return super::sdf_widget::sdf_widget_background_primitives(
                bg_type,
                node.widget_id,
                bg_rect,
                viewport,
                &node.props,
            );
        }
        Vec::new()
    }
}
