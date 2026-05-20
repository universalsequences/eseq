use std::collections::HashSet;

use crossterm::event::{KeyModifiers, MouseEventKind};

use crate::layout::LayoutNode;

use super::display::node_display_label;
use super::emit::debug_log_patch_lisp;
use super::geometry::{
    connection_endpoints, hit_patcher_cable, hit_patcher_cable_handle, hit_patcher_macro_drill_in,
    hit_patcher_node, hit_patcher_output_port, hit_patcher_segmented_cable_horizontal_segment,
    nearest_patcher_input_port, nearest_patcher_output_port, patch_content_size, patch_node_rects,
    patcher_back_button_rect, patcher_breadcrumb_rect, patcher_origin, port_center, rect_contains,
    rect_from_points, rects_intersect,
};
use super::metrics::{
    SEGMENTED_CABLE_DRAG_EXTRA_RANGE_CELLS, SEGMENTED_CABLE_DRAG_PADDING_CELLS,
    WHEEL_PAN_STEP_CELLS,
};
use super::model::{CableEndpoint, CableSegmentInfo, InputPortRef, NodeKind, OutputPortRef, Patch};
use super::render::{patch_input_indices, patch_input_slot_counts, patch_output_counts};
use super::state::{
    PatcherDragState, PatcherInteractionState, PatcherPanState, active_patcher_patch,
    active_patcher_view_key, allocate_created_connection, allocate_created_node,
    connection_id_from_ports, delete_connection_edit_or_mark_deleted, ensure_source_node_edit,
    get_patcher_interaction_state, get_patcher_pan_state, patch_with_interaction_state,
    patcher_state_key, set_connection_segment_edit, set_node_edit_position,
    set_patcher_interaction_state, set_patcher_pan_state, source_connection_id,
};
use super::text::{
    begin_patcher_text_edit, commit_patcher_text_edit, patcher_text_cursor_at_col,
    update_patcher_text_edit_pointer,
};
use super::{debug_patch_for_state, load_patch_from_props};

pub(super) fn pan_patcher_by_wheel(node: &LayoutNode, mouse_kind: MouseEventKind) {
    let (delta_x, delta_y) = match mouse_kind {
        MouseEventKind::ScrollUp => (0.0, -WHEEL_PAN_STEP_CELLS),
        MouseEventKind::ScrollDown => (0.0, WHEEL_PAN_STEP_CELLS),
        MouseEventKind::ScrollLeft => (-WHEEL_PAN_STEP_CELLS, 0.0),
        MouseEventKind::ScrollRight => (WHEEL_PAN_STEP_CELLS, 0.0),
        _ => (0.0, 0.0),
    };
    pan_patcher_by_delta(node, delta_x, delta_y);
}

pub(super) fn pan_patcher_by_delta(node: &LayoutNode, delta_x: f32, delta_y: f32) {
    if delta_x == 0.0 && delta_y == 0.0 {
        return;
    }
    let key = patcher_state_key(node);
    let mut state = get_patcher_pan_state(key);
    sync_patcher_pan_bounds(node, &mut state);
    state.offset_x += delta_x;
    state.offset_y += delta_y;
    set_patcher_pan_state(key, state);
}

pub(super) fn sync_patcher_pan_bounds(node: &LayoutNode, state: &mut PatcherPanState) {
    state.viewport_width = node.rect.width;
    state.viewport_height = node.rect.height;
    if let Ok((_, patch)) = load_patch_from_props(&node.props) {
        let interaction_state = get_patcher_interaction_state(patcher_state_key(node));
        let view_key = active_patcher_view_key(&interaction_state);
        let patch = active_patcher_patch(&patch, &interaction_state);
        let patch = patch_with_interaction_state(patch, &interaction_state, &view_key);
        let content_size = patch_content_size(&patch);
        state.content_width = content_size.0.max(node.rect.width);
        state.content_height = content_size.1.max(node.rect.height);
    } else {
        state.content_width = node.rect.width;
        state.content_height = node.rect.height;
    }
}

