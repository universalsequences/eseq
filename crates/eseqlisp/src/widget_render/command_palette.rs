//! The command palette panel (docs/command-palette-spec.md §7): a search
//! field over ranked rows, drawn by the backends' global overlay pass above
//! every tile. Its chrome and rows share the popup-menu style
//! ([`super::menu_style`]) with dropdowns and context menus.
//!
//! An item row is `[icon] Title ··· value  shortcut  ›|✓`: the title, the
//! current value dim and truncated from the left, then the key binding and a chevron (opens a
//! choice stage) or a check (the current choice / an on state). Section
//! headers are dim, smaller and unselectable.

use super::dropdown::{render_text_width_cells, truncate_text_to_width};
use super::menu_style::{
    ICON_SLOT_COLS, checkmark_primitive_at, icon_primitive, panel_chrome_primitives, rounded_rect_primitive,
    row_highlight_primitive, search_field_primitives, text_primitive,
};
use super::{
    GpuPrimitive, GpuProportionalTextPrimitive, GpuRectPrimitive, GpuTrianglePrimitive, WidgetViewport,
    ui_design_px,
};
use crate::backend::{Color, PALETTE_MAX_ROWS, PaletteFrame, PaletteRowFrame};
use crate::layout::Rect;
use crate::theme;

const FONT_SIZE: f32 = 12.5;
const HEADER_FONT_SIZE: f32 = 10.5;
const FOOTER_FONT_SIZE: f32 = 10.5;
/// Panel width in design pixels, before clamping to the window.
const PANEL_WIDTH_PX: f32 = 620.0;
const PANEL_RADIUS_PX: f32 = 22.0;
const FIELD_RADIUS_PX: f32 = 18.0;
/// Row and field heights, in rows. A list with glyphs (Apply Sound's
/// sounds) draws taller two-line rows so the glyph reads.
const ROW_HEIGHT: f32 = 1.42;
const GLYPH_ROW_HEIGHT: f32 = 3.1;
/// Virtual pixels across a row's glyph.
const GLYPH_PIXELS: f64 = 12.0;
/// The glyph's side, as a share of its row's height.
const GLYPH_SIDE: f32 = 0.84;
const FIELD_HEIGHT: f32 = 1.7;
/// Vertical padding inside the panel, in rows.
const PAD_V: f32 = 0.35;
const FOOTER_HEIGHT: f32 = 2.6;
/// Gap between the value, shortcut and chevron columns, in columns.
const COLUMN_GAP: f32 = 1.4;
/// Scrollbar thumb width, its inset from the panel edge, and the shortest
/// it gets, in design pixels.
const SCROLLBAR_PX: f32 = 6.0;
const SCROLLBAR_INSET_PX: f32 = 4.0;
const SCROLLBAR_MIN_PX: f32 = 36.0;
/// The chevron's height and stroke, in design pixels.
const CHEVRON_HEIGHT_PX: f32 = 13.0;
const CHEVRON_STROKE_PX: f32 = 2.0;

/// `base` moved `amount` of the way to white (or, negative, to black): the
/// palette sinks its panel a little and lifts its field and badge off it,
/// in any theme.
fn lift(base: Color, amount: f32) -> Color {
    let toward = if amount >= 0.0 { Color::rgb(1.0, 1.0, 1.0) } else { Color::rgb(0.0, 0.0, 0.0) };
    base.mix(toward, amount.abs())
}

fn text(row: f32, col: f32, label: String, font_size: f32, fg: Color) -> GpuPrimitive {
    GpuPrimitive::ProportionalText(text_primitive(row, col, label, font_size, fg))
}

/// `label` aligned within `width` columns ending at `right`: `h_align` 1
/// right-aligns, 0.5 centres.
fn aligned_text(
    row: f32,
    right: f32,
    width: f32,
    h_align: f32,
    label: String,
    font_size: f32,
    fg: Color,
) -> GpuPrimitive {
    GpuPrimitive::ProportionalText(GpuProportionalTextPrimitive {
        align_width: width,
        h_align,
        ..text_primitive(row, right - width, label, font_size, fg)
    })
}

fn width(text: &str, font_size: f32, viewport: WidgetViewport) -> f32 {
    render_text_width_cells(text, font_size, viewport.cell_w)
}

