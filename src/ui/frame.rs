use crate::backend::{
    Cell, CellStyle, Color, CompletionEntry, CompletionFrame, RenderFrame, StatusIndicator,
    TileFrame, TiledRenderFrame,
};
use crate::buffer::Buffer;
use crate::editor::{Editor, ViewMode};
use crate::mode::{TokenClass, TokenSpan, highlight_line};
use crate::text::matching_paren;
use crate::theme;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Maps a token class to its foreground color.
pub fn token_color(class: TokenClass) -> Color {
    match class {
        TokenClass::Comment => theme::SYN_COMMENT(),
        TokenClass::String => theme::SYN_STRING(),
        TokenClass::Number => theme::SYN_NUMBER(),
        TokenClass::Keyword => theme::SYN_KEYWORD(),
        TokenClass::Builtin => theme::SYN_BUILTIN(),
        TokenClass::Special => theme::SYN_SPECIAL(),
        TokenClass::Delimiter => theme::SYN_DELIMITER(),
    }
}

/// Returns the fg Color for the token covering `col`, or the theme foreground.
fn token_fg(col: usize, spans: Option<&Vec<TokenSpan>>) -> Color {
    spans
        .and_then(|ss| ss.iter().find(|s| col >= s.start && col < s.end))
        .map(|s| token_color(s.class))
        .unwrap_or(theme::FG())
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

fn status_cell(ch: char, fg: Color, bg: Option<Color>, bold: bool) -> Cell {
    Cell {
        ch,
        style: CellStyle { fg, bg, bold },
    }
}

fn push_status_text(cells: &mut Vec<Cell>, text: &str, fg: Color, bg: Option<Color>, bold: bool) {
    cells.extend(text.chars().map(|ch| status_cell(ch, fg, bg, bold)));
}

fn push_status_chip(cells: &mut Vec<Cell>, text: &str, fg: Color, bg: Color, bold: bool) {
    push_status_text(cells, text, fg, Some(bg), bold);
}

fn finalize_status_cells(mut cells: Vec<Cell>, width: usize) -> Vec<Cell> {
    if cells.len() > width {
        cells.truncate(width);
    }
    while cells.len() < width {
        cells.push(status_cell(
            ' ',
            theme::STATUS_FG(),
            Some(theme::STATUS_BG()),
            false,
        ));
    }
    cells
}

fn status_space() -> Cell {
    status_cell(' ', theme::STATUS_FG(), Some(theme::STATUS_BG()), false)
}

fn join_status_sections(mut left: Vec<Cell>, right: Vec<Cell>, width: usize) -> Vec<Cell> {
    if width == 0 {
        return Vec::new();
    }

    if left.len() + right.len() >= width {
        let right_budget = right.len().min(width / 3);
        let left_budget = width.saturating_sub(right_budget);
        left.truncate(left_budget);
        let mut cells = left;
        let right_tail = right
            .into_iter()
            .rev()
            .take(width.saturating_sub(cells.len()))
            .collect::<Vec<_>>()
            .into_iter()
            .rev();
        cells.extend(right_tail);
        return finalize_status_cells(cells, width);
    }

    let gap = width - left.len() - right.len();
    left.extend(std::iter::repeat_with(status_space).take(gap));
    left.extend(right);
    finalize_status_cells(left, width)
}

fn build_message_status_row(message: &str, width: usize) -> (Vec<Cell>, StatusIndicator, String) {
    let mut cells = Vec::new();
    push_status_chip(
        &mut cells,
        " :: ",
        theme::STATUS_ACCENT(),
        theme::STATUS_BG(),
        true,
    );
    push_status_text(
        &mut cells,
        message,
        theme::STATUS_FG(),
        Some(theme::STATUS_BG()),
        false,
    );
    (
        finalize_status_cells(cells, width),
        StatusIndicator { toggle_cols: None },
        format!("msg:{message}"),
    )
}

fn build_buffer_status_row(
    buffer: &Buffer,
    ui_available: bool,
    cursor_row: usize,
    cursor_col: usize,
    width: usize,
) -> (Vec<Cell>, StatusIndicator, String) {
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut toggle_cols = None;

    if ui_available {
        let start = left.len();
        let (fg, bg) = match buffer.view_mode {
            ViewMode::UiOnly => (theme::STATUS_UI_FG(), theme::STATUS_UI_BG()),
            ViewMode::Both => (theme::STATUS_MIX_FG(), theme::STATUS_MIX_BG()),
            ViewMode::TextOnly => (theme::STATUS_UI_BG(), theme::STATUS_CHIP_MUTED()),
        };
        push_status_chip(&mut left, " <> ", fg, bg, true);
        toggle_cols = Some((start, left.len()));
        left.push(status_space());
    }

    if buffer.dirty {
        push_status_chip(
            &mut left,
            " + ",
            theme::STATUS_DIRTY_FG(),
            theme::STATUS_DIRTY_BG(),
            true,
        );
        left.push(status_space());
    }

    if buffer.read_only {
        push_status_chip(
            &mut left,
            " ro ",
            theme::STATUS_FG(),
            theme::STATUS_CHIP_BG(),
            false,
        );
        left.push(status_space());
    }

    left.push(status_space());
    push_status_text(
        &mut left,
        &buffer.name,
        theme::STATUS_FG(),
        Some(theme::STATUS_BG()),
        true,
    );

    push_status_chip(
        &mut right,
        &format!(" {} ", buffer.mode.name()),
        theme::STATUS_FG(),
        theme::STATUS_MODE_BG(),
        false,
    );
    right.push(status_space());
    push_status_chip(
        &mut right,
        &format!(" {}:{} ", cursor_row + 1, cursor_col + 1),
        theme::STATUS_FG(),
        theme::STATUS_POS_BG(),
        false,
    );

    let signature = format!(
        "buf:{}:{}:{}:{}:{}:{}:{}",
        buffer.name,
        buffer.mode.name(),
        buffer.dirty,
        buffer.read_only,
        ui_available,
        buffer.view_mode.label(),
        cursor_row * 10000 + cursor_col
    );

    (
        join_status_sections(left, right, width),
        StatusIndicator { toggle_cols },
        signature,
    )
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
    editor.sync_text_horizontal_scroll(viewport_width as u16);
    if editor.active_buffer().view_mode != ViewMode::UiOnly {
        editor.active_buffer_mut().adjust_scroll(viewport_height);
    }

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
            let chars: Vec<char> = content.chars().collect();
            let line_len = chars.len();
            let mut cells: Vec<Cell> = Vec::with_capacity(line_len + 1);
            for col in 0..line_len + 1 {
                let mut fg = token_fg(col, row_spans);
                let mut bg = if in_range(row, col, eval_flash_range, line_len) {
                    Some(theme::BG_EVAL_FLASH())
                } else if in_range(row, col, sexp_range, line_len) {
                    Some(theme::BG_SEXP())
                } else if in_range(row, col, region_range, line_len) {
                    Some(theme::BG_REGION())
                } else {
                    None
                };
                if match_col == Some(col) {
                    bg = Some(theme::BG_MATCH_PAREN());
                    fg = theme::FG_MATCH_PAREN();
                }
                cells.push(Cell {
                    style: CellStyle {
                        bg,
                        fg,
                        bold: false,
                    },
                    ch: chars.get(col).copied().unwrap_or(' '),
                });
            }
            cells
        })
        .collect();

    // Cursor in visible-area coordinates
    let cursor = if !buf.read_only
        && cursor_row >= scroll_top
        && cursor_row < scroll_top + viewport_height
    {
        Some((cursor_row - scroll_top, cursor_col))
    } else {
        None
    };

    // Status bar
    let buf = editor.active_buffer();
    let (status_cells, status_indicator, status_signature) =
        if let Some(prompt) = editor.minibuffer_prompt() {
            build_message_status_row(&format!(" {prompt}"), viewport_width)
        } else if let Some(prompt) = editor.prompt_text() {
            build_message_status_row(&prompt, viewport_width)
        } else if let Some(msg) = &editor.minibuffer {
            build_message_status_row(&format!(" {msg}"), viewport_width)
        } else {
            build_buffer_status_row(
                buf,
                editor.active_buffer_has_ui(),
                cursor_row,
                cursor_col,
                viewport_width,
            )
        };

    let completion = build_completion(editor, cursor_row, cursor_col, scroll_top);

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
        status_signature.hash(&mut hasher);
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
        status_cells,
        status_indicator,
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
    // Ensure cached rects are up to date, then reuse them.
    editor.update_tile_rects(total_width as u16, total_height as u16);
    let tile_rects: Vec<_> = editor.tile_rects().to_vec();
    let active_tile = editor.active_tile;

    let buf = editor.active_buffer();
    let (cursor_row, cursor_col) = buf.cursor;
    let scroll_top = buf.scroll_top;
    let completion = build_completion(editor, cursor_row, cursor_col, scroll_top);

    // Pre-collect per-tile metadata to avoid borrow conflicts
    let tile_info: Vec<_> = tile_rects
        .iter()
        .map(|(tile_id, rect)| {
            let leaf = editor.tile_root.find_leaf(*tile_id).unwrap();
            let buf = &editor.buffers[leaf.buffer_idx];
            let frame_key = (
                buf.revision,
                leaf.layout_revision,
                buf.scroll_top,
                buf.view_mode,
            );
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
            inner_width = (rect.width.round() as usize).saturating_sub(2);
            inner_height = if show_status {
                (rect.height.round() as usize).saturating_sub(3) // border top/bottom + status
            } else {
                (rect.height.round() as usize).saturating_sub(2) // border top/bottom
            };
        } else {
            // Metal: content fills full tile, status bar takes 1 row
            inner_width = rect.width.round() as usize;
            inner_height = if show_status {
                (rect.height.round() as usize).saturating_sub(1) // status bar only
            } else {
                rect.height.round() as usize
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
            let chars: Vec<char> = content.chars().collect();
            let line_len = chars.len();
            let mut cells: Vec<Cell> = Vec::with_capacity(line_len + 1);
            for col in 0..line_len + 1 {
                let fg = token_fg(col, row_spans);
                cells.push(Cell {
                    style: CellStyle {
                        bg: None,
                        fg,
                        bold: false,
                    },
                    ch: chars.get(col).copied().unwrap_or(' '),
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

    let (status_cells, status_indicator, status_signature) = build_buffer_status_row(
        buffer,
        buffer.widget_tree.is_some() || cached_layout.is_some(),
        cursor_row,
        cursor_col,
        viewport_width,
    );

    let text_cache_key = {
        let mut hasher = DefaultHasher::new();
        buffer.id.hash(&mut hasher);
        buffer.revision.hash(&mut hasher);
        viewport_width.hash(&mut hasher);
        viewport_height.hash(&mut hasher);
        scroll_top.hash(&mut hasher);
        status_signature.hash(&mut hasher);
        hasher.finish()
    };

    RenderFrame {
        lines,
        cursor,
        buffer_name: buffer.name.clone(),
        dirty: buffer.dirty,
        status_cells,
        status_indicator,
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

fn build_completion(
    editor: &Editor,
    cursor_row: usize,
    cursor_col: usize,
    scroll_top: usize,
) -> Option<CompletionFrame> {
    editor.completion_state().map(|comp| {
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
    })
}
