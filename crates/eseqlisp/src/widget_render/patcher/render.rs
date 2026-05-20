use std::collections::HashMap;

#[cfg(target_os = "macos")]
use super::super::text_input::{cursor_x_from_char_cache, selection_range as text_selection_range};
use super::super::{CellBuffer, styled_cell};
#[cfg(target_os = "macos")]
use super::super::{
    MetalCirclePrimitive, MetalCircleVisibleHalf, MetalPatchCablePrimitive, MetalPrimitive,
    MetalProportionalTextPrimitive, MetalQuadPrimitive, MetalRectPrimitive, WidgetInstance,
    WidgetViewport, ndc_bounds,
};
#[cfg(target_os = "macos")]
use crate::layout::LayoutNode;
use crate::layout::Rect;
use crate::theme;
use crate::vm::Value;

use super::display::{node_display_label, preview};
use super::geometry::{
    cable_edit_points, connection_endpoints, patch_content_size, patch_node_rects,
    patcher_back_button_rect, patcher_macro_drill_in_rect, port_center, rect_from_points,
};
use super::load_patch_from_props;
use super::metrics::{
    CABLE_HANDLE_RADIUS_PX, CODE_NODE_FONT_SIZE, NODE_BORDER_INSET, NODE_CORNER_RADIUS_PX,
    NODE_FONT_SIZE, NODE_TEXT_COL_OFFSET, PORT_INNER_DIAMETER_PX, PORT_OUTER_DIAMETER_PX,
};
use super::model::{ArgValue, ConnectionKind, NodeKind, Patch, PatchNode};
use super::state::{
    PatcherDragState, PatcherInteractionState, PatcherPanState, PatcherTextEdit,
    active_patcher_patch, active_patcher_view_key, get_patcher_interaction_state,
    get_patcher_pan_state, patch_with_interaction_state, patcher_back_label, patcher_breadcrumb,
    patcher_state_key, set_patcher_pan_state, source_connection_id,
};

pub(super) fn render_tui(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    for row_offset in 0..rect.height.round() as u16 {
        for col_offset in 0..rect.width.round() as u16 {
            let row = rect.row.round() as u16 + row_offset;
            let col = rect.col.round() as u16 + col_offset;
            buf.set(
                row,
                col,
                styled_cell(' ', theme::PATCHER_TEXT(), Some(theme::PATCHER_BG())),
            );
        }
    }
    let title = match load_patch_from_props(props) {
        Ok((path, patch)) => format!(
            "patcher {}  nodes:{}  cables:{}",
            path.display(),
            patch.nodes.len(),
            patch.connections.len()
        ),
        Err(error) => format!("patcher error: {error}"),
    };
    for (idx, ch) in title.chars().take(rect.width as usize).enumerate() {
        buf.set(
            rect.row.round() as u16,
            rect.col.round() as u16 + idx as u16,
            styled_cell(ch, theme::PATCHER_NODE_TEXT(), Some(theme::PATCHER_BG())),
        );
    }
}

