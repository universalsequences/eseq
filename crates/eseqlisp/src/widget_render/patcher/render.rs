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
use crate::layout::{Rect, f64_to_f32};
use crate::theme;
use crate::vm::Value;

use super::display::{node_display_label, preview};
use super::geometry::{
    connection_cable_edit_points, connection_endpoints, patch_content_size, patch_input_indices,
    patch_input_slot_counts, patch_node_rects, patch_output_counts, patcher_back_button_rect,
    patcher_zoom, port_center, rect_from_points,
};
use super::load_patch_from_props;
use super::metrics::{
    CABLE_HANDLE_RADIUS_PX, CODE_NODE_FONT_SIZE, NODE_BORDER_WIDTH_PX, NODE_CORNER_RADIUS_PX,
    NODE_FONT_SIZE, NODE_TEXT_COL_OFFSET, PORT_INNER_DIAMETER_PX, PORT_OUTER_DIAMETER_PX,
    SEGMENTED_CABLE_CORNER_RADIUS_CELLS,
};
use super::model::{ConnectionKind, NodeKind, Patch, PatchConnection, PatchNode};
#[cfg(target_os = "macos")]
use super::project::OperatorPortDocumentation;
use super::state::{
    PatcherDragState, PatcherInteractionState, PatcherPanState, PatcherTextEdit,
    active_patcher_patch, active_patcher_view_key, get_patcher_interaction_state,
    get_patcher_pan_state, patch_with_interaction_state, patcher_back_label, patcher_breadcrumb,
    patcher_state_key, set_patcher_pan_state, source_connection_id,
};
#[cfg(target_os = "macos")]
use super::text::patcher_autocomplete_suggestions;

#[cfg(target_os = "macos")]
const AUTOCOMPLETE_PANEL_CORNER_RADIUS_PX: f32 = 9.0;

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
    let pan_uninitialized = pan_state.content_width == 0.0 && pan_state.content_height == 0.0;
    if pan_uninitialized {
        if let Some(initial_zoom) = patcher_initial_zoom(&node.props) {
            pan_state.zoom = initial_zoom;
        }
    }
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
            if pan_uninitialized && patcher_fit_enabled(&node.props) {
                pan_state.zoom = fit_zoom(content_size, node.rect);
                pan_state.offset_x = 0.0;
                pan_state.offset_y = 0.0;
            }
            let zoom = patcher_zoom(&pan_state);
            pan_state.content_width = (content_size.0 * zoom).max(node.rect.width);
            pan_state.content_height = (content_size.1 * zoom).max(node.rect.height);
            set_patcher_pan_state(key, pan_state.clone());
            pan_state = get_patcher_pan_state(key);
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
                    scale: 1.0,
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
                        scale: 1.0,
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
                    scale: 1.0,
                    fg: theme::PATCHER_ERROR(),
                    bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
                },
            ));
        }
    }
    prims
}

#[cfg(target_os = "macos")]
fn patcher_initial_zoom(props: &HashMap<String, Value>) -> Option<f32> {
    let Some(Value::Number(zoom)) = props.get("initial-zoom") else {
        return None;
    };
    let zoom = f64_to_f32(*zoom);
    (zoom.is_finite() && zoom > 0.0).then_some(zoom)
}

#[cfg(target_os = "macos")]
fn patcher_fit_enabled(props: &HashMap<String, Value>) -> bool {
    !matches!(props.get("fit"), Some(Value::Nil) | None)
}

