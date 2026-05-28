use super::{Align, Justify, WidgetDefinition, distribute_justify, resolve_align, resolve_justify};
use crate::layout::{
    Constraints, LayoutCtx, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
    prop_is_keyword, shrink_constraints_xy,
};
use crate::vm::Value;

pub struct WrapWidget;

pub static WRAP_WIDGET: WrapWidget = WrapWidget;

#[derive(Clone, Copy)]
struct WrappedChild<'a> {
    child: &'a Value,
    size: Size,
}

fn measured_children<'a>(
    children: &'a [Value],
    inner: Constraints,
    measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
) -> Vec<WrappedChild<'a>> {
    children
        .iter()
        .filter_map(|child| {
            measure_child(child, inner).map(|mut size| {
                if inner.max_width.is_finite() {
                    size.width = size.width.min(inner.max_width);
                }
                WrappedChild { child, size }
            })
        })
        .collect()
}

fn wrap_rows<'a>(
    children: &[WrappedChild<'a>],
    max_width: f32,
    gap: f32,
) -> Vec<Vec<WrappedChild<'a>>> {
    let mut rows: Vec<Vec<WrappedChild<'a>>> = Vec::new();
    let mut current = Vec::new();
    let mut current_width = 0.0_f32;
    let can_wrap = max_width.is_finite() && max_width > 0.0;

    for child in children {
        let next_width = if current.is_empty() {
            child.size.width
        } else {
            current_width + gap + child.size.width
        };
        if can_wrap && !current.is_empty() && next_width > max_width {
            rows.push(current);
            current = Vec::new();
            current_width = 0.0;
        }
        current_width = if current.is_empty() {
            child.size.width
        } else {
            current_width + gap + child.size.width
        };
        current.push(*child);
    }

    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

