//! The command palette panel (docs/command-palette-spec.md §7): a search
//! field over ranked rows, drawn by the backends' global overlay pass above
//! every tile. Its chrome and rows share the popup-menu style
//! ([`super::menu_style`]) with dropdowns and context menus.

use super::dropdown::{render_text_width_cells, truncate_text_to_width};
use super::menu_style::{
    checkmark_primitive_at, panel_chrome_primitives, rounded_rect_primitive, row_highlight_primitive,
    search_field_primitives, text_primitive,
};
use super::{GpuPrimitive, GpuProportionalTextPrimitive, WidgetViewport, ui_design_px};
use crate::backend::{Color, PALETTE_MAX_ROWS, PaletteFrame};
use crate::layout::Rect;
use crate::theme;

const FONT_SIZE: f32 = 12.5;
const FOOTER_FONT_SIZE: f32 = 10.5;
/// Panel width in design pixels, before clamping to the window.
const PANEL_WIDTH_PX: f32 = 620.0;
const PANEL_RADIUS_PX: f32 = 22.0;
const FIELD_RADIUS_PX: f32 = 14.0;
/// Row and field heights, in rows.
const ROW_HEIGHT: f32 = 1.75;
const FIELD_HEIGHT: f32 = 2.1;
/// Vertical padding inside the panel, in rows.
const PAD_V: f32 = 0.35;
const FOOTER_HEIGHT: f32 = 2.6;

fn text(row: f32, col: f32, label: String, font_size: f32, fg: Color) -> GpuPrimitive {
    GpuPrimitive::ProportionalText(text_primitive(row, col, label, font_size, fg))
}

/// `label` right-aligned so it ends at `right`.
fn right_text(row: f32, right: f32, width: f32, label: String, font_size: f32, fg: Color) -> GpuPrimitive {
    GpuPrimitive::ProportionalText(GpuProportionalTextPrimitive {
        align_width: width,
        h_align: 1.0,
        ..text_primitive(row, right - width, label, font_size, fg)
    })
}

/// The panel's top row and how many list rows fit below it in the window.
fn placement(frame: &PaletteFrame, viewport: WidgetViewport) -> (f32, usize) {
    let total_rows = viewport.vp_h / viewport.cell_h.max(1.0);
    let top = (total_rows * 0.14).max(2.0);
    let footer = if frame.footer.is_some() { FOOTER_HEIGHT } else { 0.0 };
    let chrome = PAD_V * 3.0 + FIELD_HEIGHT + footer;
    let available = total_rows - top - chrome - 1.0;
    let capacity = ((available / ROW_HEIGHT).floor().max(1.0) as usize).min(PALETTE_MAX_ROWS);
    (top, capacity)
}

/// Where the panel sits in the window, and the list rows it draws.
pub fn panel_rect(frame: &PaletteFrame, viewport: WidgetViewport) -> (Rect, usize) {
    let total_cols = viewport.vp_w / viewport.cell_w.max(1.0);
    let width = (ui_design_px(PANEL_WIDTH_PX) / viewport.cell_w.max(1.0)).min(total_cols - 4.0).max(10.0);
    let (top, capacity) = placement(frame, viewport);
    let list_rows = frame.rows.len().min(capacity).max(usize::from(frame.empty_text.is_some()));
    let footer = if frame.footer.is_some() { FOOTER_HEIGHT } else { 0.0 };
    let height = PAD_V + FIELD_HEIGHT + PAD_V + list_rows as f32 * ROW_HEIGHT + PAD_V + footer;
    let rect = Rect { row: top, col: ((total_cols - width) * 0.5).max(0.0), width, height };
    (rect, list_rows)
}

/// Where the last drawn palette's panel and rows are, in window cells, so
/// pointer input can hit-test what the renderer laid out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaletteHitMap {
    pub panel: Rect,
    pub list_top: f32,
    pub row_height: f32,
    pub rows: usize,
    /// Rows that fit the window: the page size the editor scrolls by.
    pub capacity: usize,
}

/// What a pointer at a window cell position is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteHit {
    Outside,
    Panel,
    /// A row, by index into the frame's visible rows.
    Row(usize),
}

impl PaletteHitMap {
    pub fn hit(&self, col: f32, row: f32) -> PaletteHit {
        let panel = self.panel;
        if col < panel.col || col >= panel.col + panel.width || row < panel.row || row >= panel.row + panel.height {
            return PaletteHit::Outside;
        }
        let offset = row - self.list_top;
        if offset >= 0.0 {
            let idx = (offset / self.row_height) as usize;
            if idx < self.rows {
                return PaletteHit::Row(idx);
            }
        }
        PaletteHit::Panel
    }
}

thread_local! {
    static HIT_MAP: std::cell::Cell<Option<PaletteHitMap>> = const { std::cell::Cell::new(None) };
}

/// The geometry of the palette drawn last (`None` once it stops drawing).
pub fn hit_map() -> Option<PaletteHitMap> {
    HIT_MAP.with(std::cell::Cell::get)
}

/// Forgets the drawn geometry; the editor calls this when the palette closes.
pub fn clear_hit_map() {
    HIT_MAP.with(|map| map.set(None));
}

