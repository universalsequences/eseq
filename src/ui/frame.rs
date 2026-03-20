use crate::backend::{
    Cell, CellStyle, Color, CompletionEntry, CompletionFrame, RenderFrame, TileFrame,
    TiledRenderFrame,
};
use crate::buffer::Buffer;
use crate::editor::{Editor, ViewMode};
use crate::layout::Rect;
use crate::mode::{TokenClass, TokenSpan, highlight_line};
use crate::text::matching_paren;
use crate::theme;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

// ── Semantic region colors (re-exported from theme) ──────────────────────────

pub const BG_REGION: Color = theme::BG_REGION;
pub const BG_SEXP: Color = theme::BG_SEXP;
pub const BG_EVAL_FLASH: Color = theme::BG_EVAL_FLASH;
pub const BG_MATCH_PAREN: Color = theme::BG_MATCH_PAREN;
pub const FG_MATCH_PAREN: Color = theme::FG_MATCH_PAREN;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Maps a token class to its foreground color.
pub fn token_color(class: TokenClass) -> Color {
    match class {
        TokenClass::Comment => theme::SYN_COMMENT,
        TokenClass::String => theme::SYN_STRING,
        TokenClass::Number => theme::SYN_NUMBER,
        TokenClass::Keyword => theme::SYN_KEYWORD,
        TokenClass::Builtin => theme::SYN_BUILTIN,
        TokenClass::Special => theme::SYN_SPECIAL,
        TokenClass::Delimiter => theme::SYN_DELIMITER,
    }
}

/// Returns the fg Color for the token covering `col`, or the theme foreground.
fn token_fg(col: usize, spans: Option<&Vec<TokenSpan>>) -> Color {
    spans
        .and_then(|ss| ss.iter().find(|s| col >= s.start && col < s.end))
        .map(|s| token_color(s.class))
        .unwrap_or(theme::FG)
}

/// Returns true if `(row, col)` falls inside a highlighted range.
fn in_range(
    row: usize,
    col: usize,
    range: Option<((usize, usize), (usize, usize))>,
    line_len: usize,
) -> bool {
    let Some(((sr, sc), (er, ec))) = range else {
        return false;
    };
    if row < sr || row > er {
        return false;
    }
    let start = if row == sr { sc } else { 0 };
    let end = if row == er { ec } else { line_len };
    col >= start && col < end
}

// ── Frame builder ─────────────────────────────────────────────────────────────

