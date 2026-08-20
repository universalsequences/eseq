//! Shared visual metrics and Metal primitives for popup menus.
//!
//! Dropdown menus and pointer-anchored context menus have different state and
//! placement policies, but their panel chrome and rows intentionally share this
//! style so the two renderers cannot drift apart.

/// Font size (px) for popup-menu rows — dropdown options and `menu-item`
/// labels alike. Popup rows are chrome, not tile content, so they deliberately
/// ignore the surrounding tile's inherited font size; an explicit `:font-size`
/// prop still overrides this.
pub(crate) const MENU_FONT_SIZE: f32 = 10.0;
pub(crate) const ROW_HEIGHT: f32 = 1.4;
pub(crate) const PANEL_PADDING_V: f32 = 0.3;
pub(crate) const TEXT_PADDING_H: f32 = 0.6;
pub(crate) const HIGHLIGHT_INSET_H: f32 = 0.15;
pub(crate) const CORNER_RADIUS_PX: f32 = 10.0;
pub(crate) const APPROX_CHAR_WIDTH: f32 = 0.55;

#[cfg(target_os = "macos")]
use super::{MetalPrimitive, WidgetInstance, WidgetViewport, ndc_bounds};
#[cfg(target_os = "macos")]
use crate::backend::Color;
#[cfg(target_os = "macos")]
use crate::layout::Rect;

#[cfg(target_os = "macos")]
fn normalized_corner_radius(rect: Rect, viewport: WidgetViewport, radius_px: f32) -> f32 {
    // The shared shader's radius is normalized to half the primitive height.
    // A tiny positive value opts out of its historical pill default for rows
    // that are intentionally square.
    if radius_px <= 0.0 {
        return 0.001;
    }
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((radius_px * 2.0) / px_h).clamp(0.001, 0.5)
}

#[cfg(target_os = "macos")]
pub(crate) fn emit_rounded_rect_overlay(
    rect: Rect,
    color: Color,
    radius_px: f32,
    viewport: WidgetViewport,
) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    super::push_overlay_primitive(MetalPrimitive::WidgetInstance {
        widget_type: "dropdown".to_string(),
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
        is_background: true,
    });
}

#[cfg(target_os = "macos")]
pub(crate) fn emit_panel_chrome(
    panel_rect: Rect,
    background: Color,
    border: Color,
    viewport: WidgetViewport,
) {
    let border_px = 1.0;
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
        CORNER_RADIUS_PX + border_px,
        viewport,
    );
    emit_rounded_rect_overlay(panel_rect, background, CORNER_RADIUS_PX, viewport);
}

pub(crate) fn row_highlight_rect(row_rect: crate::layout::Rect) -> crate::layout::Rect {
    crate::layout::Rect {
        col: row_rect.col + HIGHLIGHT_INSET_H,
        width: (row_rect.width - HIGHLIGHT_INSET_H * 2.0).max(0.0),
        ..row_rect
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn emit_row_highlight(
    row_rect: Rect,
    color: Color,
    viewport: WidgetViewport,
) {
    emit_rounded_rect_overlay(
        row_highlight_rect(row_rect),
        color,
        0.0,
        viewport,
    );
}
