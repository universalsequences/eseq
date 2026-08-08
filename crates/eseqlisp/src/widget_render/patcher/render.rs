use std::collections::HashMap;

#[cfg(target_os = "macos")]
use super::super::text_input::selection_range as text_selection_range;
use super::super::{CellBuffer, styled_cell};
#[cfg(target_os = "macos")]
use super::super::{
    MetalCirclePrimitive, MetalCircleVisibleHalf, MetalPatchCablePrimitive, MetalPrimitive,
    MetalProportionalTextPrimitive, MetalQuadPrimitive, MetalRectPrimitive, WidgetInstance,
    WidgetViewport, ndc_bounds, z_layer,
};
#[cfg(target_os = "macos")]
use crate::backend::{
    AUTOCOMPLETE_PANEL_BORDER_WIDTH_PX, AUTOCOMPLETE_PANEL_CORNER_RADIUS_PX,
    AUTOCOMPLETE_ROW_CORNER_RADIUS_PX,
};
#[cfg(target_os = "macos")]
use crate::layout::LayoutNode;
use crate::layout::{Rect, f64_to_f32};
use crate::theme;
use crate::vm::Value;

use super::display::{node_display_label, node_font_size, preview};
use super::geometry::{
    connection_cable_edit_points, connection_endpoints, node_resize_handle_centers,
    patch_content_size, patch_input_indices, patch_input_slot_counts, patch_node_rects,
    patch_output_counts, patcher_back_button_rect, patcher_zoom, port_center, rect_from_points,
};
use super::load_patch_from_props;
use super::metrics::{
    CABLE_HANDLE_RADIUS_PX, NODE_BORDER_WIDTH_PX, NODE_CORNER_RADIUS_PX,
    NODE_RESIZE_HANDLE_SIZE_CELLS, NODE_TEXT_COL_OFFSET, PORT_INNER_DIAMETER_PX,
    PORT_OUTER_DIAMETER_PX, SEGMENTED_CABLE_CORNER_RADIUS_CELLS,
};
use super::model::{
    ArgValue, BindingTarget, ConnectionKind, InputPortRef, InputPresentation, NodeKind,
    OutputPortRef, Patch, PatchConnection, PatchNode, SourceOwner,
    connection_touches_hidden_inline_node, hidden_inline_node_ids,
};
#[cfg(target_os = "macos")]
use super::project::OperatorPortDocumentation;
use super::project::dgenlisp_operator_documentation;
use super::state::{
    AgenticBubbleState, AgenticBubbleTarget, AlignmentGuide, AlignmentGuideKind,
    PATCHER_Z_SLOTS_PER_NODE, PatcherDragState, PatcherInteractionState, PatcherPanState,
    PatcherTextEdit, PatcherZSlot, active_patcher_patch, active_patcher_view_key,
    get_patcher_interaction_state, get_patcher_pan_state, max_node_z_index, node_z_index,
    ordered_patch_nodes, patch_with_interaction_state, patcher_breadcrumb, patcher_state_key,
    set_patcher_pan_state, source_connection_id, sync_patcher_z_order,
};
#[cfg(target_os = "macos")]
use super::text::patcher_autocomplete_suggestions;
#[cfg(target_os = "macos")]
use super::text_metrics::{measured_cursor_offset, measured_text_width, wrap_measured_text};