/// Build a backend-agnostic `RenderFrame` from the current editor state.
///
/// `viewport_height` is the number of text lines the backend can display
/// (excluding borders / status bar). The backend provides this via
/// `Backend::viewport_size()`.
pub fn build_render_frame(
    editor: &mut Editor,
    viewport_width: usize,
    viewport_height: usize,
) -> RenderFrame {
    editor.set_layout_viewport(viewport_width as u16, viewport_height as u16);
    editor.active_buffer_mut().adjust_scroll(viewport_height);

    let region_range = editor.active_region_range();
    let sexp_range = editor.active_sexp_range();
    let eval_flash_range = editor.active_eval_flash_range();
    let highlight_spans = editor.active_highlight_spans();

    let buf = editor.active_buffer();
    let scroll_top = buf.scroll_top;
    let match_pos = matching_paren(&buf.lines, buf.cursor);
    let (cursor_row, cursor_col) = buf.cursor;

    let visible = scroll_top..(scroll_top + viewport_height).min(buf.lines.len());

    let lines: Vec<Vec<Cell>> = buf.lines[visible]
        .iter()
        .enumerate()
        .map(|(i, content)| {
            let row = scroll_top + i;
            let match_col = match_pos.and_then(|(mr, mc)| if mr == row { Some(mc) } else { None });
            let row_spans = highlight_spans.get(row);
            let line_len = content.len();
            let mut cells: Vec<Cell> = vec![];
            for col in 0..line_len + 1 {
                let mut fg = token_fg(col, row_spans);
                let mut bg = if in_range(row, col, eval_flash_range, line_len) {
                    Some(BG_EVAL_FLASH)
                } else if in_range(row, col, sexp_range, line_len) {
                    Some(BG_SEXP)
                } else if in_range(row, col, region_range, line_len) {
                    Some(BG_REGION)
                } else {
                    None
                };
                if match_col == Some(col) {
                    bg = Some(BG_MATCH_PAREN);
                    fg = FG_MATCH_PAREN;
                }
                cells.push(Cell {
                    style: CellStyle {
                        bg,
                        fg,
                        bold: false,
                    },
                    ch: content.chars().nth(col).unwrap_or(' '),
                });
            }
            cells
        })
        .collect();

    // Cursor in visible-area coordinates
    let cursor = if !buf.read_only && cursor_row >= scroll_top && cursor_row < scroll_top + viewport_height {
        Some((cursor_row - scroll_top, cursor_col))
    } else {
        None
    };

    // Status bar text
    let buf = editor.active_buffer();
    let status = if let Some(prompt) = editor.minibuffer_prompt() {
        format!(" {prompt}")
    } else if let Some(prompt) = editor.prompt_text() {
        prompt
    } else if let Some(msg) = &editor.minibuffer {
        format!(" {msg}")
    } else {
        let dirty = if buf.dirty { "**" } else { "  " };
        let ro = if buf.read_only { "[RO] " } else { "" };
        let mode_name = buf.mode.name();
        let view_tag = match buf.view_mode {
            ViewMode::UiOnly => " [UI]",
            ViewMode::TextOnly => " [TEXT]",
            ViewMode::Both => "",
        };
        format!(
            " {dirty} {ro}{}  ({mode_name}){view_tag}  L{} C{}",
            buf.name,
            cursor_row + 1,
            cursor_col + 1
        )
    };

    // Completion popup
    let completion = editor.completion_state().map(|comp| {
        let visible_count = comp.items.len().min(8);
        let start = comp
            .scroll
            .min(comp.items.len().saturating_sub(visible_count));
        let entries = comp
            .items
            .iter()
            .enumerate()
            .skip(start)
            .take(visible_count)
            .map(|(idx, item)| CompletionEntry {
                label: item.label.clone(),
                selected: idx == comp.selected,
            })
            .collect();
        let doc = comp.items.get(comp.selected).map(|item| {
            let title = item.signature.clone().unwrap_or_else(|| item.label.clone());
            let body = item
                .docs
                .clone()
                .unwrap_or_else(|| "No documentation.".to_string())
                .lines()
                .map(str::to_string)
                .collect();
            (title, body)
        });
        CompletionFrame {
            entries,
            anchor: (cursor_row.saturating_sub(scroll_top), cursor_col),
            doc,
        }
    });

    let text_cache_key = {
        let mut hasher = DefaultHasher::new();
        editor.active_buffer().id.hash(&mut hasher);
        editor.active_buffer().revision.hash(&mut hasher);
        editor.active_buffer().mode.hash(&mut hasher);
        viewport_width.hash(&mut hasher);
        viewport_height.hash(&mut hasher);
        scroll_top.hash(&mut hasher);
        cursor_row.hash(&mut hasher);
        cursor_col.hash(&mut hasher);
        region_range.hash(&mut hasher);
        sexp_range.hash(&mut hasher);
        eval_flash_range.hash(&mut hasher);
        status.hash(&mut hasher);
        completion
            .as_ref()
            .map(|comp| {
                (
                    comp.anchor,
                    comp.entries
                        .iter()
                        .map(|e| (&e.label, e.selected))
                        .collect::<Vec<_>>(),
                    comp.doc.as_ref().map(|(title, body)| (title, body)),
                )
            })
            .hash(&mut hasher);
        hasher.finish()
    };

    let dirty_widget_ids = editor.take_dirty_widget_ids();
    let buf = editor.active_buffer();
    let view_mode = buf.view_mode;

    // In UI-only mode, suppress buffer text (but keep everything else intact)
    let (frame_lines, frame_cursor) = match view_mode {
        ViewMode::UiOnly => (vec![], None),
        _ => (lines, cursor),
    };

    // In text-only mode, suppress widget overlay
    let (frame_widget_layout, frame_focused_id) = match view_mode {
        ViewMode::TextOnly => (None, None),
        _ => (editor.widget_layout(), editor.focused_widget_id()),
    };

    RenderFrame {
        lines: frame_lines,
        cursor: frame_cursor,
        buffer_name: buf.name.clone(),
        dirty: buf.dirty,
        status,
        completion,
        text_cache_key,
        widget_layout_cache_key: editor.widget_layout_revision(),
        dirty_widget_ids,
        widget_layout: frame_widget_layout,
        focused_widget_id: frame_focused_id,
        widget_scroll_top: editor.widget_scroll_top(),
        widget_scroll_left: editor.widget_scroll_left(),
        text_scroll_top: scroll_top,
    }
}

// ── Tiled frame builder ──────────────────────────────────────────────────────

