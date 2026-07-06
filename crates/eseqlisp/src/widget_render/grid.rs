use super::{Align, WidgetDefinition, resolve_align};
use crate::layout::{
    Constraints, LayoutCtx, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
};
use crate::vm::Value;

pub struct GridWidget;
pub struct ResponsiveGridWidget;

pub static GRID_WIDGET: GridWidget = GridWidget;
pub static RESPONSIVE_GRID_WIDGET: ResponsiveGridWidget = ResponsiveGridWidget;

fn explicit_cols(node: &Value) -> Option<f32> {
    let Value::Map(map) = node else {
        return None;
    };
    match map.get("cols") {
        Some(value) => match &*value.borrow() {
            Value::Number(cols) => Some((*cols as f32).max(1.0)),
            Value::Keyword(value) | Value::String(value) if value == "auto" => None,
            _ => Some(1.0),
        },
        None => Some(1.0),
    }
}

fn layout_cols(node: &Value, area_width: f32, col_width: f32) -> f32 {
    explicit_cols(node).unwrap_or_else(|| {
        if col_width <= 0.0 {
            1.0
        } else {
            (area_width / col_width).floor().max(1.0)
        }
    })
}

fn prop_usize(node: &Value, key: &str) -> Option<usize> {
    let Value::Map(map) = node else {
        return None;
    };
    map.get(key).and_then(|value| match &*value.borrow() {
        Value::Number(value) => Some((*value as usize).max(1)),
        _ => None,
    })
}

fn responsive_levels(node: &Value) -> Vec<usize> {
    let Value::Map(map) = node else {
        return vec![1, 2, 3];
    };
    let Some(value) = map.get("levels") else {
        let min = prop_usize(node, "min-columns").unwrap_or(1);
        let max = prop_usize(node, "max-columns").unwrap_or(3).max(min);
        return (min..=max).collect();
    };
    let Value::List(items) = &*value.borrow() else {
        return vec![1, 2, 3];
    };
    let mut levels = items
        .iter()
        .filter_map(|item| match &*item.borrow() {
            Value::Number(value) if *value >= 1.0 => Some(*value as usize),
            _ => None,
        })
        .collect::<Vec<_>>();
    levels.sort_unstable();
    levels.dedup();
    if levels.is_empty() {
        vec![1, 2, 3]
    } else {
        levels
    }
}

fn responsive_cols(node: &Value, area_width: f32) -> usize {
    let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(0.0);
    let min_item_width = get_prop_num(node, "min-item-width")
        .map(f64_to_f32)
        .unwrap_or(12.0);
    let levels = responsive_levels(node);
    levels
        .iter()
        .copied()
        .filter(|cols| {
            let gaps = gap * cols.saturating_sub(1) as f32;
            let slot_width = (area_width - gaps) / *cols as f32;
            slot_width >= min_item_width || *cols == 1
        })
        .max()
        .unwrap_or(1)
}

fn responsive_slot_width(area_width: f32, cols: usize, gap: f32) -> f32 {
    let gaps = gap * cols.saturating_sub(1) as f32;
    ((area_width - gaps) / cols.max(1) as f32).max(0.0)
}

fn responsive_measure_width(node: &Value, constraints: Constraints) -> f32 {
    if constraints.max_width.is_finite() {
        return constraints.max_width.max(0.0);
    }
    let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(0.0);
    let min_item_width = get_prop_num(node, "min-item-width")
        .map(f64_to_f32)
        .unwrap_or(12.0);
    let cols = responsive_levels(node).first().copied().unwrap_or(1);
    min_item_width * cols as f32 + gap * cols.saturating_sub(1) as f32
}

fn responsive_row_height(node: &Value, slot_width: f32) -> f32 {
    get_prop_num(node, "row-height")
        .map(f64_to_f32)
        .unwrap_or_else(|| {
            let row_aspect = get_prop_num(node, "row-aspect")
                .map(f64_to_f32)
                .unwrap_or(1.0);
            (slot_width * row_aspect).max(1.0)
        })
}

fn responsive_slot_constraints(slot_width: f32, row_height: f32) -> Constraints {
    Constraints {
        min_width: 0.0,
        max_width: slot_width.max(0.0),
        min_height: 0.0,
        max_height: row_height.max(0.0),
        aspect: 1.0,
    }
}

