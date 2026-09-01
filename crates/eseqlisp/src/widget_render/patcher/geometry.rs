use std::collections::{HashMap, HashSet};

use crate::layout::Rect;

use super::display::{
    node_display_label, node_display_label_arg_spans, node_font_size, node_size_for_ports,
};
use super::metrics::{
    CABLE_HANDLE_DISTANCE_CELLS, CABLE_HANDLE_HIT_RADIUS_CELLS, CABLE_HANDLE_MAX_SPAN_FRACTION,
    CABLE_HIT_RADIUS_CELLS, CABLE_TARGET_RADIUS_CELLS, MIN_ZOOM, NODE_HEIGHT,
    NODE_RESIZE_HANDLE_HIT_SIZE_CELLS, NODE_TEXT_COL_OFFSET, PATCH_ORIGIN_COL_OFFSET,
    PATCH_ORIGIN_ROW_OFFSET, PORT_EDGE_PADDING_CELLS, PORT_OUTER_DIAMETER_PX,
    SEGMENTED_CABLE_CORNER_RADIUS_CELLS, VIEW_PADDING_X, VIEW_PADDING_Y,
};
use super::model::{
    ArgValue, CableEndpoint, InputPortRef, InputPresentation, OutputPortRef, Patch,
    PatchConnection, PatchNode, connection_touches_hidden_inline_node, hidden_inline_node_ids,
};
use super::state::{NodeResizeCorner, PatcherPanState, source_connection_id};
use super::text_metrics::measured_cursor_offset;