/// Build a `TiledRenderFrame` from the current tile tree.
///
/// Each tile leaf gets its own `RenderFrame` rendered into its sub-rect.
/// The active tile uses full editor state (highlights, completion, etc.);
/// non-active tiles get simpler text-only rendering.
/// Build a tiled render frame.
/// `cell_borders`: if true, subtract 1-cell borders from each tile's content area (TUI).
///                 if false, content fills the full tile (Metal — borders are pixel lines).
pub fn build_tiled_render_frame(
    editor: &mut Editor,
    total_width: usize,
    total_height: usize,
) -> TiledRenderFrame {
    build_tiled_render_frame_impl(editor, total_width, total_height, true)
}

pub fn build_tiled_render_frame_borderless(
    editor: &mut Editor,
    total_width: usize,
    total_height: usize,
) -> TiledRenderFrame {
    build_tiled_render_frame_impl(editor, total_width, total_height, false)
}

fn build_tiled_render_frame_impl(
    editor: &mut Editor,
    total_width: usize,
    total_height: usize,
    cell_borders: bool,
) -> TiledRenderFrame {
    let total_area = Rect {
        row: 0,
        col: 0,
        width: total_width as u16,
        height: total_height as u16,
    };
    let tile_rects = editor.tile_root.compute_rects(total_area);
    let active_tile = editor.active_tile;

    // Build completion popup from active tile
    let buf = editor.active_buffer();
    let (cursor_row, cursor_col) = buf.cursor;
    let scroll_top = buf.scroll_top;
    let completion = editor.completion_state().map(|comp| {
        let visible_count = comp.items.len().min(8);
        let start = comp
            .scroll
            .min(comp.items.len().saturating_sub(visible_count));
        let entries = comp
            .items
            .iter()
            .enumerate()
            .skip(start)
            .take(visible_count)
            .map(|(idx, item)| CompletionEntry {
                label: item.label.clone(),
                selected: idx == comp.selected,
            })
            .collect();
        let doc = comp.items.get(comp.selected).map(|item| {
            let title = item.signature.clone().unwrap_or_else(|| item.label.clone());
            let body = item
                .docs
                .clone()
                .unwrap_or_else(|| "No documentation.".to_string())
                .lines()
                .map(str::to_string)
                .collect();
            (title, body)
        });
        CompletionFrame {
            entries,
            anchor: (cursor_row.saturating_sub(scroll_top), cursor_col),
            doc,
        }
    });

    // Pre-collect per-tile metadata to avoid borrow conflicts
    let tile_info: Vec<_> = tile_rects
        .iter()
        .map(|(tile_id, rect)| {
            let leaf = editor.tile_root.find_leaf(*tile_id).unwrap();
            let buf = &editor.buffers[leaf.buffer_idx];
            let frame_key = (buf.revision, leaf.layout_revision, buf.scroll_top);
            let cached = leaf
                .cached_inactive_frame
                .as_ref()
                .filter(|(key, _)| *key == frame_key)
                .map(|(_, frame)| frame.clone());
            (
                *tile_id,
                *rect,
                leaf.buffer_idx,
                leaf.show_status,
                leaf.focused_widget_id,
                leaf.widget_scroll_top,
                leaf.widget_scroll_left,
                leaf.layout_revision,
                leaf.cached_layout.clone(),
                frame_key,
                cached,
            )
        })
        .collect();

    // Only compute symbols if we actually need to rebuild an inactive tile
    let mut symbols: Option<Vec<String>> = None;
    let mut tiles = Vec::with_capacity(tile_info.len());

    for (
        tile_id,
        rect,
        buffer_idx,
        show_status,
        focused_widget_id,
        widget_scroll_top,
        widget_scroll_left,
        layout_revision,
        cached_layout,
        frame_key,
        cached,
    ) in tile_info
    {
        let is_active = tile_id == active_tile;

        // Compute inner dimensions
        let inner_width;
        let inner_height;
        if cell_borders {
            // TUI: subtract 1-cell borders on each side
            inner_width = (rect.width as usize).saturating_sub(2);
            inner_height = if show_status {
                (rect.height as usize).saturating_sub(3) // border top/bottom + status
            } else {
                (rect.height as usize).saturating_sub(2) // border top/bottom
            };
        } else {
            // Metal: content fills full tile, status bar takes 1 row
            inner_width = rect.width as usize;
            inner_height = if show_status {
                (rect.height as usize).saturating_sub(1) // status bar only
            } else {
                rect.height as usize
            };
        }

        if is_active {
            // Active tile: use full build_render_frame (has highlights, etc.)
            let frame = build_render_frame(editor, inner_width, inner_height);
            tiles.push(TileFrame {
                tile_id,
                rect,
                is_active: true,
                show_status,
                frame,
            });
        } else {
            // Inactive tile: reuse cached frame if content is unchanged
            let frame = if let Some(frame) = cached {
                frame
            } else {
                let syms = symbols.get_or_insert_with(|| editor.runtime_mut().completion_symbols());
                let frame = build_inactive_tile_frame_from_parts(
                    &editor.buffers[buffer_idx],
                    focused_widget_id,
                    widget_scroll_top,
                    widget_scroll_left,
                    layout_revision,
                    cached_layout,
                    syms,
                    inner_width,
                    inner_height,
                );
                // Cache for next frame
                if let Some(leaf) = editor.tile_root.find_leaf_mut(tile_id) {
                    leaf.cached_inactive_frame = Some((frame_key, frame.clone()));
                }
                frame
            };
            let frame = apply_view_mode(frame, editor.buffers[buffer_idx].view_mode);
            tiles.push(TileFrame {
                tile_id,
                rect,
                is_active: false,
                show_status,
                frame,
            });
        }
    }

    TiledRenderFrame { tiles, completion }
}

