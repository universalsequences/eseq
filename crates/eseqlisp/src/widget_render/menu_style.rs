//! Shared visual metrics and Metal primitives for popup menus.
//!
//! Dropdown menus and pointer-anchored context menus have different state and
//! placement policies, but their panel chrome and rows intentionally share this
//! style so the two renderers cannot drift apart.

/// Font size (points) for popup-menu rows — dropdown options and `menu-item`
/// labels alike. Popup rows are chrome, not tile content, so they deliberately
/// ignore the surrounding tile's inherited font size; an explicit `:font-size`
/// prop still overrides this.
pub(crate) const MENU_FONT_SIZE: f32 = 10.0;
pub(crate) const ROW_HEIGHT: f32 = 1.4;
pub(crate) const PANEL_PADDING_V: f32 = 0.15;
pub(crate) const TEXT_PADDING_H: f32 = 1.0;
pub(crate) const CORNER_RADIUS_PX: f32 = 20.0;
pub(crate) const APPROX_CHAR_WIDTH: f32 = 0.55;

use super::{GpuPrimitive, WidgetInstance, WidgetViewport, ndc_bounds};
use crate::backend::Color;
use crate::layout::Rect;

fn normalized_corner_radius(rect: Rect, viewport: WidgetViewport, radius_px: f32) -> f32 {
    // The shared shader's radius is normalized to half the primitive height.
    // A tiny positive value opts out of its historical pill default for rows
    // that are intentionally square.
    if radius_px <= 0.0 {
        return 0.001;
    }
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((super::ui_design_px(radius_px) * 2.0) / px_h).clamp(0.001, 1.0)
}

fn shape_primitive(
    widget_type: &str,
    rect: Rect,
    color: Color,
    radius_px: f32,
    is_background: bool,
    viewport: WidgetViewport,
) -> GpuPrimitive {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    GpuPrimitive::WidgetInstance {
        widget_type: widget_type.to_string(),
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
            color_a: [color.r, color.g, color.b, color.a],
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: normalized_corner_radius(rect, viewport, radius_px),
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
        is_background,
    }
}

pub(crate) fn emit_rounded_rect_overlay(
    rect: Rect, color: Color, radius_px: f32, viewport: WidgetViewport,
) {
    super::push_overlay_primitive(shape_primitive("dropdown", rect, color, radius_px, true, viewport));
}

pub(crate) fn emit_panel_chrome(
    panel_rect: Rect,
    background: Color,
    border: Color,
    viewport: WidgetViewport,
) {
    let border_design_px = 1.0;
    let border_px = super::ui_design_px(border_design_px);
    let border_row = border_px / viewport.cell_h.max(1.0);
    let border_col = border_px / viewport.cell_w.max(1.0);
    let border_rect = Rect {
        row: panel_rect.row - border_row,
        col: panel_rect.col - border_col,
        width: panel_rect.width + border_col * 2.0,
        height: panel_rect.height + border_row * 2.0,
    };
    emit_rounded_rect_overlay(
        border_rect,
        border,
        CORNER_RADIUS_PX + border_design_px,
        viewport,
    );
    emit_rounded_rect_overlay(panel_rect, background, CORNER_RADIUS_PX, viewport);
}

/// Use the panel's vertical padding on both axes in framebuffer space. Cell
/// widths and heights differ, so equal cell counts would not be equal insets.
pub(crate) fn row_highlight_rect(row_rect: Rect, viewport: WidgetViewport) -> Rect {
    let inset = PANEL_PADDING_V * viewport.cell_h / viewport.cell_w.max(1.0);
    Rect {
        col: row_rect.col + inset,
        width: (row_rect.width - inset * 2.0).max(0.0),
        ..row_rect
    }
}

pub(crate) fn row_highlight_primitive(
    row_rect: Rect, color: Color, viewport: WidgetViewport,
) -> GpuPrimitive {
    // Concentric curves: the inner radius is the outer radius minus the gap.
    let inset_px = PANEL_PADDING_V * viewport.cell_h;
    let radius = (CORNER_RADIUS_PX - inset_px / super::ui_px_scale()).max(0.0);
    shape_primitive("dropdown", row_highlight_rect(row_rect, viewport), color, radius, true, viewport)
}

pub(crate) fn emit_row_highlight(row_rect: Rect, color: Color, viewport: WidgetViewport) {
    super::push_overlay_primitive(row_highlight_primitive(row_rect, color, viewport));
}

/// Same vector mark for dropdown selections and checked context-menu actions.
/// Menu fonts are point-sized; primitive bounds must use backing pixels.
pub(crate) fn checkmark_primitive(
    row_rect: Rect, font_size: f32, color: Color, viewport: WidgetViewport,
) -> GpuPrimitive {
    let height_px = super::ui_design_px(font_size * super::UI_DESIGN_REFERENCE_SCALE * 1.2);
    let width_px = height_px * 0.85;
    let width = width_px / viewport.cell_w.max(1.0);
    let height = height_px / viewport.cell_h.max(1.0);
    let rect = Rect {
        row: row_rect.row + (row_rect.height - height) * 0.5,
        col: row_rect.col + TEXT_PADDING_H + (1.5 - width) * 0.5,
        width,
        height,
    };
    shape_primitive("dropdown-checkmark", rect, color, 0.0, false, viewport)
}

/// Marker prop stamped by the layout pass next to an *inherited* `font-size`
/// injection (see `ui::layout::build_layout_node`). Popup rows use it to tell
/// "the tile around me happens to use this font" apart from "the call site
/// explicitly asked for this font", which the raw pre-layout node the measure
/// pass sees can distinguish for free.
pub(crate) const INHERITED_FONT_SIZE_MARKER: &str = "_font-size-inherited";

/// Popup-row font size resolved from the raw widget node — the measure pass.
///
/// Measurement drives the panel width, so it MUST agree with
/// [`menu_font_size_from_props`], which the render pass uses for the draw call.
pub(crate) fn menu_font_size_from_node(node: &crate::vm::Value) -> f32 {
    crate::layout::get_prop_num(node, "font-size")
        .map(crate::layout::f64_to_f32)
        .filter(|size| size.is_finite() && *size > 0.0)
        .unwrap_or(MENU_FONT_SIZE)
}

/// Popup-row font size resolved from laid-out props — the render pass.
///
/// An inherited (injected) `font-size` is deliberately ignored so popup chrome
/// keeps one size everywhere; only an explicit `:font-size` at the call site
/// overrides. Must stay in lockstep with [`menu_font_size_from_node`].
pub(crate) fn menu_font_size_from_props(
    props: &std::collections::HashMap<String, crate::vm::Value>,
) -> f32 {
    if props.contains_key(INHERITED_FONT_SIZE_MARKER) {
        return MENU_FONT_SIZE;
    }
    let size = super::get_f32_prop(props, "font-size", MENU_FONT_SIZE);
    if size > 0.0 { size } else { MENU_FONT_SIZE }
}