pub(super) fn patch_content_size(patch: &Patch) -> (f32, f32) {
    let input_indices = patch_input_indices(patch);
    let input_slot_counts = patch_input_slot_counts(patch, &input_indices);
    let output_counts = patch_output_counts(patch);
    let hidden_node_ids = hidden_inline_node_ids(patch);
    let mut max_col: f32 = VIEW_PADDING_X * 2.0;
    let mut max_row: f32 = VIEW_PADDING_Y * 2.0;
    for node in &patch.nodes {
        if hidden_node_ids.contains(&node.id) {
            continue;
        }
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
    let hidden_node_ids = hidden_inline_node_ids(patch);
    patch
        .nodes
        .iter()
        .filter(|node| !hidden_node_ids.contains(&node.id))
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
    let hidden_node_ids = hidden_inline_node_ids(patch);
    for node in &patch.nodes {
        if hidden_node_ids.contains(&node.id) {
            continue;
        }
        if node.kind == super::model::NodeKind::Param {
            indices
                .entry(node.id.clone())
                .or_default()
                .push(super::model::PARAM_OPTIONS_INPUT);
        }
        for (idx, arg) in node.args.iter().enumerate() {
            if node
                .inline_inputs
                .get(idx)
                .is_some_and(|input| input.is_some())
            {
                continue;
            }
            if matches!(arg, ArgValue::SymbolRef(_) | ArgValue::ConnectedExpr) {
                indices.entry(node.id.clone()).or_default().push(idx);
            }
        }
    }
    for connection in &patch.connections {
        if connection.presentation != InputPresentation::Cable {
            continue;
        }
        if connection_touches_hidden_inline_node(connection, &hidden_node_ids) {
            continue;
        }
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
    let hidden_node_ids = hidden_inline_node_ids(patch);
    for node in &patch.nodes {
        if hidden_node_ids.contains(&node.id) {
            continue;
        }
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
    let hidden_node_ids = hidden_inline_node_ids(patch);
    for node in &patch.nodes {
        if hidden_node_ids.contains(&node.id) {
            continue;
        }
        if !node.outputs.is_empty() {
            counts.insert(node.id.clone(), node.outputs.len());
        }
    }
    for connection in &patch.connections {
        if connection_touches_hidden_inline_node(connection, &hidden_node_ids) {
            continue;
        }
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

pub(super) fn node_resize_handle_centers(rect: Rect) -> [(NodeResizeCorner, (f32, f32)); 4] {
    [
        (NodeResizeCorner::TopLeft, (rect.col, rect.row)),
        (
            NodeResizeCorner::TopRight,
            (rect.col + rect.width, rect.row),
        ),
        (
            NodeResizeCorner::BottomLeft,
            (rect.col, rect.row + rect.height),
        ),
        (
            NodeResizeCorner::BottomRight,
            (rect.col + rect.width, rect.row + rect.height),
        ),
    ]
}

pub(super) fn hit_patcher_node_resize_handle(
    patch: &Patch,
    ordered_nodes: &[&PatchNode],
    selected_nodes: &HashSet<String>,
    rect: Rect,
    pan_state: &PatcherPanState,
    local_col: f32,
    local_row: f32,
) -> Option<(String, NodeResizeCorner)> {
    if selected_nodes.is_empty() {
        return None;
    }
    let node_rects = patch_node_rects(patch, rect, pan_state);
    let half_size = NODE_RESIZE_HANDLE_HIT_SIZE_CELLS * patcher_zoom(pan_state) * 0.5;
    ordered_nodes.iter().rev().find_map(|node| {
        if !selected_nodes.contains(&node.id) {
            return None;
        }
        let node_rect = *node_rects.get(&node.id)?;
        node_resize_handle_centers(node_rect)
            .into_iter()
            .find_map(|(corner, center)| {
                let handle_rect = Rect {
                    col: center.0 - half_size,
                    row: center.1 - half_size,
                    width: half_size * 2.0,
                    height: half_size * 2.0,
                };
                rect_contains(handle_rect, local_col, local_row).then(|| (node.id.clone(), corner))
            })
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
    let zoom = (rect.height / NODE_HEIGHT).max(MIN_ZOOM);
    let x = rect.col + port_x_offset(index, count, rect.width / zoom) * zoom;
    let y = if top {
        rect.row
    } else {
        rect.row + rect.height
    };
    (x, y)
}

pub(super) fn port_x_offset(index: usize, count: usize, width: f32) -> f32 {
    let count = count.max(1);
    let usable = (width - PORT_EDGE_PADDING_CELLS * 2.0).max(0.0);
    if count == 1 {
        PORT_EDGE_PADDING_CELLS.min(width * 0.5)
    } else {
        PORT_EDGE_PADDING_CELLS + usable * (index.min(count - 1) as f32) / ((count - 1) as f32)
    }
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

/// Hit-test the argument tokens drawn inside node labels.
///
/// An argument inlined as a literal draws no port, so hovering its text is the
/// only way to discover which inlet it is; tokens that do have a port are hit
/// here too so hovering either surface behaves the same. Returns `None` for a
/// label whose glyph advances the measure pass has not cached yet.
pub(super) fn hit_patcher_label_arg(
    patch: &Patch,
    ordered_nodes: &[&PatchNode],
    rect: Rect,
    pan_state: &PatcherPanState,
    local_col: f32,
    local_row: f32,
) -> Option<InputPortRef> {
    let node_rects = patch_node_rects(patch, rect, pan_state);
    let zoom = patcher_zoom(pan_state);
    ordered_nodes.iter().rev().find_map(|node| {
        let node_rect = *node_rects.get(&node.id)?;
        if !rect_contains(node_rect, local_col, local_row) {
            return None;
        }
        let label = node_display_label(node);
        let font_size = node_font_size(node);
        let text_col = node_rect.col + NODE_TEXT_COL_OFFSET * zoom;
        node_display_label_arg_spans(node)
            .into_iter()
            .find_map(|(arg_index, span)| {
                let start = measured_cursor_offset(&label, font_size, span.start)?;
                let end = measured_cursor_offset(&label, font_size, span.end)?;
                (local_col >= text_col + start * zoom && local_col <= text_col + end * zoom).then(
                    || InputPortRef {
                        node_id: node.id.clone(),
                        input_index: arg_index,
                    },
                )
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
    let hidden_node_ids = hidden_inline_node_ids(patch);
    patch
        .connections
        .iter()
        .filter(|connection| connection.presentation == InputPresentation::Cable)
        .filter(|connection| !connection_touches_hidden_inline_node(connection, &hidden_node_ids))
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
    super::super::cable::cable_edit_points(start, end, cable_handle_distance(start, end, zoom))
}

/// How far each endpoint handle sits in from its end of the cable. Short cables
/// scale the distance down so the two handles never cross past each other and
/// stay independently grabbable.
fn cable_handle_distance(start: (f32, f32), end: (f32, f32), zoom: f32) -> f32 {
    let span = ((end.0 - start.0).powi(2) + (end.1 - start.1).powi(2)).sqrt();
    (CABLE_HANDLE_DISTANCE_CELLS * zoom).min(span * CABLE_HANDLE_MAX_SPAN_FRACTION)
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
            cable_handle_distance(start, end, zoom),
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
    let hidden_node_ids = hidden_inline_node_ids(patch);
    patch
        .connections
        .iter()
        .filter(|connection| connection.presentation == InputPresentation::Cable)
        .filter(|connection| !connection_touches_hidden_inline_node(connection, &hidden_node_ids))
        .find_map(|connection| {
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
                CABLE_HIT_RADIUS_CELLS * zoom,
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
    let hidden_node_ids = hidden_inline_node_ids(patch);
    patch
        .connections
        .iter()
        .filter(|connection| connection.presentation == InputPresentation::Cable)
        .filter(|connection| !connection_touches_hidden_inline_node(connection, &hidden_node_ids))
        .find_map(|connection| {
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
            let (from_handle, to_handle) =
                connection_cable_edit_points(connection, start, end, zoom);
            // Nearest handle wins: on a short cable both handles can sit inside
            // one hit radius, and first-match would make the `To` end
            // unreachable.
            let from_distance = distance_squared(from_handle, (local_col, local_row));
            let to_distance = distance_squared(to_handle, (local_col, local_row));
            if from_distance > threshold && to_distance > threshold {
                return None;
            }
            let endpoint = if from_distance <= to_distance {
                CableEndpoint::From
            } else {
                CableEndpoint::To
            };
            Some((cable_id, endpoint))
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
