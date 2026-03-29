use std::cell::RefCell;
use std::collections::HashMap;

use super::{WidgetDefinition, WidgetEvent};
use crate::layout::{
    Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
};
use crate::vm::Value;

#[cfg(target_os = "macos")]
use super::{MetalPrimitive, WidgetInstance, WidgetViewport, ndc_bounds};

// ── Scroll state ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct ScrollState {
    pub offset_y: f32,
    pub content_height: f32,
    pub viewport_height: f32,
    pub synced_selection: Option<String>,
}

impl Default for ScrollState {
    fn default() -> Self {
        Self {
            offset_y: 0.0,
            content_height: 0.0,
            viewport_height: 0.0,
            synced_selection: None,
        }
    }
}

thread_local! {
    static SCROLL_STATES: RefCell<HashMap<u64, ScrollState>> = RefCell::new(HashMap::new());
    static CURRENT_EVENT_SCROLL_OFFSET: RefCell<Option<f32>> = const { RefCell::new(None) };
}

pub fn get_scroll_state(widget_id: u64) -> ScrollState {
    SCROLL_STATES.with(|s| s.borrow().get(&widget_id).cloned().unwrap_or_default())
}

pub fn set_scroll_state(widget_id: u64, state: ScrollState) {
    let changed = SCROLL_STATES.with(|s| {
        let mut states = s.borrow_mut();
        let old = states.insert(widget_id, state.clone());
        old.as_ref() != Some(&state)
    });
    if changed {
        super::bump_widget_state_generation();
    }
}

/// Per-event scroll context for child widgets that need to map pointer input
/// into content coordinates inside a scroll container.
pub fn current_event_scroll_offset() -> f32 {
    CURRENT_EVENT_SCROLL_OFFSET.with(|offset| offset.borrow().unwrap_or(0.0))
}

pub fn set_current_event_scroll_offset(offset_y: Option<f32>) {
    CURRENT_EVENT_SCROLL_OFFSET.with(|offset| *offset.borrow_mut() = offset_y);
}

/// Clamp an existing scroll offset when content/viewport dimensions change.
fn clamp_offset(state: &mut ScrollState) {
    let max_scroll = (state.content_height - state.viewport_height).max(0.0);
    state.offset_y = state.offset_y.clamp(0.0, max_scroll);
}

fn sync_selected_child_into_view(node: &LayoutNode, state: &mut ScrollState) {
    let Some(child) = node.children.first() else {
        return;
    };
    let Some((selection_key, selected_row, row_height)) =
        super::tree::selection_view_hint(child)
    else {
        state.synced_selection = None;
        return;
    };
    if state.synced_selection.as_deref() == Some(selection_key.as_str()) {
        return;
    }

    let row_top = selected_row as f32 * row_height;
    let row_bottom = row_top + row_height;
    if row_top < state.offset_y {
        state.offset_y = row_top;
    } else if row_bottom > state.offset_y + state.viewport_height {
        state.offset_y = row_bottom - state.viewport_height;
    }
    state.synced_selection = Some(selection_key);
    clamp_offset(state);
}

pub(crate) fn sync_node_state(node: &LayoutNode) -> ScrollState {
    let content_height = node
        .props
        .get("_content_height")
        .and_then(|v| if let Value::Number(n) = v { Some(*n as f32) } else { None })
        .unwrap_or(0.0);
    let viewport_height = node.rect.height;

    let mut state = get_scroll_state(node.widget_id);
    state.content_height = content_height;
    state.viewport_height = viewport_height;
    sync_selected_child_into_view(node, &mut state);
    clamp_offset(&mut state);
    set_scroll_state(node.widget_id, state.clone());
    state
}

// ── Widget definition ────────────────────────────────────────────────────────

pub struct ScrollWidget;

pub static SCROLL_WIDGET: ScrollWidget = ScrollWidget;

