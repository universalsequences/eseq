use std::time::Duration;

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::layout::{LayoutNode, hit_test_layout};
use crate::tile::{WidgetClick, WidgetGesture};
use crate::ui::hit::{self, HitGrid};
use crate::vm::Value;
use crate::widget_render::{
    self, MouseEventOutcome, WidgetCursor, begin_widget_gesture as begin_widget_gesture_data,
    captures_scroll_gesture, handle_event, map_double_click_event, map_magnify_event,
    map_mouse_event, map_scroll_gesture_event,
};

use super::Editor;
use super::widget_focus::find_node_by_id;

const PATCH_PORT_CANCEL_RADIUS_CELLS: f32 = 1.5;
const PATCH_PORT_DROP_RADIUS_CELLS: f32 = 1.75;
const WIDGET_DRAG_START_THRESHOLD_CELLS: f32 = 1.0;

fn is_slider_widget(node: &LayoutNode) -> bool {
    matches!(node.widget_type.as_str(), "hslider" | "vslider" | "slider")
}

fn node_contains_point(node: &LayoutNode, row: f32, col: f32) -> bool {
    row >= node.rect.row
        && row < node.rect.row + node.rect.height
        && col >= node.rect.col
        && col < node.rect.col + node.rect.width
}

fn list_contains_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(value) | Value::Keyword(value) => value == needle,
        Value::List(items) => items.iter().any(|item| {
            let item = item.borrow();
            matches!(&*item, Value::String(value) | Value::Keyword(value) if value == needle)
        }),
        _ => false,
    }
}

fn node_accepts_drag_type(node: &LayoutNode, drag_type: &str) -> bool {
    if !node.props.contains_key("on-drop") {
        return false;
    }
    node.props
        .get("drop-types")
        .is_some_and(|value| list_contains_string(value, drag_type))
        || node
            .props
            .get("accepts-drag-type")
            .is_some_and(|value| list_contains_string(value, drag_type))
}

fn deepest_drop_target(
    node: &LayoutNode,
    row: f32,
    col: f32,
    drag_type: &str,
) -> Option<(LayoutNode, f32, f32)> {
    if node.widget_type == "scroll" {
        if !node_contains_point(node, row, col) {
            return None;
        }
        let state =
            widget_render::scroll::get_scroll_state(widget_render::scroll::scroll_state_key(node));
        let adjusted_row = row + state.offset_y;
        return node
            .children
            .iter()
            .rev()
            .find_map(|child| deepest_drop_target(child, adjusted_row, col, drag_type))
            .or_else(|| node_accepts_drag_type(node, drag_type).then(|| (node.clone(), row, col)));
    }

    let child_target = node
        .children
        .iter()
        .rev()
        .find_map(|child| deepest_drop_target(child, row, col, drag_type));
    if child_target.is_some() {
        return child_target;
    }

    if node_contains_point(node, row, col) && node_accepts_drag_type(node, drag_type) {
        Some((node.clone(), row, col))
    } else {
        None
    }
}

fn widget_drag_data(value: &Value) -> Option<(String, Value)> {
    let Value::Map(map) = value else {
        return None;
    };
    let kind = map.get("kind").and_then(|value| match &*value.borrow() {
        Value::String(value) => Some(value.clone()),
        _ => None,
    })?;
    if kind != "widget-drag" {
        return None;
    }
    let drag_type = map
        .get("drag-type")
        .and_then(|value| match &*value.borrow() {
            Value::String(value) | Value::Keyword(value) => Some(value.clone()),
            _ => None,
        })?;
    let payload = map
        .get("payload")
        .map(|value| value.borrow().clone())
        .unwrap_or(Value::Nil);
    Some((drag_type, payload))
}

fn active_widget_drag(gesture: &WidgetGesture) -> Option<(String, Value)> {
    widget_drag_data(gesture.gesture_data.as_ref()?)
}

pub(super) fn widget_gesture_is_drag(gesture: &WidgetGesture) -> bool {
    active_widget_drag(gesture).is_some()
}

fn widget_drag_threshold_reached(
    gesture: &WidgetGesture,
    precise_col: f32,
    precise_row: f32,
) -> bool {
    let dx = precise_col - gesture.start_precise_col;
    let dy = precise_row - gesture.start_precise_row;
    dx.hypot(dy) >= WIDGET_DRAG_START_THRESHOLD_CELLS
}

fn deepest_double_click_node(node: &LayoutNode, row: f32, col: f32) -> Option<LayoutNode> {
    if !node_contains_point(node, row, col) {
        return None;
    }
    node.children
        .iter()
        .rev()
        .find_map(|child| deepest_double_click_node(child, row, col))
        .or_else(|| {
            node.props
                .contains_key("on-double-click")
                .then(|| node.clone())
        })
}

fn path_to_widget_id<'a>(
    node: &'a LayoutNode,
    target_id: u64,
    path: &mut Vec<&'a LayoutNode>,
) -> bool {
    path.push(node);
    if node.widget_id == target_id {
        return true;
    }
    for child in &node.children {
        if path_to_widget_id(child, target_id, path) {
            return true;
        }
    }
    path.pop();
    false
}

fn truthy_prop(node: &LayoutNode, key: &str) -> bool {
    match node.props.get(key) {
        Some(Value::Bool(value)) => *value,
        Some(Value::Nil) | None => false,
        Some(_) => true,
    }
}

fn node_captures_pointer(node: &LayoutNode) -> bool {
    truthy_prop(node, "capture-pointer")
}

fn nearest_pointer_capture_node(layout: &LayoutNode, hit_node: &LayoutNode) -> Option<LayoutNode> {
    let mut path = Vec::new();
    if !path_to_widget_id(layout, hit_node.widget_id, &mut path) {
        return None;
    }
    path.into_iter()
        .rev()
        .find(|node| node_captures_pointer(node))
        .cloned()
}

fn pointer_dispatch_node(layout: &LayoutNode, hit_node: LayoutNode) -> LayoutNode {
    nearest_pointer_capture_node(layout, &hit_node).unwrap_or(hit_node)
}

