use super::WidgetDefinition;
use crate::layout::{Constraints, LayoutNode, Rect, Size, f64_to_u16, get_prop_num, shrink_constraints};
use crate::vm::Value;

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
        &["padding", "width", "height"]
    }

    fn measure(
        &self,
        node: &Value,
        children: &[Value],
        constraints: Constraints,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let padding = get_prop_num(node, "padding").map(f64_to_u16).unwrap_or(0);
        let inner = shrink_constraints(constraints, padding);
        let child_size = children.first().and_then(|child| measure_child(child, inner));
        Some(Size {
            width: get_prop_num(node, "width")
                .map(f64_to_u16)
                .unwrap_or_else(|| {
                    child_size
                        .map(|size| size.width.saturating_add(padding.saturating_mul(2)))
                        .unwrap_or(padding.saturating_mul(2))
                }),
            height: get_prop_num(node, "height")
                .map(f64_to_u16)
                .unwrap_or_else(|| {
                    child_size
                        .map(|size| size.height.saturating_add(padding.saturating_mul(2)))
                        .unwrap_or(padding.saturating_mul(2))
                }),
        })
    }

    fn layout_children(
        &self,
        node: &Value,
        area: Rect,
        children: &[Value],
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let padding = get_prop_num(node, "padding").map(f64_to_u16).unwrap_or(0);
        children
            .first()
            .map(|child| {
                build_child(
                    child,
                    Rect {
                        row: area.row.saturating_add(padding),
                        col: area.col.saturating_add(padding),
                        width: area.width.saturating_sub(padding.saturating_mul(2)),
                        height: area.height.saturating_sub(padding.saturating_mul(2)),
                    },
                )
            })
            .into_iter()
            .collect()
    }
}