/// Horizontal inset of the selected-row bar from the panel edge, in cells.
#[cfg(target_os = "macos")]
const AUTOCOMPLETE_ROW_INSET_CELLS: f32 = 0.22;
#[cfg(target_os = "macos")]
const PATCHER_TOOLTIP_CORNER_RADIUS_PX: f32 = 5.0;
#[cfg(target_os = "macos")]
const PATCHER_OVERLAY_Z: i32 = 1_000_000;

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
            let mut interaction_state = get_patcher_interaction_state(key);
            let view_key = active_patcher_view_key(&interaction_state);
            let patch = active_patcher_patch(&root_patch, &interaction_state);
            let patch = patch_with_interaction_state(patch, &interaction_state, &view_key);
            let autocomplete_macros =
                super::autocomplete_macros_for_patch(&node.props, Some(&patch));
            sync_patcher_z_order(&mut interaction_state, &view_key, &patch);
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
            draw_patch_with_view_key(
                &mut prims,
                &patch,
                node.rect,
                viewport,
                &pan_state,
                &interaction_state,
                &view_key,
                &autocomplete_macros,
            );
            let mut bubble_prims = Vec::new();
            draw_agentic_bubbles(
                &mut bubble_prims,
                &patch,
                node.rect,
                viewport,
                &pan_state,
                &interaction_state,
            );
            push_z_layered(&mut prims, PATCHER_OVERLAY_Z + 100, bubble_prims);
            let mut marquee_prims = Vec::new();
            push_marquee(&mut marquee_prims, node.rect, viewport, &interaction_state);
            push_z_layered(&mut prims, PATCHER_OVERLAY_Z + 40, marquee_prims);
            let mut chrome_overlay_prims = Vec::new();
            push_back_button(
                &mut chrome_overlay_prims,
                node.rect,
                viewport,
                &interaction_state,
            );
            chrome_overlay_prims.push(MetalPrimitive::ProportionalText(
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
                chrome_overlay_prims.push(MetalPrimitive::ProportionalText(
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
            push_z_layered(&mut prims, PATCHER_OVERLAY_Z + 50, chrome_overlay_prims);
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
fn draw_agentic_bubbles(
    prims: &mut Vec<MetalPrimitive>,
    patch: &Patch,
    rect: Rect,
    viewport: WidgetViewport,
    pan_state: &PatcherPanState,
    interaction_state: &PatcherInteractionState,
) {
    let origin = super::geometry::patcher_origin(rect, pan_state);
    let zoom = patcher_zoom(pan_state);
    let node_rects = patch_node_rects(patch, rect, pan_state);
    for bubble in interaction_state.agentic_bubbles.values() {
        let (mut x, y) = match &bubble.target {
            AgenticBubbleTarget::EditMacro {
                instance_node_id, ..
            } => node_rects
                .get(instance_node_id)
                .map(|node_rect| (node_rect.col, node_rect.row))
                .unwrap_or_else(|| {
                    (
                        origin.0 + bubble.position.0 * zoom,
                        origin.1 + bubble.position.1 * zoom,
                    )
                }),
            AgenticBubbleTarget::CreateMacro => (
                origin.0 + bubble.position.0 * zoom,
                origin.1 + bubble.position.1 * zoom,
            ),
        };
        let width_cells = match &bubble.state {
            AgenticBubbleState::Answer { .. } => 34.0,
            _ => 18.0,
        };
        let max_visible_width = (rect.width - 1.0).max(8.0);
        let width = (width_cells * zoom).min(max_visible_width);
        let right_edge = rect.col + rect.width - 0.5;
        x = x.min(right_edge - width).max(rect.col + 0.5);
        let inner_width = (width - 1.3 * zoom).max(1.0);
        let prompt = match &bubble.state {
            AgenticBubbleState::Answer { text, .. } => text.clone(),
            _ if bubble.prompt.trim().is_empty() => "cmd+k prompt".to_string(),
            _ => bubble.prompt.clone(),
        };
        let Some(prompt_lines) = wrap_agentic_prompt_lines(&prompt, inner_width / zoom) else {
            continue;
        };
        let height = ((4.0 + prompt_lines.len() as f32 * 1.18).max(5.8)) * zoom;
        let pending_pulse = match &bubble.state {
            AgenticBubbleState::Pending { .. } => {
                Some(0.5 + 0.5 * (viewport.time_seconds * 4.4).sin())
            }
            _ => None,
        };
        let (fill, border, status) = match &bubble.state {
            AgenticBubbleState::Editing => (
                crate::backend::Color::rgba(0.12, 0.09, 0.08, 0.94),
                theme::PATCHER_NODE_SELECTED_BORDER(),
                "prompt",
            ),
            AgenticBubbleState::Pending { .. } => {
                let pulse = pending_pulse.unwrap_or(0.0);
                (
                    crate::backend::Color::rgba(
                        0.08 + 0.03 * pulse,
                        0.14 + 0.07 * pulse,
                        0.18 + 0.10 * pulse,
                        0.94,
                    ),
                    crate::backend::Color::rgba(
                        0.30 + 0.22 * pulse,
                        0.74 + 0.18 * pulse,
                        0.88 + 0.12 * pulse,
                        1.0,
                    ),
                    "working",
                )
            }
            AgenticBubbleState::Error { .. } => (
                crate::backend::Color::rgba(0.22, 0.07, 0.07, 0.94),
                theme::PATCHER_ERROR(),
                "error",
            ),
            AgenticBubbleState::Answer { .. } => (
                crate::backend::Color::rgba(0.08, 0.12, 0.10, 0.96),
                theme::PATCHER_NODE_TEXT(),
                "answer",
            ),
        };
        prims.push(MetalPrimitive::ForegroundRect(MetalRectPrimitive {
            rect: Rect {
                col: x,
                row: y,
                width,
                height,
            },
            color: fill,
        }));
        let border_w = 0.12 * zoom;
        for border_rect in [
            Rect {
                col: x,
                row: y,
                width,
                height: border_w,
            },
            Rect {
                col: x,
                row: y + height - border_w,
                width,
                height: border_w,
            },
            Rect {
                col: x,
                row: y,
                width: border_w,
                height,
            },
            Rect {
                col: x + width - border_w,
                row: y,
                width: border_w,
                height,
            },
        ] {
            prims.push(MetalPrimitive::ForegroundRect(MetalRectPrimitive {
                rect: border_rect,
                color: border,
            }));
        }
        let detail = match &bubble.state {
            AgenticBubbleState::Pending { .. } => bubble
                .elapsed()
                .map(|elapsed| format!("{}  {:.1}s", status, elapsed.as_secs_f32()))
                .unwrap_or_else(|| status.to_string()),
            AgenticBubbleState::Error { summary, .. } => format!("{status}: {summary}"),
            AgenticBubbleState::Answer { .. } => status.to_string(),
            AgenticBubbleState::Editing => status.to_string(),
        };
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: y + 0.65 * zoom,
                col: x + 0.65 * zoom,
                align_width: inner_width,
                h_align: 0.0,
                text: detail,
                font_size: 11.5,
                scale: zoom,
                fg: theme::PATCHER_TEXT(),
                bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
            },
        ));
        for (line_index, prompt_line) in prompt_lines.into_iter().enumerate() {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: y + (2.25 + line_index as f32 * 1.18) * zoom,
                    col: x + 0.65 * zoom,
                    align_width: inner_width,
                    h_align: 0.0,
                    text: prompt_line.text,
                    font_size: 13.0,
                    scale: zoom,
                    fg: theme::PATCHER_NODE_TEXT(),
                    bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
                },
            ));
        }
        if matches!(bubble.state, AgenticBubbleState::Editing) {
            push_agentic_bubble_cursor(prims, bubble, x, y, inner_width, zoom);
        }
    }
}

