use super::{Align, Justify, WidgetDefinition, distribute_justify, resolve_align, resolve_justify};
use crate::layout::{
    Constraints, LayoutCtx, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
    prop_is_keyword, shrink_constraints_xy,
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
        &["padding", "gap", "align", "justify", "width"]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &["padding", "gap", "align", "justify", "width", "height", "flex"]
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
        let mut inner = shrink_constraints_xy(constraints, padding, pad_y);
        if !prop_is_keyword(node, "width", "fill") {
            inner.max_width = f32::INFINITY;
        }
        let mut child_sizes: Vec<(&Value, Size)> = children
            .iter()
            .filter_map(|child| measure_child(child, inner).map(|size| (child, size)))
            .collect();
        let width = child_sizes.iter().map(|(_, size)| size.width).sum::<f32>()
            + gap * (child_sizes.len() as f32 - 1.0).max(0.0);
        // A fill row hands its flex children their final width only in layout,
        // so a width-dependent flex child (a `wrap`, a wrapped label) measured
        // at the full row width here would under-report its height. Re-measure
        // those children at the width layout will actually give them so the
        // row grows to hold them.
        if inner.max_width.is_finite() {
            let flex_of = |child: &Value| get_prop_num(child, "flex").map(f64_to_f32).unwrap_or(0.0);
            let total_flex: f32 = child_sizes.iter().map(|(child, _)| flex_of(child)).sum();
            if total_flex > 0.0 {
                let count = child_sizes.len();
                let fixed_width: f32 = child_sizes
                    .iter()
                    .filter(|(child, _)| flex_of(child) <= 0.0)
                    .map(|(_, size)| size.width)
                    .sum();
                let remaining =
                    (inner.max_width - fixed_width - gap * (count as f32 - 1.0).max(0.0)).max(0.0);
                for (child, size) in child_sizes.iter_mut() {
                    let flex = flex_of(child);
                    if flex <= 0.0 {
                        continue;
                    }
                    let mut final_constraints = inner;
                    final_constraints.max_width = remaining * (flex / total_flex);
                    if let Some(final_size) = measure_child(child, final_constraints) {
                        size.height = final_size.height;
                    }
                }
            }
        }
        // Exclude `:height :fill` children from the height max — they consume
        // the full incoming constraint, which would inflate the h-stack's
        // natural height to the parent's max and break sibling-aware layout.
        // :fill children stretch back to the h-stack's height in layout via
        // :align :stretch, so they don't need to contribute to the max.
        let height = child_sizes
            .iter()
            .filter(|(child, _)| !prop_is_keyword(child, "height", "fill"))
            .map(|(_, size)| size.height)
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
        measure_ctx: &MeasureCtx<'_>,
        _layout_ctx: LayoutCtx,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect, LayoutCtx) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let padding = get_prop_num(node, "padding").map(f64_to_f32).unwrap_or(0.0);
        let pad_y = padding / aspect;
        let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(1.0);
        let align = resolve_align(node, "align", Align::Start);
        let justify = resolve_justify(node, "justify", Justify::Start);

        let inner_width = (area.width - padding * 2.0).max(0.0);
        let inner_height = (area.height - pad_y * 2.0).max(0.0);
        let mut inner_constraints = Constraints {
            min_width: 0.0,
            max_width: inner_width,
            min_height: 0.0,
            max_height: inner_height,
            aspect: 1.0,
        };
        if !prop_is_keyword(node, "width", "fill") {
            inner_constraints.max_width = f32::INFINITY;
        }

        // Pass 1: measure all children, collect flex values. In a fill row,
        // flex children should not consume their intrinsic/default width before
        // flex distribution; they receive the remaining width after fixed
        // children and gaps are reserved.
        let measured: Vec<(&Value, Size, f32)> = children
            .iter()
            .filter_map(|child| {
                let flex = get_prop_num(child, "flex").map(f64_to_f32).unwrap_or(0.0);
                let size = if prop_is_keyword(node, "width", "fill") && flex > 0.0 {
                    let mut flex_constraints = inner_constraints;
                    flex_constraints.max_width = 0.0;
                    measure_child(child, flex_constraints)?
                } else {
                    measure_child(child, inner_constraints)?
                };
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

        // Flex children were measured at zero width so they would not eat the
        // row before distribution; now that their final width is known,
        // measure them again at it for their height. Widths are unchanged.
        let measured: Vec<(&Value, Size, f32)> = measured
            .into_iter()
            .map(|(child, size, flex)| {
                if total_flex > 0.0 && flex > 0.0 && prop_is_keyword(node, "width", "fill") {
                    let mut final_constraints = inner_constraints;
                    final_constraints.max_width = size.width + flex_consumed * (flex / total_flex);
                    let height = measure_child(child, final_constraints)
                        .map(|final_size| final_size.height)
                        .unwrap_or(size.height);
                    (child, Size { width: size.width, height }, flex)
                } else {
                    (child, size, flex)
                }
            })
            .collect();

        let baseline_offset = |child: &Value, size: &Size| -> f32 {
            super::widget_baseline_offset(child, *size, measure_ctx)
        };

        // For baseline mode: find the maximum rendered baseline offset.
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
                        // Shift the child so its rendered baseline aligns with max_baseline.
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
                build_child(child, rect, LayoutCtx::default())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::TextMeasurer;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    struct BaselineMeasurer;

    impl TextMeasurer for BaselineMeasurer {
        fn measure_text_px(&self, text: &str, font_size: f32) -> f32 {
            text.chars().count() as f32 * font_size * 0.5
        }

        fn line_height_px(&self, font_size: f32) -> f32 {
            font_size
        }

        fn cap_height_px(&self, font_size: f32) -> f32 {
            font_size * 0.7
        }
    }

    fn widget(widget_type: &str, font_size: f64) -> Value {
        Value::Map(HashMap::from([
            (
                "type".to_string(),
                Rc::new(RefCell::new(Value::Keyword(widget_type.to_string()))),
            ),
            (
                "font-size".to_string(),
                Rc::new(RefCell::new(Value::Number(font_size))),
            ),
        ]))
    }

    fn baseline_stack() -> Value {
        Value::Map(HashMap::from([(
            "align".to_string(),
            Rc::new(RefCell::new(Value::Keyword("baseline".to_string()))),
        )]))
    }

    #[test]
    fn baseline_alignment_uses_rendered_label_and_number_picker_baselines() {
        let children = vec![widget("label", 8.0), widget("number-picker", 10.0)];
        let ctx = MeasureCtx {
            text_measurer: Some(&BaselineMeasurer),
            cell_w: 10.0,
            cell_h: 20.0,
            inherited_font_size: 14.0,
        };
        let sizes = [
            Size {
                width: 2.0,
                height: 0.4,
            },
            Size {
                width: 4.0,
                height: 1.0,
            },
        ];
        let mut rects = Vec::new();
        HSTACK_WIDGET.layout_children(
            &baseline_stack(),
            Rect {
                row: 0.0,
                col: 0.0,
                width: 10.0,
                height: 1.0,
            },
            &children,
            2.0,
            &ctx,
            LayoutCtx::default(),
            &mut |child, _| {
                let widget_type = crate::layout::get_widget_type(child)?;
                Some(if widget_type == "label" { sizes[0] } else { sizes[1] })
            },
            &mut |child, rect, _| {
                rects.push(rect);
                LayoutNode {
                    widget_id: rects.len() as u64,
                    stable_widget_id: None,
                    subtree_root_id: None,
                    parent_subtree_root_id: None,
                    stable_key: None,
                    widget_type: crate::layout::get_widget_type(child).unwrap(),
                    rect,
                    props: HashMap::new(),
                    children: Vec::new(),
                    focusable: false,
                    animation: Default::default(),
                }
            },
        );

        assert_eq!(rects.len(), 2);
        let label_baseline = rects[0].row
            + super::super::widget_baseline_offset(&children[0], sizes[0], &ctx);
        let picker_baseline = rects[1].row
            + super::super::widget_baseline_offset(&children[1], sizes[1], &ctx);
        assert!((label_baseline - picker_baseline).abs() < 0.0001);
        // Rendered baselines, not widget bottoms: aligning the bottom edges of
        // a 0.4-cell label and a 1.0-cell picker would leave the label a long
        // way below where its own text actually sits.
        assert!(
            ((rects[0].row + sizes[0].height) - (rects[1].row + sizes[1].height)).abs() > 0.0001
        );
    }

    fn fill_stack() -> Value {
        Value::Map(HashMap::from([(
            "width".to_string(),
            Rc::new(RefCell::new(Value::Keyword("fill".to_string()))),
        )]))
    }

    fn flex_widget(widget_type: &str, flex: f64) -> Value {
        Value::Map(HashMap::from([
            (
                "type".to_string(),
                Rc::new(RefCell::new(Value::Keyword(widget_type.to_string()))),
            ),
            ("flex".to_string(), Rc::new(RefCell::new(Value::Number(flex)))),
        ]))
    }

    /// A width-dependent child (like `wrap`) that needs 10 cells of width per
    /// row: its height is the number of rows it wraps into.
    fn wrapping_measure(constraints: Constraints) -> Size {
        let width = constraints.max_width;
        if !width.is_finite() || width <= 0.0 {
            return Size {
                width: 0.0,
                height: 1.0,
            };
        }
        let rows = (30.0 / width).ceil().max(1.0);
        Size {
            width: width.min(30.0),
            height: rows,
        }
    }

    #[test]
    fn fill_row_measures_flex_children_at_their_final_width() {
        // 20-cell row, 4-cell fixed child, 1-cell gap: the flex child gets 15.
        let children = vec![widget("label", 8.0), flex_widget("wrap", 1.0)];
        let ctx = MeasureCtx {
            text_measurer: Some(&BaselineMeasurer),
            cell_w: 10.0,
            cell_h: 20.0,
            inherited_font_size: 14.0,
        };
        let mut measure = |child: &Value, constraints: Constraints| {
            let widget_type = crate::layout::get_widget_type(child)?;
            Some(if widget_type == "label" {
                Size {
                    width: 4.0,
                    height: 1.0,
                }
            } else {
                wrapping_measure(constraints)
            })
        };
        let constraints = Constraints {
            min_width: 0.0,
            max_width: 20.0,
            min_height: 0.0,
            max_height: 100.0,
            aspect: 1.0,
        };
        let size = HSTACK_WIDGET
            .measure(&fill_stack(), &children, constraints, &ctx, &mut measure)
            .unwrap();
        // 30 cells of content at 15 cells wide wraps into two rows, not the
        // single row the child reports when measured at the full 20 cells.
        assert!((size.height - 2.0).abs() < 0.0001, "height {}", size.height);

        let mut rects = Vec::new();
        HSTACK_WIDGET.layout_children(
            &fill_stack(),
            Rect {
                row: 0.0,
                col: 0.0,
                width: 20.0,
                height: size.height,
            },
            &children,
            1.0,
            &ctx,
            LayoutCtx::default(),
            &mut measure,
            &mut |child, rect, _| {
                rects.push(rect);
                LayoutNode {
                    widget_id: rects.len() as u64,
                    stable_widget_id: None,
                    subtree_root_id: None,
                    parent_subtree_root_id: None,
                    stable_key: None,
                    widget_type: crate::layout::get_widget_type(child).unwrap(),
                    rect,
                    props: HashMap::new(),
                    children: Vec::new(),
                    focusable: false,
                    animation: Default::default(),
                }
            },
        );
        assert_eq!(rects.len(), 2);
        assert!((rects[1].width - 15.0).abs() < 0.0001, "width {}", rects[1].width);
        assert!((rects[1].height - 2.0).abs() < 0.0001, "height {}", rects[1].height);
    }
}
