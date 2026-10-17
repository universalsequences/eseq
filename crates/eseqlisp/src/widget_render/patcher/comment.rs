//! Comment boxes — Max's `comment`: free text that annotates a patch.
//!
//! A comment is authored as editor text whose first character is `;`, the
//! Lisp comment marker, so typing `; explain the filter here` into any new
//! box makes one. It has no ports, the generator never emits it, and it
//! persists only through the layout sidecar's graph payload.
//!
//! The text word-wraps to the box: an unresized comment grows with its
//! longest line up to [`COMMENT_AUTO_MAX_WIDTH`], a resized one wraps at the
//! width the user dragged, and the height always follows the line count.
//! Everything here is in unzoomed layout cells.

use crate::layout::Rect;

use super::metrics::{NODE_FONT_SIZE, NODE_HEIGHT, NODE_TEXT_COL_OFFSET};
use super::model::PatchNode;
use super::text_metrics::{MeasuredLine, measured_glyph_widths, wrap_text_with_widths};

pub(super) const COMMENT_MARKER: char = ';';
pub(super) const COMMENT_FONT_SIZE: f32 = NODE_FONT_SIZE;
pub(super) const COMMENT_LINE_H: f32 = 1.15;
/// Chosen so a one-line comment is exactly a node's height and its text
/// sits on the same baseline as a node label beside it.
pub(super) const COMMENT_PAD_Y: f32 = (NODE_HEIGHT - COMMENT_LINE_H) * 0.5;
pub(super) const COMMENT_PAD_X: f32 = NODE_TEXT_COL_OFFSET;
/// An unresized comment stops widening here and starts wrapping.
pub(super) const COMMENT_AUTO_MAX_WIDTH: f32 = 30.0;
pub(super) const COMMENT_MIN_WIDTH: f32 = 4.0;

pub(super) fn is_comment_text(text: &str) -> bool {
    text.trim_start().starts_with(COMMENT_MARKER)
}

/// The text a comment shows when it is not being edited: the marker and the
/// space after it dropped.
pub(super) fn comment_body(text: &str) -> &str {
    text.trim_start()
        .trim_start_matches(COMMENT_MARKER)
        .trim_start_matches([' ', '\t'])
}

/// Node text for comment prose: the marker, then the prose.
pub(super) fn comment_node_text(prose: &str) -> String {
    format!("{COMMENT_MARKER} {prose}")
}

/// A comment with nothing after its marker — committing one is the same as
/// committing an empty box.
pub(super) fn is_blank_comment_text(text: &str) -> bool {
    is_comment_text(text) && comment_body(text).trim().is_empty()
}

/// Per-char advances: the measure pass's when it has seen `text`, otherwise
/// the same estimate node labels fall back to.
fn glyph_widths(text: &str) -> Vec<f32> {
    measured_glyph_widths(text, COMMENT_FONT_SIZE).unwrap_or_else(|| {
        #[cfg(target_os = "macos")]
        let advance = super::display::estimated_label_width_cells("x", COMMENT_FONT_SIZE);
        // The terminal renderer places one char per cell.
        #[cfg(not(target_os = "macos"))]
        let advance = 1.0;
        text.chars()
            .map(|ch| if ch == '\n' { 0.0 } else { advance })
            .collect()
    })
}

pub(super) fn wrap_comment_text(text: &str, text_width: f32) -> Vec<MeasuredLine> {
    wrap_text_with_widths(text, &glyph_widths(text), text_width)
}

/// Width of the widest hard line, unwrapped.
fn natural_text_width(text: &str) -> f32 {
    let widths = glyph_widths(text);
    let mut widest = 0.0f32;
    let mut line = 0.0f32;
    for (ch, width) in text.chars().zip(widths) {
        if ch == '\n' {
            line = 0.0;
        } else {
            line += width;
            widest = widest.max(line);
        }
    }
    widest
}

/// The text the box draws: the live edit text while editing (the prose,
/// marker already dropped), the body otherwise — the same string either way.
pub(super) fn comment_drawn_text<'a>(node: &'a PatchNode, edit_text: Option<&'a str>) -> &'a str {
    edit_text.unwrap_or_else(|| comment_body(&node.label))
}

pub(super) fn comment_size(node: &PatchNode) -> (f32, f32) {
    let body = comment_body(&node.label);
    let width = match node.width.filter(|width| width.is_finite()) {
        Some(width) => width.max(COMMENT_MIN_WIDTH),
        None => (natural_text_width(body) + COMMENT_PAD_X * 2.0)
            .clamp(COMMENT_MIN_WIDTH, COMMENT_AUTO_MAX_WIDTH),
    };
    let lines = wrap_comment_text(body, comment_text_width(width)).len().max(1);
    (width, comment_height_for_lines(lines))
}

pub(super) fn comment_height_for_lines(lines: usize) -> f32 {
    lines as f32 * COMMENT_LINE_H + COMMENT_PAD_Y * 2.0
}

pub(super) fn comment_text_width(box_width: f32) -> f32 {
    (box_width - COMMENT_PAD_X * 2.0).max(0.25)
}

/// Where line `index` of a comment drawn in `rect` (zoomed screen cells)
/// starts, as (col, row) of its text.
pub(super) fn comment_line_origin(rect: Rect, index: usize, zoom: f32) -> (f32, f32) {
    (
        rect.col + COMMENT_PAD_X * zoom,
        rect.row + (COMMENT_PAD_Y + index as f32 * COMMENT_LINE_H) * zoom,
    )
}

/// Wrapped lines of `text` as laid out in `rect` at `zoom`.
pub(super) fn comment_lines_in_rect(text: &str, rect: Rect, zoom: f32) -> Vec<MeasuredLine> {
    wrap_comment_text(text, comment_text_width(rect.width / zoom.max(f32::EPSILON)))
}

/// The line holding char index `cursor`, and the caret's x offset into that
/// line (unzoomed cells). A caret on a soft wrap belongs to the line it ends.
pub(super) fn comment_caret_line_offset(
    text: &str,
    lines: &[MeasuredLine],
    cursor: usize,
) -> (usize, f32) {
    let line_index = lines
        .iter()
        .position(|line| cursor >= line.start && cursor <= line.end)
        .unwrap_or_else(|| lines.len().saturating_sub(1));
    let start = lines.get(line_index).map(|line| line.start).unwrap_or(0);
    let offset = glyph_widths(text)
        .iter()
        .skip(start)
        .take(cursor.saturating_sub(start))
        .sum();
    (line_index, offset)
}

/// The char index nearest a pointer at screen (`col`, `row`) over a comment
/// drawn in `rect`.
pub(super) fn comment_cursor_at(text: &str, rect: Rect, col: f32, row: f32, zoom: f32) -> usize {
    let zoom = zoom.max(f32::EPSILON);
    let lines = comment_lines_in_rect(text, rect, zoom);
    let line_index = (((row - rect.row) / zoom - COMMENT_PAD_Y) / COMMENT_LINE_H)
        .floor()
        .clamp(0.0, lines.len().saturating_sub(1) as f32) as usize;
    let Some(line) = lines.get(line_index) else {
        return text.chars().count();
    };
    let target = (col - rect.col) / zoom - COMMENT_PAD_X;
    let widths = glyph_widths(text);
    let mut x = 0.0;
    for index in line.start..line.end {
        let width = widths.get(index).copied().unwrap_or(0.0);
        if target < x + width * 0.5 {
            return index;
        }
        x += width;
    }
    line.end
}
