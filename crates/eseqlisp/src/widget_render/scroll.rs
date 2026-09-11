use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use super::{WidgetDefinition, WidgetEvent};
use crate::layout::{
    Constraints, LayoutCtx, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
    get_stable_widget_id, prop_is_keyword,
};
use crate::vm::Value;

use super::{GpuPrimitive, WidgetInstance, WidgetViewport, ndc_bounds};

// ── Scroll state ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct ScrollState {
    pub offset_y: f32,
    pub content_height: f32,
    pub viewport_height: f32,
    pub synced_selection: Option<String>,
    /// Last `:center-row` request honored (f32 bits), so a request only
    /// moves the view when it changes and manual scrolling holds otherwise.
    pub applied_center_bits: Option<u32>,
}

impl Default for ScrollState {
    fn default() -> Self {
        Self {
            offset_y: 0.0,
            content_height: 0.0,
            viewport_height: 0.0,
            synced_selection: None,
            applied_center_bits: None,
        }
    }
}

/// `:center-row R` (cells, may be a bound float) asks the view to keep the
/// band `[R, R + :center-span)` centered — the classic tracker follow: the
/// offset is `R + span/2 - viewport/2`, clamped at 0 so the first half-screen
/// of rows stays put, and at the bottom likewise. A negative R is "no
/// request". Only a changed request scrolls, so a wheel or drag in between
/// is not fought.
/// Returns whether the offset moved.
fn apply_center_request(state: &mut ScrollState, center: Option<f32>, span: f32) -> bool {
    let Some(center) = center.filter(|c| *c >= 0.0 && c.is_finite()) else {
        return false;
    };
    let bits = center.to_bits();
    if state.applied_center_bits == Some(bits) {
        return false;
    }
    state.applied_center_bits = Some(bits);
    if state.viewport_height <= 0.0 {
        return false;
    }
    let before = state.offset_y;
    state.offset_y = center + span * 0.5 - state.viewport_height * 0.5;
    clamp_offset(state);
    state.offset_y != before
}

fn center_request_from_props(props: &HashMap<String, Value>) -> (Option<f32>, f32) {
    let center = props
        .contains_key("center-row")
        .then(|| super::get_f32_prop(props, "center-row", -1.0));
    (center, super::get_f32_prop(props, "center-span", 1.0))
}

fn center_request_from_value(node: &Value) -> (Option<f32>, f32) {
    let Value::Map(map) = node else {
        return (None, 1.0);
    };
    let read = |key: &str, default: f32| -> f32 {
        match map.get(key).map(|cell| cell.borrow().clone()) {
            Some(Value::Number(n)) => n as f32,
            Some(Value::ReactiveRef { slot, .. }) => {
                crate::reactive::read_float_slot(&slot) as f32
            }
            _ => default,
        }
    };
    let center = map.contains_key("center-row").then(|| read("center-row", -1.0));
    (center, read("center-span", 1.0))
}

thread_local! {
    static SCROLL_STATES: RefCell<HashMap<u64, ScrollState>> = RefCell::new(HashMap::new());
    static CURRENT_EVENT_SCROLL_OFFSET: RefCell<Option<f32>> = const { RefCell::new(None) };
    // Scroll state keys (stable ids) whose state changed since the last drain.
    // Scroll changes are scoped to the owning scroll widget's subtree, so they
    // flow through the dirty-widget-id path instead of bumping the global
    // widget state generation (which would invalidate every tile's caches).
    static DIRTY_SCROLL_KEYS: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
    // Scroll state keys whose offset moved at RENDER time (a `:center-row`
    // follow) rather than through a gesture. A gesture already schedules the
    // relayout a virtualizing child needs; these did not, so the editor does.
    static RELAYOUT_SCROLL_KEYS: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
}

pub fn take_relayout_scroll_keys() -> HashSet<u64> {
    RELAYOUT_SCROLL_KEYS.with(|keys| std::mem::take(&mut *keys.borrow_mut()))
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
        DIRTY_SCROLL_KEYS.with(|keys| keys.borrow_mut().insert(widget_id));
    }
}

pub fn take_dirty_scroll_keys() -> HashSet<u64> {
    DIRTY_SCROLL_KEYS.with(|keys| std::mem::take(&mut *keys.borrow_mut()))
}

pub fn has_dirty_scroll_keys() -> bool {
    DIRTY_SCROLL_KEYS.with(|keys| !keys.borrow().is_empty())
}

