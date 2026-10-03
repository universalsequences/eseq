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

/// Cross-thread wake-up for the native loop or a blocked event pump. A producer
/// on another thread (the MIDI input callback) calls `wake` after queueing
/// work so the host runs a tick now instead of waiting for its idle timeout.
///
/// The proxy is `Send` but not `Sync` on every platform, so it sits behind a
/// mutex; a wake is one uncontended lock.
#[derive(Clone)]
pub struct EventLoopWaker(Arc<std::sync::Mutex<winit::event_loop::EventLoopProxy<()>>>);

impl EventLoopWaker {
    pub fn new(proxy: winit::event_loop::EventLoopProxy<()>) -> Self {
        Self(Arc::new(std::sync::Mutex::new(proxy)))
    }

    /// Returns false once the event loop is gone.
    pub fn wake(&self) -> bool {
        self.0
            .lock()
            .map(|proxy| proxy.send_event(()).is_ok())
            .unwrap_or(false)
    }
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

// ── Toast ─────────────────────────────────────────────────────────────────────

/// Window-level toast, drawn bottom-right above every tile.
#[derive(Clone, Debug, PartialEq)]
pub struct ToastFrame {
    pub message: String,
    pub kind: crate::host::ToastKind,
    /// Clickable link text after the message (e.g. "Show in Finder").
    pub action_label: Option<String>,
    /// Sticky toasts carry a close button instead of a timer.
    pub closable: bool,
    /// Loading toasts only: completed fraction (0..=1) drawn as a bar under
    /// the message. `None` shows the spinner alone.
    pub progress: Option<f32>,
    /// Seconds since the loading toast first appeared; drives the spinner.
    pub elapsed_s: f32,
}

pub const TOAST_CORNER_RADIUS_PX: f32 = 10.0;
pub const TOAST_BORDER_WIDTH_PX: f32 = 2.0;
/// Horizontal padding, in toast cells, on each side of the icon + message.
const TOAST_PAD_COLS: usize = 2;
/// Panel height in toast cells; the single text row sits centred inside it.
const TOAST_HEIGHT_ROWS: f32 = 2.2;
const TOAST_MARGIN_COLS: usize = 2;
/// Rows between the text row and the window bottom, clear of a status line.
const TOAST_BOTTOM_ROWS: usize = 4;
/// Gap, in cells, before the action link and before the close button.
const TOAST_SEGMENT_GAP_COLS: usize = 3;
pub const TOAST_CLOSE_GLYPH: &str = "×";
/// Loading toasts keep at least this much text width, so the panel does not
/// jitter as the host rewrites the message on every step.
const TOAST_LOADING_MIN_TEXT_COLS: usize = 40;
/// A progress toast grows downward to hold the bar under the text row.
const TOAST_PROGRESS_HEIGHT_ROWS: f32 = 3.0;
/// Text row to bar centre, in rows.
const TOAST_PROGRESS_BAR_OFFSET_ROWS: f32 = 1.55;
const TOAST_PROGRESS_BAR_THICKNESS_PX: f32 = 4.0;
pub const TOAST_SPINNER_DOTS: usize = 10;
/// Spinner revolutions per second.
const TOAST_SPINNER_SPEED: f32 = 1.1;

impl ToastFrame {
    pub fn icon(&self) -> char {
        match self.kind {
            crate::host::ToastKind::Success => '✓',
            crate::host::ToastKind::Error => '✕',
            // The spinner is geometry, drawn by `toast_loading_shapes`.
            crate::host::ToastKind::Loading => ' ',
        }
    }
}

/// Toast geometry on the layout-cell grid (`cell_w` x `cell_h` pixels).
/// Text columns/rows are whole cells so glyphs land on the atlas grid; the
/// panel extends fractionally around them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToastPlacement {
    pub panel_col: f32,
    pub panel_row: f32,
    pub panel_cols: f32,
    pub panel_rows: f32,
    pub icon_col: usize,
    pub text_col: usize,
    pub text_row: usize,
    pub text_max_cols: usize,
    /// First column and width of the action link, when the toast has one.
    pub action_col: Option<usize>,
    pub action_cols: usize,
    /// Column of the close glyph, when the toast is closable.
    pub close_col: Option<usize>,
    /// Progress bar track `(col, cols, centre_row)`, when the toast has one.
    pub progress_bar: Option<(f32, f32, f32)>,
}