fn nearest_widget_gesture_node(
    layout: &LayoutNode,
    hit_node: &LayoutNode,
    local_col: f32,
    local_row: f32,
) -> Option<(LayoutNode, Option<Value>)> {
    let mut path = Vec::new();
    if !path_to_widget_id(layout, hit_node.widget_id, &mut path) {
        return None;
    }
    let pointer_capture_id = path
        .iter()
        .rev()
        .find(|node| node_captures_pointer(node))
        .map(|node| node.widget_id);
    let hit_node_handles_pointer = widget_render::node_handles_pointer_events(hit_node);
    path.into_iter().rev().find_map(|node| {
        let is_pointer_capture = pointer_capture_id == Some(node.widget_id);
        if pointer_capture_id.is_some() && !is_pointer_capture {
            return None;
        }
        if node.widget_id != hit_node.widget_id && hit_node_handles_pointer && !is_pointer_capture {
            return None;
        }
        let gesture_data = begin_widget_gesture_data(&node, local_col, local_row);
        if gesture_data.is_some() {
            Some((node.clone(), gesture_data))
        } else if widget_render::widget_captures_drag(&node.widget_type) {
            Some((node.clone(), None))
        } else {
            None
        }
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PatchPortDirection {
    In,
    Out,
}

#[derive(Clone)]
struct PatchPortLayout {
    direction: PatchPortDirection,
    track: usize,
    dest_kind: String,
    dest: usize,
    input: usize,
    active: bool,
    pending: bool,
    center: (f32, f32),
    connected_sources: Vec<usize>,
    on_cable_click: Option<Value>,
    on_patch_drop: Option<Value>,
    on_patch_cancel: Option<Value>,
    on_patch_miss: Option<Value>,
}

fn patch_port_direction(node: &LayoutNode) -> Option<PatchPortDirection> {
    match node.props.get("direction") {
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "in" => {
            Some(PatchPortDirection::In)
        }
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "out" => {
            Some(PatchPortDirection::Out)
        }
        _ => None,
    }
}

fn node_bool_prop(node: &LayoutNode, key: &str) -> bool {
    matches!(node.props.get(key), Some(Value::Bool(true)))
}

fn node_usize_prop(node: &LayoutNode, key: &str) -> Option<usize> {
    match node.props.get(key) {
        Some(Value::Number(value)) if value.is_finite() && *value >= 0.0 => Some(*value as usize),
        _ => None,
    }
}

fn node_string_prop(node: &LayoutNode, key: &str) -> Option<String> {
    match node.props.get(key) {
        Some(Value::String(value)) | Some(Value::Keyword(value)) => Some(value.clone()),
        _ => None,
    }
}

fn node_usize_list_prop(node: &LayoutNode, key: &str) -> Vec<usize> {
    let Some(Value::List(values)) = node.props.get(key) else {
        return Vec::new();
    };
    values
        .iter()
        .filter_map(|value| match &*value.borrow() {
            Value::Number(value) if value.is_finite() && *value >= 0.0 => Some(*value as usize),
            _ => None,
        })
        .collect()
}

fn collect_patch_port_layouts(node: &LayoutNode, ports: &mut Vec<PatchPortLayout>) {
    if node_bool_prop(node, "patch-port") {
        if let Some(direction) = patch_port_direction(node) {
            let track = node_usize_prop(node, "track");
            let dest_kind = node_string_prop(node, "dest-kind").unwrap_or_else(|| "track".into());
            let dest = node_usize_prop(node, "dest").or(track);
            let Some(track_or_dest) = track.or(dest) else {
                for child in &node.children {
                    collect_patch_port_layouts(child, ports);
                }
                return;
            };
            ports.push(PatchPortLayout {
                direction,
                track: track.unwrap_or(track_or_dest),
                dest_kind,
                dest: dest.unwrap_or(track_or_dest),
                input: node_usize_prop(node, "input").unwrap_or(0),
                active: node_bool_prop(node, "active"),
                pending: node_bool_prop(node, "pending"),
                center: (
                    node.rect.col + node.rect.width * 0.5,
                    node.rect.row + node.rect.height * 0.5,
                ),
                connected_sources: node_usize_list_prop(node, "connected-sources"),
                on_cable_click: node.props.get("on-cable-click").cloned(),
                on_patch_drop: node.props.get("on-patch-drop").cloned(),
                on_patch_cancel: node.props.get("on-patch-cancel").cloned(),
                on_patch_miss: node.props.get("on-patch-miss").cloned(),
            });
        }
    }
    for child in &node.children {
        collect_patch_port_layouts(child, ports);
    }
}

fn node_is_patch_port(node: &LayoutNode) -> bool {
    node_bool_prop(node, "patch-port")
}

fn patch_cable_click_output(
    layout: &LayoutNode,
    layout_col: f32,
    layout_row: f32,
    cell_w: f32,
    cell_h: f32,
) -> Option<crate::widget_render::EventOutput> {
    let mut ports = Vec::new();
    collect_patch_port_layouts(layout, &mut ports);
    if ports.is_empty() {
        return None;
    }
    let outputs: std::collections::HashMap<usize, (f32, f32)> = ports
        .iter()
        .filter(|port| port.direction == PatchPortDirection::Out)
        .map(|port| (port.track, (port.center.0 * cell_w, port.center.1 * cell_h)))
        .collect();
    let point_px = (layout_col * cell_w, layout_row * cell_h);

    let mut best: Option<(f32, usize, usize, usize, Value)> = None;
    for port in ports
        .iter()
        .filter(|port| port.direction == PatchPortDirection::In)
    {
        let Some(callback) = port.on_cable_click.clone() else {
            continue;
        };
        for source in &port.connected_sources {
            let Some(start) = outputs.get(source).copied() else {
                continue;
            };
            let end = (port.center.0 * cell_w, port.center.1 * cell_h);
            let distance = distance_to_patch_cable_px(start, end, point_px);
            if distance > 7.0 {
                continue;
            }
            match best {
                Some((best_distance, ..)) if best_distance <= distance => {}
                _ => best = Some((distance, *source, port.dest, port.input, callback.clone())),
            }
        }
    }

    let (_, source, dest, input, callback) = best?;
    Some(crate::widget_render::EventOutput {
        callback,
        args: vec![
            Value::Number(source as f64),
            Value::Number(dest as f64),
            Value::Number(input as f64),
        ],
    })
}

fn patch_miss_output(layout: &LayoutNode) -> Option<crate::widget_render::EventOutput> {
    let mut ports = Vec::new();
    collect_patch_port_layouts(layout, &mut ports);
    let callback = ports.iter().find_map(|port| port.on_patch_miss.clone())?;
    Some(crate::widget_render::EventOutput {
        callback,
        args: Vec::new(),
    })
}

fn patch_cancel_output(layout: &LayoutNode) -> Option<crate::widget_render::EventOutput> {
    let mut ports = Vec::new();
    collect_patch_port_layouts(layout, &mut ports);
    let source = ports
        .iter()
        .find(|port| port.direction == PatchPortDirection::Out && port.active && port.pending)?;
    let callback = source.on_patch_cancel.clone()?;
    Some(crate::widget_render::EventOutput {
        callback,
        args: vec![Value::Number(source.track as f64)],
    })
}

fn patch_drop_output(
    layout: &LayoutNode,
    layout_col: f32,
    layout_row: f32,
) -> Option<crate::widget_render::EventOutput> {
    let mut ports = Vec::new();
    collect_patch_port_layouts(layout, &mut ports);
    let source = ports
        .iter()
        .find(|port| port.direction == PatchPortDirection::Out && port.active && port.pending)?;
    if squared_distance(source.center, (layout_col, layout_row))
        <= PATCH_PORT_CANCEL_RADIUS_CELLS * PATCH_PORT_CANCEL_RADIUS_CELLS
    {
        return patch_cancel_output(layout);
    }
    let Some((dest, distance_sq)) = ports
        .iter()
        .filter(|port| {
            port.direction == PatchPortDirection::In
                && port.active
                && !(port.dest_kind == "track" && port.dest == source.track)
                && port.on_patch_drop.is_some()
        })
        .map(|port| {
            (
                port,
                squared_distance(port.center, (layout_col, layout_row)),
            )
        })
        .min_by(|(_, da), (_, db)| da.partial_cmp(db).unwrap_or(std::cmp::Ordering::Equal))
    else {
        return patch_cancel_output(layout);
    };
    if distance_sq > PATCH_PORT_DROP_RADIUS_CELLS * PATCH_PORT_DROP_RADIUS_CELLS {
        return patch_cancel_output(layout);
    }

    Some(crate::widget_render::EventOutput {
        callback: dest.on_patch_drop.clone()?,
        args: vec![
            Value::Number(source.track as f64),
            Value::Number(dest.dest as f64),
            Value::Number(dest.input as f64),
        ],
    })
}

fn has_pending_patch_drag(layout: &LayoutNode) -> bool {
    let mut ports = Vec::new();
    collect_patch_port_layouts(layout, &mut ports);
    ports
        .iter()
        .any(|port| port.direction == PatchPortDirection::Out && port.active && port.pending)
}

fn distance_to_patch_cable_px(start: (f32, f32), end: (f32, f32), point: (f32, f32)) -> f32 {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let distance = (dx * dx + dy * dy).sqrt();
    let slack = 0.70;
    let sag = ((28.0 + distance * 0.22) * slack).clamp(18.0, 98.0);
    let handle_x = dx.abs().clamp(42.0, 190.0) * (0.30 + 0.14 * slack);
    let direction = if dx >= 0.0 { 1.0 } else { -1.0 };
    let c1 = (start.0 + handle_x * direction, start.1 + sag);
    let c2 = (end.0 - handle_x * direction, end.1 + sag);

    let mut best = f32::MAX;
    let mut prev = start;
    for i in 1..=48 {
        let t = i as f32 / 48.0;
        let current = cubic_bezier_point(start, c1, c2, end, t);
        best = best.min(distance_to_segment(point, prev, current));
        prev = current;
    }
    best
}

fn cubic_bezier_point(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    t: f32,
) -> (f32, f32) {
    let mt = 1.0 - t;
    let a = mt * mt * mt;
    let b = 3.0 * mt * mt * t;
    let c = 3.0 * mt * t * t;
    let d = t * t * t;
    (
        a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
        a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
    )
}

fn distance_to_segment(point: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let ab = (b.0 - a.0, b.1 - a.1);
    let ap = (point.0 - a.0, point.1 - a.1);
    let len_sq = ab.0 * ab.0 + ab.1 * ab.1;
    if len_sq <= f32::EPSILON {
        return ((point.0 - a.0).powi(2) + (point.1 - a.1).powi(2)).sqrt();
    }
    let t = ((ap.0 * ab.0 + ap.1 * ab.1) / len_sq).clamp(0.0, 1.0);
    let closest = (a.0 + ab.0 * t, a.1 + ab.1 * t);
    ((point.0 - closest.0).powi(2) + (point.1 - closest.1).powi(2)).sqrt()
}

fn squared_distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::layout::{LayoutNode, Rect};
    use crate::vm::Value;

    use super::{PatchPortDirection, patch_drop_output};

    fn port(
        direction: PatchPortDirection,
        track: usize,
        input: usize,
        center: (f32, f32),
        pending: bool,
    ) -> LayoutNode {
        let mut props = HashMap::new();
        props.insert("patch-port".to_string(), Value::Bool(true));
        props.insert("active".to_string(), Value::Bool(true));
        props.insert(
            "direction".to_string(),
            Value::String(match direction {
                PatchPortDirection::In => "in".to_string(),
                PatchPortDirection::Out => "out".to_string(),
            }),
        );
        props.insert("track".to_string(), Value::Number(track as f64));
        props.insert("input".to_string(), Value::Number(input as f64));
        if pending {
            props.insert("pending".to_string(), Value::Bool(true));
        }
        match direction {
            PatchPortDirection::In => {
                props.insert(
                    "on-patch-drop".to_string(),
                    Value::String("drop".to_string()),
                );
            }
            PatchPortDirection::Out => {
                props.insert(
                    "on-patch-cancel".to_string(),
                    Value::String("cancel".to_string()),
                );
            }
        }
        LayoutNode {
            widget_id: track as u64 + 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "button".to_string(),
            rect: Rect {
                row: center.1 - 0.5,
                col: center.0 - 0.5,
                width: 1.0,
                height: 1.0,
            },
            props,
            children: Vec::new(),
            focusable: false,
        }
    }

    fn layout(children: Vec<LayoutNode>) -> LayoutNode {
        LayoutNode {
            widget_id: 0,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "root".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 100.0,
                height: 30.0,
            },
            props: HashMap::new(),
            children,
            focusable: false,
        }
    }

    #[test]
    fn patch_drop_far_from_input_cancels_pending_drag() {
        let root = layout(vec![
            port(PatchPortDirection::Out, 0, 0, (2.0, 2.0), true),
            port(PatchPortDirection::In, 1, 0, (20.0, 2.0), false),
        ]);

        let output = patch_drop_output(&root, 9.0, 12.0).expect("cancel output");

        assert!(matches!(output.callback, Value::String(ref value) if value == "cancel"));
        assert!(matches!(output.args.as_slice(), [Value::Number(track)] if *track == 0.0));
    }

    #[test]
    fn patch_drop_near_input_dispatches_drop() {
        let root = layout(vec![
            port(PatchPortDirection::Out, 0, 0, (2.0, 2.0), true),
            port(PatchPortDirection::In, 1, 3, (20.0, 2.0), false),
        ]);

        let output = patch_drop_output(&root, 20.4, 2.2).expect("drop output");

        assert!(matches!(output.callback, Value::String(ref value) if value == "drop"));
        assert!(matches!(
            output.args.as_slice(),
            [Value::Number(source), Value::Number(dest), Value::Number(input)]
                if *source == 0.0 && *dest == 1.0 && *input == 3.0
        ));
    }
}

impl Editor {
    pub(super) fn active_layout_has_pending_patch_drag(&self) -> bool {
        self.runtime
            .current_layout
            .as_ref()
            .is_some_and(|layout| has_pending_patch_drag(layout))
    }

    pub(super) fn handle_active_patch_drag_mouse(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> bool {
        let Some(layout) = self.runtime.current_layout.as_ref() else {
            return false;
        };
        if !has_pending_patch_drag(layout) {
            return false;
        }
        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Moved => {
                self.mark_needs_redraw();
                true
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some((local_col, local_row)) =
                    hit::to_local(precise_col, precise_row, content_col, content_row)
                else {
                    let output = patch_cancel_output(layout);
                    let handled = output.is_some();
                    let _ = self.apply_widget_output(output);
                    self.mark_needs_redraw();
                    return handled;
                };
                let layout_pos = (
                    local_col + self.active_leaf().widget_scroll_left,
                    local_row + self.widget_scroll_top() + self.active_buffer().scroll_top as f32,
                );
                let output = patch_drop_output(layout, layout_pos.0, layout_pos.1);
                let handled = output.is_some();
                let _ = self.apply_widget_output(output);
                self.mark_needs_redraw();
                handled
            }
            _ => false,
        }
    }

    fn dispatch_slider_drag_to_node(
        &mut self,
        node: &LayoutNode,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        start: (f32, f32),
        end: (f32, f32),
    ) {
        self.last_slider_drag_widget_id = Some(node.widget_id);
        let output = self.dispatch_widget_mouse_event(
            node,
            mouse.kind,
            content_col,
            content_row,
            end.0,
            end.1,
            Some(start),
            None,
            mouse.modifiers,
        );
        let _ = self.apply_widget_output(output);
    }

    fn dispatch_slider_drag_to_last(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        start: (f32, f32),
        end: (f32, f32),
    ) -> bool {
        let Some(widget_id) = self.last_slider_drag_widget_id else {
            return false;
        };
        let Some(layout) = self.runtime.current_layout.clone() else {
            return false;
        };
        let Some(node) = find_node_by_id(&layout, widget_id) else {
            self.last_slider_drag_widget_id = None;
            return false;
        };
        if !is_slider_widget(&node) {
            self.last_slider_drag_widget_id = None;
            return false;
        }
        self.dispatch_slider_drag_to_node(&node, mouse, content_col, content_row, start, end);
        true
    }

    pub(super) fn update_widget_cursor(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) {
        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            self.widget_cursor = WidgetCursor::Default;
            return;
        };
        let Some(node) = self.widget_node_at_local(local_col, local_row) else {
            self.widget_cursor = WidgetCursor::Default;
            return;
        };
        let scrolled_col = local_col + self.active_leaf().widget_scroll_left;
        let scrolled_row = local_row + self.total_scroll_top();
        self.widget_cursor = widget_render::cursor_for_node(&node, scrolled_col, scrolled_row);
    }

    pub(super) fn try_handle_widget_mouse_precise(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> bool {
        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            return false;
        };
        // Overlay (dropdown menu, etc.) intercepts pointer events before normal
        // hit-test. Do not gate this on overlay_contains: overlay geometry is
        // visual/screen-space and may extend over widgets that should not see
        // the same click sequence.
        if widget_render::overlay_widget_id().is_some()
            && matches!(
                mouse.kind,
                MouseEventKind::Down(MouseButton::Left)
                    | MouseEventKind::Drag(MouseButton::Left)
                    | MouseEventKind::Up(MouseButton::Left)
            )
        {
            if let Some(overlay_id) = widget_render::overlay_widget_id() {
                let Some(layout) = self.runtime.current_layout.clone() else {
                    widget_render::dropdown::close_dropdown(overlay_id);
                    widget_render::clear_overlay();
                    self.mark_needs_redraw();
                    return false;
                };
                if let Some(node) = super::widget_focus::find_node_by_id(&layout, overlay_id) {
                    let (cell_w, cell_h) = self.runtime.layout_cell_dims();
                    let widget_event = map_mouse_event(
                        &node,
                        mouse.kind,
                        local_col,
                        local_row,
                        None,
                        None,
                        mouse.modifiers,
                        cell_w,
                        cell_h,
                    );
                    let output = match widget_event {
                        MouseEventOutcome::Ignore | MouseEventOutcome::Consume => None,
                        MouseEventOutcome::Dispatch(widget_event) => {
                            handle_event(&node, widget_event)
                        }
                    };
                    let _ = self.apply_widget_output(output);
                    return true;
                }
                widget_render::dropdown::close_dropdown(overlay_id);
                widget_render::clear_overlay();
                self.mark_needs_redraw();
            }
        }

        let gen_before = widget_render::widget_state_generation();
        if matches!(
            mouse.kind,
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
        ) {
            if let Some(layout) = self.runtime.current_layout.as_ref() {
                let layout_pos = (
                    local_col + self.active_leaf().widget_scroll_left,
                    local_row + self.widget_scroll_top() + self.active_buffer().scroll_top as f32,
                );
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        let hit_patch_port = hit_test_layout(layout, layout_pos.1, layout_pos.0)
                            .is_some_and(node_is_patch_port);
                        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
                        if !hit_patch_port
                            && let Some(output) = patch_cable_click_output(
                                layout,
                                layout_pos.0,
                                layout_pos.1,
                                cell_w,
                                cell_h,
                            )
                        {
                            return self.apply_widget_output(Some(output));
                        }
                        if let Some(output) = patch_miss_output(layout) {
                            let _ = self.apply_widget_output(Some(output));
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        if let Some(output) = patch_drop_output(layout, layout_pos.0, layout_pos.1)
                        {
                            return self.apply_widget_output(Some(output));
                        }
                    }
                    _ => {}
                }
            }
        }
        let output = {
            let Some(node) = self.widget_node_at_local(local_col, local_row) else {
                return false;
            };
            let node = self
                .runtime
                .current_layout
                .as_ref()
                .map(|layout| pointer_dispatch_node(layout, node.clone()))
                .unwrap_or(node);

            self.dispatch_widget_mouse_event(
                &node,
                mouse.kind,
                content_col,
                content_row,
                precise_col,
                precise_row,
                None,
                None,
                mouse.modifiers,
            )
        };
        if self.apply_widget_output(output) {
            true
        } else if widget_render::widget_state_generation() != gen_before {
            self.runtime.invalidate_layout_deferred();
            self.mark_needs_redraw();
            true
        } else if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp
                | MouseEventKind::ScrollDown
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight
        ) && self
            .widget_node_at_local(local_col, local_row)
            .is_some_and(|node| captures_scroll_gesture(&node))
        {
            true
        } else if matches!(mouse.kind, MouseEventKind::Down(_) | MouseEventKind::Moved) {
            let has_widget = self.widget_node_at_local(local_col, local_row).is_some();
            // Only invalidate layout if widget state actually changed
            // (e.g. tree expand/collapse bumps the generation counter).
            if has_widget && widget_render::widget_state_generation() != gen_before {
                self.runtime.invalidate_layout();
                self.mark_needs_redraw();
            }
            has_widget
        } else {
            false
        }
    }

    /// Update SDF widget hover/pressed state and redraw if changed.
    pub(super) fn update_sdf_hover(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
        pressed: bool,
    ) {
        use crate::widget_render::sdf_widget::{self, SdfHitState};

        // Update time for SDF hit testing (once per event, not per hit test)
        sdf_widget::set_sdf_time_seconds(sdf_widget::current_sdf_time_fallback_seconds());

        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            if sdf_widget::clear_sdf_hit_states_except(None) {
                self.mark_needs_redraw();
            }
            return;
        };
        let Some(node) = self.widget_node_at_local(local_col, local_row) else {
            if sdf_widget::clear_sdf_hit_states_except(None) {
                self.mark_needs_redraw();
            }
            return;
        };
        let background_sdf = if node.widget_type == "box" {
            node.props
                .get("background")
                .and_then(|value| match value {
                    Value::String(name) => Some(name.as_str()),
                    _ => None,
                })
                .filter(|name| sdf_widget::sdf_widget_def(name).is_some())
                .is_some()
        } else {
            false
        };
        let direct_sdf = sdf_widget::sdf_widget_def(&node.widget_type).is_some();
        if !direct_sdf && !background_sdf {
            if sdf_widget::clear_sdf_hit_states_except(None) {
                self.mark_needs_redraw();
            }
            return;
        }
        if sdf_widget::clear_sdf_hit_states_except(Some(node.widget_id)) {
            self.mark_needs_redraw();
        }

        let widget_col = local_col + self.active_leaf().widget_scroll_left - node.rect.col;
        let widget_row = local_row + self.total_scroll_top() - node.rect.row;
        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
        let px_w = node.rect.width * cell_w;
        let px_h = node.rect.height * cell_h;
        let pixel_aspect = if px_h > 0.0 { px_w / px_h } else { 1.0 };

        let region = if direct_sdf {
            sdf_widget::sdf_widget_hit_test(&node, widget_col, widget_row, pixel_aspect)
        } else {
            0
        };

        let old = sdf_widget::get_sdf_hit_state(node.widget_id);
        if old.hit_region != region || old.hit_pressed != pressed {
            sdf_widget::set_sdf_hit_state(
                node.widget_id,
                SdfHitState {
                    hit_region: region,
                    hit_pressed: pressed,
                },
            );
            self.mark_needs_redraw();
        }
    }

    pub(super) fn update_sdf_hover_for_inactive_tile(
        &mut self,
        tile_id: crate::tile::TileId,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) {
        use crate::widget_render::sdf_widget::{self, SdfHitState};

        sdf_widget::set_sdf_time_seconds(sdf_widget::current_sdf_time_fallback_seconds());

        let Some(leaf) = self.tile_root.find_leaf(tile_id) else {
            return;
        };
        let Some(layout) = leaf.cached_layout.clone() else {
            if sdf_widget::clear_sdf_hit_states_except(None) {
                self.mark_needs_redraw();
            }
            return;
        };
        let buffer = &self.buffers[leaf.buffer_idx];
        if buffer.view_mode == super::ViewMode::TextOnly {
            if sdf_widget::clear_sdf_hit_states_except(None) {
                self.mark_needs_redraw();
            }
            return;
        }

        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            if sdf_widget::clear_sdf_hit_states_except(None) {
                self.mark_needs_redraw();
            }
            return;
        };

        let text_scroll = if buffer.view_mode == super::ViewMode::UiOnly {
            0.0
        } else {
            buffer.scroll_top as f32
        };
        let layout_col = local_col + leaf.widget_scroll_left;
        let layout_row = local_row + leaf.widget_scroll_top + text_scroll;

        let Some(node) = hit_test_layout(&layout, layout_row, layout_col).cloned() else {
            if sdf_widget::clear_sdf_hit_states_except(None) {
                self.mark_needs_redraw();
            }
            return;
        };

        let background_sdf = if node.widget_type == "box" {
            node.props
                .get("background")
                .and_then(|value| match value {
                    Value::String(name) => Some(name.as_str()),
                    _ => None,
                })
                .filter(|name| sdf_widget::sdf_widget_def(name).is_some())
                .is_some()
        } else {
            false
        };
        let direct_sdf = sdf_widget::sdf_widget_def(&node.widget_type).is_some();
        if !direct_sdf && !background_sdf {
            if sdf_widget::clear_sdf_hit_states_except(None) {
                self.mark_needs_redraw();
            }
            return;
        }
        if sdf_widget::clear_sdf_hit_states_except(Some(node.widget_id)) {
            self.mark_needs_redraw();
        }

        let widget_col = layout_col - node.rect.col;
        let widget_row = layout_row - node.rect.row;
        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
        let px_w = node.rect.width * cell_w;
        let px_h = node.rect.height * cell_h;
        let pixel_aspect = if px_h > 0.0 { px_w / px_h } else { 1.0 };
        let region = if direct_sdf {
            sdf_widget::sdf_widget_hit_test(&node, widget_col, widget_row, pixel_aspect)
        } else {
            0
        };

        let old = sdf_widget::get_sdf_hit_state(node.widget_id);
        if old.hit_region != region || old.hit_pressed {
            sdf_widget::set_sdf_hit_state(
                node.widget_id,
                SdfHitState {
                    hit_region: region,
                    hit_pressed: false,
                },
            );
            self.mark_needs_redraw();
        }
    }

    pub(super) fn try_handle_widget_double_click(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> bool {
        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            return false;
        };
        let Some(node) = self.widget_node_at_local(local_col, local_row) else {
            return false;
        };
        if !self.is_double_click_candidate(node.widget_id, precise_col, precise_row) {
            return false;
        }
        let scrolled_col = local_col + self.active_leaf().widget_scroll_left;
        let scrolled_row = local_row + self.total_scroll_top();
        let event_scroll_offset = self
            .runtime
            .current_layout
            .as_ref()
            .and_then(|layout| find_scroll_ancestor(layout, node.widget_id))
            .map(|scroll_node| {
                crate::widget_render::scroll::get_scroll_state(
                    crate::widget_render::scroll::scroll_state_key(&scroll_node),
                )
                .offset_y
            });
        crate::widget_render::scroll::set_current_event_scroll_offset(event_scroll_offset);
        if let Some(widget_event) = map_double_click_event(&node, scrolled_col, scrolled_row) {
            crate::widget_render::scroll::set_current_event_scroll_offset(None);
            let output = handle_event(&node, widget_event);
            let _ = self.apply_widget_output(output);
            self.runtime.invalidate_layout_deferred();
            self.mark_needs_redraw();
            return true;
        }
        let Some(double_click_node) = self
            .runtime
            .current_layout
            .as_ref()
            .and_then(|layout| deepest_double_click_node(layout, scrolled_row, scrolled_col))
        else {
            crate::widget_render::scroll::set_current_event_scroll_offset(None);
            return false;
        };
        let Some(widget_event) =
            map_double_click_event(&double_click_node, scrolled_col, scrolled_row)
        else {
            crate::widget_render::scroll::set_current_event_scroll_offset(None);
            return false;
        };
        crate::widget_render::scroll::set_current_event_scroll_offset(None);
        let output = handle_event(&double_click_node, widget_event);
        let _ = self.apply_widget_output(output);
        self.runtime.invalidate_layout_deferred();
        self.mark_needs_redraw();
        true
    }

    fn is_double_click_candidate(
        &self,
        widget_id: u64,
        precise_col: f32,
        precise_row: f32,
    ) -> bool {
        const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(350);
        const DOUBLE_CLICK_SLOP: f32 = 1.5;
        let probe_enabled = std::env::var_os("ESEQLISP_DOUBLE_CLICK_PROBE").is_some();
        let Some(click) = self.active_leaf().last_widget_click.as_ref() else {
            if probe_enabled {
                eprintln!("double-click probe: no previous widget click");
            }
            return false;
        };
        let elapsed = click.at.elapsed();
        let delta_col = (click.precise_col - precise_col).abs();
        let delta_row = (click.precise_row - precise_row).abs();
        let same_widget = click.widget_id == widget_id;
        let within_window = elapsed <= DOUBLE_CLICK_WINDOW;
        let within_slop = delta_col <= DOUBLE_CLICK_SLOP && delta_row <= DOUBLE_CLICK_SLOP;
        let candidate = same_widget && within_window && within_slop;
        if probe_enabled {
            eprintln!(
                "double-click probe: elapsed_ms={} same_widget={} delta_col={:.3} delta_row={:.3} within_window={} within_slop={} candidate={}",
                elapsed.as_millis(),
                same_widget,
                delta_col,
                delta_row,
                within_window,
                within_slop,
                candidate,
            );
        }
        candidate
    }

    pub(super) fn remember_widget_click(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) {
        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            self.active_leaf_mut().last_widget_click = None;
            return;
        };
        let click = self
            .widget_node_at_local(local_col, local_row)
            .map(|node| WidgetClick {
                widget_id: node.widget_id,
                precise_col,
                precise_row,
                at: std::time::Instant::now(),
            });
        self.active_leaf_mut().last_widget_click = click;
    }

    pub(super) fn begin_widget_gesture(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) {
        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            return;
        };
        let Some(hit_node) = self.widget_node_at_local(local_col, local_row) else {
            return;
        };
        let scrolled_col = local_col + self.active_leaf().widget_scroll_left;
        let scrolled_row = local_row + self.total_scroll_top();
        let event_scroll_offset = self
            .runtime
            .current_layout
            .as_ref()
            .and_then(|layout| find_scroll_ancestor(layout, hit_node.widget_id))
            .map(|scroll_node| {
                crate::widget_render::scroll::get_scroll_state(
                    crate::widget_render::scroll::scroll_state_key(&scroll_node),
                )
                .offset_y
            });
        crate::widget_render::scroll::set_current_event_scroll_offset(event_scroll_offset);
        let gesture_node = self.runtime.current_layout.as_ref().and_then(|layout| {
            nearest_widget_gesture_node(layout, &hit_node, scrolled_col, scrolled_row)
        });
        crate::widget_render::scroll::set_current_event_scroll_offset(None);
        if let Some((node, gesture_data)) = gesture_node {
            self.active_leaf_mut().active_widget_gesture = Some(WidgetGesture {
                widget_id: node.widget_id,
                node,
                start_precise_col: precise_col,
                start_precise_row: precise_row,
                drag_active: false,
                gesture_data,
            });
        }
    }

    pub(super) fn try_handle_widget_drag_segment(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        start: (f32, f32),
        end: (f32, f32),
    ) {
        let start_local = (start.0 - content_col as f32, start.1 - content_row as f32);
        let end_local = (end.0 - content_col as f32, end.1 - content_row as f32);
        let start_node = self.widget_node_at_local(start_local.0, start_local.1);
        let end_node = self.widget_node_at_local(end_local.0, end_local.1);
        let layout = self.runtime.current_layout.clone();
        let start_node = start_node.map(|node| {
            layout
                .as_ref()
                .map(|layout| pointer_dispatch_node(layout, node.clone()))
                .unwrap_or(node)
        });
        let end_node = end_node.map(|node| {
            layout
                .as_ref()
                .map(|layout| pointer_dispatch_node(layout, node.clone()))
                .unwrap_or(node)
        });
        let allow_slider_drag = self.pointer_drag_started_on_slider;

        if let Some(node) = start_node.as_ref()
            && widget_render::widget_captures_drag(&node.widget_type)
        {
            let gen_before = widget_render::widget_state_generation();
            let (drag_col, drag_row) = if widget_render::widget_unclamped_drag(&node.widget_type) {
                // Pass raw mouse position — widget handles value clamping itself
                (end.0, end.1)
            } else {
                // Clamp drag to widget bounds in terminal-cell screen space
                let scroll = self.total_scroll_top();
                let screen_row = node.rect.row - scroll;
                let screen_height = node.rect.height;
                (
                    end.0.clamp(
                        content_col as f32 + node.rect.col,
                        content_col as f32 + node.rect.col + (node.rect.width - 1.0).max(0.0),
                    ),
                    end.1.clamp(
                        content_row as f32 + screen_row,
                        content_row as f32 + screen_row + (screen_height - 1.0).max(0.0),
                    ),
                )
            };
            let output = self.dispatch_widget_mouse_event(
                node,
                mouse.kind,
                content_col,
                content_row,
                drag_col,
                drag_row,
                Some(start),
                None,
                mouse.modifiers,
            );
            let _ = self.apply_widget_output(output);
            if widget_render::widget_state_generation() != gen_before {
                self.mark_needs_redraw();
            }
            return;
        }

        if allow_slider_drag {
            let steps = ((end.0 - start.0).abs().max((end.1 - start.1).abs()) * 2.0)
                .ceil()
                .max(1.0) as usize;
            let mut last_hit_slider_id: Option<u64> = None;
            let mut target_slider: Option<LayoutNode> = None;
            for step in 0..=steps {
                let t = step as f32 / steps as f32;
                let col = start.0 + (end.0 - start.0) * t;
                let row = start.1 + (end.1 - start.1) * t;
                let local_col = col - content_col as f32;
                let local_row = row - content_row as f32;
                let Some(node) = self.widget_node_at_local(local_col, local_row) else {
                    continue;
                };
                if !is_slider_widget(&node) || Some(node.widget_id) == last_hit_slider_id {
                    continue;
                }
                if step < steps {
                    self.dispatch_slider_drag_to_node(
                        &node,
                        mouse,
                        content_col,
                        content_row,
                        start,
                        (col, row),
                    );
                }
                last_hit_slider_id = Some(node.widget_id);
                target_slider = Some(node);
            }

            if let Some(node) = target_slider {
                self.dispatch_slider_drag_to_node(
                    &node,
                    mouse,
                    content_col,
                    content_row,
                    start,
                    end,
                );
            } else {
                let _ =
                    self.dispatch_slider_drag_to_last(mouse, content_col, content_row, start, end);
            }
            return;
        }

        if !allow_slider_drag
            && (start_node.as_ref().is_some_and(is_slider_widget)
                || end_node.as_ref().is_some_and(is_slider_widget))
        {
            return;
        }

        if HitGrid::same_hit(start_node.as_ref(), end_node.as_ref()) {
            let _ =
                self.try_handle_widget_mouse_precise(mouse, content_col, content_row, end.0, end.1);
            return;
        }

        let steps = ((end.0 - start.0).abs().max((end.1 - start.1).abs()) * 2.0)
            .ceil()
            .max(1.0) as usize;
        let mut last_hit: Option<LayoutNode> = None;
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let col = start.0 + (end.0 - start.0) * t;
            let row = start.1 + (end.1 - start.1) * t;
            let local_col = col - content_col as f32;
            let local_row = row - content_row as f32;
            let node = self.widget_node_at_local(local_col, local_row);
            if !allow_slider_drag && node.as_ref().is_some_and(is_slider_widget) {
                last_hit = node;
                continue;
            }
            if node.is_some() && !HitGrid::same_hit(node.as_ref(), last_hit.as_ref()) {
                let _ =
                    self.try_handle_widget_mouse_precise(mouse, content_col, content_row, col, row);
            }
            last_hit = node;
        }
    }

    /// Hit-test the widget layout tree using f32 coordinates in layout row/col units.
    /// Takes local terminal-cell coords (relative to content area), adds scroll,
    /// and does a precise rect-contains walk.
    pub(super) fn widget_node_at_local(
        &mut self,
        local_col: f32,
        local_row: f32,
    ) -> Option<LayoutNode> {
        let layout = self.runtime.current_layout.as_ref()?;
        let widget_scroll = self.widget_scroll_top();
        let text_scroll = self.active_buffer().scroll_top as f32;
        let hscroll = self.active_leaf().widget_scroll_left;

        let layout_col = local_col + hscroll;
        let layout_row = local_row + widget_scroll + text_scroll;

        hit_test_layout(layout, layout_row, layout_col).cloned()
    }

    pub(super) fn widget_node_at_screen(
        &mut self,
        precise_col: f32,
        precise_row: f32,
        content_col: u16,
        content_row: u16,
    ) -> Option<LayoutNode> {
        let (local_col, local_row) =
            hit::to_local(precise_col, precise_row, content_col, content_row)?;
        self.widget_node_at_local(local_col, local_row)
    }

    pub(super) fn handle_text_click(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        content_width: u16,
        content_height: u16,
        precise_col: f32,
        precise_row: f32,
    ) {
        let Some(cursor) = self.text_cursor_from_mouse(
            mouse,
            content_col,
            content_row,
            content_width,
            content_height,
            precise_col,
            precise_row,
        ) else {
            return;
        };

        if self.active_buffer().read_only {
            return; // keep widget focus in read-only buffers
        }
        let previous_cursor = self.active_buffer().cursor;
        let buffer_id = self.active_buffer().id;
        self.clear_mark();
        self.active_text_drag_anchor = Some(crate::editor::Mark { buffer_id, cursor });
        self.active_buffer_mut().cursor = cursor;
        if cursor != previous_cursor {
            self.exit_search_mode_if_active();
        }
        let leaf = self.active_leaf_mut();
        leaf.focused_widget_id = None;
        leaf.focused_widget_node = None;
        leaf.active_widget_gesture = None;
        self.completion = None;
        self.minibuffer = None;
        self.sync_runtime_context();
        self.mark_needs_redraw();
    }

    pub(super) fn handle_text_drag(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        content_width: u16,
        content_height: u16,
        precise_col: f32,
        precise_row: f32,
    ) {
        let Some(anchor) = self.active_text_drag_anchor else {
            return;
        };
        if anchor.buffer_id != self.active_buffer().id {
            self.clear_text_drag_anchor();
            return;
        }
        let Some(cursor) = self.text_cursor_from_mouse(
            mouse,
            content_col,
            content_row,
            content_width,
            content_height,
            precise_col,
            precise_row,
        ) else {
            return;
        };

        let previous_cursor = self.active_buffer().cursor;
        self.active_buffer_mut().cursor = cursor;
        if cursor != previous_cursor {
            self.exit_search_mode_if_active();
        }
        self.mark = Some(anchor);
        self.completion = None;
        self.minibuffer = None;
        self.sync_runtime_context();
        self.mark_needs_redraw();
    }

    pub(super) fn finish_text_drag(&mut self) {
        let Some(anchor) = self.active_text_drag_anchor else {
            return;
        };
        self.clear_text_drag_anchor();
        if anchor.buffer_id != self.active_buffer().id
            || self.active_buffer().cursor == anchor.cursor
        {
            self.clear_mark();
        } else {
            self.mark = Some(anchor);
        }
        self.mark_needs_redraw();
    }

    fn text_cursor_from_mouse(
        &self,
        _mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        content_width: u16,
        content_height: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> Option<(usize, usize)> {
        let buffer = self.active_buffer();
        let (text_cell_width_scale, text_cell_height_scale) =
            self.text_cell_scales_for_buffer(buffer);
        let local_col_f = (precise_col - content_col as f32) / text_cell_width_scale;
        let local_row_f = (precise_row - content_row as f32) / text_cell_height_scale;
        if local_col_f < 0.0 || local_row_f < 0.0 {
            return None;
        }
        let local_col = local_col_f.floor() as u16;
        let local_row = local_row_f.floor() as u16;
        let text_content_width = (content_width as f32 / text_cell_width_scale)
            .floor()
            .max(1.0) as u16;
        let text_content_height = (content_height as f32 / text_cell_height_scale)
            .floor()
            .max(1.0) as u16;
        if local_col >= text_content_width || local_row >= text_content_height {
            return None;
        }

        let scroll_left = if buffer.view_mode != crate::editor::ViewMode::UiOnly {
            self.active_leaf().widget_scroll_left.floor() as usize
        } else {
            0
        };
        let absolute_row = buffer
            .scroll_top
            .saturating_add(local_row as usize)
            .min(buffer.lines.len().saturating_sub(1));
        let absolute_col =
            (local_col as usize + scroll_left).min(buffer.lines[absolute_row].chars().count());
        Some((absolute_row, absolute_col))
    }

    pub(super) fn dispatch_widget_mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
        drag_start: Option<(f32, f32)>,
        explicit_gesture: Option<&Value>,
        modifiers: KeyModifiers,
    ) -> Option<crate::widget_render::EventOutput> {
        let total_scroll_top = self.total_scroll_top();
        let total_scroll_left = self.active_leaf().widget_scroll_left;
        let local_col = precise_col - content_col as f32 + total_scroll_left;
        let local_row = precise_row - content_row as f32 + total_scroll_top;
        let event_scroll_offset = self
            .runtime
            .current_layout
            .as_ref()
            .and_then(|layout| find_scroll_ancestor(layout, node.widget_id))
            .map(|scroll_node| {
                crate::widget_render::scroll::get_scroll_state(
                    crate::widget_render::scroll::scroll_state_key(&scroll_node),
                )
                .offset_y
            });
        let drag_start = drag_start.map(|(start_col, start_row)| {
            (
                start_col - content_col as f32 + total_scroll_left,
                start_row - content_row as f32 + total_scroll_top,
            )
        });
        let leaf = self.active_leaf();
        let gesture = leaf
            .active_widget_gesture
            .as_ref()
            .and_then(|gesture| (gesture.widget_id == node.widget_id).then_some(gesture))
            .and_then(|gesture| gesture.gesture_data.as_ref())
            .or(explicit_gesture);
        crate::widget_render::scroll::set_current_event_scroll_offset(event_scroll_offset);
        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
        let outcome = map_mouse_event(
            node, mouse_kind, local_col, local_row, drag_start, gesture, modifiers, cell_w, cell_h,
        );
        crate::widget_render::scroll::set_current_event_scroll_offset(None);
        match outcome {
            MouseEventOutcome::Ignore | MouseEventOutcome::Consume => None,
            MouseEventOutcome::Dispatch(widget_event) => handle_event(node, widget_event),
        }
    }

    pub(super) fn dispatch_gesture_widget_mouse_event(
        &self,
        gesture: WidgetGesture,
        mouse_kind: MouseEventKind,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
        modifiers: KeyModifiers,
    ) -> Option<crate::widget_render::EventOutput> {
        let node = gesture.node;
        self.dispatch_widget_mouse_event(
            &node,
            mouse_kind,
            content_col,
            content_row,
            precise_col,
            precise_row,
            Some((gesture.start_precise_col, gesture.start_precise_row)),
            gesture.gesture_data.as_ref(),
            modifiers,
        )
    }

    pub(super) fn dispatch_widget_drop_event(
        &self,
        gesture: &WidgetGesture,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> Option<crate::widget_render::EventOutput> {
        let (drag_type, payload) = active_widget_drag(gesture)?;
        let layout = self.runtime.current_layout.as_ref()?;
        let local_col = precise_col - content_col as f32 + self.active_leaf().widget_scroll_left;
        let local_row = precise_row - content_row as f32 + self.total_scroll_top();
        let (target, hit_row, hit_col) =
            deepest_drop_target(layout, local_row, local_col, &drag_type)?;
        let callback = target.props.get("on-drop")?.clone();
        eprintln!(
            "widget-drop: dispatch drag_type={drag_type}; target_type={}; target_key={:?}; payload={payload:?}",
            target.widget_type, target.stable_key
        );
        let event = if target.widget_type == "tree" {
            crate::widget_render::tree::tree_drop_info(
                &target, &drag_type, payload, hit_col, hit_row,
            )
        } else {
            crate::widget_render::box_widget::box_drop_info(
                &target, &drag_type, payload, hit_col, hit_row,
            )
        };
        Some(crate::widget_render::EventOutput {
            callback,
            args: vec![event],
        })
    }

    pub(super) fn active_widget_drag_gesture(&self) -> Option<WidgetGesture> {
        let gesture = self.active_leaf().active_widget_gesture.clone()?;
        active_widget_drag(&gesture)?;
        Some(gesture)
    }

    pub(super) fn active_widget_drag_gesture_for_drag(
        &mut self,
        precise_col: f32,
        precise_row: f32,
    ) -> Option<WidgetGesture> {
        let gesture = self.active_widget_drag_gesture()?;
        if !gesture.drag_active
            && !widget_drag_threshold_reached(&gesture, precise_col, precise_row)
        {
            return None;
        }
        if !gesture.drag_active
            && let Some(active) = self.active_leaf_mut().active_widget_gesture.as_mut()
            && active.widget_id == gesture.widget_id
        {
            active.drag_active = true;
        }
        self.active_widget_drag_gesture()
    }

    pub(super) fn active_widget_drag_gesture_for_drop(&self) -> Option<WidgetGesture> {
        let gesture = self.active_widget_drag_gesture()?;
        gesture.drag_active.then_some(gesture)
    }

    pub(super) fn update_widget_drop_hover(
        &mut self,
        gesture: &WidgetGesture,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> bool {
        let Some((drag_type, _)) = active_widget_drag(gesture) else {
            widget_render::set_drop_hover_target(None);
            self.widget_cursor = WidgetCursor::Default;
            return false;
        };
        let target = self.runtime.current_layout.as_ref().and_then(|layout| {
            let local_col =
                precise_col - content_col as f32 + self.active_leaf().widget_scroll_left;
            let local_row = precise_row - content_row as f32 + self.total_scroll_top();
            deepest_drop_target(layout, local_row, local_col, &drag_type)
                .map(|(target, _, _)| target)
        });
        let allowed = target.is_some();
        widget_render::set_drop_hover_target(target.map(|node| node.widget_id));
        self.widget_cursor = if allowed {
            WidgetCursor::DragCopy
        } else {
            WidgetCursor::DragNotAllowed
        };
        self.mark_needs_redraw();
        allowed
    }

    pub(super) fn handle_touchpad_magnify_impl(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
        delta: f64,
    ) {
        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            return;
        };
        let Some(node) = self.widget_node_at_local(local_col, local_row) else {
            return;
        };
        let scrolled_col = local_col + self.active_leaf().widget_scroll_left;
        let scrolled_row = local_row + self.total_scroll_top();
        let gen_before = widget_render::widget_state_generation();
        let Some(widget_event) = map_magnify_event(&node, scrolled_col, scrolled_row, delta) else {
            return;
        };
        let output = handle_event(&node, widget_event);
        if !self.apply_widget_output(output) {
            self.mark_needs_redraw();
        }
        if widget_render::widget_state_generation() != gen_before {
            self.runtime.invalidate_layout_deferred();
            self.mark_needs_redraw();
        }
    }

    pub(super) fn handle_touchpad_scroll_impl(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
        delta_x: f32,
        delta_y: f32,
    ) -> bool {
        // If a dropdown overlay is open, intercept scroll events for it
        if let Some(overlay_id) = widget_render::overlay_widget_id() {
            if widget_render::dropdown::scroll_overlay(overlay_id, delta_y) {
                self.mark_needs_redraw();
                return true;
            }
        }

        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            return false;
        };
        let Some(node) = self.widget_node_at_local(local_col, local_row) else {
            return false;
        };
        let scrolled_col = local_col + self.active_leaf().widget_scroll_left;
        let scrolled_row = local_row + self.total_scroll_top();

        // Try the leaf widget first
        if let Some(widget_event) =
            map_scroll_gesture_event(&node, scrolled_col, scrolled_row, delta_x, delta_y)
        {
            let gen_before = widget_render::widget_state_generation();
            let output = handle_event(&node, widget_event);
            if !self.apply_widget_output(output) {
                // Scroll widgets update internal state without a Lisp callback,
                // so we still need to redraw even when there's no EventOutput.
                self.mark_needs_redraw();
            }
            if node.widget_type == "scroll"
                || widget_render::widget_state_generation() != gen_before
            {
                self.runtime.invalidate_layout_deferred();
                self.mark_needs_redraw();
            }
            return true;
        }
        if captures_scroll_gesture(&node) {
            return true;
        }

        // Leaf doesn't capture scroll — walk up to find a scroll container ancestor
        if let Some(layout) = self.runtime.current_layout.as_ref() {
            if let Some(scroll_node) = find_scroll_ancestor(layout, node.widget_id) {
                if let Some(widget_event) = map_scroll_gesture_event(
                    &scroll_node,
                    scrolled_col,
                    scrolled_row,
                    delta_x,
                    delta_y,
                ) {
                    let gen_before = widget_render::widget_state_generation();
                    let output = handle_event(&scroll_node, widget_event);
                    if !self.apply_widget_output(output) {
                        self.mark_needs_redraw();
                    }
                    if scroll_node.widget_type == "scroll"
                        || widget_render::widget_state_generation() != gen_before
                    {
                        self.runtime.invalidate_layout_deferred();
                        self.mark_needs_redraw();
                    }
                    return true;
                }
            }
        }

        false
    }
}