fn load_interactive_patch_for_node(node: &LayoutNode) -> Option<(Patch, PatcherPanState, String)> {
    let key = patcher_state_key(node);
    let interaction_state = get_patcher_interaction_state(key);
    let (_, root_patch) = load_patch_from_props(&node.props).ok()?;
    let view_key = active_patcher_view_key(&interaction_state);
    let patch = active_patcher_patch(&root_patch, &interaction_state);
    let patch = patch_with_interaction_state(patch, &interaction_state, &view_key);
    let mut pan_state = get_patcher_pan_state(key);
    sync_patcher_pan_bounds(node, &mut pan_state);
    Some((patch, pan_state, view_key))
}

pub(super) fn handle_patcher_pointer_down(
    node: &LayoutNode,
    local_col: f32,
    local_row: f32,
    modifiers: KeyModifiers,
    cell_w: f32,
    cell_h: f32,
) {
    let key = patcher_state_key(node);
    let mut state = get_patcher_interaction_state(key);
    if state.active_macro.is_some()
        && rect_contains(patcher_back_button_rect(node.rect), local_col, local_row)
    {
        navigate_patcher_to_root(key, &mut state);
        return;
    }
    let Some((patch, pan_state, _view_key)) = load_interactive_patch_for_node(node) else {
        return;
    };
    if let Ok((_, root_patch)) = load_patch_from_props(&node.props)
        && let Some((_node_id, macro_name)) = hit_patcher_macro_drill_in(
            &patch,
            &root_patch,
            node.rect,
            &pan_state,
            local_col,
            local_row,
        )
    {
        state.active_macro = Some(macro_name);
        state.selected_nodes.clear();
        state.hovered_node = None;
        state.hovered_macro_drill_in = None;
        state.drag = None;
        state.text_edit = None;
        state.selected_cable = None;
        set_patcher_interaction_state(key, state);
        reset_patcher_pan(key);
        return;
    }
    let hit = hit_patcher_node(&patch, node.rect, &pan_state, local_col, local_row);
    if let Some(edit_node_id) = state.text_edit.as_ref().map(|edit| edit.node_id.clone()) {
        if hit.as_deref() == Some(edit_node_id.as_str()) {
            if let Some(rect) = patch_node_rects(&patch, node.rect, &pan_state).get(&edit_node_id)
                && let Some(edit) = &mut state.text_edit
            {
                update_patcher_text_edit_pointer(edit, *rect, local_col, false, false);
                edit.state.selection_anchor = Some(edit.state.cursor_pos);
                edit.state.selecting = true;
            }
            set_patcher_interaction_state(key, state);
            return;
        }
        let view_key = active_patcher_view_key(&state);
        let changed = commit_patcher_text_edit(&mut state, &view_key);
        if changed && let Some(patch) = debug_patch_for_state(node, &state, &view_key) {
            debug_log_patch_lisp(&view_key, &patch);
        }
    }
    state.hovered_node = hit.clone();
    let input_indices = patch_input_indices(&patch);
    let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
    let output_counts = patch_output_counts(&patch);
    if let Some((cable_id, endpoint)) = hit_patcher_cable_handle(
        &patch,
        node.rect,
        &pan_state,
        &input_indices,
        &input_slot_counts,
        &output_counts,
        state.selected_cable.as_deref(),
        local_col,
        local_row,
    ) && let Some(connection) = patch
        .connections
        .iter()
        .find(|connection| source_connection_id(connection) == cable_id)
        && let Some((start, end)) = connection_endpoints(
            connection,
            &patch_node_rects(&patch, node.rect, &pan_state),
            &input_indices,
            &input_slot_counts,
            &output_counts,
        )
    {
        state.selected_nodes.clear();
        state.selected_cable = Some(cable_id.clone());
        state.drag = Some(PatcherDragState::CableEndpoint {
            cable_id,
            endpoint,
            original_from: OutputPortRef {
                node_id: connection.from_node.clone(),
                output_index: connection.from_output,
            },
            original_to: InputPortRef {
                node_id: connection.to_node.clone(),
                input_index: connection.to_input,
            },
            start_col: start.0,
            start_row: start.1,
            end_col: end.0,
            end_row: end.1,
            current_col: local_col,
            current_row: local_row,
            target_from: None,
            target_to: None,
        });
        set_patcher_interaction_state(key, state);
        return;
    }
    if let Some(output) = hit_patcher_output_port(
        &patch,
        node.rect,
        &pan_state,
        &output_counts,
        local_col,
        local_row,
        cell_w,
        cell_h,
    ) {
        let Some(node_rect) = patch_node_rects(&patch, node.rect, &pan_state)
            .get(&output.node_id)
            .copied()
        else {
            return;
        };
        let output_count = output_counts.get(&output.node_id).copied().unwrap_or(1);
        let start = port_center(node_rect, output.output_index, output_count, false);
        let target = nearest_patcher_input_port(
            &patch,
            node.rect,
            &pan_state,
            &input_indices,
            &input_slot_counts,
            &output,
            local_col,
            local_row,
        );
        state.selected_nodes.clear();
        state.selected_nodes.insert(output.node_id.clone());
        state.selected_cable = None;
        state.drag = Some(PatcherDragState::Cable {
            from: output,
            start_col: start.0,
            start_row: start.1,
            current_col: local_col,
            current_row: local_row,
            target,
        });
        set_patcher_interaction_state(key, state);
        return;
    }
    if let Some(cable_id) = hit_patcher_segmented_cable_horizontal_segment(
        &patch,
        node.rect,
        &pan_state,
        &input_indices,
        &input_slot_counts,
        &output_counts,
        local_col,
        local_row,
    ) && let Some(connection) = patch
        .connections
        .iter()
        .find(|connection| source_connection_id(connection) == cable_id)
        && let Some((start, end)) = connection_endpoints(
            connection,
            &patch_node_rects(&patch, node.rect, &pan_state),
            &input_indices,
            &input_slot_counts,
            &output_counts,
        )
    {
        let origin = patcher_origin(node.rect, &pan_state);
        state.selected_nodes.clear();
        state.selected_cable = Some(cable_id.clone());
        state.drag = Some(PatcherDragState::CableSegment {
            cable_id,
            start_col: start.0,
            start_row: start.1 - origin.1,
            end_col: end.0,
            end_row: end.1 - origin.1,
        });
        set_patcher_interaction_state(key, state);
        return;
    }
    let shift = modifiers.contains(KeyModifiers::SHIFT);
    match hit {
        Some(node_id) => {
            state.selected_cable = None;
            if shift {
                if !state.selected_nodes.insert(node_id.clone()) {
                    state.selected_nodes.remove(&node_id);
                }
            } else if !state.selected_nodes.contains(&node_id) {
                state.selected_nodes.clear();
                state.selected_nodes.insert(node_id);
            }
            let start_positions = patch
                .nodes
                .iter()
                .filter(|node| state.selected_nodes.contains(&node.id))
                .map(|node| (node.id.clone(), node.position))
                .collect();
            state.drag = Some(PatcherDragState::Nodes {
                start_col: local_col,
                start_row: local_row,
                start_positions,
            });
        }
        None => {
            if let Some(cable_id) = hit_patcher_cable(
                &patch,
                node.rect,
                &pan_state,
                &input_indices,
                &input_slot_counts,
                &output_counts,
                local_col,
                local_row,
            ) {
                state.selected_nodes.clear();
                state.selected_cable = Some(cable_id);
                state.drag = None;
                set_patcher_interaction_state(key, state);
                return;
            }
            let base_selection = if shift {
                state.selected_nodes.clone()
            } else {
                state.selected_nodes.clear();
                state.selected_cable = None;
                HashSet::new()
            };
            state.drag = Some(PatcherDragState::Marquee {
                start_col: local_col,
                start_row: local_row,
                current_col: local_col,
                current_row: local_row,
                base_selection,
            });
        }
    }
    set_patcher_interaction_state(key, state);
}