#[cfg(target_os = "macos")]
fn fit_zoom(content_size: (f32, f32), rect: Rect) -> f32 {
    let width_zoom = rect.width / content_size.0.max(1.0);
    let height_zoom = rect.height / content_size.1.max(1.0);
    width_zoom.min(height_zoom).clamp(0.05, 2.5)
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
    push_rounded_rect(prims, button_rect, border, viewport, 18.0, false);
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
        15.0,
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
            scale: 1.0,
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
    let origin = super::geometry::patcher_origin(rect, pan_state);
    let zoom = patcher_zoom(pan_state);
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
        push_cable(prims, start, end, connection, origin.1, zoom, selected);
        if selected {
            push_cable_handles(prims, connection, start, end, zoom);
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
        push_preview_cable(
            prims,
            (*start_col, *start_row),
            (*current_col, *current_row),
            ConnectionKind::Forward,
            zoom,
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
        push_preview_cable(prims, start, end, ConnectionKind::Forward, zoom, true);
        let preview_connection = PatchConnection {
            from_node: String::new(),
            from_output: 0,
            to_node: String::new(),
            to_input: 0,
            kind: ConnectionKind::Forward,
            segment: None,
            source: None,
        };
        push_cable_handles(prims, &preview_connection, start, end, zoom);
    }

    let mut active_edit_panel = None;
    for node in &patch.nodes {
        let Some(rect) = node_rects.get(&node.id).copied() else {
            continue;
        };
        let active_edit = interaction_state
            .text_edit
            .as_ref()
            .filter(|edit| edit.node_id == node.id);
        if let Some(edit) = active_edit {
            active_edit_panel = Some((rect, edit));
        }
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
            active_edit,
            &highlighted_inputs,
            &highlighted_outputs,
            zoom,
        );
    }
    if let Some((node_rect, edit)) = active_edit_panel {
        push_autocomplete_panel(prims, node_rect, edit, viewport, zoom);
    }
}

#[cfg(target_os = "macos")]
fn push_autocomplete_panel(
    prims: &mut Vec<MetalPrimitive>,
    node_rect: Rect,
    edit: &PatcherTextEdit,
    viewport: WidgetViewport,
    zoom: f32,
) {
    let suggestions = patcher_autocomplete_suggestions(edit);
    if suggestions.is_empty() {
        return;
    }
    let row_height = 1.35 * zoom;
    let padding = 0.45 * zoom;
    let panel_width = (node_rect.width.max(18.0 * zoom)).min(28.0 * zoom);
    let panel = Rect {
        row: node_rect.row + node_rect.height + 0.35 * zoom,
        col: node_rect.col,
        width: panel_width,
        height: padding * 2.0 + row_height * suggestions.len() as f32,
    };
    push_autocomplete_panel_chrome(
        prims,
        panel,
        theme::PATCHER_AUTOCOMPLETE_BG(),
        theme::PATCHER_AUTOCOMPLETE_BORDER(),
        viewport,
        zoom,
    );
    for (index, suggestion) in suggestions.iter().enumerate() {
        let row = panel.row + padding + index as f32 * row_height;
        if index == edit.autocomplete_selected.min(suggestions.len() - 1) {
            prims.push(MetalPrimitive::ForegroundRect(MetalRectPrimitive {
                rect: Rect {
                    row,
                    col: panel.col + 0.25 * zoom,
                    width: panel.width - 0.5 * zoom,
                    height: row_height,
                },
                color: theme::PATCHER_AUTOCOMPLETE_SELECTED_BG(),
            }));
        }
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: row + 0.18 * zoom,
                col: panel.col + 0.8 * zoom,
                align_width: panel.width - 1.6 * zoom,
                h_align: 0.0,
                text: suggestion.name.clone(),
                font_size: 11.5,
                scale: zoom,
                fg: theme::PATCHER_NODE_TAIL_TEXT(),
                bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
            },
        ));
    }
    let selected_index = edit.autocomplete_selected.min(suggestions.len() - 1);
    push_autocomplete_documentation_panel(
        prims,
        panel,
        &suggestions[selected_index],
        viewport,
        zoom,
    );
}

