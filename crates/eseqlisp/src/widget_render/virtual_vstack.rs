use std::cell::RefCell;
use std::collections::HashMap;

use super::{Align, WidgetDefinition, resolve_align};
use crate::layout::{
    Constraints, LayoutCtx, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
    get_prop_u64, get_stable_widget_id, get_stable_widget_key, prop_is_keyword,
};
use crate::vm::Value;

pub struct VirtualVStackWidget;

pub static VIRTUAL_VSTACK_WIDGET: VirtualVStackWidget = VirtualVStackWidget;

#[derive(Clone, Debug)]
struct VirtualVStackState {
    measured_width: f32,
    estimated_item_height: f32,
    item_heights: HashMap<String, f32>,
}

thread_local! {
    static VIRTUAL_VSTACK_STATES: RefCell<HashMap<u64, VirtualVStackState>> =
        RefCell::new(HashMap::new());
}

fn estimated_item_height(node: &Value) -> f32 {
    get_prop_num(node, "estimated-item-height")
        .map(f64_to_f32)
        .filter(|height| *height > 0.0)
        .unwrap_or(3.0)
}

fn overscan_items(node: &Value) -> f32 {
    get_prop_num(node, "overscan")
        .map(f64_to_f32)
        .filter(|count| *count >= 0.0)
        .unwrap_or(4.0)
}

fn state_key(node: &Value) -> Option<u64> {
    get_stable_widget_id(node)
}

fn child_key(child: &Value, index: usize) -> String {
    if let Some(id) = get_prop_u64(child, "__stable-widget-id") {
        return format!("wid:{id}");
    }
    if let Some(key) = get_stable_widget_key(child) {
        return format!("key:{key}");
    }
    format!("idx:{index}")
}

fn cached_state(key: u64, width: f32, estimated: f32) -> VirtualVStackState {
    VIRTUAL_VSTACK_STATES.with(|states| {
        let mut states = states.borrow_mut();
        let state = states.entry(key).or_insert_with(|| VirtualVStackState {
            measured_width: width,
            estimated_item_height: estimated,
            item_heights: HashMap::new(),
        });
        if (state.measured_width - width).abs() > 0.01
            || (state.estimated_item_height - estimated).abs() > 0.01
        {
            state.measured_width = width;
            state.estimated_item_height = estimated;
            state.item_heights.clear();
        }
        state.clone()
    })
}

fn update_cached_heights(
    key: u64,
    width: f32,
    estimated: f32,
    current_keys: &[String],
    measured: impl IntoIterator<Item = (String, f32)>,
) {
    let current = current_keys
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let mut changed = false;
    VIRTUAL_VSTACK_STATES.with(|states| {
        let mut states = states.borrow_mut();
        let state = states.entry(key).or_insert_with(|| VirtualVStackState {
            measured_width: width,
            estimated_item_height: estimated,
            item_heights: HashMap::new(),
        });
        if (state.measured_width - width).abs() > 0.01
            || (state.estimated_item_height - estimated).abs() > 0.01
        {
            state.measured_width = width;
            state.estimated_item_height = estimated;
            state.item_heights.clear();
            changed = true;
        }
        let old_len = state.item_heights.len();
        state
            .item_heights
            .retain(|item_key, _| current.contains(item_key));
        changed |= state.item_heights.len() != old_len;
        for (item_key, height) in measured {
            let height = height.max(0.0);
            let old = state.item_heights.insert(item_key, height);
            changed |= old.is_none_or(|old_height| (old_height - height).abs() > 0.01);
        }
    });
    if changed {
        super::bump_widget_state_generation();
    }
}

fn item_heights(
    children: &[Value],
    keys: &[String],
    state: Option<&VirtualVStackState>,
    estimated: f32,
) -> Vec<f32> {
    children
        .iter()
        .zip(keys)
        .map(|(_, key)| {
            state
                .and_then(|state| state.item_heights.get(key).copied())
                .unwrap_or(estimated)
        })
        .collect()
}

fn total_height(heights: &[f32], gap: f32, pad_y: f32) -> f32 {
    heights.iter().sum::<f32>() + gap * (heights.len() as f32 - 1.0).max(0.0) + pad_y * 2.0
}

fn cumulative_tops(heights: &[f32], gap: f32, pad_y: f32) -> Vec<f32> {
    let mut tops = Vec::with_capacity(heights.len());
    let mut cursor = pad_y;
    for height in heights {
        tops.push(cursor);
        cursor += *height + gap;
    }
    tops
}