/// The palette's primitives in window cell coordinates. Records the hit map.
pub fn primitives(frame: &PaletteFrame, viewport: WidgetViewport) -> Vec<GpuPrimitive> {
    let (panel, list_rows) = panel_rect(frame, viewport);
    let (_, capacity) = placement(frame, viewport);
    let drawn_rows = frame.rows.len().min(capacity);
    let fg = theme::DROPDOWN_FG();
    let dim = theme::DIM();
    // Equal visual insets on both axes: convert the vertical pad to columns.
    let pad_h = PAD_V * viewport.cell_h / viewport.cell_w.max(1.0);
    let text_pad = 1.0;
    let mut prims = Vec::new();
    prims.extend(panel_chrome_primitives(
        panel,
        theme::DROPDOWN_MENU_BG(),
        theme::DROPDOWN_MENU_BORDER(),
        PANEL_RADIUS_PX,
        viewport,
    ));
    prims.push(GpuPrimitive::PushClipRect(panel));

    // ── Search field: magnifier, optional badge, input or placeholder, caret.
    let field = Rect {
        row: panel.row + PAD_V,
        col: panel.col + pad_h,
        width: (panel.width - pad_h * 2.0).max(0.0),
        height: FIELD_HEIGHT,
    };
    let text_row = |rect: Rect| rect.row + (rect.height - 1.0) * 0.5;
    let start = field.col + 0.8;
    // A choice stage shows its badge where the magnifier would be.
    let badge_w = frame
        .badge
        .as_ref()
        .map(|badge| render_text_width_cells(badge, FONT_SIZE, viewport.cell_w) + 1.6);
    let (icon_col, text_col) = match badge_w {
        Some(width) => (None, start + width + 0.7),
        None => (Some(start), start + 1.6),
    };
    prims.extend(search_field_primitives(
        field,
        FIELD_RADIUS_PX,
        icon_col,
        text_col,
        &frame.input,
        &frame.placeholder,
        FONT_SIZE,
        viewport,
    ));
    if let (Some(badge), Some(width)) = (&frame.badge, badge_w) {
        let pill = Rect { row: field.row + 0.3, col: start, width, height: field.height - 0.6 };
        prims.push(rounded_rect_primitive(pill, fg.with_alpha(0.16), FIELD_RADIUS_PX - 4.0, viewport));
        prims.push(text(text_row(pill), start + 0.8, badge.clone(), FONT_SIZE, fg));
    }

    // ── Rows: label left; detail, then `›` or `✓`, right.
    let list_top = field.row + field.height + PAD_V;
    HIT_MAP.with(|map| {
        map.set(Some(PaletteHitMap { panel, list_top, row_height: ROW_HEIGHT, rows: drawn_rows, capacity }))
    });
    let row_rect = |idx: usize| Rect {
        row: list_top + idx as f32 * ROW_HEIGHT,
        col: panel.col,
        width: panel.width,
        height: ROW_HEIGHT,
    };
    if let Some(empty) = &frame.empty_text {
        let rect = row_rect(0);
        prims.push(text(text_row(rect), panel.col + pad_h + text_pad, empty.clone(), FONT_SIZE, dim));
    }
    let content_left = panel.col + pad_h + text_pad;
    let content_right = panel.col + panel.width - pad_h - text_pad;
    for (idx, row) in frame.rows.iter().take(drawn_rows).enumerate() {
        let rect = row_rect(idx);
        let selected = frame.selected == Some(idx);
        if selected {
            prims.push(row_highlight_primitive(rect, theme::DROPDOWN_HOVER_BG(), viewport));
        }
        let mut right = content_right;
        if row.checked {
            prims.push(checkmark_primitive_at(rect, right - 1.5, FONT_SIZE, theme::DROPDOWN_CHECK(), viewport));
            right -= 2.0;
        } else if row.submenu {
            let chevron_fg = if selected { fg } else { fg.with_alpha(0.55) };
            prims.push(right_text(text_row(rect), right, 1.0, "›".to_string(), FONT_SIZE, chevron_fg));
            right -= 1.6;
        }
        let mut detail_w = 0.0;
        if let Some(detail) = row.detail.as_ref().filter(|detail| !detail.is_empty()) {
            let max_w = (right - content_left) * 0.45;
            let detail = truncate_text_to_width(detail, max_w, FONT_SIZE, viewport.cell_w);
            detail_w = render_text_width_cells(&detail, FONT_SIZE, viewport.cell_w);
            let detail_fg = if selected { fg.with_alpha(0.8) } else { fg.with_alpha(0.5) };
            prims.push(right_text(text_row(rect), right, detail_w, detail, FONT_SIZE, detail_fg));
            detail_w += 1.2;
        }
        let label_w = (right - detail_w - content_left).max(0.0);
        let label = truncate_text_to_width(&row.label, label_w, FONT_SIZE, viewport.cell_w);
        prims.push(text(text_row(rect), content_left, label, FONT_SIZE, fg));
    }

    // ── Footer: the highlighted entry's signature and first doc line.
    if let Some((signature, summary)) = &frame.footer {
        let top = list_top + list_rows as f32 * ROW_HEIGHT + PAD_V;
        let rule = Rect { row: top, col: panel.col + pad_h, width: (panel.width - pad_h * 2.0).max(0.0), height: 0.04 };
        prims.push(GpuPrimitive::Rect(super::GpuRectPrimitive { rect: rule, color: fg.with_alpha(0.12) }));
        let width = (content_right - content_left).max(0.0);
        let signature = truncate_text_to_width(signature, width, FOOTER_FONT_SIZE, viewport.cell_w);
        prims.push(GpuPrimitive::ProportionalText(GpuProportionalTextPrimitive {
            mono: true,
            ..text_primitive(top + 0.3, content_left, signature, FOOTER_FONT_SIZE, fg.with_alpha(0.75))
        }));
        if !summary.is_empty() {
            let summary = truncate_text_to_width(summary, width, FOOTER_FONT_SIZE, viewport.cell_w);
            prims.push(text(top + 1.3, content_left, summary, FOOTER_FONT_SIZE, dim));
        }
    }
    prims.push(GpuPrimitive::PopClipRect);
    prims
}