#[cfg(target_os = "macos")]
fn push_autocomplete_documentation_panel(
    prims: &mut Vec<MetalPrimitive>,
    list_panel: Rect,
    suggestion: &super::text::PatcherAutocompleteSuggestion,
    viewport: WidgetViewport,
    zoom: f32,
) {
    let lines = autocomplete_doc_lines(suggestion);
    if lines.is_empty() {
        return;
    }
    let row_height = 1.08 * zoom;
    let padding = 0.65 * zoom;
    let panel_width = 46.0 * zoom;
    let align_width = panel_width - 1.6 * zoom;
    let visual_line_count =
        autocomplete_wrapped_doc_line_count(&lines, align_width, viewport, zoom);
    let panel = Rect {
        row: list_panel.row,
        col: list_panel.col + list_panel.width + 0.55 * zoom,
        width: panel_width,
        height: padding * 2.0 + row_height * visual_line_count as f32,
    };
    push_autocomplete_panel_chrome(
        prims,
        panel,
        theme::PATCHER_AUTOCOMPLETE_BG(),
        theme::PATCHER_AUTOCOMPLETE_BORDER(),
        viewport,
        zoom,
    );
    let max_chars = autocomplete_doc_wrap_chars(align_width, viewport, zoom);
    let mut visual_index = 0;
    for line in lines {
        for wrapped in wrap_autocomplete_doc_line(&line, max_chars) {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: panel.row + padding + visual_index as f32 * row_height,
                    col: panel.col + 0.8 * zoom,
                    align_width,
                    h_align: 0.0,
                    text: wrapped,
                    font_size: 10.0,
                    scale: zoom,
                    fg: theme::PATCHER_TEXT_MUTED(),
                    bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
                },
            ));
            visual_index += 1;
        }
    }
}

#[cfg(target_os = "macos")]
fn autocomplete_doc_lines(suggestion: &super::text::PatcherAutocompleteSuggestion) -> Vec<String> {
    let Some(documentation) = suggestion.documentation.as_ref() else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    if let Some(summary) = &documentation.summary {
        lines.push(summary.clone());
    }
    lines.extend(documentation.signatures.iter().cloned());
    if !documentation.inputs.is_empty() {
        lines.push(format!(
            "inlets: {}",
            format_operator_ports(&documentation.inputs)
        ));
    }
    if !documentation.outputs.is_empty() {
        lines.push(format!(
            "outlets: {}",
            format_operator_ports(&documentation.outputs)
        ));
    }
    lines
}