/// `text` cut from the left to fit `max` columns (`…k/Apps/rex-app`): the
/// end of a value (a path, a name) is the part that tells entries apart.
/// Sums character widths from the end, so it measures each once.
fn truncate_start(text: &str, max: f32, font_size: f32, viewport: WidgetViewport) -> String {
    if width(text, font_size, viewport) <= max {
        return text.to_string();
    }
    let budget = max - width("…", font_size, viewport);
    let mut used = 0.0;
    let mut start = text.len();
    for (at, ch) in text.char_indices().rev() {
        let mut buf = [0u8; 4];
        let ch_w = width(ch.encode_utf8(&mut buf), font_size, viewport);
        if used + ch_w > budget {
            break;
        }
        used += ch_w;
        start = at;
    }
    if start == text.len() {
        return String::new();
    }
    format!("…{}", &text[start..])
}

/// A right-pointing chevron as two stroked segments, its right tip at
/// `right` and centred on `center_row`; returns its width in columns. A
/// vector mark, so it reads at the size of the text rather than of the
/// `›` glyph.
fn chevron(right: f32, center_row: f32, color: Color, viewport: WidgetViewport, prims: &mut Vec<GpuPrimitive>) -> f32 {
    let (cw, ch) = (viewport.cell_w.max(1.0), viewport.cell_h.max(1.0));
    let height = ui_design_px(CHEVRON_HEIGHT_PX);
    let half = height * 0.5;
    let stroke = ui_design_px(CHEVRON_STROKE_PX);
    // In pixels: the tip, then the two arm ends half a height to the left.
    let tip = (right * cw, center_row * ch);
    let arms = [(tip.0 - half, tip.1 - half), (tip.0 - half, tip.1 + half)];
    for arm in arms {
        let (dx, dy) = (tip.0 - arm.0, tip.1 - arm.1);
        let length = (dx * dx + dy * dy).sqrt().max(1e-3);
        let (nx, ny) = (-dy / length * stroke * 0.5, dx / length * stroke * 0.5);
        // Extend past the tip by half a stroke so the arms meet cleanly.
        let (ex, ey) = (dx / length * stroke * 0.5, dy / length * stroke * 0.5);
        let corners = [
            (arm.0 + nx, arm.1 + ny),
            (arm.0 - nx, arm.1 - ny),
            (tip.0 + ex - nx, tip.1 + ey - ny),
            (tip.0 + ex + nx, tip.1 + ey + ny),
        ];
        let cell = |(x, y): (f32, f32)| [x / cw, y / ch];
        for [a, b, c] in [[0, 1, 2], [0, 2, 3]] {
            prims.push(GpuPrimitive::Triangle(GpuTrianglePrimitive {
                points: [cell(corners[a]), cell(corners[b]), cell(corners[c])],
                color,
            }));
        }
    }
    (half + stroke) / cw
}

/// Where the last drawn palette's panel, rows and clear button are, in
/// window cells, so pointer input can hit-test what the renderer laid out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaletteHitMap {
    pub panel: Rect,
    pub list_top: f32,
    pub row_height: f32,
    pub rows: usize,
    /// The list's height that fits the window, in rows: the page size the
    /// editor scrolls by follows from it ([`page_size`]).
    pub available: f32,
    /// The field's clear button, while there is input to clear.
    pub clear: Option<Rect>,
}

/// What a pointer at a window cell position is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteHit {
    Outside,
    Panel,
    Clear,
    /// A row, by index into the frame's visible rows.
    Row(usize),
}

fn contains(rect: Rect, col: f32, row: f32) -> bool {
    col >= rect.col && col < rect.col + rect.width && row >= rect.row && row < rect.row + rect.height
}

