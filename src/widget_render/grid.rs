use super::WidgetDefinition;
use crate::layout::{Constraints, LayoutNode, Rect, Size, f64_to_u16, get_prop_num};
use crate::vm::Value;

pub struct GridWidget;

pub static GRID_WIDGET: GridWidget = GridWidget;

impl WidgetDefinition for GridWidget {
    fn names(&self) -> &'static [&'static str] {
        &["grid"]
    }

    fn is_container(&self) -> bool {
        true
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["cols", "col-width", "row-height"]
    }

    fn measure(
        &self,
        node: &Value,
        children: &[Value],
        constraints: Constraints,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let cols = get_prop_num(node, "cols").map(f64_to_u16).unwrap_or(1).max(1);
        let measured_children = children
            .iter()
            .filter_map(|child| measure_child(child, constraints))
            .collect::<Vec<_>>();
        let col_width = get_prop_num(node, "col-width")
            .map(f64_to_u16)
            .unwrap_or_else(|| (constraints.max_width / cols).max(1));
        let row_height = get_prop_num(node, "row-height")
            .map(f64_to_u16)
            .unwrap_or_else(|| measured_children.iter().map(|size| size.height).max().unwrap_or(1));
        let rows = ((children.len() as u16).saturating_add(cols - 1)) / cols;
        Some(Size {
            width: cols.saturating_mul(col_width),
            height: rows.saturating_mul(row_height),
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
        let cols = get_prop_num(node, "cols").map(f64_to_u16).unwrap_or(1).max(1);
        let col_width = get_prop_num(node, "col-width")
            .map(f64_to_u16)
            .unwrap_or_else(|| (area.width / cols).max(1));
        let measure_constraints = Constraints {
            min_width: 0,
            max_width: col_width,
            min_height: 0,
            max_height: area.height,
        };
        let row_height = get_prop_num(node, "row-height")
            .map(f64_to_u16)
            .unwrap_or_else(|| {
                children
                    .iter()
                    .filter_map(|child| measure_child(child, measure_constraints))
                    .map(|size| size.height)
                    .max()
                    .unwrap_or(1)
            });

        children
            .iter()
            .enumerate()
            .map(|(idx, child)| {
                let row = idx as u16 / cols;
                let col = idx as u16 % cols;
                build_child(
                    child,
                    Rect {
                        row: area.row.saturating_add(row.saturating_mul(row_height)),
                        col: area.col.saturating_add(col.saturating_mul(col_width)),
                        width: col_width,
                        height: row_height,
                    },
                )
            })
            .collect()
    }
}
