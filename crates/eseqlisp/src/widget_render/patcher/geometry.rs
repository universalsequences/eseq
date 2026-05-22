use std::collections::HashMap;

use crate::layout::Rect;

use super::display::node_size_for_ports;
use super::metrics::{
    CABLE_HANDLE_DISTANCE_CELLS, CABLE_HANDLE_HIT_RADIUS_CELLS, CABLE_HIT_RADIUS_CELLS,
    CABLE_TARGET_RADIUS_CELLS, MIN_ZOOM, NODE_HEIGHT, PATCH_ORIGIN_COL_OFFSET,
    PATCH_ORIGIN_ROW_OFFSET, PORT_EDGE_PADDING_CELLS, PORT_OUTER_DIAMETER_PX,
    SEGMENTED_CABLE_CORNER_RADIUS_CELLS, VIEW_PADDING_X, VIEW_PADDING_Y,
};
use super::model::{
    ArgValue, CableEndpoint, InputPortRef, OutputPortRef, Patch, PatchConnection, PatchNode,
};
use super::state::{PatcherPanState, source_connection_id};

pub(super) fn patch_content_size(patch: &Patch) -> (f32, f32) {
    let input_indices = patch_input_indices(patch);
    let input_slot_counts = patch_input_slot_counts(patch, &input_indices);
    let output_counts = patch_output_counts(patch);
    let mut max_col: f32 = VIEW_PADDING_X * 2.0;
    let mut max_row: f32 = VIEW_PADDING_Y * 2.0;
    for node in &patch.nodes {
        let (width, height) = node_size_for_ports(
            node,
            input_slot_counts.get(&node.id).copied().unwrap_or(0),
            output_counts.get(&node.id).copied().unwrap_or(0),
        );
        max_col = max_col.max(PATCH_ORIGIN_COL_OFFSET + node.position.0 + width + VIEW_PADDING_X);
        max_row = max_row.max(PATCH_ORIGIN_ROW_OFFSET + node.position.1 + height + VIEW_PADDING_Y);
    }
    (max_col, max_row)
}

pub(super) fn patcher_origin(rect: Rect, pan_state: &PatcherPanState) -> (f32, f32) {
    (
        rect.col + PATCH_ORIGIN_COL_OFFSET - pan_state.offset_x,
        rect.row + PATCH_ORIGIN_ROW_OFFSET - pan_state.offset_y,
    )
}

pub(super) fn patcher_zoom(pan_state: &PatcherPanState) -> f32 {
    pan_state.zoom.max(MIN_ZOOM)
}

pub(super) fn screen_to_model(
    rect: Rect,
    pan_state: &PatcherPanState,
    point: (f32, f32),
) -> (f32, f32) {
    let origin = patcher_origin(rect, pan_state);
    let zoom = patcher_zoom(pan_state);
    ((point.0 - origin.0) / zoom, (point.1 - origin.1) / zoom)
}

pub(super) fn patch_node_rects(
    patch: &Patch,
    rect: Rect,
    pan_state: &PatcherPanState,
) -> HashMap<String, Rect> {
    let origin = patcher_origin(rect, pan_state);
    let zoom = patcher_zoom(pan_state);
    let input_indices = patch_input_indices(patch);
    let input_slot_counts = patch_input_slot_counts(patch, &input_indices);
    let output_counts = patch_output_counts(patch);
    patch
        .nodes
        .iter()
        .map(|node| {
            let size = node_size_for_ports(
                node,
                input_slot_counts.get(&node.id).copied().unwrap_or(0),
                output_counts.get(&node.id).copied().unwrap_or(0),
            );
            (
                node.id.clone(),
                Rect {
                    col: origin.0 + node.position.0 * zoom,
                    row: origin.1 + node.position.1 * zoom,
                    width: size.0 * zoom,
                    height: size.1 * zoom,
                },
            )
        })
        .collect()
}

