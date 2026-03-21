use super::WidgetDefinition;
use crate::layout::{Constraints, LayoutNode, Rect, Size, f64_to_f32, get_prop_num};
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
        let cols = get_prop_num(node, "cols")
            .map(f64_to_f32)
            .unwrap_or(1.0)
            .max(1.0);
        let cols_int = cols as usize;
        let measured_children = children
            .iter()
            .filter_map(|child| measure_child(child, constraints))
            .collect::<Vec<_>>();
        let col_width = get_prop_num(node, "col-width")
            .map(f64_to_f32)
            .unwrap_or_else(|| (constraints.max_width / cols).max(1.0));
        let row_height = get_prop_num(node, "row-height")
            .map(f64_to_f32)
            .unwrap_or_else(|| {
                let tallest_child = measured_children
                    .iter()
                    .map(|size| size.height)
                    .fold(0.0_f32, f32::max);
                col_width.max(tallest_child).max(1.0)
            });
        let rows = ((children.len() + cols_int - 1) / cols_int) as f32;
        Some(Size {
            width: cols * col_width,
            height: rows * row_height,
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
        let cols = get_prop_num(node, "cols")
            .map(f64_to_f32)
            .unwrap_or(1.0)
            .max(1.0);
        let cols_int = cols as usize;
        let col_width = get_prop_num(node, "col-width")
            .map(f64_to_f32)
            .unwrap_or_else(|| (area.width / cols).max(1.0));
        let measure_constraints = Constraints {
            min_width: 0.0,
            max_width: col_width,
            min_height: 0.0,
            max_height: area.height,
            aspect: 1.0,
        };
        let row_height = get_prop_num(node, "row-height")
            .map(f64_to_f32)
            .unwrap_or_else(|| {
                let tallest_child = children
                    .iter()
                    .filter_map(|child| measure_child(child, measure_constraints))
                    .map(|size| size.height)
                    .fold(0.0_f32, f32::max);
                col_width.max(tallest_child).max(1.0)
            });

        children
            .iter()
            .enumerate()
            .map(|(idx, child)| {
                let row = (idx / cols_int) as f32;
                let col = (idx % cols_int) as f32;
                build_child(
                    child,
                    Rect {
                        row: area.row + row * row_height,
                        col: area.col + col * col_width,
                        width: col_width,
                        height: row_height,
                    },
                )
            })
            .collect()
    }
}