#[cfg(target_os = "macos")]
fn push_agentic_bubble_cursor(
    prims: &mut Vec<MetalPrimitive>,
    bubble: &super::state::AgenticBubble,
    x: f32,
    y: f32,
    inner_width: f32,
    zoom: f32,
) {
    let cursor_pos = bubble
        .text_state
        .cursor_pos
        .min(bubble.prompt.chars().count());
    let Some(lines) = wrap_agentic_prompt_lines(&bubble.prompt, inner_width / zoom) else {
        return;
    };
    let cursor_line_index = lines
        .iter()
        .position(|line| cursor_pos >= line.start && cursor_pos <= line.end)
        .unwrap_or_else(|| lines.len().saturating_sub(1));
    let line_start = lines
        .get(cursor_line_index)
        .map(|line| line.start)
        .unwrap_or(0);
    let Some(cursor_before_line) = measured_cursor_offset(&bubble.prompt, 13.0, line_start) else {
        return;
    };
    let Some(cursor_before_prompt) = measured_cursor_offset(&bubble.prompt, 13.0, cursor_pos)
    else {
        return;
    };
    let cursor_x = x + 0.65 * zoom + (cursor_before_prompt - cursor_before_line).max(0.0) * zoom;
    prims.push(MetalPrimitive::ForegroundRect(MetalRectPrimitive {
        rect: Rect {
            row: y + (2.1 + cursor_line_index as f32 * 1.18) * zoom,
            col: cursor_x,
            width: 0.08 * zoom,
            height: 1.1 * zoom,
        },
        color: theme::PATCHER_EDIT_CURSOR(),
    }));
}

#[cfg(target_os = "macos")]
fn push_z_layered(prims: &mut Vec<MetalPrimitive>, z_index: i32, layer: Vec<MetalPrimitive>) {
    prims.extend(
        layer
            .into_iter()
            .map(|primitive| z_layer(z_index, primitive)),
    );
}

#[cfg(target_os = "macos")]
fn wrap_agentic_prompt_lines(
    text: &str,
    max_width_cells: f32,
) -> Option<Vec<super::text_metrics::MeasuredLine>> {
    wrap_measured_text(text, max_width_cells, 13.0)
}

#[cfg(target_os = "macos")]
fn push_back_button(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    viewport: WidgetViewport,
    interaction_state: &PatcherInteractionState,
) {
    if interaction_state.active_macro.is_none() {
        return;
    }
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
    push_back_chevron_icon(
        prims,
        button_rect,
        viewport,
        interaction_state.hover_back_button,
    );
}