#[cfg(target_os = "macos")]
pub(super) fn build_metal_primitives_for_patcher(
    node: &LayoutNode,
    viewport: WidgetViewport,
) -> Vec<MetalPrimitive> {
    let mut prims = Vec::new();
    prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
        rect: node.rect,
        color: theme::PATCHER_BG(),
    }));
    let key = patcher_state_key(node);
    let mut pan_state = get_patcher_pan_state(key);
    pan_state.viewport_width = node.rect.width;
    pan_state.viewport_height = node.rect.height;

    let loaded = load_patch_from_props(&node.props);
    match loaded {
        Ok((path, root_patch)) => {
            let interaction_state = get_patcher_interaction_state(key);
            let view_key = active_patcher_view_key(&interaction_state);
            let patch = active_patcher_patch(&root_patch, &interaction_state);
            let patch = patch_with_interaction_state(patch, &interaction_state, &view_key);
            let content_size = patch_content_size(&patch);
            pan_state.content_width = content_size.0.max(node.rect.width);
            pan_state.content_height = content_size.1.max(node.rect.height);
            set_patcher_pan_state(key, pan_state.clone());
            pan_state = get_patcher_pan_state(key);
            push_grid(
                &mut prims,
                node.rect,
                pan_state.offset_x,
                pan_state.offset_y,
            );
            draw_patch(
                &mut prims,
                &patch,
                node.rect,
                viewport,
                &pan_state,
                &interaction_state,
            );
            push_marquee(&mut prims, node.rect, viewport, &interaction_state);
            push_back_button(&mut prims, node.rect, viewport, &interaction_state);
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: node.rect.row + 0.7,
                    col: if interaction_state.active_macro.is_some() {
                        node.rect.col + 3.2
                    } else {
                        node.rect.col + 1.0
                    },
                    align_width: if interaction_state.active_macro.is_some() {
                        node.rect.width - 4.2
                    } else {
                        node.rect.width - 2.0
                    },
                    h_align: 0.0,
                    text: format!("{}", patcher_breadcrumb(&path, &interaction_state)),
                    font_size: 12.0,
                    fg: theme::PATCHER_TEXT_MUTED(),
                    bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
                },
            ));
            if !patch.diagnostics.is_empty() {
                prims.push(MetalPrimitive::ProportionalText(
                    MetalProportionalTextPrimitive {
                        row: node.rect.row + node.rect.height - 1.7,
                        col: node.rect.col + 1.0,
                        align_width: node.rect.width - 2.0,
                        h_align: 0.0,
                        text: format!(
                            "{} unsupported form(s) rendered as code islands",
                            patch.diagnostics.len()
                        ),
                        font_size: 11.0,
                        fg: theme::PATCHER_ERROR(),
                        bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
                    },
                ));
            }
        }
        Err(error) => {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: node.rect.row + 2.0,
                    col: node.rect.col + 2.0,
                    align_width: node.rect.width - 4.0,
                    h_align: 0.0,
                    text: error,
                    font_size: 13.0,
                    fg: theme::PATCHER_ERROR(),
                    bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
                },
            ));
        }
    }
    prims
}

#[cfg(target_os = "macos")]
fn push_grid(prims: &mut Vec<MetalPrimitive>, rect: Rect, offset_x: f32, offset_y: f32) {
    let minor = theme::PATCHER_GRID_MINOR();
    let major = theme::PATCHER_GRID_MAJOR();
    let col_spacing = 4.0;
    let row_spacing = 2.5;
    let col_phase = offset_x.rem_euclid(col_spacing);
    let row_phase = offset_y.rem_euclid(row_spacing);
    let mut col = rect.col - col_phase;
    let mut idx = (offset_x / col_spacing).floor().max(0.0) as usize;
    while col < rect.col + rect.width {
        prims.push(MetalPrimitive::Quad(MetalQuadPrimitive {
            x: col,
            y: rect.row,
            width: 0.035,
            height: rect.height,
            color: if idx % 5 == 0 { major } else { minor },
        }));
        col += col_spacing;
        idx += 1;
    }
    let mut row = rect.row - row_phase;
    idx = (offset_y / row_spacing).floor().max(0.0) as usize;
    while row < rect.row + rect.height {
        prims.push(MetalPrimitive::Quad(MetalQuadPrimitive {
            x: rect.col,
            y: row,
            width: rect.width,
            height: 0.035,
            color: if idx % 5 == 0 { major } else { minor },
        }));
        row += row_spacing;
        idx += 1;
    }
}

#[cfg(target_os = "macos")]
fn push_marquee(
    prims: &mut Vec<MetalPrimitive>,
    _rect: Rect,
    _viewport: WidgetViewport,
    interaction_state: &PatcherInteractionState,
) {
    let Some(PatcherDragState::Marquee {
        start_col,
        start_row,
        current_col,
        current_row,
        ..
    }) = &interaction_state.drag
    else {
        return;
    };
    let marquee = rect_from_points(*start_col, *start_row, *current_col, *current_row);
    if marquee.width < 0.05 || marquee.height < 0.05 {
        return;
    }
    let fill = theme::PATCHER_MARQUEE_FILL();
    let border = theme::PATCHER_MARQUEE_BORDER();
    prims.push(MetalPrimitive::Quad(MetalQuadPrimitive {
        x: marquee.col,
        y: marquee.row,
        width: marquee.width,
        height: marquee.height,
        color: fill,
    }));
    let thickness = 0.08;
    prims.push(MetalPrimitive::Quad(MetalQuadPrimitive {
        x: marquee.col,
        y: marquee.row,
        width: marquee.width,
        height: thickness,
        color: border,
    }));
    prims.push(MetalPrimitive::Quad(MetalQuadPrimitive {
        x: marquee.col,
        y: marquee.row + marquee.height - thickness,
        width: marquee.width,
        height: thickness,
        color: border,
    }));
    prims.push(MetalPrimitive::Quad(MetalQuadPrimitive {
        x: marquee.col,
        y: marquee.row,
        width: thickness,
        height: marquee.height,
        color: border,
    }));
    prims.push(MetalPrimitive::Quad(MetalQuadPrimitive {
        x: marquee.col + marquee.width - thickness,
        y: marquee.row,
        width: thickness,
        height: marquee.height,
        color: border,
    }));
}