fn visible_range(
    heights: &[f32],
    tops: &[f32],
    layout_ctx: LayoutCtx,
    estimated: f32,
    overscan: f32,
) -> std::ops::Range<usize> {
    if heights.is_empty() {
        return 0..0;
    }
    if !layout_ctx.has_scroll_viewport() {
        return 0..heights.len();
    }

    let overscan_height = overscan * estimated;
    let visible_top = (layout_ctx.scroll_offset_y - overscan_height).max(0.0);
    let visible_bottom =
        layout_ctx.scroll_offset_y + layout_ctx.scroll_viewport_height + overscan_height;

    let mut start = 0;
    while start < heights.len() && tops[start] + heights[start] <= visible_top {
        start += 1;
    }

    let mut end = start;
    while end < heights.len() && tops[end] <= visible_bottom {
        end += 1;
    }

    start..end.max((start + 1).min(heights.len()))
}

fn debug_virtual_vstack_enabled() -> bool {
    std::env::var_os("ESEQLISP_DEBUG_VIRTUAL_VSTACK").is_some()
}

impl WidgetDefinition for VirtualVStackWidget {
    fn names(&self) -> &'static [&'static str] {
        &["virtual-v-stack"]
    }

    fn is_container(&self) -> bool {
        true
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &[
            "padding",
            "gap",
            "align",
            "width",
            "estimated-item-height",
            "overscan",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let padding = get_prop_num(node, "padding").map(f64_to_f32).unwrap_or(0.0);
        let pad_y = padding / constraints.aspect;
        let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(0.0);
        let own_width = get_prop_num(node, "width").map(f64_to_f32);
        let width = own_width.unwrap_or_else(|| {
            if constraints.max_width.is_finite() {
                constraints.max_width
            } else {
                0.0
            }
        });
        let estimated = estimated_item_height(node);
        let keys = children
            .iter()
            .enumerate()
            .map(|(index, child)| child_key(child, index))
            .collect::<Vec<_>>();
        let state = state_key(node).map(|key| cached_state(key, width, estimated));
        let heights = item_heights(children, &keys, state.as_ref(), estimated);
        Some(Size {
            width,
            height: total_height(&heights, gap, pad_y),
        })
    }

    fn layout_children(
        &self,
        node: &Value,
        area: Rect,
        children: &[Value],
        aspect: f32,
        layout_ctx: LayoutCtx,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect, LayoutCtx) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let padding = get_prop_num(node, "padding").map(f64_to_f32).unwrap_or(0.0);
        let pad_y = padding / aspect;
        let gap = get_prop_num(node, "gap").map(f64_to_f32).unwrap_or(0.0);
        let align = resolve_align(node, "align", Align::Start);
        let estimated = estimated_item_height(node);
        let keys = children
            .iter()
            .enumerate()
            .map(|(index, child)| child_key(child, index))
            .collect::<Vec<_>>();
        let state = state_key(node).map(|key| cached_state(key, area.width, estimated));
        let mut heights = item_heights(children, &keys, state.as_ref(), estimated);
        let tops = cumulative_tops(&heights, gap, pad_y);
        let range = visible_range(&heights, &tops, layout_ctx, estimated, overscan_items(node));
        if debug_virtual_vstack_enabled() {
            let known_heights = state
                .as_ref()
                .map(|state| state.item_heights.len())
                .unwrap_or(0);
            eprintln!(
                "[virtual-v-stack] key={:?} children={} range={}..{} scroll_y={:.2} viewport_h={:.2} estimated_h={:.2} known_heights={}",
                state_key(node),
                children.len(),
                range.start,
                range.end,
                layout_ctx.scroll_offset_y,
                layout_ctx.scroll_viewport_height,
                estimated,
                known_heights
            );
        }

        let inner_width = (area.width - padding * 2.0).max(0.0);
        let inner_constraints = Constraints {
            min_width: 0.0,
            max_width: inner_width,
            min_height: 0.0,
            max_height: f32::INFINITY,
            aspect: 1.0,
        };

        let mut measured_updates = Vec::new();
        for index in range.clone() {
            let size = measure_child(&children[index], inner_constraints).unwrap_or(Size {
                width: inner_width,
                height: estimated,
            });
            heights[index] = size.height;
            measured_updates.push((keys[index].clone(), size.height));
        }

        if let Some(key) = state_key(node) {
            update_cached_heights(key, area.width, estimated, &keys, measured_updates);
        }

        let tops = cumulative_tops(&heights, gap, pad_y);
        range
            .map(|index| {
                let child = &children[index];
                let size = measure_child(child, inner_constraints).unwrap_or(Size {
                    width: inner_width,
                    height: heights[index],
                });
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
                build_child(
                    child,
                    Rect {
                        row: area.row + tops[index],
                        col,
                        width: child_width,
                        height: size.height,
                    },
                    LayoutCtx::default(),
                )
            })
            .collect()
    }
}