#[cfg(target_os = "macos")]
fn push_back_chevron_icon(
    prims: &mut Vec<MetalPrimitive>,
    button_rect: Rect,
    viewport: WidgetViewport,
    hovered: bool,
) {
    let icon_rect = Rect {
        row: button_rect.row + 0.29,
        col: button_rect.col + 0.56,
        width: 0.92,
        height: button_rect.height - 0.58,
    };
    let (ndc_min, ndc_max) = ndc_bounds(icon_rect, viewport);
    let px_w = icon_rect.width * viewport.cell_w;
    let px_h = icon_rect.height * viewport.cell_h;
    let color = if hovered {
        theme::PATCHER_BACK_BUTTON_HOVER_TEXT()
    } else {
        theme::PATCHER_BACK_BUTTON_TEXT()
    };
    prims.push(MetalPrimitive::WidgetInstance {
        widget_type: "patcher-back-chevron".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [0.0; 4],
            uniform_b: [0.0; 4],
            uniform_c: [0.0; 4],
            uniform_d: [0.0; 4],
            color_a: color.to_rgba(),
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: 0.0,
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
        is_background: false,
    });
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
    let mut interaction_state = interaction_state.clone();
    sync_patcher_z_order(&mut interaction_state, "root", patch);
    draw_patch_with_view_key(
        prims,
        patch,
        rect,
        viewport,
        pan_state,
        &interaction_state,
        "root",
        &patch.macros,
    );
}

#[cfg(target_os = "macos")]
fn draw_patch_with_view_key(
    prims: &mut Vec<MetalPrimitive>,
    patch: &Patch,
    rect: Rect,
    viewport: WidgetViewport,
    pan_state: &PatcherPanState,
    interaction_state: &PatcherInteractionState,
    view_key: &str,
    autocomplete_macros: &[super::model::MacroPatch],
) {
    let node_rects = patch_node_rects(patch, rect, pan_state);
    let origin = super::geometry::patcher_origin(rect, pan_state);
    let zoom = patcher_zoom(pan_state);
    let input_indices = patch_input_indices(patch);
    let input_slot_counts = patch_input_slot_counts(patch, &input_indices);
    let output_counts = patch_output_counts(patch);
    let cable_z = max_node_z_index(patch) + PATCHER_Z_SLOTS_PER_NODE;
    let cable_handle_z = cable_z + 1;
    let dragged_cable = match &interaction_state.drag {
        Some(PatcherDragState::CableEndpoint { cable_id, .. }) => Some(cable_id.as_str()),
        _ => None,
    };
    let hidden_node_ids = hidden_inline_node_ids(patch);

    for connection in &patch.connections {
        if connection.presentation != InputPresentation::Cable {
            continue;
        }
        if connection_touches_hidden_inline_node(connection, &hidden_node_ids) {
            continue;
        }
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
        let mut cable_prims = Vec::new();
        push_cable(
            &mut cable_prims,
            start,
            end,
            connection,
            origin.1,
            zoom,
            selected,
        );
        push_z_layered(prims, cable_z, cable_prims);
        if selected {
            let mut handle_prims = Vec::new();
            push_cable_handles(&mut handle_prims, connection, start, end, zoom);
            push_z_layered(prims, cable_handle_z, handle_prims);
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
        let mut cable_prims = Vec::new();
        push_preview_cable(
            &mut cable_prims,
            (*start_col, *start_row),
            (*current_col, *current_row),
            ConnectionKind::Forward,
            zoom,
            false,
        );
        push_z_layered(prims, cable_handle_z, cable_prims);
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
        let mut cable_prims = Vec::new();
        push_preview_cable(
            &mut cable_prims,
            start,
            end,
            ConnectionKind::Forward,
            zoom,
            true,
        );
        let preview_connection = PatchConnection {
            from_node: String::new(),
            from_output: 0,
            to_node: String::new(),
            to_input: 0,
            kind: ConnectionKind::Forward,
            segment: None,
            presentation: InputPresentation::Cable,
            presentation_override: None,
            source: None,
        };
        push_cable_handles(&mut cable_prims, &preview_connection, start, end, zoom);
        push_z_layered(prims, cable_handle_z, cable_prims);
    }
    let mut alignment_prims = Vec::new();
    push_alignment_guides(
        &mut alignment_prims,
        rect,
        viewport,
        origin,
        zoom,
        interaction_state,
    );
    push_z_layered(prims, PATCHER_OVERLAY_Z + 10, alignment_prims);

    let mut active_edit_panel = None;
    for node in ordered_patch_nodes(patch, interaction_state, view_key) {
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
            interaction_state
                .agentic_morph_nodes
                .get(&node.id)
                .is_some_and(|started| started.elapsed().as_secs_f32() < 1.2),
            active_edit,
            &highlighted_inputs,
            &highlighted_outputs,
            zoom,
            node_z_index(
                interaction_state,
                view_key,
                &node.id,
                PatcherZSlot::NodeChrome,
            ),
        );
    }
    if let Some((node_rect, edit)) = active_edit_panel {
        let mut overlay_prims = Vec::new();
        push_autocomplete_panel(
            &mut overlay_prims,
            node_rect,
            edit,
            &autocomplete_macros,
            viewport,
            zoom,
        );
        push_z_layered(prims, PATCHER_OVERLAY_Z + 20, overlay_prims);
    }
    let mut tooltip_prims = Vec::new();
    push_hovered_port_tooltip(
        &mut tooltip_prims,
        patch,
        &node_rects,
        &input_slot_counts,
        &output_counts,
        interaction_state,
        viewport,
        zoom,
    );
    push_z_layered(prims, PATCHER_OVERLAY_Z + 30, tooltip_prims);
}

