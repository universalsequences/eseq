use std::path::PathBuf;
use std::sync::Arc;

use std::time::Duration;

use crate::layout::LayoutNode;
use crossterm::event::Event;

#[derive(Clone, Debug, PartialEq)]
pub enum BackendEvent {
    Terminal(Event),
    /// OS files dropped on the window, with the pointer position of the drop
    /// in precise cell coordinates when the backend can report it. macOS
    /// delivers no CursorMoved events during an external drag, so the last
    /// tracked mouse position is stale at drop time; backends that can query
    /// the pointer at the drop supply it here.
    FileDrop(Vec<PathBuf>, Option<(f32, f32)>),
    /// The window system requested that the application close.
    Quit,
}

// ── Colors ───────────────────────────────────────────────────────────────────

/// Backend-agnostic color in linear RGBA (0.0–1.0).
///
/// Metal wants f32 RGBA natively. The ratatui backend converts to u8 on the
/// way out. Keeping f32 here avoids precision loss when the Metal backend
/// passes colors directly to the GPU.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self::rgba(r, g, b, 1.0)
    }
    pub fn from_rgb_u8(r: u8, g: u8, b: u8) -> Self {
        Self::rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
    }
    /// Const-friendly conversion from 0–255 RGB components.
    pub const fn from_hex(r: u8, g: u8, b: u8) -> Self {
        Self::rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
    }
    /// Convert to an RGBA f32 array (useful for Metal instance data).
    pub const fn to_rgba(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);
    pub const DARK_GRAY: Self = Self::rgb(0.25, 0.25, 0.25);
    pub const YELLOW: Self = Self::rgb(1.0, 1.0, 0.0);
    pub const GREEN: Self = Self::rgb(0.0, 0.8, 0.0);
    pub const CYAN: Self = Self::rgb(0.0, 0.8, 0.8);
    pub const MAGENTA: Self = Self::rgb(0.8, 0.0, 0.8);
    pub const LIGHT_BLUE: Self = Self::rgb(0.6, 0.8, 1.0);
    pub const GRAY: Self = Self::rgb(0.7, 0.7, 0.7);

    /// Relative luminance (ITU-R BT.709).
    pub fn luma(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }
}

// ── Cell styling ─────────────────────────────────────────────────────────────

/// Style applied to a single glyph cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellStyle {
    pub fg: Color,
    pub bg: Option<Color>,
    pub bold: bool,
}

impl Default for CellStyle {
    fn default() -> Self {
        Self {
            fg: Color::WHITE,
            bg: None,
            bold: false,
        }
    }
}

/// One rendered cell: a single character with its visual style.
///
/// Both backends operate on this unit — the ratatui backend maps it to a
/// terminal cell; the Metal backend maps it to a textured quad in the glyph
/// atlas.
#[derive(Clone, Debug)]
pub struct Cell {
    pub ch: char,
    pub style: CellStyle,
}

impl Cell {
    pub fn plain(ch: char) -> Self {
        Self {
            ch,
            style: CellStyle::default(),
        }
    }
}

// ── Completion popup ──────────────────────────────────────────────────────────

/// Shared geometry for the patcher and code-editor completion overlays.
pub const AUTOCOMPLETE_PANEL_CORNER_RADIUS_PX: f32 = 14.0;
pub const AUTOCOMPLETE_PANEL_BORDER_WIDTH_PX: f32 = 1.0;
pub const AUTOCOMPLETE_ROW_CORNER_RADIUS_PX: f32 = 8.0;

/// Cell size of the code-editor completion popup, as a fraction of the buffer's
/// own text cell (`text_cell_*_scale`). Laying the popup out on the raw terminal
/// grid made it read far larger than the code it completes; keeping it relative
/// to the text cell also lets it track the editor's text zoom.
pub const AUTOCOMPLETE_TEXT_CELL_SCALE: f32 = 0.82;
/// Gap between the bottom of the cursor's text row and the top of the popup.
pub const AUTOCOMPLETE_ANCHOR_GAP_PX: f32 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CompletionPanelColumns {
    pub popup_col: usize,
    pub pane_width: usize,
    pub show_doc: bool,
}