pub(super) fn handle_patcher_pointer_drag(node: &LayoutNode, local_col: f32, local_row: f32) {
    let key = patcher_state_key(node);
    let Some((patch, pan_state, view_key)) = load_interactive_patch_for_node(node) else {
        return;
    };
    let mut state = get_patcher_interaction_state(key);
    if let Some(edit_node_id) = state.text_edit.as_ref().map(|edit| edit.node_id.clone()) {
        if let Some(rect) = patch_node_rects(&patch, node.rect, &pan_state).get(&edit_node_id)
            && let Some(edit) = &mut state.text_edit
        {
            update_patcher_text_edit_pointer(edit, *rect, local_col, true, false);
        }
        set_patcher_interaction_state(key, state);
        return;
    }
    match state.drag.clone() {
        Some(PatcherDragState::Cable {
            from,
            start_col,
            start_row,
            ..
        }) => {
            let input_indices = patch_input_indices(&patch);
            let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
            let target = nearest_patcher_input_port(
                &patch,
                node.rect,
                &pan_state,
                &input_indices,
                &input_slot_counts,
                &from,
                local_col,
                local_row,
            );
            state.drag = Some(PatcherDragState::Cable {
                from,
                start_col,
                start_row,
                current_col: local_col,
                current_row: local_row,
                target,
            });
        }
        Some(PatcherDragState::CableEndpoint {
            cable_id,
            endpoint,
            original_from,
            original_to,
            start_col,
            start_row,
            end_col,
            end_row,
            ..
        }) => {
            let input_indices = patch_input_indices(&patch);
            let input_slot_counts = patch_input_slot_counts(&patch, &input_indices);
            let output_counts = patch_output_counts(&patch);
            let (target_from, target_to) = match endpoint {
                CableEndpoint::From => (
                    nearest_patcher_output_port(
                        &patch,
                        node.rect,
                        &pan_state,
                        &output_counts,
                        local_col,
                        local_row,
                    ),
                    None,
                ),
                CableEndpoint::To => (
                    None,
                    nearest_patcher_input_port(
                        &patch,
                        node.rect,
                        &pan_state,
                        &input_indices,
                        &input_slot_counts,
                        &original_from,
                        local_col,
                        local_row,
                    ),
                ),
            };
            state.drag = Some(PatcherDragState::CableEndpoint {
                cable_id,
                endpoint,
                original_from,
                original_to,
                start_col,
                start_row,
                end_col,
                end_row,
                current_col: local_col,
                current_row: local_row,
                target_from,
                target_to,
            });
        }
        Some(PatcherDragState::Nodes {
            start_col,
            start_row,
            start_positions,
        }) => {
            let delta = (local_col - start_col, local_row - start_row);
            for (node_id, start_position) in start_positions {
                if let Some(patch_node) = patch.nodes.iter().find(|node| node.id == node_id) {
                    set_node_edit_position(
                        &mut state,
                        &view_key,
                        patch_node,
                        (start_position.0 + delta.0, start_position.1 + delta.1),
                        node_display_label(patch_node),
                    );
                }
            }
        }
        Some(PatcherDragState::CableSegment {
            cable_id,
            start_col,
            start_row,
            end_col,
            end_row,
        }) => {
            if let Some(connection) = patch
                .connections
                .iter()
                .find(|connection| source_connection_id(connection) == cable_id)
                .cloned()
            {
                let origin = patcher_origin(node.rect, &pan_state);
                let segment_row = super::super::cable::segment_row_for_drag(
                    (start_col, start_row),
                    (end_col, end_row),
                    local_row - origin.1,
                    SEGMENTED_CABLE_DRAG_PADDING_CELLS,
                    SEGMENTED_CABLE_DRAG_EXTRA_RANGE_CELLS,
                );
                set_connection_segment_edit(
                    &mut state,
                    &view_key,
                    &connection,
                    Some(CableSegmentInfo {
                        is_segmented: true,
                        segment_row,
                    }),
                );
            }
            state.drag = Some(PatcherDragState::CableSegment {
                cable_id,
                start_col,
                start_row,
                end_col,
                end_row,
            });
        }
        Some(PatcherDragState::Marquee {
            start_col,
            start_row,
            base_selection,
            ..
        }) => {
            let marquee = rect_from_points(start_col, start_row, local_col, local_row);
            let node_rects = patch_node_rects(&patch, node.rect, &pan_state);
            state.selected_nodes = base_selection.clone();
            for patch_node in &patch.nodes {
                if let Some(node_rect) = node_rects.get(&patch_node.id)
                    && rects_intersect(marquee, *node_rect)
                {
                    state.selected_nodes.insert(patch_node.id.clone());
                }
            }
            state.drag = Some(PatcherDragState::Marquee {
                start_col,
                start_row,
                current_col: local_col,
                current_row: local_row,
                base_selection,
            });
        }
        None => {}
    }
    state.hovered_node = hit_patcher_node(&patch, node.rect, &pan_state, local_col, local_row);
    set_patcher_interaction_state(key, state);
}