/// What a pointer at `(col, row)` lands on inside a placed toast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastHit {
    Action,
    Close,
    Panel,
}

impl ToastPlacement {
    pub fn hit(&self, col: f32, row: f32) -> Option<ToastHit> {
        let inside = col >= self.panel_col
            && col < self.panel_col + self.panel_cols
            && row >= self.panel_row
            && row < self.panel_row + self.panel_rows;
        if !inside {
            return None;
        }
        // One cell of slack either side: the close glyph is a single cell.
        if let Some(close) = self.close_col
            && col >= close as f32 - 1.0
            && col < close as f32 + 2.0
        {
            return Some(ToastHit::Close);
        }
        if let Some(action) = self.action_col
            && col >= action as f32
            && col < (action + self.action_cols) as f32
        {
            return Some(ToastHit::Action);
        }
        Some(ToastHit::Panel)
    }
}

pub fn toast_placement(toast: &ToastFrame, total_cols: usize, total_rows: usize) -> Option<ToastPlacement> {
    // icon, gap, message[, gap, action][, gap, close]; the action and close
    // button keep their width and the message is what gets clipped.
    let action_cols = toast.action_label.as_ref().map_or(0, |label| label.chars().count());
    let action_extra = if action_cols > 0 { TOAST_SEGMENT_GAP_COLS + action_cols } else { 0 };
    let close_extra = if toast.closable { TOAST_SEGMENT_GAP_COLS + 1 } else { 0 };
    let chrome_cols =
        TOAST_PAD_COLS * 2 + 2 + TOAST_MARGIN_COLS * 2 + action_extra + close_extra;
    let text_max_cols = total_cols.checked_sub(chrome_cols)?.min(80);
    if text_max_cols == 0 || total_rows < TOAST_BOTTOM_ROWS + 2 {
        return None;
    }
    let loading = toast.kind == crate::host::ToastKind::Loading;
    let min_text_cols = if loading { TOAST_LOADING_MIN_TEXT_COLS } else { 0 };
    let text_cols = toast.message.chars().count().max(min_text_cols).min(text_max_cols);
    let panel_cols = TOAST_PAD_COLS * 2 + 2 + text_cols + action_extra + close_extra;
    let panel_col = total_cols - TOAST_MARGIN_COLS - panel_cols;
    let with_bar = toast.kind == crate::host::ToastKind::Loading && toast.progress.is_some();
    // A progress toast is one row taller; lift its text so the panel bottom
    // stays where a plain toast's does, clear of the status line.
    let text_row = total_rows - TOAST_BOTTOM_ROWS - usize::from(with_bar);
    let text_col = panel_col + TOAST_PAD_COLS + 2;
    let action_col =
        (action_cols > 0).then_some(text_col + text_cols + TOAST_SEGMENT_GAP_COLS);
    let close_col = toast
        .closable
        .then_some(text_col + text_cols + action_extra + TOAST_SEGMENT_GAP_COLS);
    let panel_row = text_row as f32 - (TOAST_HEIGHT_ROWS - 1.0) / 2.0;
    let panel_rows = if with_bar { TOAST_PROGRESS_HEIGHT_ROWS } else { TOAST_HEIGHT_ROWS };
    let progress_bar = with_bar.then(|| {
        let end = (panel_col + panel_cols - TOAST_PAD_COLS) as f32;
        (text_col as f32, end - text_col as f32, text_row as f32 + TOAST_PROGRESS_BAR_OFFSET_ROWS)
    });
    Some(ToastPlacement {
        panel_col: panel_col as f32,
        panel_row,
        panel_cols: panel_cols as f32,
        panel_rows,
        icon_col: panel_col + TOAST_PAD_COLS,
        text_col,
        text_row,
        text_max_cols: text_cols,
        action_col,
        action_cols,
        close_col,
        progress_bar,
    })
}

