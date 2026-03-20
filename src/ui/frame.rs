use crate::backend::{Cell, CellStyle, Color, CompletionEntry, CompletionFrame, RenderFrame};
use crate::editor::Editor;
use crate::mode::{TokenClass, TokenSpan};
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
    let cursor = if cursor_row >= scroll_top && cursor_row < scroll_top + viewport_height {
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
        format!(
            " {dirty} {ro}{}  ({mode_name})  L{} C{}   C-q: quit  C-x C-e: eval sexp",
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
    RenderFrame {
        lines,
        cursor,
        buffer_name: buf.name.clone(),
        dirty: buf.dirty,
        status,
        completion,
        text_cache_key,
        widget_layout_cache_key: editor.widget_layout_revision(),
        dirty_widget_ids,
        widget_layout: editor.widget_layout(),
        focused_widget_id: editor.focused_widget_id(),
        widget_scroll_top: editor.widget_scroll_top(),
    }
}
