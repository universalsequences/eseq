use std::path::PathBuf;
use std::sync::Arc;

use std::time::Duration;

use crate::layout::LayoutNode;
use crossterm::event::Event;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendEvent {
    Terminal(Event),
    FileDrop(Vec<PathBuf>),
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

#[derive(Clone, Debug)]
pub struct CompletionEntry {
    pub label: String,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub struct CompletionFrame {
    /// Visible completion entries (already sliced to the scrolled window).
    pub entries: Vec<CompletionEntry>,
    /// Where to anchor the popup: (row, col) in visible-area coordinates.
    pub anchor: (usize, usize),
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
/// Built by `crate::tui::build_render_frame` from live `Editor` state and
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
    /// Text scroll offset — how many rows the text has scrolled.
    /// Used by Metal to sync widget vertical position with text scrolling.
    pub text_scroll_top: usize,
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