/// Build a simple RenderFrame for an inactive (non-focused) tile.
fn build_inactive_tile_frame_from_parts(
    buffer: &Buffer,
    focused_widget_id: Option<u64>,
    widget_scroll_top: u16,
    widget_scroll_left: u16,
    layout_revision: u64,
    cached_layout: Option<std::sync::Arc<crate::layout::LayoutNode>>,
    symbols: &[String],
    viewport_width: usize,
    viewport_height: usize,
) -> RenderFrame {
    let scroll_top = buffer.scroll_top;
    let (cursor_row, cursor_col) = buffer.cursor;
    let visible = scroll_top..(scroll_top + viewport_height).min(buffer.lines.len());

    // Only highlight visible lines — not the entire buffer.
    let visible_lines = &buffer.lines[visible];
    let highlight_spans: Vec<Vec<TokenSpan>> = visible_lines
        .iter()
        .map(|line| highlight_line(&buffer.mode, line, symbols, buffer))
        .collect();

    let lines: Vec<Vec<Cell>> = visible_lines
        .iter()
        .enumerate()
        .map(|(i, content)| {
            let row_spans = highlight_spans.get(i);
            let line_len = content.len();
            let mut cells: Vec<Cell> = vec![];
            for col in 0..line_len + 1 {
                let fg = token_fg(col, row_spans);
                cells.push(Cell {
                    style: CellStyle {
                        bg: None,
                        fg,
                        bold: false,
                    },
                    ch: content.chars().nth(col).unwrap_or(' '),
                });
            }
            cells
        })
        .collect();

    let cursor = if !buffer.read_only
        && cursor_row >= scroll_top
        && cursor_row < scroll_top + viewport_height
    {
        Some((cursor_row - scroll_top, cursor_col))
    } else {
        None
    };

    let dirty_marker = if buffer.dirty { "**" } else { "  " };
    let ro = if buffer.read_only { "[RO] " } else { "" };
    let mode_name = buffer.mode.name();
    let view_tag = match buffer.view_mode {
        ViewMode::UiOnly => " [UI]",
        ViewMode::TextOnly => " [TEXT]",
        ViewMode::Both => "",
    };
    let status = format!(
        " {dirty_marker} {ro}{}  ({mode_name}){view_tag}  L{} C{}",
        buffer.name,
        cursor_row + 1,
        cursor_col + 1
    );

    let text_cache_key = {
        let mut hasher = DefaultHasher::new();
        buffer.id.hash(&mut hasher);
        buffer.revision.hash(&mut hasher);
        viewport_width.hash(&mut hasher);
        viewport_height.hash(&mut hasher);
        scroll_top.hash(&mut hasher);
        hasher.finish()
    };

    RenderFrame {
        lines,
        cursor,
        buffer_name: buffer.name.clone(),
        dirty: buffer.dirty,
        status,
        completion: None,
        text_cache_key,
        widget_layout_cache_key: layout_revision,
        dirty_widget_ids: vec![],
        widget_layout: cached_layout,
        focused_widget_id,
        widget_scroll_top,
        widget_scroll_left,
        text_scroll_top: scroll_top,
    }
}

/// Apply view mode filtering to a RenderFrame.
fn apply_view_mode(mut frame: RenderFrame, mode: ViewMode) -> RenderFrame {
    match mode {
        ViewMode::UiOnly => {
            frame.lines = vec![];
            frame.cursor = None;
        }
        ViewMode::TextOnly => {
            frame.widget_layout = None;
            frame.focused_widget_id = None;
        }
        ViewMode::Both => {}
    }
    frame
}