#[cfg(target_os = "macos")]
fn push_hovered_port_tooltip(
    prims: &mut Vec<MetalPrimitive>,
    patch: &Patch,
    node_rects: &HashMap<String, Rect>,
    input_slot_counts: &HashMap<String, usize>,
    output_counts: &HashMap<String, usize>,
    interaction_state: &PatcherInteractionState,
    viewport: WidgetViewport,
    zoom: f32,
) {
    let tooltip = interaction_state
        .hovered_input_port
        .as_ref()
        .and_then(|port| {
            input_port_tooltip(patch, port)
                .map(|text| (port.node_id.as_str(), true, port.input_index, text))
        })
        .or_else(|| {
            interaction_state
                .hovered_output_port
                .as_ref()
                .and_then(|port| {
                    output_port_tooltip(patch, port)
                        .map(|text| (port.node_id.as_str(), false, port.output_index, text))
                })
        });
    let Some((node_id, is_input, port_index, text)) = tooltip else {
        return;
    };
    let Some(node_rect) = node_rects.get(node_id).copied() else {
        return;
    };
    let port_count = if is_input {
        input_slot_counts.get(node_id).copied().unwrap_or(1)
    } else {
        output_counts.get(node_id).copied().unwrap_or(1)
    };
    let center = port_center(node_rect, port_index, port_count, is_input);
    let font_size = 10.5;
    let text = preview(&text, 48);
    let Some(measured_text_width) = measured_text_width(&text, font_size) else {
        return;
    };
    let text_width_cells = (measured_text_width * zoom)
        .max(4.0 * zoom)
        .min(32.0 * zoom);
    let padding_x = 0.55 * zoom;
    let padding_y = 0.12 * zoom;
    let width = text_width_cells + padding_x * 2.0;
    let height = 1.0 * zoom + padding_y * 2.0;
    let row = if is_input {
        center.1 - height - 0.45 * zoom
    } else {
        center.1 + 0.45 * zoom
    };
    let col = center.0 - width * 0.5;
    let panel = Rect {
        col,
        row,
        width,
        height,
    };
    push_flat_panel_chrome(
        prims,
        panel,
        theme::PATCHER_TOOLTIP_BG(),
        theme::PATCHER_TOOLTIP_BORDER(),
        viewport,
        zoom,
        PATCHER_TOOLTIP_CORNER_RADIUS_PX,
    );
    prims.push(MetalPrimitive::ProportionalText(
        MetalProportionalTextPrimitive {
            row: panel.row + padding_y + 0.04 * zoom,
            col: panel.col + padding_x,
            align_width: panel.width - padding_x * 2.0,
            h_align: 0.0,
            text,
            font_size,
            scale: zoom,
            fg: theme::PATCHER_TOOLTIP_TEXT(),
            bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
        },
    ));
}

#[cfg(target_os = "macos")]
fn push_alignment_guides(
    prims: &mut Vec<MetalPrimitive>,
    panel: Rect,
    viewport: WidgetViewport,
    origin: (f32, f32),
    zoom: f32,
    interaction_state: &PatcherInteractionState,
) {
    let Some(PatcherDragState::Nodes { alignment, .. }) = &interaction_state.drag else {
        return;
    };
    for guide in &alignment.guides {
        if let Some(rect) = alignment_guide_rect(*guide, panel, viewport, origin, zoom) {
            prims.push(MetalPrimitive::ForegroundRect(MetalRectPrimitive {
                rect,
                color: theme::PATCHER_ALIGNMENT_GUIDE(),
            }));
        }
    }
}

#[cfg(target_os = "macos")]
fn alignment_guide_rect(
    guide: AlignmentGuide,
    panel: Rect,
    viewport: WidgetViewport,
    origin: (f32, f32),
    zoom: f32,
) -> Option<Rect> {
    let thickness_px = 2.0;
    match guide.kind {
        AlignmentGuideKind::Vertical => {
            let x = origin.0 + guide.position * zoom;
            let y1 = origin.1 + guide.extent_start.min(guide.extent_end) * zoom;
            let y2 = origin.1 + guide.extent_start.max(guide.extent_end) * zoom;
            let top = y1.max(panel.row);
            let bottom = y2.min(panel.row + panel.height);
            let width = thickness_px / viewport.cell_w.max(1.0);
            let min_height = thickness_px / viewport.cell_h.max(1.0);
            if x < panel.col || x > panel.col + panel.width || bottom <= top {
                return None;
            }
            Some(Rect {
                col: x - width * 0.5,
                row: top,
                width,
                height: (bottom - top).max(min_height),
            })
        }
        AlignmentGuideKind::Horizontal => {
            let y = origin.1 + guide.position * zoom;
            let x1 = origin.0 + guide.extent_start.min(guide.extent_end) * zoom;
            let x2 = origin.0 + guide.extent_start.max(guide.extent_end) * zoom;
            let left = x1.max(panel.col);
            let right = x2.min(panel.col + panel.width);
            let height = thickness_px / viewport.cell_h.max(1.0);
            let min_width = thickness_px / viewport.cell_w.max(1.0);
            if y < panel.row || y > panel.row + panel.height || right <= left {
                return None;
            }
            Some(Rect {
                col: left,
                row: y - height * 0.5,
                width: (right - left).max(min_width),
                height,
            })
        }
    }
}