/// Walk the layout tree to find the nearest "scroll" ancestor of the widget with the given ID.
fn find_scroll_ancestor(node: &LayoutNode, target_id: u64) -> Option<LayoutNode> {
    find_scroll_ancestor_impl(node, target_id, None)
}

fn find_scroll_ancestor_impl(
    node: &LayoutNode,
    target_id: u64,
    current_scroll: Option<&LayoutNode>,
) -> Option<LayoutNode> {
    if node.widget_id == target_id {
        return current_scroll.cloned();
    }
    let next_scroll = if node.widget_type == "scroll" {
        Some(node)
    } else {
        current_scroll
    };
    for child in &node.children {
        if let Some(found) = find_scroll_ancestor_impl(child, target_id, next_scroll) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod pointer_capture_tests {
    use std::collections::HashMap;

    use crate::layout::Rect;

    use super::*;

    fn test_node(
        widget_id: u64,
        widget_type: &str,
        props: HashMap<String, Value>,
        children: Vec<LayoutNode>,
    ) -> LayoutNode {
        LayoutNode {
            widget_id,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: widget_type.to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 10.0,
                height: 10.0,
            },
            props,
            children,
            focusable: false,
        }
    }

    fn props(entries: &[(&str, Value)]) -> HashMap<String, Value> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn pointer_dispatch_uses_nearest_capture_ancestor() {
        let hit = test_node(
            4,
            "knob-number",
            props(&[("bind", Value::String("param".to_string()))]),
            vec![],
        );
        let inner_capture = test_node(
            3,
            "box",
            props(&[("capture-pointer", Value::Bool(true))]),
            vec![hit.clone()],
        );
        let outer_capture = test_node(
            2,
            "box",
            props(&[("capture-pointer", Value::Bool(true))]),
            vec![inner_capture],
        );
        let root = test_node(1, "v-stack", HashMap::new(), vec![outer_capture]);

        let dispatch = pointer_dispatch_node(&root, hit);

        assert_eq!(
            dispatch.widget_id, 3,
            "the closest capture wrapper should receive the pointer event"
        );
    }

    #[test]
    fn gesture_node_uses_capture_wrapper_even_when_child_handles_pointer() {
        let hit = test_node(
            3,
            "knob-number",
            props(&[("bind", Value::String("param".to_string()))]),
            vec![],
        );
        let capture = test_node(
            2,
            "box",
            props(&[
                ("capture-pointer", Value::Bool(true)),
                ("on-click", Value::Symbol("bind-target".to_string())),
            ]),
            vec![hit.clone()],
        );
        let root = test_node(1, "v-stack", HashMap::new(), vec![capture]);

        let Some((gesture_node, gesture_data)) = nearest_widget_gesture_node(&root, &hit, 1.0, 1.0)
        else {
            panic!("capture wrapper should create a gesture");
        };

        assert_eq!(gesture_node.widget_id, 2);
        assert!(
            matches!(gesture_data, Some(Value::String(value)) if value == "capture-pointer"),
            "capture wrapper should prevent child drag handlers from taking over"
        );
    }
}