pub(super) fn handle_patcher_pointer_up(node: &LayoutNode, local_col: f32, local_row: f32) -> bool {
    let key = patcher_state_key(node);
    let mut state = get_patcher_interaction_state(key);
    let mut semantic_changed = false;
    if let Some((patch, pan_state, view_key)) = load_interactive_patch_for_node(node) {
        if let Some(edit_node_id) = state.text_edit.as_ref().map(|edit| edit.node_id.clone())
            && let Some(rect) = patch_node_rects(&patch, node.rect, &pan_state).get(&edit_node_id)
            && let Some(edit) = &mut state.text_edit
        {
            update_patcher_text_edit_pointer(edit, *rect, local_col, true, true);
            set_patcher_interaction_state(key, state);
            return false;
        }
        if let Some(PatcherDragState::Cable {
            from,
            target: Some(target),
            ..
        }) = state.drag.clone()
        {
            allocate_created_connection(&mut state, &view_key, from, target);
            semantic_changed = true;
        }
        if let Some(PatcherDragState::CableEndpoint {
            cable_id,
            endpoint,
            original_from,
            original_to,
            target_from,
            target_to,
            ..
        }) = state.drag.clone()
        {
            match endpoint {
                CableEndpoint::From => {
                    if let Some(new_from) = target_from {
                        delete_connection_edit_or_mark_deleted(&mut state, &view_key, &cable_id);
                        state.selected_cable =
                            Some(connection_id_from_ports(&new_from, &original_to));
                        allocate_created_connection(&mut state, &view_key, new_from, original_to);
                        semantic_changed = true;
                    }
                }
                CableEndpoint::To => {
                    if let Some(new_to) = target_to {
                        delete_connection_edit_or_mark_deleted(&mut state, &view_key, &cable_id);
                        state.selected_cable =
                            Some(connection_id_from_ports(&original_from, &new_to));
                        allocate_created_connection(&mut state, &view_key, original_from, new_to);
                        semantic_changed = true;
                    }
                }
            }
        }
        state.hovered_node = hit_patcher_node(&patch, node.rect, &pan_state, local_col, local_row);
        if semantic_changed && let Some(patch) = debug_patch_for_state(node, &state, &view_key) {
            debug_log_patch_lisp(&view_key, &patch);
        }
    }
    state.drag = None;
    set_patcher_interaction_state(key, state);
    semantic_changed
}