/// One filled rounded rect of loading-toast chrome, in window pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToastShape {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: Color,
    pub radius_px: f32,
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color::rgb(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t)
}

/// How far the toast panel fill leans toward its kind's accent colour. Every
/// theme's `toast-bg` is close to the window background, so an untinted
/// panel disappears against it.
const TOAST_BG_ACCENT_TINT: f32 = 0.14;

/// Panel fill, border and icon colour for a toast of `kind`. The border is
/// always the kind's accent (theme accent while loading, success green,
/// error red) so the toast stands out against any background.
pub fn toast_colors(kind: crate::host::ToastKind) -> (Color, Color, Color) {
    use super::theme;
    let accent = match kind {
        crate::host::ToastKind::Success => theme::TOAST_SUCCESS(),
        crate::host::ToastKind::Error => theme::TOAST_ERROR(),
        crate::host::ToastKind::Loading => theme::ACCENT(),
    };
    let base = theme::TOAST_BG();
    let bg = Color { a: base.a, ..mix(base, accent, TOAST_BG_ACCENT_TINT) };
    (bg, accent, accent)
}

/// Soft drop shadow behind the toast panel: translucent black rounded rects,
/// each wider and fainter than the last, nudged down. Draw before the panel.
pub fn toast_shadow_shapes(place: &ToastPlacement, cell_w: f32, cell_h: f32, px_scale: f32) -> Vec<ToastShape> {
    let (x, y) = (place.panel_col * cell_w, place.panel_row * cell_h);
    let (w, h) = (place.panel_cols * cell_w, place.panel_rows * cell_h);
    [(2.0, 0.30), (6.0, 0.16), (12.0, 0.08)]
        .into_iter()
        .map(|(spread, alpha): (f32, f32)| {
            let s = spread * px_scale;
            ToastShape {
                x: x - s,
                y: y - s + 3.0 * px_scale,
                w: w + 2.0 * s,
                h: h + 2.0 * s,
                color: Color::rgba(0.0, 0.0, 0.0, alpha),
                radius_px: TOAST_CORNER_RADIUS_PX * px_scale + s,
            }
        })
        .collect()
}

/// One dot of the loading spinner, centre and diameter in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpinnerDot {
    pub x: f32,
    pub y: f32,
    pub d: f32,
    /// 0..1: how far toward the accent colour this dot is drawn.
    pub intensity: f32,
}

/// The loading spinner: a comet of dots on a ring of radius `ring` around
/// `(cx, cy)`. The brightest, largest dot sweeps clockwise and the tail
/// fades behind it. Shared by the loading toast and the tree's loading row.
pub fn spinner_dots(cx: f32, cy: f32, ring: f32, elapsed_s: f32) -> [SpinnerDot; TOAST_SPINNER_DOTS] {
    use std::f32::consts::TAU;
    let head = (elapsed_s * TOAST_SPINNER_SPEED).fract() * TAU;
    std::array::from_fn(|i| {
        let angle = i as f32 / TOAST_SPINNER_DOTS as f32 * TAU;
        let behind = (head - angle).rem_euclid(TAU);
        let t = 1.0 - behind / TAU;
        // Angle 0 at twelve o'clock, turning clockwise on screen.
        SpinnerDot {
            x: cx + ring * angle.sin(),
            y: cy - ring * angle.cos(),
            d: ring * 0.5 * (0.45 + 0.55 * t),
            intensity: 0.12 + 0.88 * t * t,
        }
    })
}