pub fn scroll_state_key(node: &LayoutNode) -> u64 {
    node.stable_widget_id.unwrap_or(node.widget_id)
}

pub fn scroll_state_key_for_value(node: &Value) -> Option<u64> {
    get_stable_widget_id(node)
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

fn stick_to_bottom_enabled(node: &LayoutNode) -> bool {
    matches!(node.props.get("stick-to-bottom"), Some(Value::Bool(true)))
}

fn stick_to_bottom_enabled_value(node: &Value) -> bool {
    matches!(
        crate::layout::get_map(node).and_then(|props| props.get("stick-to-bottom").cloned()),
        Some(Value::Bool(true))
    )
}

fn current_content_height(node: &LayoutNode) -> f32 {
    if let Some(child) = node.children.first()
        && let Some(height) = super::tree::current_content_height(child)
    {
        return height;
    }
    node.props
        .get("_content_height")
        .and_then(|v| {
            if let Value::Number(n) = v {
                Some(*n as f32)
            } else {
                None
            }
        })
        .unwrap_or(0.0)
}

fn sync_selected_child_into_view(node: &LayoutNode, state: &mut ScrollState) {
    let Some(child) = node.children.first() else {
        return;
    };
    let Some((selection_key, selected_row, row_height)) = super::tree::selection_view_hint(child)
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
    let content_height = current_content_height(node);
    let viewport_height = node.rect.height;

    let key = scroll_state_key(node);
    let mut state = get_scroll_state(key);
    let old_content_height = state.content_height;
    let old_max_scroll = (state.content_height - state.viewport_height).max(0.0);
    let was_at_bottom = state.offset_y >= old_max_scroll - 0.001;
    state.content_height = content_height;
    state.viewport_height = viewport_height;
    sync_selected_child_into_view(node, &mut state);
    if stick_to_bottom_enabled(node) && (old_content_height <= 0.0 || was_at_bottom) {
        state.offset_y = (state.content_height - state.viewport_height).max(0.0);
    }
    let (center, span) = center_request_from_props(&node.props);
    if apply_center_request(&mut state, center, span) {
        RELAYOUT_SCROLL_KEYS.with(|keys| keys.borrow_mut().insert(key));
    }
    clamp_offset(&mut state);
    set_scroll_state(key, state.clone());
    state
}

fn sync_layout_state(node: &Value, content_height: f32, viewport_height: f32) -> ScrollState {
    let Some(key) = scroll_state_key_for_value(node) else {
        return ScrollState {
            content_height,
            viewport_height,
            ..ScrollState::default()
        };
    };

    let mut state = get_scroll_state(key);
    let old_content_height = state.content_height;
    let old_max_scroll = (state.content_height - state.viewport_height).max(0.0);
    let was_at_bottom = state.offset_y >= old_max_scroll - 0.001;
    state.content_height = content_height;
    state.viewport_height = viewport_height;
    if stick_to_bottom_enabled_value(node) && (old_content_height <= 0.0 || was_at_bottom) {
        state.offset_y = (state.content_height - state.viewport_height).max(0.0);
    }
    let (center, span) = center_request_from_value(node);
    apply_center_request(&mut state, center, span);
    clamp_offset(&mut state);
    set_scroll_state(key, state.clone());
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

    fn bindable_props(&self) -> &'static [&'static str] {
        &["center-row"]
    }

    fn measure(
        &self,
        node: &Value,
        children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        // Width: use explicit :width, or fill available space (like a block element).
        // Falls back to child width only if max_width is unbounded.
        let explicit_width = get_prop_num(node, "width").map(f64_to_f32);
        let child_max_width = explicit_width.unwrap_or(constraints.max_width);
        let child_size = children.first().and_then(|child| {
            measure_child(
                child,
                Constraints {
                    max_width: child_max_width,
                    max_height: f32::INFINITY,
                    ..constraints
                },
            )
        });
        let width = explicit_width.unwrap_or_else(|| {
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
        _measure_ctx: &MeasureCtx<'_>,
        _layout_ctx: LayoutCtx,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect, LayoutCtx) -> LayoutNode,
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
                max_height: f32::INFINITY,
                aspect: 1.0,
            },
        )
        .unwrap_or(Size {
            width: area.width,
            height: area.height,
        });

        // A fill child has the viewport as its minimum extent, not its maximum:
        // short content can distribute spare space while long content still scrolls.
        let content_height = if prop_is_keyword(child, "height", "fill") {
            child_size.height.max(area.height)
        } else {
            child_size.height
        };

        // Child is positioned at scroll container's origin with full content height.
        // The scroll offset is applied at render time, not here.
        let child_rect = Rect {
            row: area.row,
            col: area.col,
            width: area.width,
            height: content_height,
        };

        let scroll_state = sync_layout_state(_node, content_height, area.height);
        let child_layout_ctx = LayoutCtx::with_scroll(scroll_state.offset_y, area.height);

        vec![build_child(child, child_rect, child_layout_ctx)]
    }

    fn captures_scroll_gesture(&self, _node: &LayoutNode) -> bool {
        true
    }

    fn scroll_gesture_event(
        &self,
        node: &LayoutNode,
        _local_col: f32,
        _local_row: f32,
        delta_x: f32,
        delta_y: f32,
    ) -> Option<WidgetEvent> {
        // This widget only scrolls vertically. A mostly-sideways gesture is
        // not ours: returning None lets the editor fall through to the tile's
        // smooth horizontal widget scroll, so a wide buffer (the tracker)
        // pans under a pinned viewport instead of swallowing the swipe.
        if delta_x != 0.0 && delta_x.abs() > delta_y.abs() {
            return None;
        }
        let key = scroll_state_key(node);
        let mut state = get_scroll_state(key);

        // Update dimensions from current child state; tree children can change
        // visible height via expand/collapse without changing widget props.
        state.content_height = current_content_height(node);
        state.viewport_height = node
            .props
            .get("_viewport_height")
            .and_then(|v| {
                if let Value::Number(n) = v {
                    Some(*n as f32)
                } else {
                    None
                }
            })
            .unwrap_or(state.viewport_height);

        // Trackpad delta_y: negative = scroll content up (reveal below), positive = scroll down
        // We want: scrolling down (two-finger swipe up) increases offset_y
        let scroll_speed = 0.05; // cells per pixel-delta
        state.offset_y -= delta_y * scroll_speed;
        clamp_offset(&mut state);

        set_scroll_state(key, state);

        // Return Custom(Nil) to trigger a redraw without invoking a Lisp callback
        Some(WidgetEvent::Custom(Value::Nil))
    }

    fn fragment_shader(
        &self,
        _widget_type: &str,
        backend: super::ShaderBackend,
    ) -> Option<&'static str> {
        SCROLL_FRAGMENT_SHADER.source(backend)
    }

    fn build_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
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

        vec![GpuPrimitive::WidgetInstance {
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
                uniform_c: [0.0; 4],
                uniform_d: [0.0; 4],
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

const SCROLL_FRAGMENT_SHADER: super::ShaderSources = super::ShaderSources::both(r#"
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
"#, super::wgsl::SCROLL_FRAGMENT_SHADER);

#[cfg(test)]
mod center_row_tests {
    use super::*;

    fn state(viewport: f32, content: f32) -> ScrollState {
        ScrollState {
            content_height: content,
            viewport_height: viewport,
            ..ScrollState::default()
        }
    }

    #[test]
    fn center_row_holds_the_top_until_the_row_passes_the_middle() {
        let mut s = state(20.0, 64.0);
        apply_center_request(&mut s, Some(3.0), 1.0);
        assert_eq!(s.offset_y, 0.0, "row 3 is above the middle of a 20-row view");
        apply_center_request(&mut s, Some(9.5), 1.0);
        assert_eq!(s.offset_y, 0.0, "row 9.5 sits exactly on the middle");
        apply_center_request(&mut s, Some(30.0), 1.0);
        assert_eq!(s.offset_y, 20.5, "past the middle the row is kept centered");
        apply_center_request(&mut s, Some(63.0), 1.0);
        assert_eq!(s.offset_y, 44.0, "clamped at the bottom");
    }

    #[test]
    fn a_repeated_center_request_does_not_fight_manual_scrolling() {
        let mut s = state(20.0, 64.0);
        apply_center_request(&mut s, Some(30.0), 1.0);
        s.offset_y = 5.0;
        apply_center_request(&mut s, Some(30.0), 1.0);
        assert_eq!(s.offset_y, 5.0);
        apply_center_request(&mut s, Some(-1.0), 1.0);
        assert_eq!(s.offset_y, 5.0, "negative is no request");
    }
}