/// Lay out the completion panel against the whole viewport, then clamp the
/// combined list/document surface around its cursor anchor. Basing doc
/// visibility only on columns to the right of the anchor made the documentation
/// disappear one character at a time even when both panes fit after shifting.
pub(crate) fn completion_panel_columns(
    anchor_col: usize,
    total_cols: usize,
    label_width: usize,
    has_doc: bool,
) -> CompletionPanelColumns {
    const DOC_GAP: usize = 1;
    const MIN_DOC_PANE_WIDTH: usize = 26;
    const DESIRED_DOC_PANE_WIDTH: usize = 54;
    const MAX_PANE_WIDTH: usize = 64;

    let viewport_width = total_cols.saturating_sub(1).max(1);
    let min_list_width = label_width.saturating_add(4).min(viewport_width);
    let max_two_pane_width = viewport_width.saturating_sub(DOC_GAP) / 2;
    let show_doc = has_doc
        && max_two_pane_width >= MIN_DOC_PANE_WIDTH
        && max_two_pane_width >= min_list_width;
    let pane_width = if show_doc {
        DESIRED_DOC_PANE_WIDTH
            .min(max_two_pane_width)
            .max(min_list_width)
            .min(MAX_PANE_WIDTH)
    } else {
        min_list_width.max(12).min(viewport_width).min(MAX_PANE_WIDTH)
    };
    let total_panel_width = if show_doc {
        pane_width * 2 + DOC_GAP
    } else {
        pane_width
    };
    let popup_col = anchor_col.min(total_cols.saturating_sub(total_panel_width + 1));

    CompletionPanelColumns { popup_col, pane_width, show_doc }
}