impl PaletteHitMap {
    pub fn hit(&self, col: f32, row: f32) -> PaletteHit {
        if !contains(self.panel, col, row) {
            return PaletteHit::Outside;
        }
        if self.clear.is_some_and(|clear| contains(clear, col, row)) {
            return PaletteHit::Clear;
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

/// A row's `sound-glyph` props: pixelated, as the mixer's clip cells are
/// (`:pixelate`, a virtual-pixel grid across the glyph), so a small glyph
/// reads as a clean blocky image rather than gooey noise.
fn glyph_props(source: &str) -> std::collections::HashMap<String, crate::vm::Value> {
    use crate::vm::Value;
    std::collections::HashMap::from([
        ("source".to_string(), Value::String(source.to_string())),
        ("pixelate".to_string(), Value::Number(GLYPH_PIXELS)),
    ])
}

/// How many rows of the given height fit `available` rows of list.
fn capacity(available: f32, glyph_rows: bool) -> usize {
    ((available / row_height_for(glyph_rows)).floor().max(1.0) as usize).min(PALETTE_MAX_ROWS)
}

fn row_height_for(glyph_rows: bool) -> f32 {
    if glyph_rows { GLYPH_ROW_HEIGHT } else { ROW_HEIGHT }
}

/// The rows a list shows at once (glyph rows are taller): what fit the
/// window when the palette was last drawn.
pub fn page_size(glyph_rows: bool) -> usize {
    hit_map().map_or(PALETTE_MAX_ROWS, |map| capacity(map.available, glyph_rows))
}

/// Forgets the drawn geometry (the palette closed or stopped drawing).
pub fn clear_hit_map() {
    HIT_MAP.with(|map| map.set(None));
}

fn glyph_rows(frame: &PaletteFrame) -> bool {
    frame.rows.iter().any(|row| row.glyph.is_some())
}

fn row_height(frame: &PaletteFrame) -> f32 {
    row_height_for(glyph_rows(frame))
}

/// The panel's top row, the list height that fits below it in the window,
/// and how many of the frame's rows that holds.
fn placement(frame: &PaletteFrame, viewport: WidgetViewport) -> (f32, f32, usize) {
    let total_rows = viewport.vp_h / viewport.cell_h.max(1.0);
    let top = (total_rows * 0.14).max(2.0);
    let footer = if frame.footer.is_some() { FOOTER_HEIGHT } else { 0.0 };
    let chrome = PAD_V * 3.0 + FIELD_HEIGHT + footer;
    let available = total_rows - top - chrome - 1.0;
    (top, available, capacity(available, glyph_rows(frame)))
}

/// Where the panel sits in the window, and the list rows it draws.
pub fn panel_rect(frame: &PaletteFrame, viewport: WidgetViewport) -> (Rect, usize) {
    let total_cols = viewport.vp_w / viewport.cell_w.max(1.0);
    let width = (ui_design_px(PANEL_WIDTH_PX) / viewport.cell_w.max(1.0)).min(total_cols - 4.0).max(10.0);
    let (top, _, capacity) = placement(frame, viewport);
    let list_rows = frame.rows.len().min(capacity).max(usize::from(frame.empty_text.is_some()));
    let footer = if frame.footer.is_some() { FOOTER_HEIGHT } else { 0.0 };
    let height = PAD_V + FIELD_HEIGHT + PAD_V + list_rows as f32 * row_height(frame) + PAD_V + footer;
    let rect = Rect { row: top, col: ((total_cols - width) * 0.5).max(0.0), width, height };
    (rect, list_rows)
}

/// The palette's primitives in window cell coordinates. Records the hit map.
pub fn primitives(frame: &PaletteFrame, viewport: WidgetViewport) -> Vec<GpuPrimitive> {
    let (panel, list_rows) = panel_rect(frame, viewport);
    let (_, available, capacity) = placement(frame, viewport);
    let drawn_rows = frame.rows.len().min(capacity);
    let row_h = row_height(frame);
    let fg = theme::DROPDOWN_FG();
    let dim = theme::DIM();
    // Equal visual insets on both axes: convert the vertical pad to columns.
    let pad_h = PAD_V * viewport.cell_h / viewport.cell_w.max(1.0);
    let text_pad = 1.0;
    let mut prims = Vec::new();
    let panel_bg = lift(theme::DROPDOWN_MENU_BG(), -0.22);
    // Lifted off the interface by the theme's drop shadow (crucial on
    // light themes, where the panel and what it covers are close in tone).
    prims.extend(super::menu_style::drop_shadow_primitives(panel, PANEL_RADIUS_PX, viewport));
    // A faint light rim, as native popovers draw.
    prims.extend(panel_chrome_primitives(panel, panel_bg, fg.with_alpha(0.14), PANEL_RADIUS_PX, viewport));
    prims.push(GpuPrimitive::PushClipRect(panel));
    let text_row = |rect: Rect| rect.row + (rect.height - 1.0) * 0.5;

    // ── Search field: magnifier or badge, input or placeholder, caret,
    // and a clear button while there is input.
    let field = Rect {
        row: panel.row + PAD_V,
        col: panel.col + pad_h,
        width: (panel.width - pad_h * 2.0).max(0.0),
        height: FIELD_HEIGHT,
    };
    let start = field.col + 0.8;
    let badge_w = frame.badge.as_ref().map(|badge| width(badge, FONT_SIZE, viewport) + 1.6);
    let (icon_col, text_col) = match badge_w {
        Some(badge_w) => (None, start + badge_w + 0.7),
        None => (Some(start), start + 1.6),
    };
    let field_bg = lift(panel_bg, 0.07);
    prims.extend(search_field_primitives(
        field,
        field_bg,
        FIELD_RADIUS_PX,
        icon_col,
        text_col,
        &frame.input,
        &frame.placeholder,
        FONT_SIZE,
        viewport,
    ));
    if let (Some(badge), Some(badge_w)) = (&frame.badge, badge_w) {
        let pill = Rect { row: field.row + 0.22, col: start, width: badge_w, height: field.height - 0.44 };
        prims.push(rounded_rect_primitive(pill, lift(field_bg, 0.16), FIELD_RADIUS_PX - 4.0, viewport));
        prims.push(text(text_row(pill), start + 0.8, badge.clone(), FONT_SIZE, fg));
    }
    let clear = (!frame.input.is_empty()).then(|| {
        let height = field.height * 0.56;
        let width = height * viewport.cell_h / viewport.cell_w.max(1.0);
        Rect {
            row: field.row + (field.height - height) * 0.5,
            col: field.col + field.width - width - 0.7,
            width,
            height,
        }
    });
    if let Some(button) = clear {
        prims.push(rounded_rect_primitive(button, fg.with_alpha(0.45), 100.0, viewport));
        prims.push(aligned_text(
            text_row(button),
            button.col + button.width,
            button.width,
            0.5,
            "×".to_string(),
            FONT_SIZE,
            field_bg,
        ));
    }

    // ── Rows.
    let list_top = field.row + field.height + PAD_V;
    HIT_MAP.with(|map| {
        map.set(Some(PaletteHitMap { panel, list_top, row_height: row_h, rows: drawn_rows, available, clear }))
    });
    // A scrolling list gives the scrollbar its own gutter: rows (and the
    // highlight) stop short of it rather than running underneath.
    let scrollbar_w = ui_design_px(SCROLLBAR_PX) / viewport.cell_w.max(1.0);
    let scrollbar_inset = ui_design_px(SCROLLBAR_INSET_PX) / viewport.cell_w.max(1.0);
    let gutter = if frame.scroll.is_some() { scrollbar_w + scrollbar_inset * 2.0 - pad_h * 0.5 } else { 0.0 };
    let row_rect = |idx: usize| Rect {
        row: list_top + idx as f32 * row_h,
        col: panel.col,
        width: panel.width - gutter.max(0.0),
        height: row_h,
    };
    if let Some(empty) = &frame.empty_text {
        prims.push(text(text_row(row_rect(0)), panel.col + pad_h + text_pad, empty.clone(), FONT_SIZE, dim));
    }
    let content_left = panel.col + pad_h + text_pad;
    let content_right = panel.col + panel.width - gutter.max(0.0) - pad_h - text_pad;
    for (idx, row) in frame.rows.iter().take(drawn_rows).enumerate() {
        let rect = row_rect(idx);
        if row.header {
            prims.extend(header_row(rect, row, content_left, dim, viewport));
        } else {
            let selected = frame.selected == Some(idx);
            prims.extend(item_row(rect, row, selected, content_left..content_right, viewport));
        }
    }

    // ── Scrollbar: a thin rounded thumb in the gutter, sized to the
    // visible share of the list and placed by how far it has scrolled.
    if let Some((first, total)) = frame.scroll {
        let track = Rect {
            row: list_top + 0.2,
            col: panel.col + panel.width - scrollbar_inset - scrollbar_w,
            width: scrollbar_w,
            height: (list_rows as f32 * row_h - 0.4).max(0.0),
        };
        let min_height = (ui_design_px(SCROLLBAR_MIN_PX) / viewport.cell_h.max(1.0)).min(track.height);
        let height = (track.height * (drawn_rows as f32 / total.max(1) as f32).min(1.0)).max(min_height);
        let scrollable = total.saturating_sub(drawn_rows).max(1) as f32;
        let thumb = Rect {
            row: track.row + (track.height - height) * (first as f32 / scrollable).min(1.0),
            height,
            ..track
        };
        prims.push(rounded_rect_primitive(thumb, fg.with_alpha(0.32), 100.0, viewport));
    }

    // ── Footer: the highlighted entry's signature and first doc line.
    if let Some((signature, summary)) = &frame.footer {
        let top = list_top + list_rows as f32 * row_h + PAD_V;
        let rule =
            Rect { row: top, col: panel.col + pad_h, width: (panel.width - pad_h * 2.0).max(0.0), height: 0.04 };
        prims.push(GpuPrimitive::Rect(GpuRectPrimitive { rect: rule, color: fg.with_alpha(0.12) }));
        let max = (content_right - content_left).max(0.0);
        let signature = truncate_text_to_width(signature, max, FOOTER_FONT_SIZE, viewport.cell_w);
        prims.push(GpuPrimitive::ProportionalText(GpuProportionalTextPrimitive {
            face: crate::ui::glyph_atlas::TextFace::Mono,
            ..text_primitive(top + 0.3, content_left, signature, FOOTER_FONT_SIZE, fg.with_alpha(0.75))
        }));
        if !summary.is_empty() {
            let summary = truncate_text_to_width(summary, max, FOOTER_FONT_SIZE, viewport.cell_w);
            prims.push(text(top + 1.3, content_left, summary, FOOTER_FONT_SIZE, dim));
        }
    }
    prims.push(GpuPrimitive::PopClipRect);
    prims
}

/// A section header: a small icon and the group name, dim, sitting low in
/// its row near the items it introduces.
fn header_row(rect: Rect, row: &PaletteRowFrame, left: f32, dim: Color, viewport: WidgetViewport) -> Vec<GpuPrimitive> {
    let mut prims = Vec::new();
    let mut col = left;
    let row_at = rect.row + rect.height - 1.05;
    if let Some(icon) = row.icon.as_deref().and_then(super::button::icon_name_value) {
        let icon_rect = Rect { row: row_at, height: 1.0, ..rect };
        prims.push(icon_primitive(icon_rect, col - 0.4, HEADER_FONT_SIZE, icon, dim, viewport));
        col += ICON_SLOT_COLS - 0.6;
    }
    prims.push(text(row_at, col, row.label.clone(), HEADER_FONT_SIZE, dim));
    prims
}

/// An item: `[icon] Title ··· value  shortcut  ›|✓` between `span`'s ends.
fn item_row(
    rect: Rect,
    row: &PaletteRowFrame,
    selected: bool,
    span: std::ops::Range<f32>,
    viewport: WidgetViewport,
) -> Vec<GpuPrimitive> {
    let fg = theme::list_row_fg(selected, theme::DROPDOWN_FG());
    let mut prims = Vec::new();
    if selected {
        prims.push(row_highlight_primitive(rect, theme::DROPDOWN_HOVER_BG(), viewport));
    }
    let text_row = rect.row + (rect.height - 1.0) * 0.5;
    let quiet = if selected { fg.with_alpha(0.85) } else { fg.with_alpha(0.5) };

    // Right edge inwards: the chevron or check, the shortcut, the value.
    let mut edge = span.end;
    if row.checked {
        let check = if selected { fg } else { theme::DROPDOWN_CHECK() };
        prims.push(checkmark_primitive_at(rect, edge - 1.5, FONT_SIZE, check, viewport));
        edge -= 1.5 + COLUMN_GAP * 0.5;
    } else if row.submenu {
        let center = rect.row + rect.height * 0.5;
        edge -= chevron(edge - 0.2, center, if selected { fg } else { quiet }, viewport, &mut prims) + COLUMN_GAP;
    }
    if let Some(shortcut) = row.shortcut.as_ref().filter(|shortcut| !shortcut.is_empty()) {
        let shortcut_w = width(shortcut, FONT_SIZE, viewport);
        prims.push(aligned_text(text_row, edge, shortcut_w, 1.0, shortcut.clone(), FONT_SIZE, quiet));
        edge -= shortcut_w + COLUMN_GAP;
    }
    if let Some(glyph) = &row.glyph {
        prims.extend(glyph_row_body(rect, row, glyph, selected, span.start..edge, viewport));
        return prims;
    }
    let mut label_left = span.start;
    if let Some(icon) = row.icon.as_deref().and_then(super::button::icon_name_value) {
        // Glyphs sit quieter than the title, as native menus draw them.
        let tint = if selected { fg } else { fg.with_alpha(0.55) };
        prims.push(icon_primitive(rect, span.start - 0.3, FONT_SIZE, icon, tint, viewport));
        label_left += ICON_SLOT_COLS;
    }
    // The title keeps its full width when it fits beside a short value; a
    // value takes the rest and is cut from the left, the title from the
    // right only when even that is too little.
    let space = (edge - label_left).max(0.0);
    let full_label = width(&row.label, FONT_SIZE, viewport);
    let label_max = match &row.detail {
        Some(detail) if !detail.is_empty() => full_label.min(space * 0.65),
        _ => space,
    };
    if let Some(detail) = row.detail.as_ref().filter(|detail| !detail.is_empty()) {
        let detail_max = (space - label_max - COLUMN_GAP).max(0.0);
        let detail = truncate_start(detail, detail_max, FONT_SIZE, viewport);
        let detail_w = width(&detail, FONT_SIZE, viewport);
        prims.push(aligned_text(text_row, edge, detail_w, 1.0, detail, FONT_SIZE, quiet));
    }
    let label = truncate_text_to_width(&row.label, label_max, FONT_SIZE, viewport.cell_w);
    prims.push(text(text_row, label_left, label, FONT_SIZE, fg));
    prims
}

/// A glyph row's left part: the sound glyph on a dark tile, then the title
/// over its detail (where the sound is used, what it was loaded from).
fn glyph_row_body(
    rect: Rect,
    row: &PaletteRowFrame,
    glyph: &str,
    selected: bool,
    span: std::ops::Range<f32>,
    viewport: WidgetViewport,
) -> Vec<GpuPrimitive> {
    let fg = theme::list_row_fg(selected, theme::DROPDOWN_FG());
    let mut prims = Vec::new();
    let side = rect.height * GLYPH_SIDE;
    let tile = Rect {
        row: rect.row + (rect.height - side) * 0.5,
        col: span.start - 0.3,
        width: side * viewport.cell_h / viewport.cell_w.max(1.0),
        height: side,
    };
    prims.push(rounded_rect_primitive(tile, theme::BG(), 8.0, viewport));
    let node = crate::layout::LayoutNode {
        widget_id: 0,
        stable_widget_id: None,
        subtree_root_id: None,
        parent_subtree_root_id: None,
        stable_key: None,
        widget_type: "sound-glyph".to_string(),
        rect: tile,
        props: glyph_props(glyph),
        children: Vec::new(),
        focusable: false,
        animation: Default::default(),
    };
    use super::WidgetDefinition;
    prims.extend(super::sound_glyph::SOUND_GLYPH_WIDGET.build_primitives("sound-glyph", &node, viewport));

    let left = tile.col + tile.width + 1.0;
    let max = (span.end - left).max(0.0);
    let middle = rect.row + rect.height * 0.5;
    let label = truncate_text_to_width(&row.label, max, FONT_SIZE, viewport.cell_w);
    prims.push(text(middle - 1.0, left, label, FONT_SIZE, fg));
    if let Some(detail) = row.detail.as_ref().filter(|detail| !detail.is_empty()) {
        let quiet = if selected { fg.with_alpha(0.75) } else { fg.with_alpha(0.5) };
        let detail = truncate_text_to_width(detail, max, HEADER_FONT_SIZE, viewport.cell_w);
        prims.push(text(middle + 0.05, left, detail, HEADER_FONT_SIZE, quiet));
    }
    prims
}