#[cfg(target_os = "macos")]
fn format_operator_ports(ports: &[OperatorPortDocumentation]) -> String {
    ports
        .iter()
        .map(|port| {
            let mut parts = Vec::new();
            if let Some(index) = port.index {
                parts.push(index.to_string());
            }
            if let Some(name) = &port.name {
                let name = if port.required == Some(false) {
                    format!("{name}?")
                } else {
                    name.clone()
                };
                parts.push(name);
            }
            if let Some(kind) = &port.kind {
                parts.push(kind.clone());
            }
            if let Some(summary) = &port.summary {
                parts.push(summary.clone());
            }
            parts.join(" ")
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(target_os = "macos")]
fn autocomplete_wrapped_doc_line_count(
    lines: &[String],
    align_width: f32,
    viewport: WidgetViewport,
    zoom: f32,
) -> usize {
    let max_chars = autocomplete_doc_wrap_chars(align_width, viewport, zoom);
    lines
        .iter()
        .map(|line| wrap_autocomplete_doc_line(line, max_chars).len())
        .sum()
}

#[cfg(target_os = "macos")]
fn autocomplete_doc_wrap_chars(align_width: f32, viewport: WidgetViewport, _zoom: f32) -> usize {
    let width_px = align_width * viewport.cell_w;
    let char_px = (10.0_f32 * 0.74).max(1.0);
    ((width_px / char_px).floor() as usize).clamp(20, 56)
}

#[cfg(target_os = "macos")]
fn wrap_autocomplete_doc_line(line: &str, max_chars: usize) -> Vec<String> {
    if line.chars().count() <= max_chars {
        return vec![line.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in line.split_whitespace() {
        let current_len = current.chars().count();
        let word_len = word.chars().count();
        if current_len > 0 && current_len + 1 + word_len > max_chars {
            lines.push(current);
            current = String::new();
        }
        if word_len > max_chars {
            if !current.is_empty() {
                lines.push(current);
                current = String::new();
            }
            let chars: Vec<char> = word.chars().collect();
            for chunk in chars.chunks(max_chars) {
                lines.push(chunk.iter().collect());
            }
            continue;
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(target_os = "macos")]
fn push_cable(
    prims: &mut Vec<MetalPrimitive>,
    start: (f32, f32),
    end: (f32, f32),
    connection: &PatchConnection,
    origin_row: f32,
    zoom: f32,
    selected: bool,
) {
    let color = if selected {
        theme::PATCHER_ERROR()
    } else {
        match connection.kind {
            ConnectionKind::Forward => theme::PATCHER_CABLE(),
            ConnectionKind::Feedback => theme::PATCHER_FEEDBACK_CABLE(),
        }
    };
    let curve = super::super::cable::cable_curve(start, end);
    let is_segmented = connection
        .segment
        .is_some_and(|segment| segment.is_segmented)
        && super::super::cable::should_render_segmented_cable(start, end);
    prims.push(MetalPrimitive::PatchCable(MetalPatchCablePrimitive {
        start: [curve.p0.0, curve.p0.1],
        control1: [curve.p1.0, curve.p1.1],
        control2: [curve.p2.0, curve.p2.1],
        end: [curve.p3.0, curve.p3.1],
        radius_px: if connection.kind == ConnectionKind::Feedback {
            3.6 * zoom
        } else {
            4.4 * zoom
        },
        color,
        is_segmented,
        segment_row: connection
            .segment
            .map(|segment| origin_row + segment.segment_row * zoom)
            .unwrap_or(0.0),
        corner_radius_cells: SEGMENTED_CABLE_CORNER_RADIUS_CELLS * zoom,
    }));
}

#[cfg(target_os = "macos")]
fn push_preview_cable(
    prims: &mut Vec<MetalPrimitive>,
    start: (f32, f32),
    end: (f32, f32),
    kind: ConnectionKind,
    zoom: f32,
    selected: bool,
) {
    let connection = PatchConnection {
        from_node: String::new(),
        from_output: 0,
        to_node: String::new(),
        to_input: 0,
        kind,
        segment: None,
        source: None,
    };
    push_cable(prims, start, end, &connection, 0.0, zoom, selected);
}

#[cfg(target_os = "macos")]
fn push_cable_handles(
    prims: &mut Vec<MetalPrimitive>,
    connection: &PatchConnection,
    start: (f32, f32),
    end: (f32, f32),
    zoom: f32,
) {
    let (from_handle, to_handle) = connection_cable_edit_points(connection, start, end, zoom);
    for center in [from_handle, to_handle] {
        prims.push(MetalPrimitive::Circle(MetalCirclePrimitive {
            center: [center.0, center.1],
            radius_px: CABLE_HANDLE_RADIUS_PX * zoom,
            color: theme::PATCHER_ERROR(),
            visible_half: MetalCircleVisibleHalf::Full,
        }));
        prims.push(MetalPrimitive::Circle(MetalCirclePrimitive {
            center: [center.0, center.1],
            radius_px: CABLE_HANDLE_RADIUS_PX * 0.52 * zoom,
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
    highlighted_inputs: &[usize],
    highlighted_outputs: &[usize],
    zoom: f32,
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
    push_node_chrome(prims, rect, bg, border, viewport, zoom);
    for &index in input_indices {
        push_port(
            prims,
            port_center(rect, index, input_slot_count, true),
            true,
            bg,
            highlighted_inputs.contains(&index),
            viewport,
            zoom,
        );
    }
    for index in 0..output_count {
        push_port(
            prims,
            port_center(rect, index, output_count, false),
            false,
            bg,
            highlighted_outputs.contains(&index),
            viewport,
            zoom,
        );
    }
    push_node_edit_selection(prims, rect, viewport, edit, zoom);
    push_node_label(prims, node, rect, text, edit, viewport, zoom);
    push_node_edit_cursor(prims, rect, viewport, edit, zoom);
    if let Some(diagnostic) = &node.diagnostic {
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: rect.row + 2.65 * zoom,
                col: rect.col + 1.0 * zoom,
                align_width: rect.width - 2.0 * zoom,
                h_align: 0.0,
                text: preview(diagnostic, 32),
                font_size: 9.5,
                scale: zoom,
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
fn push_node_chrome(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    bg: crate::backend::Color,
    border: crate::backend::Color,
    viewport: WidgetViewport,
    zoom: f32,
) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    prims.push(MetalPrimitive::WidgetInstance {
        widget_type: "patcher-node".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [NODE_BORDER_WIDTH_PX * zoom, 0.0, 0.0, 0.0],
            uniform_b: [0.0; 4],
            color_a: border.to_rgba(),
            color_b: bg.to_rgba(),
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: normalized_corner_radius(rect, viewport, NODE_CORNER_RADIUS_PX * zoom),
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
        is_background: false,
    });
}

#[cfg(target_os = "macos")]
fn push_autocomplete_panel_chrome(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    bg: crate::backend::Color,
    border: crate::backend::Color,
    viewport: WidgetViewport,
    zoom: f32,
) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    prims.push(MetalPrimitive::WidgetInstance {
        widget_type: "patcher-node".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [NODE_BORDER_WIDTH_PX * zoom, 0.0, 0.0, 0.0],
            uniform_b: [0.0; 4],
            color_a: border.to_rgba(),
            color_b: bg.to_rgba(),
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: normalized_corner_radius(
                rect,
                viewport,
                AUTOCOMPLETE_PANEL_CORNER_RADIUS_PX * zoom,
            ),
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
        is_background: false,
    });
}

#[cfg(target_os = "macos")]
fn push_node_edit_selection(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    viewport: WidgetViewport,
    edit: Option<&PatcherTextEdit>,
    zoom: f32,
) {
    let Some(edit) = edit else {
        return;
    };
    let Some((start, end)) = text_selection_range(&edit.state) else {
        return;
    };
    let x = rect.col
        + NODE_TEXT_COL_OFFSET * zoom
        + cursor_x_from_char_cache(&edit.text, NODE_FONT_SIZE, start, viewport.cell_w) * zoom;
    let end_x = rect.col
        + NODE_TEXT_COL_OFFSET * zoom
        + cursor_x_from_char_cache(&edit.text, NODE_FONT_SIZE, end, viewport.cell_w) * zoom;
    let width = end_x - x;
    if width <= 0.0 {
        return;
    }
    prims.push(MetalPrimitive::ForegroundRect(MetalRectPrimitive {
        rect: Rect {
            row: rect.row + 0.18 * zoom,
            col: x,
            width,
            height: (rect.height - 0.36 * zoom).max(0.1),
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
    zoom: f32,
) {
    let Some(edit) = edit else {
        return;
    };
    let cursor_pos = edit.state.cursor_pos.min(edit.text.chars().count());
    let x = rect.col
        + NODE_TEXT_COL_OFFSET * zoom
        + cursor_x_from_char_cache(&edit.text, NODE_FONT_SIZE, cursor_pos, viewport.cell_w) * zoom;
    prims.push(MetalPrimitive::ForegroundRect(MetalRectPrimitive {
        rect: Rect {
            row: rect.row + 0.23 * zoom,
            col: x,
            width: 0.08 * zoom,
            height: (rect.height - 0.46 * zoom).max(0.1),
        },
        color: theme::PATCHER_EDIT_CURSOR(),
    }));
}

#[cfg(target_os = "macos")]
fn push_node_label(
    prims: &mut Vec<MetalPrimitive>,
    node: &PatchNode,
    rect: Rect,
    head_color: crate::backend::Color,
    edit: Option<&PatcherTextEdit>,
    viewport: WidgetViewport,
    zoom: f32,
) {
    let font_size = if node.kind == NodeKind::CodeIsland {
        CODE_NODE_FONT_SIZE
    } else {
        NODE_FONT_SIZE
    };
    let baseline_row = if node.kind == NodeKind::CodeIsland {
        rect.row + 0.55 * zoom
    } else {
        rect.row + 0.36 * zoom
    };
    let text_col = rect.col + NODE_TEXT_COL_OFFSET * zoom;
    if let Some(edit) = edit {
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: baseline_row,
                col: text_col,
                align_width: rect.width - 1.84 * zoom,
                h_align: 0.0,
                text: edit.text.clone(),
                font_size,
                scale: zoom,
                fg: head_color,
                bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
            },
        ));
        return;
    }
    let label = node_display_label(node);
    let (head, tail, tail_start) = split_label_head_tail(&label);
    let bg = crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0);
    prims.push(MetalPrimitive::ProportionalText(
        MetalProportionalTextPrimitive {
            row: baseline_row,
            col: text_col,
            align_width: rect.width - 1.84 * zoom,
            h_align: 0.0,
            text: head.to_string(),
            font_size,
            scale: zoom,
            fg: head_color,
            bg,
        },
    ));
    if !tail.is_empty() {
        let tail_col = text_col
            + cursor_x_from_char_cache(&label, font_size, tail_start, viewport.cell_w) * zoom;
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: baseline_row,
                col: tail_col,
                align_width: (rect.col + rect.width - tail_col - 0.92 * zoom).max(0.0),
                h_align: 0.0,
                text: tail.to_string(),
                font_size,
                scale: zoom,
                fg: theme::PATCHER_NODE_TAIL_TEXT(),
                bg,
            },
        ));
    }
}

#[cfg(target_os = "macos")]
fn split_label_head_tail(label: &str) -> (&str, &str, usize) {
    let trimmed_start = label
        .find(|ch: char| !ch.is_whitespace())
        .unwrap_or(label.len());
    let trimmed = label[trimmed_start..].trim_end();
    match trimmed.find(char::is_whitespace) {
        Some(idx) => {
            let tail_start = trimmed[idx..]
                .find(|ch: char| !ch.is_whitespace())
                .map(|tail_idx| idx + tail_idx)
                .unwrap_or(trimmed.len());
            let head = &trimmed[..idx];
            let tail = &trimmed[tail_start..];
            let tail_start = label[..trimmed_start + tail_start].chars().count();
            (head, tail, tail_start)
        }
        None => (trimmed, "", trimmed.chars().count()),
    }
}

#[cfg(target_os = "macos")]
fn push_port(
    prims: &mut Vec<MetalPrimitive>,
    center: (f32, f32),
    input: bool,
    node_bg: crate::backend::Color,
    highlighted: bool,
    viewport: WidgetViewport,
    zoom: f32,
) {
    let color = if highlighted {
        theme::PATCHER_NODE_SELECTED_BORDER()
    } else if input {
        theme::PATCHER_PORT_INPUT()
    } else {
        theme::PATCHER_PORT_OUTPUT()
    };
    let outer_radius_px = PORT_OUTER_DIAMETER_PX * 0.5 * zoom;
    let rect = Rect {
        row: center.1 - outer_radius_px / viewport.cell_h.max(1.0),
        col: center.0 - outer_radius_px / viewport.cell_w.max(1.0),
        width: (outer_radius_px * 2.0) / viewport.cell_w.max(1.0),
        height: (outer_radius_px * 2.0) / viewport.cell_h.max(1.0),
    };
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let visible_half = if input { 1.0 } else { -1.0 };
    prims.push(MetalPrimitive::WidgetInstance {
        widget_type: "patcher-port".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: visible_half,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [
                PORT_INNER_DIAMETER_PX / PORT_OUTER_DIAMETER_PX,
                0.0,
                0.0,
                0.0,
            ],
            uniform_b: [0.0; 4],
            color_a: color.to_rgba(),
            color_b: node_bg.to_rgba(),
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: 0.0,
            pixel_aspect: 1.0,
        },
        is_background: false,
    });
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
