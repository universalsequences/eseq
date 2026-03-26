use super::{Align, Justify, WidgetDefinition, distribute_justify, resolve_align, resolve_justify};
use crate::layout::{
    Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num, get_widget_type,
    shrink_constraints_xy,
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
        _ctx: &MeasureCtx<'_>,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let padding = get_prop_num(node, "padding").map(f64_to_f32).unwrap_or(0.0);
        let pad_y = padding / constraints.aspect;
        let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(1.0);
        let inner = shrink_constraints_xy(constraints, padding, pad_y);
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
        let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(1.0);
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
        let total_content_width: f32 = measured.iter().map(|(_, s, _)| s.width).sum();
        let total_gap = gap * (count as f32 - 1.0).max(0.0);
        let remaining = (inner_width - total_content_width - total_gap).max(0.0);
        let total_flex: f32 = measured.iter().map(|(_, _, f)| *f).sum();
        let flex_consumed = if total_flex > 0.0 { remaining } else { 0.0 };
        let justify_remaining = remaining - flex_consumed;

        let (start_offset, effective_gap) =
            distribute_justify(justify, justify_remaining, count, gap);

        // For baseline alignment, compute the baseline offset for each child.
        // Labels: baseline = ascent ratio × height (~0.75 of line height).
        // Non-text widgets: center them vertically on the baseline.
        let baseline_offset = |child: &Value, size: &Size| -> f32 {
            let is_label = get_widget_type(child)
                .map(|t| t == "label")
                .unwrap_or(false);
            if is_label {
                // Ascent ≈ 75% of line height for most fonts.
                size.height * 0.75
            } else {
                // Center non-text widgets on the baseline.
                size.height * 0.5
            }
        };

        // For baseline mode: find the maximum baseline offset.
        let max_baseline = if align == Align::Baseline {
            measured
                .iter()
                .map(|(child, size, _)| baseline_offset(child, size))
                .fold(0.0_f32, f32::max)
        } else {
            0.0
        };

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
                    Align::Start | Align::Stretch => area.row + pad_y,
                    Align::Center => area.row + pad_y + (inner_height - child_height) / 2.0,
                    Align::End => area.row + pad_y + inner_height - child_height,
                    Align::Baseline => {
                        // Shift child down so its baseline aligns with max_baseline.
                        let child_bl = baseline_offset(child, &size);
                        area.row + pad_y + (max_baseline - child_bl)
                    }
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