/// Spinner ring and progress bar for a loading toast. Colors are pre-blended
/// over `bg`, so both backends draw them opaque on the panel. Empty for any
/// other toast kind.
pub fn toast_loading_shapes(
    toast: &ToastFrame,
    place: &ToastPlacement,
    cell_w: f32,
    cell_h: f32,
    bg: Color,
    fg: Color,
    accent: Color,
) -> Vec<ToastShape> {
    let mut shapes = Vec::new();
    if toast.kind != crate::host::ToastKind::Loading {
        return shapes;
    }
    // Centred on the icon cell's left edge, the ring borrows the panel
    // padding and stays clear of the message.
    let cx = place.icon_col as f32 * cell_w;
    let cy = (place.text_row as f32 + 0.5) * cell_h;
    let ring = (cell_h * 0.42).min(cell_w * 1.3);
    for dot in spinner_dots(cx, cy, ring, toast.elapsed_s) {
        shapes.push(ToastShape {
            x: dot.x - dot.d / 2.0,
            y: dot.y - dot.d / 2.0,
            w: dot.d,
            h: dot.d,
            color: mix(bg, accent, dot.intensity),
            radius_px: dot.d / 2.0,
        });
    }
    if let (Some(progress), Some((col, cols, row))) = (toast.progress, place.progress_bar) {
        let h = TOAST_PROGRESS_BAR_THICKNESS_PX;
        let (x, y, w) = (col * cell_w, row * cell_h - h / 2.0, cols * cell_w);
        let pill = |x: f32, w: f32, color: Color| ToastShape { x, y, w, h, color, radius_px: h / 2.0 };
        shapes.push(pill(x, w, mix(bg, fg, 0.12)));
        let fill = w * progress.clamp(0.0, 1.0);
        if fill >= h {
            shapes.push(pill(x, fill, accent));
            // A highlight glides along the filled part so the bar reads as
            // alive between steps.
            let sheen_w = (w * 0.18).min(fill);
            let travel = fill + sheen_w;
            let start = x - sheen_w + (toast.elapsed_s * 0.7).fract() * travel;
            let (s0, s1) = (start.max(x), (start + sheen_w).min(x + fill));
            if s1 - s0 >= h {
                shapes.push(pill(s0, s1 - s0, mix(accent, Color::WHITE, 0.45)));
            }
        }
    }
    shapes
}

#[cfg(test)]
mod toast_tests {
    use super::*;
    use crate::host::ToastKind;

    fn toast(message: &str) -> ToastFrame {
        ToastFrame {
            message: message.to_string(),
            kind: ToastKind::Success,
            action_label: None,
            closable: false,
            progress: None,
            elapsed_s: 0.0,
        }
    }

    #[test]
    fn sticky_toast_places_action_and_close_after_the_message() {
        let frame = ToastFrame {
            action_label: Some("Show in Finder".to_string()),
            closable: true,
            ..toast("Saved take.wav")
        };
        let place = toast_placement(&frame, 120, 40).unwrap();
        assert_eq!(place.panel_col + place.panel_cols, 118.0);
        let action = place.action_col.unwrap();
        let close = place.close_col.unwrap();
        assert!(place.text_col + "Saved take.wav".len() < action);
        assert!(action + "Show in Finder".len() < close);
        assert!(((close + 1 + 2) as f32) <= place.panel_col + place.panel_cols);
        let row = place.text_row as f32 + 0.5;
        assert_eq!(place.hit(action as f32 + 0.5, row), Some(ToastHit::Action));
        assert_eq!(place.hit(close as f32 + 0.5, row), Some(ToastHit::Close));
        assert_eq!(place.hit(place.icon_col as f32 + 0.5, row), Some(ToastHit::Panel));
        assert_eq!(place.hit(place.panel_col - 1.0, row), None);

        // A narrow window clips the message, never the link or close button.
        let narrow = toast_placement(&ToastFrame { message: "x".repeat(200), ..frame }, 60, 20).unwrap();
        assert_eq!(narrow.panel_col, 2.0);
        assert_eq!(narrow.close_col.unwrap() + 1 + 2, 58);
    }

