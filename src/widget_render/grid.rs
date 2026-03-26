use super::{Align, WidgetDefinition, resolve_align};
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
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
        &[
            "cols",
            "col-width",
            "row-height",
            "align",
            "h-align",
            "v-align",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
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
        let widest_child = measured_children
            .iter()
            .map(|size| size.width)
            .fold(0.0_f32, f32::max)
            .max(1.0);
        let tallest_child = measured_children
            .iter()
            .map(|size| size.height)
            .fold(0.0_f32, f32::max)
            .max(1.0);
        let col_width = get_prop_num(node, "col-width")
            .map(f64_to_f32)
            .unwrap_or(widest_child);
        let row_height = get_prop_num(node, "row-height")
            .map(f64_to_f32)
            .unwrap_or(tallest_child);
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
        _aspect: f32,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let cols = get_prop_num(node, "cols")
            .map(f64_to_f32)
            .unwrap_or(1.0)
            .max(1.0);
        let cols_int = cols as usize;
        let fallback = resolve_align(node, "align", Align::Start);
        let h_align = resolve_align(node, "h-align", fallback);
        let v_align = resolve_align(node, "v-align", fallback);
        let measured_children = children
            .iter()
            .filter_map(|child| {
                measure_child(child, constraints_for_slot(area.width / cols, area.height))
            })
            .collect::<Vec<_>>();
        let widest_child = measured_children
            .iter()
            .map(|size| size.width)
            .fold(0.0_f32, f32::max)
            .max(1.0);
        let col_width = get_prop_num(node, "col-width")
            .map(f64_to_f32)
            .unwrap_or(widest_child);
        let measure_constraints = constraints_for_slot(col_width, area.height);
        let tallest_child = children
            .iter()
            .filter_map(|child| measure_child(child, measure_constraints))
            .map(|size| size.height)
            .fold(0.0_f32, f32::max)
            .max(1.0);
        let row_height = get_prop_num(node, "row-height")
            .map(f64_to_f32)
            .unwrap_or(tallest_child);
        let slot_constraints = constraints_for_slot(col_width, row_height);

        children
            .iter()
            .enumerate()
            .map(|(idx, child)| {
                let row = (idx / cols_int) as f32;
                let col = (idx % cols_int) as f32;
                let size = measure_child(child, slot_constraints).unwrap_or(Size {
                    width: col_width,
                    height: row_height,
                });
                let child_width = if h_align == Align::Stretch {
                    col_width
                } else {
                    size.width.min(col_width)
                };
                let child_height = if v_align == Align::Stretch {
                    row_height
                } else {
                    size.height.min(row_height)
                };
                let slot_col = area.col + col * col_width;
                let slot_row = area.row + row * row_height;
                let child_col = match h_align {
                    Align::Start | Align::Stretch | Align::Baseline => slot_col,
                    Align::Center => slot_col + (col_width - child_width) / 2.0,
                    Align::End => slot_col + col_width - child_width,
                };
                let child_row = match v_align {
                    Align::Start | Align::Stretch | Align::Baseline => slot_row,
                    Align::Center => slot_row + (row_height - child_height) / 2.0,
                    Align::End => slot_row + row_height - child_height,
                };
                build_child(
                    child,
                    Rect {
                        row: child_row,
                        col: child_col,
                        width: child_width,
                        height: child_height,
                    },
                )
            })
            .collect()
    }
}

fn constraints_for_slot(max_width: f32, max_height: f32) -> Constraints {
    Constraints {
        min_width: 0.0,
        max_width: max_width.max(0.0),
        min_height: 0.0,
        max_height: max_height.max(0.0),
        aspect: 1.0,
    }
}
