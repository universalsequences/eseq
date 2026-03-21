use super::{Align, Justify, WidgetDefinition, distribute_justify, resolve_align, resolve_justify};
use crate::layout::{
    Constraints, LayoutNode, Rect, Size, f64_to_f32, get_prop_num, shrink_constraints,
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
        &["padding", "gap", "align", "justify"]
    }

    fn measure(
        &self,
        node: &Value,
        children: &[Value],
        constraints: Constraints,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let padding = get_prop_num(node, "padding").map(f64_to_f32).unwrap_or(0.0);
        let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(1.0);
        let inner = shrink_constraints(constraints, padding);
        let child_sizes = children
            .iter()
            .filter_map(|child| measure_child(child, inner))
            .collect::<Vec<_>>();
        let width = child_sizes.iter().map(|size| size.width).sum::<f32>()
            + gap * (child_sizes.len() as f32 - 1.0).max(0.0);
        let height = child_sizes
            .iter()
            .map(|size| size.height)
            .fold(0.0_f32, f32::max);
        Some(Size {
            width: width + padding * 2.0,
            height: height + padding * 2.0,
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
        let padding = get_prop_num(node, "padding").map(f64_to_f32).unwrap_or(0.0);
        let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(1.0);
        let align = resolve_align(node, "align", Align::Start);
        let justify = resolve_justify(node, "justify", Justify::Start);

        let inner_width = (area.width - padding * 2.0).max(0.0);
        let inner_height = (area.height - padding * 2.0).max(0.0);
        let inner_constraints = Constraints {
            min_width: 0.0,
            max_width: inner_width,
            min_height: 0.0,
            max_height: inner_height,
            aspect: 1.0,
        };

        // Pass 1: measure all children, collect flex values
        let measured: Vec<(&Value, Size, f32)> = children
            .iter()
            .filter_map(|child| {
                let size = measure_child(child, inner_constraints)?;
                let flex = get_prop_num(child, "flex").map(f64_to_f32).unwrap_or(0.0);
                Some((child, size, flex))
            })
            .collect();

        let count = measured.len();
        if count == 0 {
            return vec![];
        }

        // Compute remaining space, then let flex children absorb it first
        let total_content_width: f32 = measured.iter().map(|(_, s, _)| s.width).sum();
        let total_gap = gap * (count as f32 - 1.0).max(0.0);
        let remaining = (inner_width - total_content_width - total_gap).max(0.0);
        let total_flex: f32 = measured.iter().map(|(_, _, f)| *f).sum();
        let flex_consumed = if total_flex > 0.0 { remaining } else { 0.0 };
        let justify_remaining = remaining - flex_consumed;

        let (start_offset, effective_gap) =
            distribute_justify(justify, justify_remaining, count, gap);

        // Pass 2: position children
        let mut cursor_col = area.col + padding + start_offset;
        measured
            .into_iter()
            .map(|(child, size, flex)| {
                let extra = if total_flex > 0.0 && flex > 0.0 {
                    flex_consumed * (flex / total_flex)
                } else {
                    0.0
                };
                let child_width = size.width + extra;
                let child_height = if align == Align::Stretch {
                    inner_height
                } else {
                    size.height
                };
                let row = match align {
                    Align::Start | Align::Stretch => area.row + padding,
                    Align::Center => area.row + padding + (inner_height - child_height) / 2.0,
                    Align::End => area.row + padding + inner_height - child_height,
                };
                let rect = Rect {
                    row,
                    col: cursor_col,
                    width: child_width,
                    height: child_height,
                };
                cursor_col += child_width + effective_gap;
                build_child(child, rect)
            })
            .collect()
    }
}