fn row_size(row: &[WrappedChild<'_>], gap: f32) -> Size {
    Size {
        width: row.iter().map(|child| child.size.width).sum::<f32>()
            + gap * (row.len() as f32 - 1.0).max(0.0),
        height: row
            .iter()
            .map(|child| child.size.height)
            .fold(0.0_f32, f32::max),
    }
}

impl WidgetDefinition for WrapWidget {
    fn names(&self) -> &'static [&'static str] {
        &["wrap"]
    }

    fn is_container(&self) -> bool {
        true
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["padding", "gap", "row-gap", "align", "justify", "width"]
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
        let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(0.5);
        let row_gap = get_prop_num(node, "row-gap").map(f64_to_f32).unwrap_or(gap);
        let fixed_width =
            if prop_is_keyword(node, "width", "fill") && constraints.max_width.is_finite() {
                Some(constraints.max_width)
            } else {
                get_prop_num(node, "width").map(f64_to_f32)
            };
        let mut inner = shrink_constraints_xy(constraints, padding, pad_y);
        if let Some(width) = fixed_width {
            inner.max_width = (width - padding * 2.0).max(0.0);
        }

        let measured = measured_children(children, inner, measure_child);
        let rows = wrap_rows(&measured, inner.max_width, gap);
        let row_sizes = rows
            .iter()
            .map(|row| row_size(row, gap))
            .collect::<Vec<_>>();
        let natural_width = row_sizes
            .iter()
            .map(|size| size.width)
            .fold(0.0_f32, f32::max);
        let natural_height = row_sizes.iter().map(|size| size.height).sum::<f32>()
            + row_gap * (row_sizes.len() as f32 - 1.0).max(0.0);
        let width = fixed_width.unwrap_or_else(|| natural_width + padding * 2.0);
        Some(Size {
            width,
            height: natural_height + pad_y * 2.0,
        })
    }

    fn layout_children(
        &self,
        node: &Value,
        area: Rect,
        children: &[Value],
        aspect: f32,
        _layout_ctx: LayoutCtx,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect, LayoutCtx) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let padding = get_prop_num(node, "padding").map(f64_to_f32).unwrap_or(0.0);
        let pad_y = padding / aspect;
        let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(0.5);
        let row_gap = get_prop_num(node, "row-gap").map(f64_to_f32).unwrap_or(gap);
        let align = resolve_align(node, "align", Align::Center);
        let justify = resolve_justify(node, "justify", Justify::Start);
        let inner_width = (area.width - padding * 2.0).max(0.0);
        let inner_height = (area.height - pad_y * 2.0).max(0.0);
        let inner = Constraints {
            min_width: 0.0,
            max_width: inner_width,
            min_height: 0.0,
            max_height: inner_height,
            aspect: 1.0,
        };
        let measured = measured_children(children, inner, measure_child);
        let rows = wrap_rows(&measured, inner_width, gap);

        let mut laid_out = Vec::new();
        let mut row_top = area.row + pad_y;
        for row in rows {
            let row_size = row_size(&row, gap);
            let (row_offset, effective_gap) = distribute_justify(
                justify,
                (inner_width - row_size.width).max(0.0),
                row.len(),
                gap,
            );
            let mut col = area.col + padding + row_offset;
            for item in row {
                let child_height = item.size.height.min(row_size.height);
                let row = match align {
                    Align::Start | Align::Stretch | Align::Baseline => row_top,
                    Align::Center => row_top + (row_size.height - child_height) / 2.0,
                    Align::End => row_top + row_size.height - child_height,
                };
                laid_out.push(build_child(
                    item.child,
                    Rect {
                        row,
                        col,
                        width: item.size.width,
                        height: if align == Align::Stretch {
                            row_size.height
                        } else {
                            child_height
                        },
                    },
                    LayoutCtx::default(),
                ));
                col += item.size.width + effective_gap;
            }
            row_top += row_size.height + row_gap;
        }
        laid_out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::MeasureCtx;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    fn wrap_node(width: f64) -> Value {
        let mut map = HashMap::new();
        map.insert(
            "width".to_string(),
            Rc::new(RefCell::new(Value::Number(width))),
        );
        map.insert("gap".to_string(), Rc::new(RefCell::new(Value::Number(1.0))));
        Value::Map(map)
    }

    fn child(width: f64, height: f64) -> Value {
        let mut map = HashMap::new();
        map.insert("w".to_string(), Rc::new(RefCell::new(Value::Number(width))));
        map.insert(
            "h".to_string(),
            Rc::new(RefCell::new(Value::Number(height))),
        );
        Value::Map(map)
    }

    fn child_size(value: &Value) -> Size {
        let Value::Map(map) = value else {
            panic!("child should be a map");
        };
        let num = |key: &str| match map.get(key).map(|value| value.borrow().clone()) {
            Some(Value::Number(value)) => value as f32,
            other => panic!("expected numeric child field {key}, got {other:?}"),
        };
        Size {
            width: num("w"),
            height: num("h"),
        }
    }

    #[test]
    fn wrap_measures_children_into_rows() {
        let node = wrap_node(10.0);
        let children = vec![child(4.0, 1.0), child(4.0, 1.0), child(4.0, 1.0)];
        let size = WRAP_WIDGET
            .measure(
                &node,
                &children,
                Constraints {
                    min_width: 0.0,
                    max_width: 10.0,
                    min_height: 0.0,
                    max_height: 100.0,
                    aspect: 1.0,
                },
                &MeasureCtx {
                    text_measurer: None,
                    cell_w: 1.0,
                    cell_h: 1.0,
                    inherited_font_size: 12.0,
                },
                &mut |child, _| Some(child_size(child)),
            )
            .expect("measure wrap");

        assert_eq!(
            size,
            Size {
                width: 10.0,
                height: 3.0
            }
        );
    }

    #[test]
    fn wrap_lays_out_intrinsic_width_children_without_equal_columns() {
        let node = wrap_node(10.0);
        let children = vec![child(2.0, 1.0), child(5.0, 1.0), child(4.0, 1.0)];
        let mut rects = Vec::new();
        WRAP_WIDGET.layout_children(
            &node,
            Rect {
                row: 0.0,
                col: 0.0,
                width: 10.0,
                height: 10.0,
            },
            &children,
            1.0,
            LayoutCtx::default(),
            &mut |child, _| Some(child_size(child)),
            &mut |_, rect, _| {
                rects.push(rect);
                LayoutNode {
                    widget_id: rects.len() as u64,
                    stable_widget_id: None,
                    subtree_root_id: None,
                    parent_subtree_root_id: None,
                    stable_key: None,
                    widget_type: "test".to_string(),
                    rect,
                    props: HashMap::new(),
                    children: Vec::new(),
                    focusable: false,
                }
            },
        );

        assert_eq!(rects[0].width, 2.0);
        assert_eq!(rects[1].width, 5.0);
        assert_eq!(rects[2].width, 4.0);
        assert_eq!(rects[0].row, 0.0);
        assert_eq!(rects[1].row, 0.0);
        assert_eq!(rects[2].row, 2.0);
    }
}