#[cfg(target_os = "macos")]
fn push_back_button(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    viewport: WidgetViewport,
    interaction_state: &PatcherInteractionState,
) {
    let Some(label) = patcher_back_label(interaction_state) else {
        return;
    };
    let button_rect = patcher_back_button_rect(rect);
    let border = if interaction_state.hover_back_button {
        theme::PATCHER_BACK_BUTTON_HOVER_BORDER()
    } else {
        theme::PATCHER_BACK_BUTTON_BORDER()
    };
    let bg = if interaction_state.hover_back_button {
        theme::PATCHER_BACK_BUTTON_HOVER_BG()
    } else {
        theme::PATCHER_BACK_BUTTON_BG()
    };
    push_rounded_rect(prims, button_rect, border, viewport, 9.0, false);
    push_rounded_rect(
        prims,
        Rect {
            row: button_rect.row + 0.08,
            col: button_rect.col + 0.08,
            width: (button_rect.width - 0.16).max(0.0),
            height: (button_rect.height - 0.16).max(0.0),
        },
        bg,
        viewport,
        7.0,
        false,
    );
    prims.push(MetalPrimitive::ProportionalText(
        MetalProportionalTextPrimitive {
            row: button_rect.row + 0.24,
            col: button_rect.col + 0.7,
            align_width: button_rect.width - 1.1,
            h_align: 0.0,
            text: label.to_string(),
            font_size: 11.0,
            fg: if interaction_state.hover_back_button {
                theme::PATCHER_BACK_BUTTON_HOVER_TEXT()
            } else {
                theme::PATCHER_BACK_BUTTON_TEXT()
            },
            bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
        },
    ));
}

