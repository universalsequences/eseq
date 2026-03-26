use std::cell::RefCell;
use std::rc::Rc;

use super::{Align, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, resolve_align};
#[cfg(target_os = "macos")]
use super::{MetalPrimitive, WidgetViewport};
use crate::layout::{
    Constraints, LayoutNode, MeasureCtx, Size, f64_to_f32, get_prop_num, shrink_constraints_xy,
};
use crate::layout::Rect;
use crate::vm::Value;
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

/// Compute the bounding rect that covers the box itself and all its children.
/// This ensures the SDF background covers the full scrollable content.
fn content_extent(node: &LayoutNode) -> Rect {
    let padding = get_prop_num_from_layout(node, "padding");
    let mut rect = node.rect;
    for child in &node.children {
        let cr = child_extent(child);
        // Add padding on the right/bottom side of the content
        let right = (cr.col + cr.width + padding).max(rect.col + rect.width);
        let bottom = (cr.row + cr.height + padding).max(rect.row + rect.height);
        rect.width = right - rect.col;
        rect.height = bottom - rect.row;
    }
    rect
}

fn get_prop_num_from_layout(node: &LayoutNode, key: &str) -> f32 {
    match node.props.get(key) {
        Some(Value::Number(n)) => *n as f32,
        _ => 0.0,
    }
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

                let child_width = if h_align == Align::Stretch {
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
        if node.props.contains_key("on-click") {
            if let MouseEventKind::Down(MouseButton::Left) = mouse_kind {
                return MouseEventOutcome::Dispatch(WidgetEvent::Activate(modifiers));
            }
        }
        MouseEventOutcome::Ignore
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Activate(modifiers) = event else {
            return None;
        };
        let callback = node.props.get("on-click")?.clone();
        let mut info = std::collections::HashMap::new();
        info.insert("shift".to_string(), Rc::new(RefCell::new(Value::Bool(modifiers.contains(KeyModifiers::SHIFT)))));
        info.insert("ctrl".to_string(), Rc::new(RefCell::new(Value::Bool(modifiers.contains(KeyModifiers::CONTROL)))));
        info.insert("alt".to_string(), Rc::new(RefCell::new(Value::Bool(modifiers.contains(KeyModifiers::ALT)))));
        Some(EventOutput {
            callback,
            args: vec![Value::Map(info)],
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
            // Check if any child is a scroll container. If so, use the box's own
            // layout rect — content_extent would still be correct but node.rect
            // is the definitive bounds for a scroll-containing box.
            let has_scroll_child = node.children.iter().any(|c| c.widget_type == "scroll");
            let bg_rect = if has_scroll_child {
                node.rect
            } else {
                content_extent(node)
            };
            return super::sdf_widget::sdf_widget_background_primitives(bg_type, bg_rect, viewport, &node.props);
        }
        Vec::new()
    }
}
