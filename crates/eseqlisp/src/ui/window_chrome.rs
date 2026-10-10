//! The native window chrome that overlaps the content view.
//!
//! On macOS the window uses a full-size content view: the transparent
//! titlebar band sits on top of our pixels, with the traffic-light buttons in
//! its left end. The backend measures that band and publishes it here; the
//! editor reads it when it lays out tiles, keeping user tiles below the band
//! and placing the titlebar tile (`set-titlebar`) inside it. Backends without
//! native chrome (TUI, wgpu, headless capture) leave it at zero.

use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowChrome {
    /// Height of the native titlebar band over the content, in layout cells.
    pub titlebar_rows: f32,
    /// Width the native window buttons take at the band's left end, in cells.
    pub leading_cols: f32,
    /// The window's exact content height in layout cells, fractional part
    /// included (0 when unknown). With it the tiles run to the window's
    /// bottom edge instead of stopping at the last whole row, and no row is
    /// held back for a global status line: a windowed app shows status in
    /// each tile's own bar.
    pub content_rows: f32,
}

thread_local! {
    static CHROME: Cell<WindowChrome> = const {
        Cell::new(WindowChrome { titlebar_rows: 0.0, leading_cols: 0.0, content_rows: 0.0 })
    };
}

pub fn current() -> WindowChrome {
    CHROME.with(Cell::get)
}

/// Record the measured chrome. Returns whether it changed.
pub fn set(chrome: WindowChrome) -> bool {
    CHROME.with(|cell| cell.replace(chrome) != chrome)
}