pub(super) fn input_port_tooltip(patch: &Patch, port: &InputPortRef) -> Option<String> {
    let node = patch.nodes.iter().find(|node| node.id == port.node_id)?;
    let name = if node.kind == NodeKind::MacroInstance {
        patch
            .macros
            .iter()
            .find(|macro_patch| macro_patch.name == node.op)
            .and_then(|macro_patch| macro_patch.params.get(port.input_index))
            .cloned()
    } else {
        dgenlisp_operator_documentation()
            .get(&node.op)
            .and_then(|docs| docs.inputs.get(port.input_index))
            .and_then(|input| input.name.clone())
            .or_else(|| match node.args.get(port.input_index) {
                Some(ArgValue::SymbolRef(name)) => Some(name.clone()),
                Some(ArgValue::Literal(value)) => Some(value.clone()),
                _ => None,
            })
    }
    .unwrap_or_else(|| format!("in {}", port.input_index + 1));
    Some(format!("in {}: {name}", port.input_index + 1))
}

pub(super) fn output_port_tooltip(patch: &Patch, port: &OutputPortRef) -> Option<String> {
    let node = patch.nodes.iter().find(|node| node.id == port.node_id)?;
    let name = output_reference_name(node)
        .filter(|_| port.output_index == 0)
        .or_else(|| node.outputs.get(port.output_index).cloned())
        .or_else(|| {
            if node.kind != NodeKind::MacroInstance {
                return None;
            }
            patch
                .macros
                .iter()
                .find(|macro_patch| macro_patch.name == node.op)
                .and_then(|macro_patch| macro_patch.outputs.get(port.output_index))
                .cloned()
        })
        .or_else(|| {
            dgenlisp_operator_documentation()
                .get(&node.op)
                .and_then(|docs| docs.outputs.get(port.output_index))
                .and_then(|output| output.name.clone())
        })
        .unwrap_or_else(|| format!("out {}", port.output_index + 1));
    Some(format!("out {}: {name}", port.output_index + 1))
}

fn output_reference_name(node: &PatchNode) -> Option<String> {
    let source = node.source.as_ref()?;
    match &source.owner {
        SourceOwner::BindingValue {
            binding: BindingTarget::Symbol(name),
            ..
        } => Some(name.clone()),
        SourceOwner::MacroParameter { binding, .. } if node.kind == NodeKind::In => {
            Some(binding.name.clone())
        }
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn push_autocomplete_panel(
    prims: &mut Vec<MetalPrimitive>,
    node_rect: Rect,
    edit: &PatcherTextEdit,
    local_macros: &[super::model::MacroPatch],
    viewport: WidgetViewport,
    zoom: f32,
) {
    let suggestions = patcher_autocomplete_suggestions(edit, local_macros);
    if suggestions.is_empty() {
        return;
    }
    let row_height = 1.35 * zoom;
    let padding = 0.45 * zoom;
    let panel_width = (node_rect.width.max(36.0 * zoom)).min(56.0 * zoom);
    let panel = Rect {
        row: node_rect.row + node_rect.height + 0.35 * zoom,
        col: node_rect.col,
        width: panel_width,
        height: padding * 2.0 + row_height * suggestions.len() as f32,
    };
    push_flat_panel_chrome(
        prims,
        panel,
        theme::COMP_UNSELECTED_BG(),
        theme::COMP_BORDER(),
        viewport,
        zoom,
        AUTOCOMPLETE_PANEL_CORNER_RADIUS_PX,
    );
    let name_font_size = 11.5;
    let category_font_size = 9.5;
    let text_col = panel.col + 1.4 * zoom;
    let text_right = panel.col + panel.width - 1.4 * zoom;
    let selected_index = edit.autocomplete_selected.min(suggestions.len() - 1);
    for (index, suggestion) in suggestions.iter().enumerate() {
        let row = panel.row + padding + index as f32 * row_height;
        let selected = index == selected_index;
        if selected {
            push_rounded_rect(
                prims,
                Rect {
                    row: row + 0.05 * zoom,
                    col: panel.col + AUTOCOMPLETE_ROW_INSET_CELLS * zoom,
                    width: panel.width - AUTOCOMPLETE_ROW_INSET_CELLS * 2.0 * zoom,
                    height: (row_height - 0.1 * zoom).max(0.1),
                },
                theme::COMP_SELECTED_BG(),
                viewport,
                AUTOCOMPLETE_ROW_CORNER_RADIUS_PX * zoom,
                false,
            );
        }
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: row + 0.18 * zoom,
                col: text_col,
                align_width: text_right - text_col,
                h_align: 0.0,
                text: suggestion.name.clone(),
                font_size: name_font_size,
                scale: zoom,
                fg: if selected {
                    theme::COMP_SELECTED_FG()
                } else {
                    theme::COMP_FG()
                },
                bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
            },
        ));
        let Some(category) = suggestion
            .documentation
            .as_ref()
            .and_then(|documentation| documentation.category.clone())
        else {
            continue;
        };
        let name_width = approx_text_width_cells(&suggestion.name, name_font_size);
        let category_width = approx_text_width_cells(&category, category_font_size);
        // Right-align the category at the panel edge (Zed-style); skip it when
        // the name could plausibly collide with it. Widths are estimates, so
        // leave a generous gap rather than trusting them to the pixel.
        if (name_width + category_width + 2.0) * zoom > text_right - text_col {
            continue;
        }
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: row + 0.24 * zoom,
                col: text_col,
                align_width: text_right - text_col,
                h_align: 1.0,
                text: category,
                font_size: category_font_size,
                scale: zoom,
                fg: theme::COMP_CATEGORY_FG(),
                bg: crate::backend::Color::rgba(0.0, 0.0, 0.0, 0.0),
            },
        ));
    }
    push_autocomplete_documentation_panel(
        prims,
        panel,
        &suggestions[selected_index],
        viewport,
        zoom,
    );
}