pub(super) fn patch_input_indices(patch: &Patch) -> HashMap<String, Vec<usize>> {
    let mut indices: HashMap<String, Vec<usize>> = HashMap::new();
    for node in &patch.nodes {
        for (idx, arg) in node.args.iter().enumerate() {
            if matches!(arg, ArgValue::SymbolRef(_) | ArgValue::ConnectedExpr) {
                indices.entry(node.id.clone()).or_default().push(idx);
            }
        }
    }
    for connection in &patch.connections {
        let node_indices = indices.entry(connection.to_node.clone()).or_default();
        if !node_indices.contains(&connection.to_input) {
            node_indices.push(connection.to_input);
        }
    }
    for node_indices in indices.values_mut() {
        node_indices.sort_unstable();
        node_indices.dedup();
    }
    indices
}

pub(super) fn patch_input_slot_counts(
    patch: &Patch,
    input_indices: &HashMap<String, Vec<usize>>,
) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for node in &patch.nodes {
        let count = input_indices
            .get(&node.id)
            .map(|indices| {
                let connected_span = indices
                    .iter()
                    .copied()
                    .max()
                    .map(|idx| idx + 1)
                    .unwrap_or(0);
                connected_span.max(node.args.len())
            })
            .unwrap_or(0);
        if count > 0 {
            counts.insert(node.id.clone(), count);
        }
    }
    counts
}

pub(super) fn patch_output_counts(patch: &Patch) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for node in &patch.nodes {
        if !node.outputs.is_empty() {
            counts.insert(node.id.clone(), node.outputs.len());
        }
    }
    for connection in &patch.connections {
        let needed = connection.from_output + 1;
        counts
            .entry(connection.from_node.clone())
            .and_modify(|count| *count = (*count).max(needed))
            .or_insert(needed);
    }
    counts
}

pub(super) fn hit_patcher_node(
    patch: &Patch,
    ordered_nodes: &[&PatchNode],
    rect: Rect,
    pan_state: &PatcherPanState,
    local_col: f32,
    local_row: f32,
) -> Option<String> {
    let node_rects = patch_node_rects(patch, rect, pan_state);
    ordered_nodes.iter().rev().find_map(|node| {
        let node_rect = node_rects.get(&node.id)?;
        rect_contains(*node_rect, local_col, local_row).then(|| node.id.clone())
    })
}

pub(super) fn rect_contains(rect: Rect, col: f32, row: f32) -> bool {
    col >= rect.col
        && col <= rect.col + rect.width
        && row >= rect.row
        && row <= rect.row + rect.height
}

pub(super) fn rects_intersect(a: Rect, b: Rect) -> bool {
    a.col <= b.col + b.width
        && a.col + a.width >= b.col
        && a.row <= b.row + b.height
        && a.row + a.height >= b.row
}

pub(super) fn rect_from_points(
    start_col: f32,
    start_row: f32,
    current_col: f32,
    current_row: f32,
) -> Rect {
    let col = start_col.min(current_col);
    let row = start_row.min(current_row);
    Rect {
        col,
        row,
        width: (start_col - current_col).abs(),
        height: (start_row - current_row).abs(),
    }
}

pub(super) fn port_center(rect: Rect, index: usize, count: usize, top: bool) -> (f32, f32) {
    let count = count.max(1);
    let zoom = (rect.height / NODE_HEIGHT).max(MIN_ZOOM);
    let edge_padding = PORT_EDGE_PADDING_CELLS * zoom;
    let usable = (rect.width - edge_padding * 2.0).max(0.0);
    let x = if count == 1 {
        rect.col + edge_padding.min(rect.width * 0.5)
    } else {
        rect.col + edge_padding + usable * (index.min(count - 1) as f32) / ((count - 1) as f32)
    };
    let y = if top {
        rect.row
    } else {
        rect.row + rect.height
    };
    (x, y)
}

