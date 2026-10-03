use super::{Align, Justify, WidgetDefinition, distribute_justify, resolve_align, resolve_justify};
use crate::layout::{
    Constraints, LayoutCtx, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
    prop_is_keyword, shrink_constraints_xy,
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

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "padding",
            "gap",
            "align",
            "justify",
            "width",
            "height",
            "flex",
            "min-height",
            "shrink",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        children: &[&Value],
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
        let mut child_sizes = children
            .iter()
            .copied()
            .filter_map(|child| {
                let mut size = measure_child(child, inner)?;
                if let Some(min_height) = get_prop_num(child, "min-height").map(f64_to_f32) {
                    size.height = size.height.max(min_height);
                }
                Some((child, size, 0.0))
            })
            .collect::<Vec<_>>();
        if inner.max_height.is_finite() {
            let total_gap = gap * (child_sizes.len() as f32 - 1.0).max(0.0);
            shrink_to_fit(&mut child_sizes, inner.max_height - total_gap);
        }
        let child_sizes = child_sizes
            .into_iter()
            .map(|(_, size, _)| size)
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
        children: &[&Value],
        aspect: f32,
        _measure_ctx: &MeasureCtx<'_>,
        _layout_ctx: LayoutCtx,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect, LayoutCtx) -> LayoutNode,
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

        // Pass 1: measure all children, collect flex values. A child's
        // `:min-height` floors its measured height, so a flex child can
        // reserve room it would otherwise only get from leftover space.
        let mut measured: Vec<(&Value, Size, f32)> = children
            .iter()
            .copied()
            .filter_map(|child| {
                let mut size = measure_child(child, inner_constraints)?;
                if let Some(min_height) = get_prop_num(child, "min-height").map(f64_to_f32) {
                    size.height = size.height.max(min_height);
                }
                let flex = get_prop_num(child, "flex").map(f64_to_f32).unwrap_or(0.0);
                Some((child, size, flex))
            })
            .collect();

        let count = measured.len();
        if count == 0 {
            return vec![];
        }

        let total_gap = gap * (count as f32 - 1.0).max(0.0);
        shrink_to_fit(&mut measured, inner_height - total_gap);

        // Compute remaining space, then let flex children absorb it first
        let total_content_height: f32 = measured.iter().map(|(_, s, _)| s.height).sum();
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
                build_child(child, rect, LayoutCtx::default())
            })
            .collect()
    }
}

/// When the children overflow `available`, take the overflow first out of
/// flex children (their natural height is only a basis; flex hands leftover
/// space back afterwards), then out of children marked `:shrink N` (weighted
/// by N). Neither goes below its `:min-height`. A shrinking child is expected
/// to be a scroll (or wrap one) so its clipped content stays reachable.
/// Children with neither prop keep their size.
fn shrink_to_fit(measured: &mut [(&Value, Size, f32)], available: f32) {
    let total: f32 = measured.iter().map(|(_, s, _)| s.height).sum();
    let mut overflow = total - available;
    if overflow <= 0.0 || !measured.iter().any(|(c, _, _)| shrink_weight(c) > 0.0) {
        return;
    }
    let flex_weight = |child: &Value| get_prop_num(child, "flex").map(f64_to_f32).unwrap_or(0.0);
    overflow = take_overflow(measured, overflow, &flex_weight);
    take_overflow(measured, overflow, &shrink_weight);
}

fn shrink_weight(child: &Value) -> f32 {
    get_prop_num(child, "shrink").map(f64_to_f32).unwrap_or(0.0)
}

/// Removes up to `overflow` from children with a positive `weight`,
/// proportionally, clamping each at its `:min-height`. Returns what is left.
fn take_overflow(
    measured: &mut [(&Value, Size, f32)],
    mut overflow: f32,
    weight: &dyn Fn(&Value) -> f32,
) -> f32 {
    // Each pass clamps children that hit their floor and redistributes the
    // rest among those still able to give.
    let mut active: Vec<usize> = (0..measured.len())
        .filter(|&i| weight(measured[i].0) > 0.0)
        .collect();
    while overflow > 0.0 && !active.is_empty() {
        let total_weight: f32 = active.iter().map(|&i| weight(measured[i].0)).sum();
        let mut taken = 0.0;
        active.retain(|&i| {
            let (child, size, _) = &mut measured[i];
            let floor = get_prop_num(child, "min-height").map(f64_to_f32).unwrap_or(0.0);
            let give = (overflow * weight(child) / total_weight)
                .min(size.height - floor)
                .max(0.0);
            size.height -= give;
            taken += give;
            size.height > floor
        });
        overflow -= taken;
        if taken <= f32::EPSILON {
            break;
        }
    }
    overflow.max(0.0)
}