#[cfg(target_os = "macos")]
pub(super) fn draw_patch(
    prims: &mut Vec<MetalPrimitive>,
    patch: &Patch,
    rect: Rect,
    viewport: WidgetViewport,
    pan_state: &PatcherPanState,
    interaction_state: &PatcherInteractionState,
) {
    let node_rects = patch_node_rects(patch, rect, pan_state);
    let input_indices = patch_input_indices(patch);
    let input_slot_counts = patch_input_slot_counts(patch, &input_indices);
    let output_counts = patch_output_counts(patch);
    let dragged_cable = match &interaction_state.drag {
        Some(PatcherDragState::CableEndpoint { cable_id, .. }) => Some(cable_id.as_str()),
        _ => None,
    };

    for connection in &patch.connections {
        let connection_id = source_connection_id(connection);
        if dragged_cable == Some(connection_id.as_str()) {
            continue;
        }
        let Some((start, end)) = connection_endpoints(
            connection,
            &node_rects,
            &input_indices,
            &input_slot_counts,
            &output_counts,
        ) else {
            continue;
        };
        let selected = interaction_state.selected_cable.as_deref() == Some(connection_id.as_str());
        push_cable(prims, start, end, connection.kind, selected);
        if selected {
            push_cable_handles(prims, start, end);
        }
    }
    if let Some(PatcherDragState::Cable {
        start_col,
        start_row,
        current_col,
        current_row,
        ..
    }) = &interaction_state.drag
    {
        push_cable(
            prims,
            (*start_col, *start_row),
            (*current_col, *current_row),
            ConnectionKind::Forward,
            false,
        );
    }
    if let Some(PatcherDragState::CableEndpoint {
        endpoint,
        start_col,
        start_row,
        end_col,
        end_row,
        current_col,
        current_row,
        ..
    }) = &interaction_state.drag
    {
        let (start, end) = match endpoint {
            super::model::CableEndpoint::From => {
                ((*current_col, *current_row), (*end_col, *end_row))
            }
            super::model::CableEndpoint::To => {
                ((*start_col, *start_row), (*current_col, *current_row))
            }
        };
        push_cable(prims, start, end, ConnectionKind::Forward, true);
        push_cable_handles(prims, start, end);
    }

    for node in &patch.nodes {
        let Some(rect) = node_rects.get(&node.id).copied() else {
            continue;
        };
        let highlighted_inputs = highlighted_inputs_for_node(&interaction_state.drag, &node.id);
        let highlighted_outputs = highlighted_outputs_for_node(&interaction_state.drag, &node.id);
        push_node(
            prims,
            node,
            rect,
            input_indices
                .get(&node.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            input_slot_counts.get(&node.id).copied().unwrap_or(0),
            output_counts.get(&node.id).copied().unwrap_or(0),
            viewport,
            interaction_state.selected_nodes.contains(&node.id),
            interaction_state.hovered_node.as_deref() == Some(node.id.as_str()),
            interaction_state
                .text_edit
                .as_ref()
                .filter(|edit| edit.node_id == node.id),
            interaction_state.hovered_macro_drill_in.as_deref() == Some(node.id.as_str()),
            &highlighted_inputs,
            &highlighted_outputs,
        );
    }
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
        let count = input_indices.get(&node.id).map(Vec::len).unwrap_or(0);
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

#[cfg(target_os = "macos")]
fn push_cable(
    prims: &mut Vec<MetalPrimitive>,
    start: (f32, f32),
    end: (f32, f32),
    kind: ConnectionKind,
    selected: bool,
) {
    let color = if selected {
        theme::PATCHER_ERROR()
    } else {
        match kind {
            ConnectionKind::Forward => theme::PATCHER_CABLE(),
            ConnectionKind::Feedback => theme::PATCHER_FEEDBACK_CABLE(),
        }
    };
    let curve = super::super::cable::cable_curve(start, end);
    prims.push(MetalPrimitive::PatchCable(MetalPatchCablePrimitive {
        start: [curve.p0.0, curve.p0.1],
        control1: [curve.p1.0, curve.p1.1],
        control2: [curve.p2.0, curve.p2.1],
        end: [curve.p3.0, curve.p3.1],
        radius_px: if kind == ConnectionKind::Feedback {
            3.6
        } else {
            4.4
        },
        color,
    }));
}

#[cfg(target_os = "macos")]
fn push_cable_handles(prims: &mut Vec<MetalPrimitive>, start: (f32, f32), end: (f32, f32)) {
    let (from_handle, to_handle) = cable_edit_points(start, end);
    for center in [from_handle, to_handle] {
        prims.push(MetalPrimitive::Circle(MetalCirclePrimitive {
            center: [center.0, center.1],
            radius_px: CABLE_HANDLE_RADIUS_PX,
            color: theme::PATCHER_ERROR(),
            visible_half: MetalCircleVisibleHalf::Full,
        }));
        prims.push(MetalPrimitive::Circle(MetalCirclePrimitive {
            center: [center.0, center.1],
            radius_px: CABLE_HANDLE_RADIUS_PX * 0.52,
            color: theme::PATCHER_BG(),
            visible_half: MetalCircleVisibleHalf::Full,
        }));
    }
}

#[cfg(target_os = "macos")]
fn push_node(
    prims: &mut Vec<MetalPrimitive>,
    node: &PatchNode,
    rect: Rect,
    input_indices: &[usize],
    input_slot_count: usize,
    output_count: usize,
    viewport: WidgetViewport,
    selected: bool,
    hovered: bool,
    edit: Option<&PatcherTextEdit>,
    macro_drill_in_hovered: bool,
    highlighted_inputs: &[usize],
    highlighted_outputs: &[usize],
) {
    let (bg, mut border, text) = match node.kind {
        NodeKind::In | NodeKind::Out => (
            theme::PATCHER_IO_NODE_BG(),
            theme::PATCHER_IO_NODE_BORDER(),
            theme::PATCHER_IO_NODE_TEXT(),
        ),
        NodeKind::Param => (
            theme::PATCHER_PARAM_NODE_BG(),
            theme::PATCHER_PARAM_NODE_BORDER(),
            theme::PATCHER_PARAM_NODE_TEXT(),
        ),
        NodeKind::CodeIsland => (
            theme::PATCHER_CODE_NODE_BG(),
            theme::PATCHER_CODE_NODE_BORDER(),
            theme::PATCHER_CODE_NODE_TEXT(),
        ),
        _ => (
            theme::PATCHER_NODE_BG(),
            if node.diagnostic.is_some() {
                theme::PATCHER_ERROR()
            } else {
                theme::PATCHER_NODE_BORDER()
            },
            theme::PATCHER_NODE_TEXT(),
        ),
    };
    if hovered {
        border = theme::PATCHER_NODE_HOVER_BORDER();
    }
    if selected {
        border = theme::PATCHER_NODE_SELECTED_BORDER();
    }
    push_rounded_rect(prims, rect, border, viewport, NODE_CORNER_RADIUS_PX, false);
    push_rounded_rect(
        prims,
        Rect {
            row: rect.row + NODE_BORDER_INSET,
            col: rect.col + NODE_BORDER_INSET,
            width: (rect.width - NODE_BORDER_INSET * 2.0).max(0.0),
            height: (rect.height - NODE_BORDER_INSET * 2.0).max(0.0),
        },
        bg,
        viewport,
        (NODE_CORNER_RADIUS_PX - 3.0).max(0.0),
        false,
    );
    for (visible_index, &index) in input_indices.iter().enumerate() {
        push_port(
            prims,
            port_center(rect, visible_index, input_slot_count, true),
            true,
            bg,
            highlighted_inputs.contains(&index),
        );
    }
    for index in 0..output_count {
        push_port(
            prims,
            port_center(rect, index, output_count, false),
            false,
            bg,
            highlighted_outputs.contains(&index),
        );
    }
    push_node_edit_selection(prims, rect, viewport, edit);
    push_node_label(prims, node, rect, text);
    push_macro_drill_in(prims, node, rect, macro_drill_in_hovered);
    push_node_edit_cursor(prims, rect, viewport, edit);
    if let Some(diagnostic) = &node.diagnostic {
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: rect.row + 2.65,
                col: rect.col + 1.0,
                align_width: rect.width - 2.0,
                h_align: 0.0,
                text: preview(diagnostic, 32),
                font_size: 9.5,
                fg: theme::PATCHER_ERROR(),
                bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
            },
        ));
    }
}