    #[test]
    fn toast_sits_bottom_right_inside_the_viewport() {
        let place = toast_placement(&toast("Saved demo"), 120, 40).unwrap();
        assert_eq!(place.panel_col + place.panel_cols, 118.0);
        assert!(place.panel_row + place.panel_rows < 40.0);
        assert!(place.panel_row > 30.0);
        assert!(place.icon_col < place.text_col);
        assert_eq!(place.text_col + "Saved demo".len(), 118 - 2);
    }

    #[test]
    fn long_toast_text_is_clipped_to_the_viewport() {
        let place = toast_placement(&toast(&"x".repeat(200)), 40, 20).unwrap();
        assert_eq!(place.panel_col, 2.0);
        assert_eq!(place.text_col + place.text_max_cols, 40 - 4);
        assert!(toast_placement(&toast("Saved"), 6, 20).is_none());
        assert!(toast_placement(&toast("Saved"), 80, 3).is_none());
    }

    #[test]
    fn progress_toast_keeps_a_stable_width_and_draws_its_bar_below_the_text() {
        let loading = |message: &str, progress| ToastFrame {
            kind: ToastKind::Loading,
            progress,
            ..toast(message)
        };
        let short = toast_placement(&loading("Loading", Some(0.25)), 120, 40).unwrap();
        let long = toast_placement(&loading("Loading project · track 3/12", Some(0.5)), 120, 40).unwrap();
        assert_eq!(short.panel_cols, long.panel_cols, "the panel must not jitter per step");
        let (col, cols, row) = long.progress_bar.unwrap();
        assert_eq!(col, long.text_col as f32);
        assert!(row > long.text_row as f32 + 1.0);
        assert!(row < long.panel_row + long.panel_rows);
        assert!(col + cols < long.panel_col + long.panel_cols);
        assert_eq!(toast_placement(&loading("x", None), 120, 40).unwrap().progress_bar, None);

        let (bg, fg, accent) = (Color::BLACK, Color::WHITE, Color::GREEN);
        let frame = loading("Loading", Some(0.5));
        let shapes = toast_loading_shapes(&frame, &long, 8.0, 16.0, bg, fg, accent);
        // Ten spinner dots, the bar track, its fill, and (maybe) the sheen.
        assert!(shapes.len() >= TOAST_SPINNER_DOTS + 2);
        let track = shapes[TOAST_SPINNER_DOTS];
        let fill = shapes[TOAST_SPINNER_DOTS + 1];
        assert_eq!(fill.x, track.x);
        assert!((fill.w - track.w * 0.5).abs() < 1e-3);
        // Every dot stays left of the message.
        for dot in &shapes[..TOAST_SPINNER_DOTS] {
            assert!(dot.x + dot.w <= long.text_col as f32 * 8.0);
        }
        assert!(toast_loading_shapes(&toast("Saved"), &long, 8.0, 16.0, bg, fg, accent).is_empty());
    }

    #[test]
    fn spinner_head_rotates_with_elapsed_time() {
        let place = toast_placement(&toast("Loading"), 120, 40).unwrap();
        let brightest = |elapsed_s| {
            let frame = ToastFrame { kind: ToastKind::Loading, elapsed_s, ..toast("Loading") };
            let shapes = toast_loading_shapes(&frame, &place, 8.0, 16.0, Color::BLACK, Color::WHITE, Color::WHITE);
            (0..shapes.len()).max_by(|&a, &b| shapes[a].color.r.total_cmp(&shapes[b].color.r)).unwrap()
        };
        assert_ne!(brightest(0.0), brightest(0.3));
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
    pub toast: Option<ToastFrame>,           // bottom-right toast (global)
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