/// Width of `text` in layout cells. Falls back to an average-advance estimate
/// when the glyph-advance cache has no entry: suggestion names and category
/// labels are never measured during layout, so the cache is empty for them.
#[cfg(target_os = "macos")]
fn approx_text_width_cells(text: &str, font_size: f32) -> f32 {
    measured_text_width(text, font_size)
        .unwrap_or_else(|| super::display::estimated_label_width_cells(text, font_size))
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
    let padding = 0.8 * zoom;
    let panel_width = 46.0 * zoom;
    let align_width = panel_width - 1.8 * zoom;
    let visual_line_count =
        autocomplete_wrapped_doc_line_count(&lines, align_width, viewport, zoom);
    let panel = Rect {
        row: list_panel.row,
        col: list_panel.col + list_panel.width + 0.55 * zoom,
        width: panel_width,
        height: padding * 2.0 + row_height * visual_line_count as f32,
    };
    push_flat_panel_chrome(
        prims,
        panel,
        theme::COMP_DOC_BG(),
        theme::COMP_DOC_BORDER(),
        viewport,
        zoom,
        AUTOCOMPLETE_PANEL_CORNER_RADIUS_PX,
    );
    let max_chars = autocomplete_doc_wrap_chars(align_width, viewport, zoom);
    let mut visual_index = 0;
    for line in lines {
        for wrapped in wrap_autocomplete_doc_line(&line, max_chars) {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: panel.row + padding + visual_index as f32 * row_height,
                    col: panel.col + 0.9 * zoom,
                    align_width,
                    h_align: 0.0,
                    text: wrapped,
                    font_size: 10.0,
                    scale: zoom,
                    fg: theme::COMP_DOC_FG(),
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
        presentation: InputPresentation::Cable,
        presentation_override: None,
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
    morphing: bool,
    edit: Option<&PatcherTextEdit>,
    highlighted_inputs: &[usize],
    highlighted_outputs: &[usize],
    zoom: f32,
    node_chrome_z: i32,
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
    if morphing {
        border = theme::PATCHER_CABLE();
    }
    let node_base_z = node_chrome_z - PatcherZSlot::NodeChrome as i32;
    let mut chrome_prims = Vec::new();
    push_node_chrome(&mut chrome_prims, rect, bg, border, viewport, zoom);
    push_z_layered(
        prims,
        node_base_z + PatcherZSlot::NodeChrome as i32,
        chrome_prims,
    );
    let mut port_prims = Vec::new();
    for &index in input_indices {
        push_port(
            &mut port_prims,
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
            &mut port_prims,
            port_center(rect, index, output_count, false),
            false,
            bg,
            highlighted_outputs.contains(&index),
            viewport,
            zoom,
        );
    }
    push_z_layered(prims, node_base_z + PatcherZSlot::Ports as i32, port_prims);
    let font_size = node_font_size(node);
    let mut selection_prims = Vec::new();
    push_node_edit_selection(&mut selection_prims, rect, edit, font_size, zoom);
    push_z_layered(
        prims,
        node_base_z + PatcherZSlot::EditSelection as i32,
        selection_prims,
    );
    let mut text_prims = Vec::new();
    push_node_label(&mut text_prims, node, rect, text, edit, zoom);
    if let Some(diagnostic) = &node.diagnostic {
        text_prims.push(MetalPrimitive::ProportionalText(
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
    push_z_layered(prims, node_base_z + PatcherZSlot::Text as i32, text_prims);
    let mut cursor_prims = Vec::new();
    push_node_edit_cursor(&mut cursor_prims, rect, edit, font_size, zoom);
    push_z_layered(
        prims,
        node_base_z + PatcherZSlot::EditCursor as i32,
        cursor_prims,
    );
    let mut handle_prims = Vec::new();
    push_node_resize_handles(
        &mut handle_prims,
        rect,
        selected,
        edit.is_some(),
        viewport,
        zoom,
    );
    push_z_layered(
        prims,
        node_base_z + PatcherZSlot::ResizeHandles as i32,
        handle_prims,
    );
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
            uniform_c: [0.0; 4],
            uniform_d: [0.0; 4],
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
fn push_flat_panel_chrome(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    bg: crate::backend::Color,
    border: crate::backend::Color,
    viewport: WidgetViewport,
    zoom: f32,
    radius_px: f32,
) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    prims.push(MetalPrimitive::WidgetInstance {
        widget_type: "patcher-panel".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [AUTOCOMPLETE_PANEL_BORDER_WIDTH_PX, 0.0, 0.0, 0.0],
            uniform_b: [0.0; 4],
            uniform_c: [0.0; 4],
            uniform_d: [0.0; 4],
            color_a: border.to_rgba(),
            color_b: bg.to_rgba(),
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: normalized_corner_radius(rect, viewport, radius_px * zoom),
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
        is_background: false,
    });
}

#[cfg(target_os = "macos")]
fn push_node_edit_selection(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    edit: Option<&PatcherTextEdit>,
    font_size: f32,
    zoom: f32,
) {
    let Some(edit) = edit else {
        return;
    };
    let Some((start, end)) = text_selection_range(&edit.state) else {
        return;
    };
    let Some(start_x) = measured_cursor_offset(&edit.text, font_size, start) else {
        return;
    };
    let Some(selection_end_x) = measured_cursor_offset(&edit.text, font_size, end) else {
        return;
    };
    let x = rect.col + NODE_TEXT_COL_OFFSET * zoom + start_x * zoom;
    let end_x = rect.col + NODE_TEXT_COL_OFFSET * zoom + selection_end_x * zoom;
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
    edit: Option<&PatcherTextEdit>,
    font_size: f32,
    zoom: f32,
) {
    let Some(edit) = edit else {
        return;
    };
    let cursor_pos = edit.state.cursor_pos.min(edit.text.chars().count());
    let Some(cursor_x) = measured_cursor_offset(&edit.text, font_size, cursor_pos) else {
        return;
    };
    let x = rect.col + NODE_TEXT_COL_OFFSET * zoom + cursor_x * zoom;
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
    zoom: f32,
) {
    let font_size = node_font_size(node);
    let text_row = if node.kind == NodeKind::CodeIsland {
        rect.row + 0.55 * zoom
    } else {
        rect.row + (rect.height - zoom) * 0.5
    };
    let text_col = rect.col + NODE_TEXT_COL_OFFSET * zoom;
    if let Some(edit) = edit {
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: text_row,
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
            row: text_row,
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
        let Some(tail_offset) = measured_cursor_offset(&label, font_size, tail_start) else {
            return;
        };
        let tail_col = text_col + tail_offset * zoom;
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: text_row,
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
            uniform_c: [0.0; 4],
            uniform_d: [0.0; 4],
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
fn push_node_resize_handles(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    selected: bool,
    editing: bool,
    viewport: WidgetViewport,
    zoom: f32,
) {
    if !selected || editing {
        return;
    }
    let size_px = NODE_RESIZE_HANDLE_SIZE_CELLS * zoom * viewport.cell_h.max(1.0);
    let width = size_px / viewport.cell_w.max(1.0);
    let height = size_px / viewport.cell_h.max(1.0);
    for (_, center) in node_resize_handle_centers(rect) {
        prims.push(MetalPrimitive::ForegroundRect(MetalRectPrimitive {
            rect: Rect {
                col: center.0 - width * 0.5,
                row: center.1 - height * 0.5,
                width,
                height,
            },
            color: theme::PATCHER_NODE_SELECTED_BORDER(),
        }));
    }
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
            uniform_c: [0.0; 4],
            uniform_d: [0.0; 4],
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
