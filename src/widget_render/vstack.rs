use super::WidgetDefinition;
use crate::layout::{
    Constraints, LayoutNode, Rect, Size, f64_to_u16, get_prop_num, shrink_constraints,
};
use crate::vm::Value;

pub struct VStackWidget;

pub static VSTACK_WIDGET: VStackWidget = VStackWidget;

impl WidgetDefinition for VStackWidget {
    fn names(&self) -> &'static [&'static str] {
        &["v-stack"]
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
        let gap = get_prop_num(node, "gap").map(f64_to_u16).unwrap_or(0);
        let inner = shrink_constraints(constraints, padding);
        let child_sizes = children
            .iter()
            .filter_map(|child| measure_child(child, inner))
            .collect::<Vec<_>>();
        let width = child_sizes.iter().map(|size| size.width).max().unwrap_or(0);
        let height = child_sizes
            .iter()
            .map(|size| size.height)
            .fold(0_u16, u16::saturating_add)
            .saturating_add(gap.saturating_mul(child_sizes.len().saturating_sub(1) as u16));
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
        let gap = get_prop_num(node, "gap").map(f64_to_u16).unwrap_or(0);
        let inner_width = area.width.saturating_sub(padding.saturating_mul(2));
        let inner_constraints = Constraints {
            min_width: 0,
            max_width: inner_width,
            min_height: 0,
            max_height: area.height.saturating_sub(padding.saturating_mul(2)),
        };
        let mut cursor_row = area.row.saturating_add(padding);
        children
            .iter()
            .filter_map(|child| {
                let size = measure_child(child, inner_constraints)?;
                let rect = Rect {
                    row: cursor_row,
                    col: area.col.saturating_add(padding),
                    width: size.width,
                    height: size.height,
                };
                cursor_row = cursor_row.saturating_add(size.height).saturating_add(gap);
                Some(build_child(child, rect))
            })
            .collect()
    }
}