pub(super) fn handle_patcher_pointer_moved(node: &LayoutNode, local_col: f32, local_row: f32) {
    let key = patcher_state_key(node);
    let Some((patch, pan_state, _view_key)) = load_interactive_patch_for_node(node) else {
        return;
    };
    let mut state = get_patcher_interaction_state(key);
    state.hover_back_button = state.active_macro.is_some()
        && rect_contains(patcher_back_button_rect(node.rect), local_col, local_row);
    state.hovered_macro_drill_in =
        load_patch_from_props(&node.props)
            .ok()
            .and_then(|(_, root_patch)| {
                hit_patcher_macro_drill_in(
                    &patch,
                    &root_patch,
                    node.rect,
                    &pan_state,
                    local_col,
                    local_row,
                )
                .map(|(node_id, _)| node_id)
            });
    state.hovered_node = hit_patcher_node(&patch, node.rect, &pan_state, local_col, local_row);
    set_patcher_interaction_state(key, state);
}

pub(super) fn open_selected_macro_node(
    node: &LayoutNode,
    state: &mut PatcherInteractionState,
) -> bool {
    let Ok((_, root_patch)) = load_patch_from_props(&node.props) else {
        return false;
    };
    let view_key = active_patcher_view_key(state);
    let patch = active_patcher_patch(&root_patch, state);
    let patch = patch_with_interaction_state(patch, state, &view_key);
    let Some(macro_name) = state.selected_nodes.iter().find_map(|selected| {
        patch
            .nodes
            .iter()
            .find(|patch_node| {
                patch_node.id == *selected && patch_node.kind == NodeKind::MacroInstance
            })
            .map(|patch_node| patch_node.op.clone())
    }) else {
        return false;
    };
    if !root_patch
        .macros
        .iter()
        .any(|macro_patch| macro_patch.name == macro_name)
    {
        return false;
    }
    state.active_macro = Some(macro_name);
    state.selected_nodes.clear();
    state.selected_cable = None;
    state.hovered_node = None;
    state.hovered_macro_drill_in = None;
    state.hover_back_button = false;
    state.drag = None;
    state.text_edit = None;
    true
}