pub(super) fn distance_squared(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

pub(super) fn hit_patcher_output_port(
    patch: &Patch,
    ordered_nodes: &[&PatchNode],
    rect: Rect,
    pan_state: &PatcherPanState,
    output_counts: &HashMap<String, usize>,
    local_col: f32,
    local_row: f32,
    cell_w: f32,
    cell_h: f32,
) -> Option<OutputPortRef> {
    let node_rects = patch_node_rects(patch, rect, pan_state);
    let radius_px = PORT_OUTER_DIAMETER_PX * 0.5;
    let threshold_px = radius_px * radius_px;
    ordered_nodes.iter().rev().find_map(|node| {
        let node_rect = *node_rects.get(&node.id)?;
        let output_count = output_counts.get(&node.id).copied().unwrap_or(0);
        (0..output_count).find_map(|output_index| {
            let center = port_center(node_rect, output_index, output_count, false);
            let dx_px = (center.0 - local_col) * cell_w.max(1.0);
            let dy_px = (center.1 - local_row) * cell_h.max(1.0);
            (dx_px * dx_px + dy_px * dy_px <= threshold_px).then(|| OutputPortRef {
                node_id: node.id.clone(),
                output_index,
            })
        })
    })
}

pub(super) fn hit_patcher_input_port(
    patch: &Patch,
    ordered_nodes: &[&PatchNode],
    rect: Rect,
    pan_state: &PatcherPanState,
    input_indices: &HashMap<String, Vec<usize>>,
    input_slot_counts: &HashMap<String, usize>,
    local_col: f32,
    local_row: f32,
    cell_w: f32,
    cell_h: f32,
) -> Option<InputPortRef> {
    let node_rects = patch_node_rects(patch, rect, pan_state);
    let radius_px = PORT_OUTER_DIAMETER_PX * 0.5;
    let threshold_px = radius_px * radius_px;
    ordered_nodes.iter().rev().find_map(|node| {
        let node_rect = *node_rects.get(&node.id)?;
        let input_indices = input_indices.get(&node.id)?;
        let input_slot_count = input_slot_counts.get(&node.id).copied().unwrap_or(0);
        input_indices.iter().find_map(|input_index| {
            let center = port_center(node_rect, *input_index, input_slot_count, true);
            let dx_px = (center.0 - local_col) * cell_w.max(1.0);
            let dy_px = (center.1 - local_row) * cell_h.max(1.0);
            (dx_px * dx_px + dy_px * dy_px <= threshold_px).then(|| InputPortRef {
                node_id: node.id.clone(),
                input_index: *input_index,
            })
        })
    })
}

pub(super) fn nearest_patcher_output_port(
    patch: &Patch,
    rect: Rect,
    pan_state: &PatcherPanState,
    output_counts: &HashMap<String, usize>,
    local_col: f32,
    local_row: f32,
) -> Option<OutputPortRef> {
    let node_rects = patch_node_rects(patch, rect, pan_state);
    let threshold_radius = CABLE_TARGET_RADIUS_CELLS * patcher_zoom(pan_state);
    let threshold = threshold_radius * threshold_radius;
    patch
        .nodes
        .iter()
        .flat_map(|node| {
            let Some(node_rect) = node_rects.get(&node.id).copied() else {
                return Vec::new();
            };
            let output_count = output_counts.get(&node.id).copied().unwrap_or(0);
            (0..output_count)
                .filter_map(move |output_index| {
                    let center = port_center(node_rect, output_index, output_count, false);
                    let distance = distance_squared(center, (local_col, local_row));
                    (distance <= threshold).then(|| {
                        (
                            distance,
                            OutputPortRef {
                                node_id: node.id.clone(),
                                output_index,
                            },
                        )
                    })
                })
                .collect::<Vec<_>>()
        })
        .min_by(|(a, _), (b, _)| a.total_cmp(b))
        .map(|(_, port)| port)
}

pub(super) fn nearest_patcher_input_port(
    patch: &Patch,
    rect: Rect,
    pan_state: &PatcherPanState,
    input_indices: &HashMap<String, Vec<usize>>,
    input_slot_counts: &HashMap<String, usize>,
    source: &OutputPortRef,
    local_col: f32,
    local_row: f32,
) -> Option<InputPortRef> {
    let node_rects = patch_node_rects(patch, rect, pan_state);
    let threshold_radius = CABLE_TARGET_RADIUS_CELLS * patcher_zoom(pan_state);
    let threshold = threshold_radius * threshold_radius;
    patch
        .nodes
        .iter()
        .filter(|node| node.id != source.node_id)
        .flat_map(|node| {
            let Some(node_rect) = node_rects.get(&node.id).copied() else {
                return Vec::new();
            };
            let Some(input_indices) = input_indices.get(&node.id) else {
                return Vec::new();
            };
            let slot_count = input_slot_counts.get(&node.id).copied().unwrap_or(0);
            input_indices
                .iter()
                .filter_map(move |input_index| {
                    let center = port_center(node_rect, *input_index, slot_count, true);
                    let distance = distance_squared(center, (local_col, local_row));
                    (distance <= threshold).then(|| {
                        (
                            distance,
                            InputPortRef {
                                node_id: node.id.clone(),
                                input_index: *input_index,
                            },
                        )
                    })
                })
                .collect::<Vec<_>>()
        })
        .min_by(|(a, _), (b, _)| a.total_cmp(b))
        .map(|(_, port)| port)
}

pub(super) fn connection_endpoints(
    connection: &PatchConnection,
    node_rects: &HashMap<String, Rect>,
    input_indices: &HashMap<String, Vec<usize>>,
    input_slot_counts: &HashMap<String, usize>,
    output_counts: &HashMap<String, usize>,
) -> Option<((f32, f32), (f32, f32))> {
    let from = *node_rects.get(&connection.from_node)?;
    let to = *node_rects.get(&connection.to_node)?;
    let start = port_center(
        from,
        connection.from_output,
        output_counts
            .get(&connection.from_node)
            .copied()
            .unwrap_or(1),
        false,
    );
    let to_input_indices = input_indices.get(&connection.to_node)?;
    let semantic_input = visible_input_slot(to_input_indices, connection.to_input)?;
    let end = port_center(
        to,
        semantic_input,
        input_slot_counts
            .get(&connection.to_node)
            .copied()
            .unwrap_or(to_input_indices.len()),
        true,
    );
    Some((start, end))
}

fn visible_input_slot(input_indices: &[usize], semantic_input: usize) -> Option<usize> {
    input_indices
        .iter()
        .find(|input_index| **input_index == semantic_input)
        .copied()
}

pub(super) fn hit_patcher_cable(
    patch: &Patch,
    rect: Rect,
    pan_state: &PatcherPanState,
    input_indices: &HashMap<String, Vec<usize>>,
    input_slot_counts: &HashMap<String, usize>,
    output_counts: &HashMap<String, usize>,
    local_col: f32,
    local_row: f32,
) -> Option<String> {
    let node_rects = patch_node_rects(patch, rect, pan_state);
    let origin = patcher_origin(rect, pan_state);
    let zoom = patcher_zoom(pan_state);
    patch
        .connections
        .iter()
        .filter_map(|connection| {
            let (start, end) = connection_endpoints(
                connection,
                &node_rects,
                input_indices,
                input_slot_counts,
                output_counts,
            )?;
            let distance = match connection.segment {
                Some(segment)
                    if segment.is_segmented
                        && super::super::cable::should_render_segmented_cable(start, end) =>
                {
                    super::super::cable::distance_to_segmented_cable_px(
                        start,
                        end,
                        origin.1 + segment.segment_row * zoom,
                        SEGMENTED_CABLE_CORNER_RADIUS_CELLS * zoom,
                        (local_col, local_row),
                    )
                }
                _ => super::super::cable::distance_to_cable_px(start, end, (local_col, local_row)),
            };
            (distance <= CABLE_HIT_RADIUS_CELLS * zoom)
                .then(|| (distance, source_connection_id(connection)))
        })
        .min_by(|(a, _), (b, _)| a.total_cmp(b))
        .map(|(_, id)| id)
}

pub(super) fn cable_edit_points(
    start: (f32, f32),
    end: (f32, f32),
    zoom: f32,
) -> ((f32, f32), (f32, f32)) {
    super::super::cable::cable_edit_points(start, end, CABLE_HANDLE_DISTANCE_CELLS * zoom)
}

pub(super) fn connection_cable_edit_points(
    connection: &PatchConnection,
    start: (f32, f32),
    end: (f32, f32),
    zoom: f32,
) -> ((f32, f32), (f32, f32)) {
    if connection
        .segment
        .is_some_and(|segment| segment.is_segmented)
        && super::super::cable::should_render_segmented_cable(start, end)
    {
        super::super::cable::segmented_cable_edit_points(
            start,
            end,
            CABLE_HANDLE_DISTANCE_CELLS * zoom,
        )
    } else {
        cable_edit_points(start, end, zoom)
    }
}

pub(super) fn hit_patcher_segmented_cable_horizontal_segment(
    patch: &Patch,
    rect: Rect,
    pan_state: &PatcherPanState,
    input_indices: &HashMap<String, Vec<usize>>,
    input_slot_counts: &HashMap<String, usize>,
    output_counts: &HashMap<String, usize>,
    local_col: f32,
    local_row: f32,
) -> Option<String> {
    let node_rects = patch_node_rects(patch, rect, pan_state);
    let origin = patcher_origin(rect, pan_state);
    let zoom = patcher_zoom(pan_state);
    patch.connections.iter().find_map(|connection| {
        let segment = connection.segment?;
        if !segment.is_segmented {
            return None;
        }
        let (start, end) = connection_endpoints(
            connection,
            &node_rects,
            input_indices,
            input_slot_counts,
            output_counts,
        )?;
        if !super::super::cable::should_render_segmented_cable(start, end) {
            return None;
        }
        super::super::cable::segmented_horizontal_segment_hit(
            start,
            end,
            origin.1 + segment.segment_row * zoom,
            SEGMENTED_CABLE_CORNER_RADIUS_CELLS * zoom,
            (local_col, local_row),
        )
        .then(|| source_connection_id(connection))
    })
}

pub(super) fn hit_patcher_cable_handle(
    patch: &Patch,
    rect: Rect,
    pan_state: &PatcherPanState,
    input_indices: &HashMap<String, Vec<usize>>,
    input_slot_counts: &HashMap<String, usize>,
    output_counts: &HashMap<String, usize>,
    selected_cable: Option<&str>,
    local_col: f32,
    local_row: f32,
) -> Option<(String, CableEndpoint)> {
    let selected_cable = selected_cable?;
    let node_rects = patch_node_rects(patch, rect, pan_state);
    let zoom = patcher_zoom(pan_state);
    let threshold_radius = CABLE_HANDLE_HIT_RADIUS_CELLS * zoom;
    let threshold = threshold_radius * threshold_radius;
    patch.connections.iter().find_map(|connection| {
        let cable_id = source_connection_id(connection);
        if cable_id != selected_cable {
            return None;
        }
        let (start, end) = connection_endpoints(
            connection,
            &node_rects,
            input_indices,
            input_slot_counts,
            output_counts,
        )?;
        let (from_handle, to_handle) = connection_cable_edit_points(connection, start, end, zoom);
        if distance_squared(from_handle, (local_col, local_row)) <= threshold {
            Some((cable_id, CableEndpoint::From))
        } else if distance_squared(to_handle, (local_col, local_row)) <= threshold {
            Some((cable_id, CableEndpoint::To))
        } else {
            None
        }
    })
}

pub(super) fn patcher_breadcrumb_rect(rect: Rect) -> Rect {
    Rect {
        row: rect.row,
        col: rect.col,
        width: rect.width,
        height: 2.0,
    }
}

pub(super) fn patcher_back_button_rect(rect: Rect) -> Rect {
    Rect {
        row: rect.row + 0.45,
        col: rect.col + 0.9,
        width: 2.0,
        height: 1.4,
    }
}