#[derive(Clone, Debug)]
pub struct CompletionEntry {
    pub label: String,
    pub category: Option<String>,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub struct CompletionFrame {
    /// Visible completion entries (already sliced to the scrolled window).
    pub entries: Vec<CompletionEntry>,
    /// Where to anchor the popup: (row, col) in visible-area coordinates.
    pub anchor: (usize, usize),
    /// Text-cell scale for converting the text anchor into layout/tile cells.
    pub text_cell_width_scale: f32,
    pub text_cell_height_scale: f32,
    /// Optional doc panel: title + body lines.
    pub doc: Option<(String, Vec<String>)>,
}

#[derive(Clone, Debug)]
pub struct StatusIndicator {
    /// Columns in the status row occupied by the UI toggle affordance.
    pub toggle_cols: Option<(usize, usize)>,
}

// ── RenderFrame ───────────────────────────────────────────────────────────────

/// A complete snapshot of everything a backend needs to draw one frame.
///
/// Built by `crate::ui::frame::build_render_frame` from live `Editor` state and
/// passed to whichever `Backend` is active. Backends must not mutate editor
/// state — they only read this frame.
#[derive(Clone)]
pub struct RenderFrame {
    /// Visible lines of styled cells, top-to-bottom, left-to-right.
    pub lines: Vec<Vec<Cell>>,
    /// Cursor position as (row, col) in visible-area coordinates, if visible.
    pub cursor: Option<(usize, usize)>,
    /// Buffer name shown in the title / window bar.
    pub buffer_name: String,
    /// True when the buffer has unsaved changes.
    pub dirty: bool,
    /// Styled cells for the status bar / minibuffer row.
    pub status_cells: Vec<Cell>,
    /// Metadata for manually rendered status affordances.
    pub status_indicator: StatusIndicator,
    /// Optional completion popup to overlay on top of the text area.
    pub completion: Option<CompletionFrame>,
    /// Revision token for the text/editor portion of the frame. Backends can
    /// use this to cache expensive text-layer work across layout-only redraws.
    pub text_cache_key: u64,
    /// Revision token for widget layout geometry. This only changes when the
    /// widget rect tree changes, not when render-only props like slider values
    /// update.
    pub widget_layout_cache_key: u64,
    /// Revision token for semantic widget-tree content. This changes when the
    /// widget tree output changes even if layout geometry stays the same.
    pub widget_content_cache_key: u64,
    /// Widget IDs whose props changed without a geometry change. Backends can
    /// use this to patch persistent instance buffers in place.
    pub dirty_widget_ids: Vec<u64>,
    /// Reactive UI widget tree to render. Each backend renders this in its own way:
    /// Ratatui draws characters into the cell buffer; Metal dispatches instanced GPU draw calls.
    pub widget_layout: Option<Arc<LayoutNode>>,
    /// Currently focused widget ID for highlight rendering.
    pub focused_widget_id: Option<u64>,
    /// Widget scroll offset (rows to skip when rendering widget overlay).
    pub widget_scroll_top: f32,
    /// Widget horizontal scroll offset (cols to skip when rendering widget overlay).
    pub widget_scroll_left: f32,
    /// Horizontal scroll expressed in widget/layout cells. Inline code layouts
    /// use zoomed text-cell geometry, while `widget_scroll_left` remains in
    /// logical text columns for text rendering.
    pub widget_layout_scroll_left: f32,
    /// Text scroll offset — how many rows the text has scrolled.
    /// Used by Metal to sync widget vertical position with text scrolling.
    pub text_scroll_top: usize,
    /// Width of one text cell relative to the widget/layout cell.
    pub text_cell_width_scale: f32,
    /// Height of one text cell relative to the widget/layout cell.
    pub text_cell_height_scale: f32,
}

// ── Tiled rendering ──────────────────────────────────────────────────────────

use crate::layout::Rect;
use crate::tile::{TileId, TileTabLayout};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InspectOverlay {
    /// Tile-content-local widget rect in logical cells.
    pub rect: Rect,
    pub fill: Color,
    pub border: Color,
}

/// One tile's worth of rendering data, positioned within the full screen.
pub struct TileFrame {
    pub tile_id: TileId,
    pub rect: Rect,                              // screen position for this tile
    pub body_rect: Rect,                         // screen position for the tile content body
    pub tabs: Vec<TileTabLayout>,                // folder-style tile tabs in screen coordinates
    pub is_active: bool,                         // colored border for active tile
    pub show_status: bool,                       // whether to render per-tile status bar
    pub show_border: bool,                       // whether to render tile border
    pub border_width_px: f32,                    // Metal tile border width in pixels
    pub border_radius_px: f32,                   // Metal tile border radius in pixels
    pub background_color: Option<Color>,         // Metal default buffer background color
    pub background_color_name: Option<String>,   // Theme color name for live-resolved backgrounds
    pub inspect_overlay: Option<InspectOverlay>, // hovered inspect target overlay
    pub frame: RenderFrame,                      // the per-buffer frame
}

/// A complete frame with all tiles rendered, plus global UI elements.
pub struct TiledRenderFrame {
    pub tiles: Vec<TileFrame>,
    pub completion: Option<CompletionFrame>, // completion popup (global)
}

// ── Backend trait ─────────────────────────────────────────────────────────────
pub enum BackendError {
    EventPollError,
    MetalError,
}

// For now use crossterm events — Metal will queue events and expose them via the same poll.
pub trait Backend {
    fn initialize(&mut self) -> Result<(), BackendError>;
    fn teardown(&mut self) -> Result<(), BackendError>;
    /// Returns the drawable area in (cols, rows) — used by build_render_frame
    /// to compute the visible line range and adjust scroll.
    fn viewport_size(&self) -> (usize, usize);
    fn poll_backend_event(&mut self, timeout: Duration) -> Option<BackendEvent> {
        self.poll_event(timeout).map(BackendEvent::Terminal)
    }
    fn poll_event(&mut self, timeout: Duration) -> Option<Event>;
    fn render(&mut self, frame: &RenderFrame) -> Result<(), BackendError>;
}

#[cfg(test)]
mod tests {
    use super::completion_panel_columns;

    #[test]
    fn completion_docs_stay_visible_as_anchor_moves_right() {
        let early = completion_panel_columns(6, 72, 12, true);
        let later = completion_panel_columns(18, 72, 12, true);

        assert!(early.show_doc);
        assert!(later.show_doc);
        assert_eq!(early.pane_width, later.pane_width);
        assert!(later.popup_col <= early.popup_col + 12);
        assert!(later.popup_col + later.pane_width * 2 + 1 < 72);
    }

    #[test]
    fn completion_docs_hide_only_when_viewport_is_genuinely_too_narrow() {
        let panel = completion_panel_columns(8, 48, 12, true);

        assert!(!panel.show_doc);
        assert!(panel.popup_col + panel.pane_width < 48);
    }
}
