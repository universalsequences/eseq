use crate::backend::{
    Cell, CellStyle, Color, CompletionEntry, CompletionFrame, InspectOverlay, RenderFrame,
    StatusIndicator, TileFrame, TiledRenderFrame,
};
use crate::buffer::{Buffer, BufferTextStyle};
use crate::editor::{Editor, ViewMode};
use crate::layout::{Rect, layout_contains_widget_id};
use crate::mode::{TokenClass, TokenSpan, highlight_lines};
use crate::text::matching_paren;
use crate::theme;
use crate::tile::{tile_body_rect, tile_tab_layouts_with_hover};
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

fn contrasting_text_for(bg: Color) -> Color {
    if bg.luma() >= 0.55 {
        theme::BG()
    } else {
        theme::FG()
    }
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
    vim_status: Option<&str>,
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

    if let Some(vim_status) = vim_status {
        push_status_chip(
            &mut left,
            &format!(" {vim_status} "),
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
        "buf:{}:{}:{}:{}:{}:{}:{}:{}",
        buffer.name,
        buffer.mode.name(),
        buffer.dirty,
        buffer.read_only,
        vim_status.unwrap_or("-"),
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
    build_render_frame_with_layout_viewport(
        editor,
        viewport_width,
        viewport_height,
        viewport_width as f32,
        viewport_height as f32,
    )
}

fn build_render_frame_with_layout_viewport(
    editor: &mut Editor,
    viewport_width: usize,
    viewport_height: usize,
    layout_width: f32,
    layout_height: f32,
) -> RenderFrame {
    editor.set_layout_viewport_exact(layout_width, layout_height);
    editor.active_leaf_mut().widget_viewport_height = layout_height;
    editor.clamp_widget_scroll_offsets();
    if let Some(layout) = editor.widget_layout() {
        let aspect = editor.layout_aspect();
        let max_h = (crate::ui::hit::max_extent_exact(&layout, aspect).0 - layout_width).max(0.0);
        let leaf = editor.active_leaf_mut();
        if leaf.widget_scroll_left > max_h {
            leaf.widget_scroll_left = max_h;
        }
    } else {
        editor.active_leaf_mut().widget_scroll_left = 0.0;
    }
    let view_mode = editor.active_buffer().view_mode;
    if view_mode == ViewMode::UiOnly {
        let (cursor_row, cursor_col) = editor.active_buffer().cursor;
        let (status_cells, status_indicator, status_signature) =
            build_active_status_row(editor, cursor_row, cursor_col, viewport_width);

        let text_cache_key = {
            let mut hasher = DefaultHasher::new();
            editor.active_buffer().id.hash(&mut hasher);
            editor.active_buffer().revision.hash(&mut hasher);
            editor.active_buffer().mode.hash(&mut hasher);
            view_mode.hash(&mut hasher);
            viewport_width.hash(&mut hasher);
            viewport_height.hash(&mut hasher);
            status_signature.hash(&mut hasher);
            hasher.finish()
        };

        let dirty_widget_ids = editor.take_dirty_widget_ids();
        let buf = editor.active_buffer();

        return RenderFrame {
            lines: Vec::new(),
            cursor: None,
            buffer_name: buf.name.clone(),
            dirty: buf.dirty,
            status_cells,
            status_indicator,
            completion: None,
            text_cache_key,
            widget_layout_cache_key: editor.widget_layout_revision(),
            widget_content_cache_key: buf.widget_tree_revision,
            dirty_widget_ids,
            widget_layout: editor.widget_layout(),
            focused_widget_id: editor.focused_widget_id(),
            widget_scroll_top: editor.widget_scroll_top(),
            widget_scroll_left: editor.widget_scroll_left(),
            text_scroll_top: 0,
        };
    }

    editor.sync_text_horizontal_scroll(viewport_width as u16);
    if view_mode != ViewMode::UiOnly {
        editor.active_buffer_mut().clamp_scroll(viewport_height);
    }

    let region_range = editor.active_region_range();
    let sexp_range = editor.active_sexp_range();
    let eval_flash_range = editor.active_eval_flash_range();
    let (scroll_top, match_pos, cursor_row, cursor_col) = {
        let buf = editor.active_buffer();
        (
            buf.scroll_top.min(buf.lines.len()),
            matching_paren(&buf.lines, buf.cursor),
            buf.cursor.0,
            buf.cursor.1,
        )
    };
    let highlight_spans = editor.active_highlight_spans_for_visible(scroll_top, viewport_height);
    let buf = editor.active_buffer();
    let visible = scroll_top..(scroll_top + viewport_height).min(buf.lines.len());

    let lines: Vec<Vec<Cell>> = buf.lines[visible]
        .iter()
        .enumerate()
        .map(|(i, content)| {
            let row = scroll_top + i;
            let match_col = match_pos.and_then(|(mr, mc)| if mr == row { Some(mc) } else { None });
            let row_spans = highlight_spans.get(i);
            let row_styles = buf
                .text_styles
                .iter()
                .filter(|style| style_applies_to_row(style, row, cursor_row))
                .collect::<Vec<_>>();
            let chars: Vec<char> = content.chars().collect();
            let line_len = chars.len();
            let render_cols = if row_styles.iter().any(|style| style.full_line) {
                viewport_width.max(line_len + 1)
            } else {
                line_len + 1
            };
            let mut cells: Vec<Cell> = Vec::with_capacity(render_cols);
            for col in 0..render_cols {
                let mut fg = token_fg(col, row_spans);
                let mut bg = None;
                let mut bold = false;
                for style in &row_styles {
                    if style_applies_to_col(style, col) {
                        if let Some(style_fg) = style.fg {
                            fg = style_fg;
                        }
                        if let Some(style_bg) = style.bg {
                            bg = Some(style_bg);
                        }
                        if style.bold {
                            bold = true;
                        }
                    }
                }
                let in_region = in_range(row, col, region_range, line_len);
                bg = if in_range(row, col, eval_flash_range, line_len) {
                    Some(theme::BG_EVAL_FLASH())
                } else if in_range(row, col, sexp_range, line_len) {
                    Some(theme::BG_SEXP())
                } else if in_region {
                    Some(theme::BG_REGION())
                } else {
                    bg
                };
                if match_col == Some(col) {
                    bg = Some(theme::BG_MATCH_PAREN());
                    fg = theme::FG_MATCH_PAREN();
                }
                if in_region {
                    bg = Some(theme::BG_REGION());
                    fg = contrasting_text_for(theme::BG_REGION());
                }
                cells.push(Cell {
                    style: CellStyle { bg, fg, bold },
                    ch: chars.get(col).copied().unwrap_or(' '),
                });
            }
            cells
        })
        .collect();

    // Cursor in visible-area coordinates
    let cursor = if cursor_row >= scroll_top && cursor_row < scroll_top + viewport_height {
        Some((cursor_row - scroll_top, cursor_col))
    } else {
        None
    };

    // Status bar
    let (status_cells, status_indicator, status_signature) =
        build_active_status_row(editor, cursor_row, cursor_col, viewport_width);

    let completion = build_completion(editor, cursor_row, cursor_col, scroll_top);
    if Editor::trace_completion_enabled() {
        eprintln!(
            "{} render_kind=buffer-frame show={} viewport={}x{} scroll_top={} cursor_visible={}",
            editor.completion_debug_summary("render:buffer-frame"),
            completion.is_some(),
            viewport_width,
            viewport_height,
            scroll_top,
            cursor.is_some()
        );
    }

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

    let frame_text_scroll_top = if view_mode == ViewMode::UiOnly {
        0
    } else {
        scroll_top
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
        widget_content_cache_key: buf.widget_tree_revision,
        dirty_widget_ids,
        widget_layout: frame_widget_layout,
        focused_widget_id: frame_focused_id,
        widget_scroll_top: editor.widget_scroll_top(),
        widget_scroll_left: editor.widget_scroll_left(),
        text_scroll_top: frame_text_scroll_top,
    }
}

fn build_active_status_row(
    editor: &Editor,
    cursor_row: usize,
    cursor_col: usize,
    viewport_width: usize,
) -> (Vec<Cell>, StatusIndicator, String) {
    if let Some(prompt) = editor.minibuffer_prompt() {
        build_message_status_row(&format!(" {prompt}"), viewport_width)
    } else if let Some(prompt) = editor.prompt_text() {
        build_message_status_row(&prompt, viewport_width)
    } else if let Some(prompt) = editor.pending_key_prompt() {
        build_message_status_row(&format!(" {prompt}"), viewport_width)
    } else if let Some(msg) = &editor.minibuffer {
        build_message_status_row(&format!(" {msg}"), viewport_width)
    } else {
        build_buffer_status_row(
            editor.active_buffer(),
            editor.active_buffer_has_ui(),
            editor.vim_status_label(),
            cursor_row,
            cursor_col,
            viewport_width,
        )
    }
}

fn style_applies_to_row(style: &BufferTextStyle, row: usize, cursor_row: usize) -> bool {
    (style.current_line && row == cursor_row) || style.line == Some(row)
}

fn style_applies_to_col(style: &BufferTextStyle, col: usize) -> bool {
    if style.full_line {
        return true;
    }
    let start = style.start.unwrap_or(0);
    let end = style.end.unwrap_or(usize::MAX);
    col >= start && col < end
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

fn metal_content_cells(logical_extent: f32) -> usize {
    logical_extent.max(0.0).floor() as usize
}

fn metal_tile_inner_extents(
    rect: Rect,
    show_status: bool,
    show_border: bool,
    border_width_px: f32,
    cell_w: f32,
    cell_h: f32,
) -> (f32, f32) {
    let tile_width_px = rect.width.max(0.0) * cell_w.max(1.0);
    let tile_height_px = rect.height.max(0.0) * cell_h.max(1.0);
    let border_inset_px = if show_border {
        border_width_px
            .max(0.0)
            .min(tile_width_px * 0.5)
            .min(tile_height_px * 0.5)
    } else {
        0.0
    };
    let content_width = rect.width - (border_inset_px * 2.0 / cell_w.max(1.0));
    let content_height = rect.height
        - (border_inset_px * 2.0 / cell_h.max(1.0))
        - if show_status { 1.0 } else { 0.0 };
    (content_width.max(0.0), content_height.max(0.0))
}

fn metal_tile_inner_cells(
    rect: Rect,
    show_status: bool,
    show_border: bool,
    border_width_px: f32,
    cell_w: f32,
    cell_h: f32,
) -> (usize, usize) {
    let (content_width, content_height) = metal_tile_inner_extents(
        rect,
        show_status,
        show_border,
        border_width_px,
        cell_w,
        cell_h,
    );
    (
        metal_content_cells(content_width),
        metal_content_cells(content_height),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        build_tiled_render_frame_borderless, metal_content_cells, metal_tile_inner_cells,
        metal_tile_inner_extents,
    };
    use crate::editor::{Editor, EditorConfig, ViewMode};
    use crate::layout::Rect;
    use crate::runtime::Runtime;

    #[test]
    fn metal_content_cells_never_round_fractional_tiles_up() {
        assert_eq!(metal_content_cells(120.0), 120);
        assert_eq!(metal_content_cells(120.49), 120);
        assert_eq!(metal_content_cells(120.99), 120);
    }

    #[test]
    fn metal_content_cells_clamps_negative_extents() {
        assert_eq!(metal_content_cells(-0.25), 0);
    }

    #[test]
    fn metal_tile_inner_cells_subtracts_pixel_border_from_fill_width() {
        let (cols, rows) = metal_tile_inner_cells(
            Rect {
                col: 0.0,
                row: 0.0,
                width: 100.0,
                height: 10.0,
            },
            false,
            true,
            2.0,
            10.0,
            20.0,
        );

        assert_eq!(cols, 99);
        assert_eq!(rows, 9);
    }

    #[test]
    fn metal_tile_inner_cells_reserves_status_after_border_inset() {
        let (cols, rows) = metal_tile_inner_cells(
            Rect {
                col: 0.0,
                row: 0.0,
                width: 100.0,
                height: 10.0,
            },
            true,
            true,
            2.0,
            10.0,
            20.0,
        );

        assert_eq!(cols, 99);
        assert_eq!(rows, 8);
    }

    #[test]
    fn metal_tile_inner_extents_preserve_fractional_fill_space() {
        let (cols, rows) = metal_tile_inner_extents(
            Rect {
                col: 0.0,
                row: 0.0,
                width: 100.0,
                height: 10.0,
            },
            false,
            true,
            2.0,
            10.0,
            20.0,
        );

        assert_eq!(cols, 99.6);
        assert_eq!(rows, 9.8);
    }

    #[test]
    fn metal_tiled_fill_root_uses_fractional_content_height() {
        let runtime = Runtime::new();
        let mut editor = Editor::new(runtime, EditorConfig::default());
        editor
            .runtime_mut()
            .eval_str(r#"(effect (box :width :fill :height :fill :background-color :black))"#)
            .unwrap();
        editor.refresh_runtime_side_effects();
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor.active_leaf_mut().show_status = false;
        editor.active_leaf_mut().show_border = true;
        editor.active_leaf_mut().border_width_px = 0.25;

        let frame = build_tiled_render_frame_borderless(&mut editor, 20, 9);
        let layout = frame.tiles[0]
            .frame
            .widget_layout
            .as_ref()
            .expect("fill box should produce a widget layout");

        assert_eq!(layout.rect.height, 7.5);
    }
}

fn build_tiled_render_frame_impl(
    editor: &mut Editor,
    total_width: usize,
    total_height: usize,
    cell_borders: bool,
) -> TiledRenderFrame {
    // Ensure cached rects are up to date, then reuse them.
    editor.update_tile_rects(total_width as u16, total_height as u16);
    editor.sync_reactive_bindings_for_visible_layouts();
    let dirty_widget_ids = editor.take_dirty_widget_ids();
    if !dirty_widget_ids.is_empty() {
        let tile_ids = editor.tile_root.leaf_ids();
        for tile_id in tile_ids {
            let matching_dirty_ids = {
                let Some(leaf) = editor.tile_root.find_leaf(tile_id) else {
                    continue;
                };
                let Some(layout) = leaf.cached_layout.as_ref() else {
                    continue;
                };
                dirty_widget_ids
                    .iter()
                    .copied()
                    .filter(|widget_id| layout_contains_widget_id(layout.as_ref(), *widget_id))
                    .collect::<Vec<_>>()
            };
            if matching_dirty_ids.is_empty() {
                continue;
            }
            if let Some(leaf) = editor.tile_root.find_leaf_mut(tile_id) {
                for widget_id in matching_dirty_ids {
                    if !leaf.dirty_widget_ids.contains(&widget_id) {
                        leaf.dirty_widget_ids.push(widget_id);
                    }
                }
                leaf.cached_inactive_frame = None;
            }
        }
    }
    let tile_rects: Vec<_> = editor.tile_rects().to_vec();
    let active_tile = editor.active_tile;
    let (cell_w, cell_h) = editor.layout_cell_dims();

    let buf = editor.active_buffer();
    let (cursor_row, cursor_col) = buf.cursor;
    let scroll_top = buf.scroll_top;
    let text_visible = buf.view_mode != ViewMode::UiOnly;
    let completion = if text_visible {
        build_completion(editor, cursor_row, cursor_col, scroll_top)
    } else {
        None
    };
    if Editor::trace_completion_enabled() {
        let show = completion.is_some();
        eprintln!(
            "{} render_kind=tiled-global show={} cursor_row={} cursor_col={} scroll_top={}",
            editor.completion_debug_summary("render:tiled-global"),
            show,
            cursor_row,
            cursor_col,
            scroll_top
        );
    }

    // Pre-collect per-tile metadata to avoid borrow conflicts
    let tile_info: Vec<_> = tile_rects
        .iter()
        .map(|(tile_id, rect)| {
            let leaf = editor.tile_root.find_leaf(*tile_id).unwrap();
            let buf = &editor.buffers[leaf.buffer_idx];
            let body_rect = tile_body_rect(*rect, !leaf.tabs.is_empty());
            let tabs = tile_tab_layouts_with_hover(
                *rect,
                &leaf.tabs,
                leaf.selected_tab,
                editor.hovered_tab_for_tile(*tile_id),
            );
            let show_status = editor
                .tile_effective_show_status(*tile_id)
                .unwrap_or(leaf.show_status);
            (
                *tile_id,
                *rect,
                body_rect,
                tabs,
                leaf.buffer_idx,
                show_status,
                leaf.show_border,
                leaf.border_width_px,
                leaf.border_radius_px,
                leaf.background_color,
                leaf.background_color_name.clone(),
                leaf.focused_widget_id,
                leaf.widget_scroll_top,
                leaf.widget_scroll_left,
                leaf.layout_revision,
                leaf.cached_layout.clone(),
                leaf.dirty_widget_ids.clone(),
                buf.id,
                buf.revision,
                buf.widget_tree_revision,
                buf.scroll_top,
                buf.view_mode,
                leaf.cached_inactive_frame.clone(),
            )
        })
        .collect();

    // Only compute symbols if we actually need to rebuild an inactive tile
    let mut symbols: Option<Vec<String>> = None;
    let mut tiles = Vec::with_capacity(tile_info.len());

    for (
        tile_id,
        rect,
        body_rect,
        tabs,
        buffer_idx,
        show_status,
        show_border,
        border_width_px,
        border_radius_px,
        background_color,
        background_color_name,
        focused_widget_id,
        widget_scroll_top,
        widget_scroll_left,
        layout_revision,
        cached_layout,
        dirty_widget_ids,
        buffer_id,
        buffer_revision,
        widget_tree_revision,
        buffer_scroll_top,
        view_mode,
        cached_frame,
    ) in tile_info
    {
        let is_active = tile_id == active_tile;
        let inspect_status_message = editor
            .tile_inspect_status_message(tile_id)
            .map(str::to_string);
        let inspect_overlay =
            editor
                .tile_inspect_overlay_rect(tile_id)
                .map(|rect| InspectOverlay {
                    rect,
                    fill: theme::INSPECT_OVERLAY_FILL(),
                    border: theme::INSPECT_OVERLAY_BORDER(),
                });

        // Compute inner dimensions
        let inner_width;
        let inner_height;
        let inner_width_exact;
        let inner_height_exact;
        if cell_borders {
            // TUI: subtract 1-cell borders on each side
            inner_width = (body_rect.width.round() as usize).saturating_sub(2);
            inner_height = if show_status {
                (body_rect.height.round() as usize).saturating_sub(3) // border top/bottom + status
            } else {
                (body_rect.height.round() as usize).saturating_sub(2) // border top/bottom
            };
            inner_width_exact = inner_width as f32;
            inner_height_exact = inner_height as f32;
        } else {
            // Metal tile rects can be fractional because margins/splits are
            // stored in logical cell units, and the Metal backend clips content
            // inside pixel borders. Lay widgets out to the same inset content
            // rect so :width :fill children do not clip their right edge.
            let (cols_exact, rows_exact) = metal_tile_inner_extents(
                body_rect,
                show_status,
                show_border,
                border_width_px,
                cell_w,
                cell_h,
            );
            let (cols, rows) = metal_tile_inner_cells(
                body_rect,
                show_status,
                show_border,
                border_width_px,
                cell_w,
                cell_h,
            );
            inner_width = cols;
            inner_height = rows;
            inner_width_exact = cols_exact;
            inner_height_exact = rows_exact;
        }

        if let Some(leaf) = editor.tile_root.find_leaf_mut(tile_id) {
            leaf.widget_viewport_height = inner_height_exact;
        }

        let frame_key = (
            buffer_id,
            buffer_revision,
            widget_tree_revision,
            layout_revision,
            buffer_scroll_top,
            inner_width,
            inner_height,
            inner_width_exact.to_bits(),
            inner_height_exact.to_bits(),
            view_mode,
        );
        let cached = cached_frame
            .as_ref()
            .filter(|_| inspect_status_message.is_none())
            .filter(|_| dirty_widget_ids.is_empty())
            .filter(|(key, _)| *key == frame_key)
            .map(|(_, frame)| frame.clone());

        if is_active {
            // Active tile: use full build_render_frame for highlights/cursor, but
            // completion is rendered once globally by the tiled backend.
            let mut frame = build_render_frame_with_layout_viewport(
                editor,
                inner_width,
                inner_height,
                inner_width_exact,
                inner_height_exact,
            );
            if let Some(message) = inspect_status_message.as_deref() {
                let (status_cells, status_indicator, _) =
                    build_message_status_row(&format!(" {message}"), inner_width);
                frame.status_cells = status_cells;
                frame.status_indicator = status_indicator;
            }
            frame.completion = None;
            frame.dirty_widget_ids = dirty_widget_ids.clone();
            if Editor::trace_completion_enabled() {
                eprintln!(
                    "{} render_kind=active-tile frame_show={} tile_id={} inner={}x{}",
                    editor.completion_debug_summary("render:active-tile"),
                    frame.completion.is_some(),
                    tile_id,
                    inner_width,
                    inner_height
                );
            }
            if let Some(leaf) = editor.tile_root.find_leaf_mut(tile_id) {
                leaf.dirty_widget_ids.clear();
            }
            tiles.push(TileFrame {
                tile_id,
                rect,
                body_rect,
                tabs,
                is_active: true,
                show_status,
                show_border,
                border_width_px,
                border_radius_px,
                background_color,
                background_color_name,
                inspect_overlay,
                frame,
            });
        } else {
            // Inactive tile: reuse cached frame if content is unchanged
            let mut frame = if let Some(frame) = cached {
                frame
            } else {
                let syms = if view_mode == ViewMode::UiOnly {
                    None
                } else {
                    Some(
                        symbols
                            .get_or_insert_with(|| editor.runtime_mut().completion_symbols())
                            .as_slice(),
                    )
                };
                let frame = build_inactive_tile_frame_from_parts(
                    &editor.buffers[buffer_idx],
                    focused_widget_id,
                    widget_scroll_top,
                    widget_scroll_left,
                    layout_revision,
                    cached_layout,
                    dirty_widget_ids.clone(),
                    syms,
                    inner_width,
                    inner_height,
                );
                // Cache for next frame
                if let Some(leaf) = editor.tile_root.find_leaf_mut(tile_id) {
                    leaf.dirty_widget_ids.clear();
                    if inspect_status_message.is_none() {
                        let mut clean_frame = frame.clone();
                        clean_frame.dirty_widget_ids.clear();
                        leaf.cached_inactive_frame = Some((frame_key, clean_frame));
                    }
                }
                frame
            };
            if let Some(message) = inspect_status_message.as_deref() {
                let (status_cells, status_indicator, _) =
                    build_message_status_row(&format!(" {message}"), inner_width);
                frame.status_cells = status_cells;
                frame.status_indicator = status_indicator;
            }
            if let Some(leaf) = editor.tile_root.find_leaf_mut(tile_id) {
                leaf.dirty_widget_ids.clear();
            }
            let frame = apply_view_mode(frame, editor.buffers[buffer_idx].view_mode);
            tiles.push(TileFrame {
                tile_id,
                rect,
                body_rect,
                tabs,
                is_active: false,
                show_status,
                show_border,
                border_width_px,
                border_radius_px,
                background_color,
                background_color_name,
                inspect_overlay,
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
    widget_scroll_top: f32,
    widget_scroll_left: f32,
    layout_revision: u64,
    cached_layout: Option<std::sync::Arc<crate::layout::LayoutNode>>,
    dirty_widget_ids: Vec<u64>,
    symbols: Option<&[String]>,
    viewport_width: usize,
    viewport_height: usize,
) -> RenderFrame {
    let scroll_top = buffer.scroll_top.min(buffer.lines.len());
    let (cursor_row, cursor_col) = buffer.cursor;

    let (status_cells, status_indicator, status_signature) = build_buffer_status_row(
        buffer,
        buffer.widget_tree.is_some() || cached_layout.is_some(),
        None,
        cursor_row,
        cursor_col,
        viewport_width,
    );

    let text_cache_key = {
        let mut hasher = DefaultHasher::new();
        buffer.id.hash(&mut hasher);
        buffer.revision.hash(&mut hasher);
        buffer.mode.hash(&mut hasher);
        buffer.view_mode.hash(&mut hasher);
        viewport_width.hash(&mut hasher);
        viewport_height.hash(&mut hasher);
        scroll_top.hash(&mut hasher);
        status_signature.hash(&mut hasher);
        hasher.finish()
    };

    if buffer.view_mode == ViewMode::UiOnly {
        return RenderFrame {
            lines: Vec::new(),
            cursor: None,
            buffer_name: buffer.name.clone(),
            dirty: buffer.dirty,
            status_cells,
            status_indicator,
            completion: None,
            text_cache_key,
            widget_layout_cache_key: layout_revision,
            widget_content_cache_key: buffer.widget_tree_revision,
            dirty_widget_ids,
            widget_layout: cached_layout,
            focused_widget_id,
            widget_scroll_top,
            widget_scroll_left,
            text_scroll_top: 0,
        };
    }

    let visible = scroll_top..(scroll_top + viewport_height).min(buffer.lines.len());
    let symbols = symbols.expect("inactive text frame requires completion symbols");

    // Only highlight visible lines — not the entire buffer.
    let visible_lines = &buffer.lines[visible];
    let highlight_spans = highlight_lines(&buffer.mode, visible_lines.iter(), symbols, buffer);

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
        widget_content_cache_key: buffer.widget_tree_revision,
        dirty_widget_ids,
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
            frame.text_scroll_top = 0;
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
    let Some(comp) = editor.completion_state() else {
        if Editor::trace_completion_enabled() {
            eprintln!(
                "{} build_completion=none reason=no-editor-completion-state anchor=({}, {}) scroll_top={}",
                editor.completion_debug_summary("render:build-completion"),
                cursor_row.saturating_sub(scroll_top),
                cursor_col,
                scroll_top
            );
        }
        return None;
    };
    let frame = {
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
    };
    if Editor::trace_completion_enabled() {
        eprintln!(
            "{} build_completion=some entries={} anchor=({}, {}) selected_doc={}",
            editor.completion_debug_summary("render:build-completion"),
            frame.entries.len(),
            frame.anchor.0,
            frame.anchor.1,
            frame.doc.is_some()
        );
    }
    Some(frame)
}
