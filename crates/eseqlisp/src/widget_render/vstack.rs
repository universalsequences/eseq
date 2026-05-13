use super::{Align, Justify, WidgetDefinition, distribute_justify, resolve_align, resolve_justify};
use crate::layout::{
    Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num, prop_is_keyword,
    shrink_constraints_xy,
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
        &["padding", "gap", "align", "justify", "width"]
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
        let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(0.0);
        // If the v-stack has its own `:width N`, that's a hard cap on the
        // inner width — otherwise `:width :fill` children would inflate to
        // the grandparent's max_width and blow past the column.
        let own_width = get_prop_num(node, "width").map(f64_to_f32);
        let mut inner = shrink_constraints_xy(constraints, padding, pad_y);
        if let Some(w) = own_width {
            inner.max_width = (w - padding * 2.0).max(0.0);
        }
        let child_sizes = children
            .iter()
            .filter_map(|child| measure_child(child, inner))
            .collect::<Vec<_>>();
        let natural_width = child_sizes
            .iter()
            .map(|size| size.width)
            .fold(0.0_f32, f32::max);
        let width = own_width
            .map(|w| w - padding * 2.0)
            .unwrap_or(natural_width);
        let height = child_sizes.iter().map(|size| size.height).sum::<f32>()
            + gap * (child_sizes.len() as f32 - 1.0).max(0.0);
        Some(Size {
            width: width + padding * 2.0,
            height: height + pad_y * 2.0,
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
        let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(0.0);
        let align = resolve_align(node, "align", Align::Start);
        let justify = resolve_justify(node, "justify", Justify::Start);

        let inner_width = (area.width - padding * 2.0).max(0.0);
        let inner_height = (area.height - pad_y * 2.0).max(0.0);
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
        let total_content_height: f32 = measured.iter().map(|(_, s, _)| s.height).sum();
        let total_gap = gap * (count as f32 - 1.0).max(0.0);
        let remaining = (inner_height - total_content_height - total_gap).max(0.0);
        let total_flex: f32 = measured.iter().map(|(_, _, f)| *f).sum();
        let flex_consumed = if total_flex > 0.0 { remaining } else { 0.0 };
        let justify_remaining = remaining - flex_consumed;

        let (start_offset, effective_gap) =
            distribute_justify(justify, justify_remaining, count, gap);

        // Pass 2: position children
        let mut cursor_row = area.row + pad_y + start_offset;
        measured
            .into_iter()
            .map(|(child, size, flex)| {
                let extra = if total_flex > 0.0 && flex > 0.0 {
                    flex_consumed * (flex / total_flex)
                } else {
                    0.0
                };
                let child_height = size.height + extra;
                let child_width =
                    if align == Align::Stretch || prop_is_keyword(child, "width", "fill") {
                        inner_width
                    } else {
                        size.width
                    };
                let col = match align {
                    Align::Start | Align::Stretch | Align::Baseline => area.col + padding,
                    Align::Center => area.col + padding + (inner_width - child_width) / 2.0,
                    Align::End => area.col + padding + inner_width - child_width,
                };
                let rect = Rect {
                    row: cursor_row,
                    col,
                    width: child_width,
                    height: child_height,
                };
                cursor_row += child_height + effective_gap;
                build_child(child, rect)
            })
            .collect()
    }
}