fn debug_responsive_grid_enabled() -> bool {
    std::env::var("ESEQLISP_DEBUG_RESPONSIVE_GRID")
        .ok()
        .is_some_and(|value| value == "1" || value == "true")
}

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
        let explicit_col_width = get_prop_num(node, "col-width").map(f64_to_f32);
        let explicit_row_height = get_prop_num(node, "row-height").map(f64_to_f32);
        let measured_children = if explicit_col_width.is_none() || explicit_row_height.is_none() {
            children
                .iter()
                .filter_map(|child| measure_child(child, constraints))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let widest_child = || {
            measured_children
                .iter()
                .map(|size| size.width)
                .fold(0.0_f32, f32::max)
                .max(1.0)
        };
        let tallest_child = || {
            measured_children
                .iter()
                .map(|size| size.height)
                .fold(0.0_f32, f32::max)
                .max(1.0)
        };
        let col_width = explicit_col_width.unwrap_or_else(widest_child);
        let cols = explicit_cols(node).unwrap_or_else(|| {
            if col_width <= 0.0 {
                1.0
            } else {
                (constraints.max_width / col_width).floor().max(1.0)
            }
        });
        let cols_int = cols as usize;
        let row_height = explicit_row_height.unwrap_or_else(tallest_child);
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
        _layout_ctx: LayoutCtx,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect, LayoutCtx) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let fallback = resolve_align(node, "align", Align::Start);
        let h_align = resolve_align(node, "h-align", fallback);
        let v_align = resolve_align(node, "v-align", fallback);
        let explicit_col_width = get_prop_num(node, "col-width").map(f64_to_f32);
        let width_measurements = if explicit_col_width.is_none() {
            children
                .iter()
                .map(|child| measure_child(child, constraints_for_slot(area.width, area.height)))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let widest_child = || {
            width_measurements
                .iter()
                .filter_map(|size| *size)
                .map(|size| size.width)
                .fold(0.0_f32, f32::max)
                .max(1.0)
        };
        let col_width = explicit_col_width.unwrap_or_else(widest_child);
        let cols = layout_cols(node, area.width, col_width);
        let cols_int = cols as usize;
        let measure_constraints = constraints_for_slot(col_width, area.height);
        let explicit_row_height = get_prop_num(node, "row-height").map(f64_to_f32);
        let slot_measurements = if explicit_row_height.is_none() {
            children
                .iter()
                .map(|child| measure_child(child, measure_constraints))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let tallest_child = || {
            slot_measurements
                .iter()
                .filter_map(|size| *size)
                .map(|size| size.height)
                .fold(0.0_f32, f32::max)
                .max(1.0)
        };
        let row_height = explicit_row_height.unwrap_or_else(tallest_child);
        let slot_constraints = constraints_for_slot(col_width, row_height);

        children
            .iter()
            .enumerate()
            .map(|(idx, child)| {
                let row = (idx / cols_int) as f32;
                let col = (idx % cols_int) as f32;
                let size = if h_align == Align::Stretch && v_align == Align::Stretch {
                    Size {
                        width: col_width,
                        height: row_height,
                    }
                } else if let Some(Some(size)) = slot_measurements.get(idx) {
                    *size
                } else {
                    measure_child(child, slot_constraints).unwrap_or(Size {
                        width: col_width,
                        height: row_height,
                    })
                };
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
                    LayoutCtx::default(),
                )
            })
            .collect()
    }
}

impl WidgetDefinition for ResponsiveGridWidget {
    fn names(&self) -> &'static [&'static str] {
        &["responsive-grid"]
    }

    fn is_container(&self) -> bool {
        true
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &[
            "levels",
            "min-columns",
            "max-columns",
            "min-item-width",
            "gap",
            "row-height",
            "row-aspect",
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
        let width = responsive_measure_width(node, constraints);
        let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(0.0);
        let cols = responsive_cols(node, width);
        let slot_width = responsive_slot_width(width, cols, gap);
        let fallback_row_height = responsive_row_height(node, slot_width);
        let slot_constraints = responsive_slot_constraints(slot_width, fallback_row_height);
        let row_height = get_prop_num(node, "row-height")
            .map(f64_to_f32)
            .unwrap_or_else(|| {
                children
                    .iter()
                    .filter_map(|child| measure_child(child, slot_constraints))
                    .map(|size| size.height)
                    .fold(fallback_row_height, f32::max)
            });
        let rows = ((children.len() + cols - 1) / cols) as f32;
        if debug_responsive_grid_enabled() {
            eprintln!(
                "[responsive-grid measure] children={} constraints=({:.2}x{:.2}) width={:.2} cols={} slot={:.2} row={:.2} rows={:.2} height={:.2}",
                children.len(),
                constraints.max_width,
                constraints.max_height,
                width,
                cols,
                slot_width,
                row_height,
                rows,
                rows * row_height + gap * (rows - 1.0).max(0.0),
            );
        }
        Some(Size {
            width,
            height: rows * row_height + gap * (rows - 1.0).max(0.0),
        })
    }

    fn layout_children(
        &self,
        node: &Value,
        area: Rect,
        children: &[Value],
        _aspect: f32,
        _layout_ctx: LayoutCtx,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect, LayoutCtx) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(0.0);
        let cols = responsive_cols(node, area.width);
        let slot_width = responsive_slot_width(area.width, cols, gap);
        let fallback_row_height = responsive_row_height(node, slot_width);
        let slot_constraints = responsive_slot_constraints(slot_width, fallback_row_height);
        let row_height = get_prop_num(node, "row-height")
            .map(f64_to_f32)
            .unwrap_or_else(|| {
                children
                    .iter()
                    .filter_map(|child| measure_child(child, slot_constraints))
                    .map(|size| size.height)
                    .fold(fallback_row_height, f32::max)
            });
        if debug_responsive_grid_enabled() {
            eprintln!(
                "[responsive-grid layout] children={} area=({:.2},{:.2} {:.2}x{:.2}) cols={} slot={:.2} row={:.2} gap={:.2}",
                children.len(),
                area.col,
                area.row,
                area.width,
                area.height,
                cols,
                slot_width,
                row_height,
                gap,
            );
        }

        children
            .iter()
            .enumerate()
            .map(|(idx, child)| {
                let row = (idx / cols) as f32;
                let col = (idx % cols) as f32;
                build_child(
                    child,
                    Rect {
                        row: area.row + row * (row_height + gap),
                        col: area.col + col * (slot_width + gap),
                        width: slot_width,
                        height: row_height,
                    },
                    LayoutCtx::default(),
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