fn highlighted_inputs_for_node(drag: &Option<PatcherDragState>, node_id: &str) -> Vec<usize> {
    match drag {
        Some(PatcherDragState::Cable {
            target: Some(target),
            ..
        })
        | Some(PatcherDragState::CableEndpoint {
            target_to: Some(target),
            ..
        }) if target.node_id == node_id => vec![target.input_index],
        _ => Vec::new(),
    }
}

fn highlighted_outputs_for_node(drag: &Option<PatcherDragState>, node_id: &str) -> Vec<usize> {
    match drag {
        Some(PatcherDragState::CableEndpoint {
            target_from: Some(target),
            ..
        }) if target.node_id == node_id => vec![target.output_index],
        _ => Vec::new(),
    }
}

#[cfg(target_os = "macos")]
fn push_node_edit_selection(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    viewport: WidgetViewport,
    edit: Option<&PatcherTextEdit>,
) {
    let Some(edit) = edit else {
        return;
    };
    let Some((start, end)) = text_selection_range(&edit.state) else {
        return;
    };
    let x = rect.col
        + NODE_TEXT_COL_OFFSET
        + cursor_x_from_char_cache(&edit.text, NODE_FONT_SIZE, start, viewport.cell_w);
    let end_x = rect.col
        + NODE_TEXT_COL_OFFSET
        + cursor_x_from_char_cache(&edit.text, NODE_FONT_SIZE, end, viewport.cell_w);
    let width = end_x - x;
    if width <= 0.0 {
        return;
    }
    prims.push(MetalPrimitive::ForegroundRect(MetalRectPrimitive {
        rect: Rect {
            row: rect.row + 0.18,
            col: x,
            width,
            height: (rect.height - 0.36).max(0.1),
        },
        color: theme::PATCHER_EDIT_SELECTION(),
    }));
}

#[cfg(target_os = "macos")]
fn push_node_edit_cursor(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    viewport: WidgetViewport,
    edit: Option<&PatcherTextEdit>,
) {
    let Some(edit) = edit else {
        return;
    };
    let cursor_pos = edit.state.cursor_pos.min(edit.text.chars().count());
    let x = rect.col
        + NODE_TEXT_COL_OFFSET
        + cursor_x_from_char_cache(&edit.text, NODE_FONT_SIZE, cursor_pos, viewport.cell_w);
    prims.push(MetalPrimitive::ForegroundRect(MetalRectPrimitive {
        rect: Rect {
            row: rect.row + 0.23,
            col: x,
            width: 0.08,
            height: (rect.height - 0.46).max(0.1),
        },
        color: theme::PATCHER_EDIT_CURSOR(),
    }));
}

