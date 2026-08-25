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
        let child_sizes: Vec<(&Value, Size)> = children
            .iter()
            .filter_map(|child| measure_child(child, inner).map(|size| (child, size)))
            .collect();
        let width = child_sizes.iter().map(|(_, size)| size.width).sum::<f32>()
            + gap * (child_sizes.len() as f32 - 1.0).max(0.0);
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

        fn ascent_px(&self, font_size: f32) -> f32 {
            if font_size < 9.0 { font_size * 0.75 } else { font_size * 0.7 }
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
        assert!((rects[0].row - rects[1].row).abs() < 0.0001);
    }
}
