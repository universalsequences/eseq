use crate::layout::MeasureCtx;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Exact per-glyph advances in layout cells, populated by patcher measure.
    /// Entries are replaced on every measurement so a cell-size change cannot
    /// leave geometry from an older viewport behind.
    static GLYPH_ADVANCES: RefCell<HashMap<(u32, String), Vec<f32>>> =
        RefCell::new(HashMap::new());
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct MeasuredLine {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) text: String,
}

pub(super) fn cache_text_widths(text: String, font_size: f32, ctx: &MeasureCtx<'_>) {
    let Some(measurer) = ctx.text_measurer else {
        return;
    };
    let widths = text
        .chars()
        .map(|ch| measurer.measure_text_px(&ch.to_string(), font_size) / ctx.cell_w.max(1.0))
        .collect();
    GLYPH_ADVANCES.with(|cache| {
        cache
            .borrow_mut()
            .insert((font_size.to_bits(), text), widths);
    });
}

pub(super) fn measured_cursor_offset(
    text: &str,
    font_size: f32,
    cursor_pos: usize,
) -> Option<f32> {
    GLYPH_ADVANCES.with(|cache| {
        cache
            .borrow()
            .get(&(font_size.to_bits(), text.to_string()))
            .map(|widths| widths.iter().take(cursor_pos).sum())
    })
}

pub(super) fn measured_text_width(text: &str, font_size: f32) -> Option<f32> {
    measured_cursor_offset(text, font_size, text.chars().count())
}

pub(super) fn measured_closest_char_index(
    text: &str,
    font_size: f32,
    target_x: f32,
) -> Option<usize> {
    GLYPH_ADVANCES.with(|cache| {
        let cache = cache.borrow();
        let widths = cache.get(&(font_size.to_bits(), text.to_string()))?;
        let mut x = 0.0;
        for (idx, width) in widths.iter().enumerate() {
            if target_x < x + width * 0.5 {
                return Some(idx);
            }
            x += width;
        }
        Some(widths.len())
    })
}

pub(super) fn wrap_measured_text(
    text: &str,
    max_width_cells: f32,
    font_size: f32,
) -> Option<Vec<MeasuredLine>> {
    let widths = GLYPH_ADVANCES.with(|cache| {
        cache
            .borrow()
            .get(&(font_size.to_bits(), text.to_string()))
            .cloned()
    })?;
    let max_width_cells = max_width_cells.max(0.25);
    let chars = text.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return Some(vec![MeasuredLine {
            start: 0,
            end: 0,
            text: String::new(),
        }]);
    }

    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut idx = 0usize;
    let mut width = 0.0f32;
    let mut last_break_after: Option<usize> = None;
    while idx < chars.len() {
        if chars[idx] == '\n' {
            lines.push(MeasuredLine {
                start,
                end: idx,
                text: chars[start..idx].iter().collect(),
            });
            idx += 1;
            start = idx;
            width = 0.0;
            last_break_after = None;
            continue;
        }

        let next_width = width + widths[idx];
        if idx > start && next_width > max_width_cells {
            let break_after = last_break_after
                .filter(|break_after| *break_after > start)
                .unwrap_or(idx);
            lines.push(MeasuredLine {
                start,
                end: break_after,
                text: chars[start..break_after].iter().collect(),
            });
            start = break_after;
            idx = break_after;
            width = 0.0;
            last_break_after = None;
            continue;
        }

        width = next_width;
        idx += 1;
        if chars[idx - 1].is_whitespace() {
            last_break_after = Some(idx);
        }
    }
    lines.push(MeasuredLine {
        start,
        end: chars.len(),
        text: chars[start..].iter().collect(),
    });
    Some(lines)
}