#[cfg(target_os = "macos")]
fn push_macro_drill_in(
    prims: &mut Vec<MetalPrimitive>,
    node: &PatchNode,
    rect: Rect,
    hovered: bool,
) {
    if node.kind != NodeKind::MacroInstance {
        return;
    }
    let button_rect = patcher_macro_drill_in_rect(rect);
    let color = if hovered {
        theme::PATCHER_BACK_BUTTON_HOVER_TEXT()
    } else {
        theme::PATCHER_TEXT_MUTED()
    };
    prims.push(MetalPrimitive::ProportionalText(
        MetalProportionalTextPrimitive {
            row: button_rect.row + 0.1,
            col: button_rect.col + 0.34,
            align_width: button_rect.width,
            h_align: 0.0,
            text: ">".to_string(),
            font_size: 12.0,
            fg: color,
            bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
        },
    ));
}

#[cfg(target_os = "macos")]
fn push_node_label(
    prims: &mut Vec<MetalPrimitive>,
    node: &PatchNode,
    rect: Rect,
    head_color: crate::backend::Color,
) {
    let font_size = if node.kind == NodeKind::CodeIsland {
        CODE_NODE_FONT_SIZE
    } else {
        NODE_FONT_SIZE
    };
    let baseline_row = if node.kind == NodeKind::CodeIsland {
        rect.row + 0.55
    } else {
        rect.row + 0.36
    };
    let text_col = rect.col + NODE_TEXT_COL_OFFSET;
    let label = node_display_label(node);
    let (head, tail) = split_label_head_tail(&label);
    let bg = crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0);
    prims.push(MetalPrimitive::ProportionalText(
        MetalProportionalTextPrimitive {
            row: baseline_row,
            col: text_col,
            align_width: rect.width - 1.84,
            h_align: 0.0,
            text: head.to_string(),
            font_size,
            fg: head_color,
            bg,
        },
    ));
    if !tail.is_empty() {
        let tail_col = text_col + estimated_text_cells(head, font_size) + 0.72;
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: baseline_row,
                col: tail_col,
                align_width: (rect.col + rect.width - tail_col - 0.92).max(0.0),
                h_align: 0.0,
                text: tail.to_string(),
                font_size,
                fg: theme::PATCHER_NODE_TAIL_TEXT(),
                bg,
            },
        ));
    }
}

#[cfg(target_os = "macos")]
fn split_label_head_tail(label: &str) -> (&str, &str) {
    let trimmed = label.trim();
    match trimmed.find(char::is_whitespace) {
        Some(idx) => {
            let (head, tail) = trimmed.split_at(idx);
            (head, tail.trim_start())
        }
        None => (trimmed, ""),
    }
}

#[cfg(target_os = "macos")]
fn estimated_text_cells(text: &str, font_size: f32) -> f32 {
    let width_per_char = (font_size / NODE_FONT_SIZE) * 1.03;
    text.chars().count() as f32 * width_per_char
}

#[cfg(target_os = "macos")]
fn push_port(
    prims: &mut Vec<MetalPrimitive>,
    center: (f32, f32),
    input: bool,
    node_bg: crate::backend::Color,
    highlighted: bool,
) {
    let color = if highlighted {
        theme::PATCHER_NODE_SELECTED_BORDER()
    } else if input {
        theme::PATCHER_PORT_INPUT()
    } else {
        theme::PATCHER_PORT_OUTPUT()
    };
    let visible_half = if input {
        MetalCircleVisibleHalf::Bottom
    } else {
        MetalCircleVisibleHalf::Top
    };
    prims.push(MetalPrimitive::Circle(MetalCirclePrimitive {
        center: [center.0, center.1],
        radius_px: PORT_OUTER_DIAMETER_PX * 0.5,
        color,
        visible_half,
    }));
    prims.push(MetalPrimitive::Circle(MetalCirclePrimitive {
        center: [center.0, center.1],
        radius_px: PORT_INNER_DIAMETER_PX * 0.5,
        color: node_bg,
        visible_half,
    }));
}

#[cfg(target_os = "macos")]
fn normalized_corner_radius(rect: Rect, viewport: WidgetViewport, radius_px: f32) -> f32 {
    if radius_px <= 0.0 {
        return 0.001;
    }
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((radius_px * 2.0) / px_h).clamp(0.001, 0.5)
}

#[cfg(target_os = "macos")]
fn push_rounded_rect(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    color: crate::backend::Color,
    viewport: WidgetViewport,
    radius_px: f32,
    is_background: bool,
) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    prims.push(MetalPrimitive::WidgetInstance {
        widget_type: "box".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [0.0; 4],
            uniform_b: [0.0; 4],
            color_a: color.to_rgba(),
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: normalized_corner_radius(rect, viewport, radius_px),
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
        is_background,
    });
}
