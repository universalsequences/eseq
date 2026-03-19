use super::WidgetDefinition;
use crate::layout::{
    Constraints, LayoutNode, Rect, Size, f64_to_u16, get_prop_num, shrink_constraints,
};
use crate::vm::Value;

pub struct HStackWidget;

pub static HSTACK_WIDGET: HStackWidget = HStackWidget;

impl WidgetDefinition for HStackWidget {
    fn names(&self) -> &'static [&'static str] {
        &["h-stack"]
    }

    fn is_container(&self) -> bool {
        true
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["padding", "gap"]
    }

    fn measure(
        &self,
        node: &Value,
        children: &[Value],
        constraints: Constraints,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let padding = get_prop_num(node, "padding").map(f64_to_u16).unwrap_or(0);
        let gap = get_prop_num(node, "gap").map(f64_to_u16).unwrap_or(1);
        let inner = shrink_constraints(constraints, padding);
        let child_sizes = children
            .iter()
            .filter_map(|child| measure_child(child, inner))
            .collect::<Vec<_>>();
        let width = child_sizes
            .iter()
            .map(|size| size.width)
            .fold(0_u16, u16::saturating_add)
            .saturating_add(gap.saturating_mul(child_sizes.len().saturating_sub(1) as u16));
        let height = child_sizes
            .iter()
            .map(|size| size.height)
            .max()
            .unwrap_or(0);
        Some(Size {
            width: width.saturating_add(padding.saturating_mul(2)),
            height: height.saturating_add(padding.saturating_mul(2)),
        })
    }

    fn layout_children(
        &self,
        node: &Value,
        area: Rect,
        children: &[Value],
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let padding = get_prop_num(node, "padding").map(f64_to_u16).unwrap_or(0);
        let gap = get_prop_num(node, "gap").map(f64_to_u16).unwrap_or(1);
        let inner_height = area.height.saturating_sub(padding.saturating_mul(2));
        let inner_constraints = Constraints {
            min_width: 0,
            max_width: area.width.saturating_sub(padding.saturating_mul(2)),
            min_height: 0,
            max_height: inner_height,
        };
        let mut cursor_col = area.col.saturating_add(padding);
        children
            .iter()
            .filter_map(|child| {
                let size = measure_child(child, inner_constraints)?;
                let rect = Rect {
                    row: area.row.saturating_add(padding),
                    col: cursor_col,
                    width: size.width,
                    height: size.height,
                };
                cursor_col = cursor_col.saturating_add(size.width).saturating_add(gap);
                Some(build_child(child, rect))
            })
            .collect()
    }
}