pub(super) fn handle_patcher_double_click(
    node: &LayoutNode,
    local_col: f32,
    local_row: f32,
) -> bool {
    let key = patcher_state_key(node);
    let Ok((_, root_patch)) = load_patch_from_props(&node.props) else {
        return false;
    };
    let mut state = get_patcher_interaction_state(key);
    if state.active_macro.is_some()
        && rect_contains(patcher_breadcrumb_rect(node.rect), local_col, local_row)
    {
        navigate_patcher_to_root(key, &mut state);
        return true;
    }

    let view_key = active_patcher_view_key(&state);
    let patch = active_patcher_patch(&root_patch, &state);
    let patch = patch_with_interaction_state(patch, &state, &view_key);
    let mut pan_state = get_patcher_pan_state(key);
    sync_patcher_pan_bounds(node, &mut pan_state);
    if let Some(node_id) = hit_patcher_node(&patch, node.rect, &pan_state, local_col, local_row) {
        let node_rects = patch_node_rects(&patch, node.rect, &pan_state);
        let Some(patch_node) = patch
            .nodes
            .iter()
            .find(|patch_node| patch_node.id == node_id)
        else {
            return false;
        };
        let Some(rect) = node_rects.get(&node_id) else {
            return false;
        };
        let text = node_display_label(patch_node);
        let cursor_pos = patcher_text_cursor_at_col(*rect, &text, local_col);
        ensure_source_node_edit(&mut state, &view_key, patch_node, text.clone());
        begin_patcher_text_edit(&mut state, node_id, text, cursor_pos);
        set_patcher_interaction_state(key, state);
        return true;
    }

    let origin = patcher_origin(node.rect, &pan_state);
    let created_id = allocate_created_node(
        &mut state,
        &view_key,
        (local_col - origin.0, local_row - origin.1),
    );
    begin_patcher_text_edit(&mut state, created_id, String::new(), 0);
    set_patcher_interaction_state(key, state);
    true
}

fn navigate_patcher_to_root(key: u64, state: &mut PatcherInteractionState) {
    state.active_macro = None;
    state.selected_nodes.clear();
    state.selected_cable = None;
    state.hovered_node = None;
    state.hovered_macro_drill_in = None;
    state.hover_back_button = false;
    state.drag = None;
    set_patcher_interaction_state(key, state.clone());
    reset_patcher_pan(key);
}

pub(super) fn reset_patcher_pan(key: u64) {
    let mut pan = get_patcher_pan_state(key);
    pan.offset_x = 0.0;
    pan.offset_y = 0.0;
    set_patcher_pan_state(key, pan);
}