impl WidgetDefinition for ScrollWidget {
    fn names(&self) -> &'static [&'static str] {
        &["scroll"]
    }

    fn is_container(&self) -> bool {
        true
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["padding"]
    }

    fn measure(
        &self,
        node: &Value,
        children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        // Measure child with unbounded height to get its natural width.
        // Report height=0 so flex distributes remaining space to us.
        let child_size = children.first().and_then(|child| {
            measure_child(
                child,
                Constraints {
                    max_height: f32::MAX,
                    ..constraints
                },
            )
        });
        // Width: use explicit :width, or fill available space (like a block element).
        // Falls back to child width only if max_width is unbounded.
        let width = get_prop_num(node, "width").map(f64_to_f32).unwrap_or_else(|| {
            if constraints.max_width < f32::MAX {
                constraints.max_width
            } else {
                child_size.map(|s| s.width).unwrap_or(0.0)
            }
        });
        Some(Size {
            width,
            height: get_prop_num(node, "height").map(f64_to_f32).unwrap_or(0.0),
        })
    }

    fn layout_children(
        &self,
        _node: &Value,
        area: Rect,
        children: &[Value],
        _aspect: f32,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let Some(child) = children.first() else {
            return vec![];
        };

        // Measure child with full parent width but unbounded height
        let child_size = measure_child(
            child,
            Constraints {
                min_width: 0.0,
                max_width: area.width,
                min_height: 0.0,
                max_height: f32::MAX,
                aspect: 1.0,
            },
        )
        .unwrap_or(Size {
            width: area.width,
            height: area.height,
        });

        // Child is positioned at scroll container's origin with full content height.
        // The scroll offset is applied at render time, not here.
        let child_rect = Rect {
            row: area.row,
            col: area.col,
            width: area.width,
            height: child_size.height,
        };

        vec![build_child(child, child_rect)]
    }

    fn captures_scroll_gesture(&self) -> bool {
        true
    }

    fn scroll_gesture_event(
        &self,
        node: &LayoutNode,
        _local_col: f32,
        _local_row: f32,
        _delta_x: f32,
        delta_y: f32,
    ) -> Option<WidgetEvent> {
        let mut state = get_scroll_state(node.widget_id);

        // Update dimensions from layout props
        state.content_height = node
            .props
            .get("_content_height")
            .and_then(|v| if let Value::Number(n) = v { Some(*n as f32) } else { None })
            .unwrap_or(state.content_height);
        state.viewport_height = node
            .props
            .get("_viewport_height")
            .and_then(|v| if let Value::Number(n) = v { Some(*n as f32) } else { None })
            .unwrap_or(state.viewport_height);

        // Trackpad delta_y: negative = scroll content up (reveal below), positive = scroll down
        // We want: scrolling down (two-finger swipe up) increases offset_y
        let scroll_speed = 0.05; // cells per pixel-delta
        state.offset_y -= delta_y * scroll_speed;
        clamp_offset(&mut state);

        set_scroll_state(node.widget_id, state);

        // Return Custom(Nil) to trigger a redraw without invoking a Lisp callback
        Some(WidgetEvent::Custom(Value::Nil))
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(SCROLL_FRAGMENT_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let state = sync_node_state(node);
        let content_height = state.content_height;
        let viewport_height = state.viewport_height;

        // No scrollbar if content fits
        if content_height <= viewport_height || viewport_height <= 0.0 {
            return Vec::new();
        }

        // The scrollbar is drawn as a thin vertical strip on the right edge.
        // The shader handles the rounded pill shape internally.
        let bar_width: f32 = 0.4; // cells
        let bar_rect = Rect {
            row: node.rect.row,
            col: node.rect.col + node.rect.width - bar_width,
            width: bar_width,
            height: viewport_height,
        };
        let (ndc_min, ndc_max) = ndc_bounds(bar_rect, viewport);
        let px_w = bar_rect.width * viewport.cell_w;
        let px_h = bar_rect.height * viewport.cell_h;

        let max_scroll = (content_height - viewport_height).max(1.0);
        let scroll_ratio = state.offset_y / max_scroll;
        let thumb_ratio = (viewport_height / content_height).clamp(0.05, 1.0);

        vec![MetalPrimitive::WidgetInstance {
            widget_type: widget_type.to_string(),
            instance: WidgetInstance {
                ndc_min,
                ndc_max,
                value_t: scroll_ratio,
                orientation: 0.0,
                itime: viewport.time_seconds,
                // uniform_a.x = thumb height ratio, uniform_a.y = track padding (cells)
                uniform_a: [thumb_ratio, 0.15, 0.0, 0.0],
                uniform_b: [0.0; 4],
                // color_a = thumb color (semi-transparent white)
                color_a: [1.0, 1.0, 1.0, 0.35],
                // color_b = track color
                color_b: [1.0, 1.0, 1.0, 0.06],
                color_c: [0.0, 0.0, 1.0, 1.0], // full UV bounds
                color_d: [0.0; 4],
                corner_radius: 1.0,
                pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
            },
            is_background: false,
        }]
    }
}

// ── Metal shader ─────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
const SCROLL_FRAGMENT_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;

    // Uniforms: value_t = scroll position [0,1]
    //           uniform_a.x = thumb height ratio (viewport/content)
    //           uniform_a.y = horizontal padding in normalized coords
    float scroll_t = in.value_t;
    float thumb_ratio = in.uniform_a.x;
    float pad = in.uniform_a.y;

    // Thumb vertical position and size in UV space
    float thumb_h = max(thumb_ratio, 0.04);
    float thumb_y = scroll_t * (1.0 - thumb_h);

    // Thumb horizontal bounds (centered pill, narrow)
    float bar_left = pad;
    float bar_right = 1.0 - pad;
    float bar_cx = 0.5;
    float bar_hw = (bar_right - bar_left) * 0.5;

    // SDF for the thumb pill (rounded rect)
    // Map to centered coords for the thumb
    float2 thumb_center = float2(bar_cx, thumb_y + thumb_h * 0.5);
    float2 half_size = float2(bar_hw, thumb_h * 0.5);

    // Pill radius = half the width (fully round on short axis)
    float radius = min(half_size.x * aspect, half_size.y);

    // Aspect-correct SDF
    float2 p = float2((uv.x - thumb_center.x) * aspect, uv.y - thumb_center.y);
    float2 q = abs(p) - float2(half_size.x * aspect - radius, half_size.y - radius);
    float dist = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;

    // Anti-aliased edge
    float edge = fwidth(dist) * 1.0;
    float thumb_mask = smoothstep(edge, -edge, dist);

    // Thumb color with alpha
    float4 thumb_color = in.color_a;
    float4 result = float4(thumb_color.rgb, thumb_color.a * thumb_mask);

    // Discard fully transparent pixels
    if (result.a < 0.002) { discard_fragment(); }

    return result;
}
"#;
