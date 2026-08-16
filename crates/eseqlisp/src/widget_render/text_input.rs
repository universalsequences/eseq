use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent,
    get_f32_prop, resolve_named_color, styled_cell,
};
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutCtx, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32,
    get_prop_num, get_prop_str,
};
use crate::vm::Value;

#[cfg(target_os = "macos")]
use super::{
    MetalPrimitive, MetalProportionalTextPrimitive, MetalRectPrimitive, WidgetInstance,
    WidgetViewport, ndc_bounds,
};
#[cfg(target_os = "macos")]
use crate::backend::Color;

// ── Constants ────────────────────────────────────────────────────────────────

const TEXT_PADDING_H: f32 = 0.5;
const TEXT_PADDING_V: f32 = 0.15;
const RING_WIDTH: f32 = 0.15;
const ICON_GAP: f32 = 0.3;
const TEXTBOX_LINE_HEIGHT: f32 = 1.15;
const TEXTBOX_APPROX_CHAR_WIDTH: f32 = 0.62;

// ── Internal state ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TextInputState {
    pub(crate) cursor_pos: usize,
    pub(crate) selection_anchor: Option<usize>,
    pub(crate) selecting: bool,
}

thread_local! {
    static STATES: RefCell<HashMap<u64, TextInputState>> = RefCell::new(HashMap::new());
    /// Per-character cell widths cached during measure().
    static CHAR_WIDTH_CACHE: RefCell<HashMap<(u32, String), Vec<f32>>> = RefCell::new(HashMap::new());
}

fn get_state(widget_id: u64) -> TextInputState {
    STATES.with(|s| s.borrow().get(&widget_id).cloned().unwrap_or_default())
}

pub(crate) fn cache_char_widths(text: String, font_size: f32, ctx: &MeasureCtx<'_>) {
    let Some(measurer) = ctx.text_measurer else {
        return;
    };
    let key = (font_size.to_bits(), text.clone());
    CHAR_WIDTH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.entry(key).or_insert_with(|| {
            text.chars()
                .map(|ch| measurer.measure_text_px(&ch.to_string(), font_size) / ctx.cell_w)
                .collect()
        });
    });
}

pub(crate) fn text_width_from_char_cache(
    text: &str,
    font_size: f32,
    fallback_char_width_cells: f32,
) -> f32 {
    CHAR_WIDTH_CACHE.with(|c| {
        let key = (font_size.to_bits(), text.to_string());
        if let Some(widths) = c.borrow().get(&key) {
            return widths.iter().sum();
        }
        text.chars().count() as f32 * fallback_char_width_cells
    })
}

fn set_state(widget_id: u64, state: TextInputState) {
    let changed = STATES.with(|s| {
        let mut states = s.borrow_mut();
        if states.get(&widget_id) == Some(&state) {
            false
        } else {
            states.insert(widget_id, state);
            true
        }
    });
    if changed {
        super::bump_widget_state_generation();
    }
}

pub(crate) fn selection_range(state: &TextInputState) -> Option<(usize, usize)> {
    let anchor = state.selection_anchor?;
    if anchor == state.cursor_pos {
        None
    } else if anchor < state.cursor_pos {
        Some((anchor, state.cursor_pos))
    } else {
        Some((state.cursor_pos, anchor))
    }
}

fn clear_selection(state: &mut TextInputState) {
    state.selection_anchor = None;
    state.selecting = false;
}

fn replace_range(text: &str, range: (usize, usize), replacement: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut new = String::new();
    new.extend(chars[..range.0].iter());
    new.push_str(replacement);
    new.extend(chars[range.1..].iter());
    new
}

fn selected_text(text: &str, range: (usize, usize)) -> String {
    text.chars()
        .skip(range.0)
        .take(range.1.saturating_sub(range.0))
        .collect()
}

fn insert_text_edit(text: &str, state: &mut TextInputState, insertion: &str) -> String {
    let range = selection_range(state).unwrap_or((state.cursor_pos, state.cursor_pos));
    let new = replace_range(text, range, insertion);
    state.cursor_pos = range.0 + insertion.chars().count();
    clear_selection(state);
    new
}

fn backspace_edit(text: &str, state: &mut TextInputState) -> Option<String> {
    if let Some(range) = selection_range(state) {
        let new = replace_range(text, range, "");
        state.cursor_pos = range.0;
        clear_selection(state);
        return Some(new);
    }
    if state.cursor_pos == 0 {
        return None;
    }
    let range = (state.cursor_pos - 1, state.cursor_pos);
    let new = replace_range(text, range, "");
    state.cursor_pos -= 1;
    clear_selection(state);
    Some(new)
}

fn delete_edit(text: &str, state: &mut TextInputState) -> Option<String> {
    if let Some(range) = selection_range(state) {
        let new = replace_range(text, range, "");
        state.cursor_pos = range.0;
        clear_selection(state);
        return Some(new);
    }
    if state.cursor_pos >= text.chars().count() {
        return None;
    }
    let range = (state.cursor_pos, state.cursor_pos + 1);
    let new = replace_range(text, range, "");
    clear_selection(state);
    Some(new)
}

fn copy_selection_to_clipboard(text: &str, state: &TextInputState) {
    if let Some(range) = selection_range(state) {
        let _ = write_system_clipboard(&selected_text(text, range));
    }
}

pub(crate) enum TextEditOutcome {
    Changed(String),
    StateOnly,
}

pub(crate) fn apply_text_entry_key(
    text: &str,
    state: &mut TextInputState,
    key: WidgetKeyEvent,
    allow_newline: bool,
    vertical_wrap_chars: Option<usize>,
) -> Option<TextEditOutcome> {
    let char_count = text.chars().count();
    state.cursor_pos = state.cursor_pos.min(char_count);

    match key.code {
        KeyCode::Char('a') | KeyCode::Char('A') if clipboard_modifier(key.modifiers) => {
            state.cursor_pos = char_count;
            state.selection_anchor = Some(0);
            state.selecting = false;
            Some(TextEditOutcome::StateOnly)
        }
        KeyCode::Char('c') | KeyCode::Char('C') if clipboard_modifier(key.modifiers) => {
            copy_selection_to_clipboard(text, state);
            Some(TextEditOutcome::StateOnly)
        }
        KeyCode::Char('x') | KeyCode::Char('X') if clipboard_modifier(key.modifiers) => {
            copy_selection_to_clipboard(text, state);
            if let Some(range) = selection_range(state) {
                let new = replace_range(text, range, "");
                state.cursor_pos = range.0;
                clear_selection(state);
                Some(TextEditOutcome::Changed(new))
            } else {
                Some(TextEditOutcome::StateOnly)
            }
        }
        KeyCode::Char('v') | KeyCode::Char('V') if clipboard_modifier(key.modifiers) => {
            match read_system_clipboard() {
                Ok(mut paste) if !paste.is_empty() => {
                    if !allow_newline {
                        paste = paste.replace("\r\n", " ").replace('\n', " ");
                    }
                    let new = insert_text_edit(text, state, &paste);
                    Some(TextEditOutcome::Changed(new))
                }
                _ => Some(TextEditOutcome::StateOnly),
            }
        }
        KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            let new = insert_text_edit(text, state, &c.to_string());
            Some(TextEditOutcome::Changed(new))
        }
        KeyCode::Enter if allow_newline => {
            let new = insert_text_edit(text, state, "\n");
            Some(TextEditOutcome::Changed(new))
        }
        KeyCode::Backspace => {
            if let Some(new) = backspace_edit(text, state) {
                Some(TextEditOutcome::Changed(new))
            } else {
                Some(TextEditOutcome::StateOnly)
            }
        }
        KeyCode::Delete => {
            if let Some(new) = delete_edit(text, state) {
                Some(TextEditOutcome::Changed(new))
            } else {
                Some(TextEditOutcome::StateOnly)
            }
        }
        KeyCode::Left => {
            if state.cursor_pos > 0 {
                state.cursor_pos -= 1;
            }
            clear_selection(state);
            Some(TextEditOutcome::StateOnly)
        }
        KeyCode::Right => {
            if state.cursor_pos < char_count {
                state.cursor_pos += 1;
            }
            clear_selection(state);
            Some(TextEditOutcome::StateOnly)
        }
        KeyCode::Up => {
            if let Some(max_chars) = vertical_wrap_chars {
                state.cursor_pos = move_cursor_vertically(text, state.cursor_pos, max_chars, -1);
                clear_selection(state);
                Some(TextEditOutcome::StateOnly)
            } else {
                None
            }
        }
        KeyCode::Down => {
            if let Some(max_chars) = vertical_wrap_chars {
                state.cursor_pos = move_cursor_vertically(text, state.cursor_pos, max_chars, 1);
                clear_selection(state);
                Some(TextEditOutcome::StateOnly)
            } else {
                None
            }
        }
        KeyCode::Home => {
            state.cursor_pos = 0;
            clear_selection(state);
            Some(TextEditOutcome::StateOnly)
        }
        KeyCode::End => {
            state.cursor_pos = char_count;
            clear_selection(state);
            Some(TextEditOutcome::StateOnly)
        }
        _ => None,
    }
}

fn text_entry_key_event(
    node: &LayoutNode,
    key: WidgetKeyEvent,
    allow_newline: bool,
    vertical_wrap_chars: Option<usize>,
) -> Option<WidgetEvent> {
    let text = get_text(&node.props);
    let mut state = get_state(node.widget_id);
    match apply_text_entry_key(&text, &mut state, key, allow_newline, vertical_wrap_chars)? {
        TextEditOutcome::Changed(new) => {
            set_state(node.widget_id, state);
            Some(WidgetEvent::Custom(Value::String(new)))
        }
        TextEditOutcome::StateOnly => {
            set_state(node.widget_id, state);
            Some(WidgetEvent::Custom(Value::Nil))
        }
    }
}

fn clipboard_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::SUPER) || modifiers.contains(KeyModifiers::CONTROL)
}

fn write_system_clipboard(text: &str) -> Result<(), String> {
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start pbcopy: {error}"))?;
    let stdin = child
        .stdin
        .as_mut()
        .ok_or_else(|| "failed to open pbcopy stdin".to_string())?;
    stdin
        .write_all(text.as_bytes())
        .map_err(|error| format!("failed to write to pbcopy: {error}"))?;
    let status = child
        .wait()
        .map_err(|error| format!("failed to wait for pbcopy: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("pbcopy exited with {status}"))
    }
}

fn read_system_clipboard() -> Result<String, String> {
    let output = Command::new("pbpaste")
        .output()
        .map_err(|error| format!("failed to start pbpaste: {error}"))?;
    if !output.status.success() {
        return Err(format!("pbpaste exited with {}", output.status));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("clipboard did not contain UTF-8 text: {error}"))
}

fn get_text(props: &HashMap<String, Value>) -> String {
    match props.get("value") {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn get_placeholder(props: &HashMap<String, Value>) -> String {
    match props.get("placeholder") {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn max_line_chars(width_cells: f32, font_size: f32, cell_w: f32, padding_h: f32) -> usize {
    let content_width = (width_cells - padding_h * 2.0).max(1.0);
    let char_width_cells = (font_size * TEXTBOX_APPROX_CHAR_WIDTH / cell_w.max(1.0)).max(0.25);
    (content_width / char_width_cells).floor().max(1.0) as usize
}

fn wrap_text_to_lines(text: &str, max_chars: usize) -> Vec<String> {
    if max_chars == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_inclusive(' ') {
            let word_len = word.chars().count();
            let line_len = line.chars().count();
            if line_len > 0 && line_len + word_len > max_chars {
                lines.push(std::mem::take(&mut line));
            }
            if word_len > max_chars {
                for ch in word.chars() {
                    if line.chars().count() >= max_chars {
                        lines.push(std::mem::take(&mut line));
                    }
                    line.push(ch);
                }
            } else {
                line.push_str(word);
            }
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WrappedLine {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) text: String,
}

fn char_widths_for_text(text: &str, font_size: f32, cell_w: f32) -> Vec<f32> {
    CHAR_WIDTH_CACHE.with(|cache| {
        let key = (font_size.to_bits(), text.to_string());
        cache.borrow().get(&key).cloned().unwrap_or_else(|| {
            vec![font_size * TEXTBOX_APPROX_CHAR_WIDTH / cell_w.max(1.0); text.chars().count()]
        })
    })
}

pub(crate) fn wrap_text_to_measured_lines(
    text: &str,
    max_width_cells: f32,
    font_size: f32,
    cell_w: f32,
) -> Vec<WrappedLine> {
    let max_width_cells = max_width_cells.max(0.25);
    let chars = text.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return vec![WrappedLine {
            start: 0,
            end: 0,
            text: String::new(),
        }];
    }
    let widths = char_widths_for_text(text, font_size, cell_w);
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut idx = 0usize;
    let mut width = 0.0f32;
    let mut last_break_after: Option<usize> = None;

    while idx < chars.len() {
        if chars[idx] == '\n' {
            lines.push(WrappedLine {
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

        let next_width = width + widths.get(idx).copied().unwrap_or(0.0);
        if idx > start && next_width > max_width_cells {
            let break_after = last_break_after
                .filter(|break_after| *break_after > start)
                .unwrap_or(idx);
            lines.push(WrappedLine {
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

    lines.push(WrappedLine {
        start,
        end: chars.len(),
        text: chars[start..].iter().collect(),
    });
    lines
}

fn cursor_line_col(text: &str, cursor_pos: usize, max_chars: usize) -> (usize, usize) {
    let before_cursor: String = text.chars().take(cursor_pos).collect();
    let lines = wrap_text_to_lines(&before_cursor, max_chars);
    let row = lines.len().saturating_sub(1);
    let col = lines.last().map(|line| line.chars().count()).unwrap_or(0);
    (row, col.min(max_chars))
}

#[cfg(target_os = "macos")]
fn measured_cursor_line(lines: &[WrappedLine], cursor_pos: usize) -> usize {
    lines
        .iter()
        .position(|line| cursor_pos >= line.start && cursor_pos <= line.end)
        .unwrap_or_else(|| lines.len().saturating_sub(1))
}

fn line_start_char_indices(text: &str, max_chars: usize) -> Vec<usize> {
    let mut starts = vec![0];
    let mut pos = 0usize;
    for paragraph in text.split('\n') {
        let mut line_len = 0usize;
        for word in paragraph.split_inclusive(' ') {
            let word_len = word.chars().count();
            if line_len > 0 && line_len + word_len > max_chars {
                starts.push(pos);
                line_len = 0;
            }
            for _ in word.chars() {
                if line_len >= max_chars {
                    starts.push(pos);
                    line_len = 0;
                }
                line_len += 1;
                pos += 1;
            }
        }
        pos += 1;
        starts.push(pos);
    }
    starts.pop();
    starts
}

fn move_cursor_vertically(text: &str, cursor_pos: usize, max_chars: usize, delta: isize) -> usize {
    let starts = line_start_char_indices(text, max_chars);
    if starts.is_empty() {
        return 0;
    }
    let (row, col) = cursor_line_col(text, cursor_pos, max_chars);
    let target_row = (row as isize + delta).clamp(0, starts.len() as isize - 1) as usize;
    let target_start = starts[target_row];
    let next_start = starts
        .get(target_row + 1)
        .copied()
        .unwrap_or_else(|| text.chars().count());
    (target_start + col).min(next_start)
}

/// Sum per-character widths up to cursor_pos from cache. Falls back to approximation.
#[cfg(target_os = "macos")]
pub(crate) fn cursor_x_from_char_cache(
    text: &str,
    font_size: f32,
    cursor_pos: usize,
    cell_w: f32,
) -> f32 {
    CHAR_WIDTH_CACHE.with(|c| {
        let key = (font_size.to_bits(), text.to_string());
        if let Some(widths) = c.borrow().get(&key) {
            return widths.iter().take(cursor_pos).sum();
        }
        // Fallback: approximate
        cursor_pos as f32 * font_size * 0.55 / cell_w
    })
}

#[cfg(target_os = "macos")]
fn text_range_width_from_char_cache(
    text: &str,
    font_size: f32,
    start: usize,
    end: usize,
    cell_w: f32,
) -> f32 {
    CHAR_WIDTH_CACHE.with(|c| {
        let key = (font_size.to_bits(), text.to_string());
        if let Some(widths) = c.borrow().get(&key) {
            return widths
                .iter()
                .skip(start)
                .take(end.saturating_sub(start))
                .sum();
        }
        end.saturating_sub(start) as f32 * font_size * TEXTBOX_APPROX_CHAR_WIDTH / cell_w
    })
}

#[cfg(target_os = "macos")]
pub(crate) fn closest_char_index_for_x(
    text: &str,
    font_size: f32,
    target_x: f32,
    cell_w: f32,
) -> usize {
    CHAR_WIDTH_CACHE.with(|cache| {
        let key = (font_size.to_bits(), text.to_string());
        let Some(widths) = cache.borrow().get(&key).cloned() else {
            return (target_x / (font_size * 0.55 / cell_w).max(0.25))
                .round()
                .max(0.0) as usize;
        };
        let mut x = 0.0;
        for (idx, width) in widths.iter().enumerate() {
            if target_x < x + width * 0.5 {
                return idx;
            }
            x += width;
        }
        widths.len()
    })
}

#[cfg(target_os = "macos")]
fn closest_char_index_in_range_for_x(
    text: &str,
    font_size: f32,
    start: usize,
    end: usize,
    target_x: f32,
    cell_w: f32,
) -> usize {
    CHAR_WIDTH_CACHE.with(|cache| {
        let key = (font_size.to_bits(), text.to_string());
        let Some(widths) = cache.borrow().get(&key).cloned() else {
            return start
                + (target_x / (font_size * TEXTBOX_APPROX_CHAR_WIDTH / cell_w).max(0.25))
                    .round()
                    .max(0.0) as usize;
        };
        let mut x = 0.0;
        for idx in start..end {
            let width = widths.get(idx).copied().unwrap_or(0.0);
            if target_x < x + width * 0.5 {
                return idx;
            }
            x += width;
        }
        end
    })
}

fn closest_char_index_for_text_input(
    node: &LayoutNode,
    local_col: f32,
    font_size: f32,
    cell_w: f32,
) -> usize {
    let text = get_text(&node.props);
    let icon_width = if !node.children.is_empty() {
        node.children[0].rect.width + ICON_GAP
    } else {
        0.0
    };
    let target_x = local_col - node.rect.col - TEXT_PADDING_H - icon_width;
    #[cfg(target_os = "macos")]
    {
        closest_char_index_for_x(&text, font_size, target_x.max(0.0), cell_w)
            .min(text.chars().count())
    }
    #[cfg(not(target_os = "macos"))]
    {
        (target_x.max(0.0) / 0.6).round() as usize
    }
}

#[cfg(target_os = "macos")]
fn closest_char_index_for_textbox(
    node: &LayoutNode,
    local_col: f32,
    local_row: f32,
    font_size: f32,
    cell_w: f32,
) -> usize {
    let text = get_text(&node.props);
    let content_width = (node.rect.width - TEXT_PADDING_H * 2.0).max(1.0);
    let lines = wrap_text_to_measured_lines(&text, content_width, font_size, cell_w);
    let max_lines = ((node.rect.height - TEXT_PADDING_V * 2.0) / TEXTBOX_LINE_HEIGHT)
        .floor()
        .max(1.0) as usize;
    let state = get_state(node.widget_id);
    let cursor_row = measured_cursor_line(&lines, state.cursor_pos.min(text.chars().count()));
    let first_visible_line = if cursor_row >= max_lines {
        cursor_row + 1 - max_lines
    } else {
        0
    };
    let visible_row = ((local_row - node.rect.row - TEXT_PADDING_V) / TEXTBOX_LINE_HEIGHT)
        .floor()
        .max(0.0) as usize;
    let line_idx = (first_visible_line + visible_row).min(lines.len().saturating_sub(1));
    let Some(line) = lines.get(line_idx) else {
        return 0;
    };
    let target_x = (local_col - node.rect.col - TEXT_PADDING_H).max(0.0);
    closest_char_index_in_range_for_x(&text, font_size, line.start, line.end, target_x, cell_w)
}

// ── Widget definition ───────────────────────────────────────────────────────

pub struct TextInputWidget;
pub struct TextboxWidget;
pub static TEXT_INPUT_WIDGET: TextInputWidget = TextInputWidget;
pub static TEXTBOX_WIDGET: TextboxWidget = TextboxWidget;

impl WidgetDefinition for TextInputWidget {
    fn names(&self) -> &'static [&'static str] {
        &["text-input"]
    }

    fn is_container(&self) -> bool {
        true // first child is positioned as a leading icon
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["value", "placeholder", "width", "height", "font-size"]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "value", "placeholder", "width", "height", "font-size", "text-color",
            "placeholder-color", "bg", "bg-color", "cursor-color", "ring-color", "on-change",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or_else(|| {
                if constraints.max_width < f32::MAX {
                    constraints.max_width
                } else {
                    20.0
                }
            });

        let height = get_prop_num(node, "height").map(f64_to_f32).unwrap_or(1.5);

        // Cache per-character widths for cursor positioning (O(n) measurements)
        let font_size = get_prop_num(node, "font-size")
            .map(f64_to_f32)
            .unwrap_or(ctx.inherited_font_size);
        cache_char_widths(
            get_prop_str(node, "value").unwrap_or_default(),
            font_size,
            ctx,
        );

        Some(Size { width, height })
    }

    fn layout_children(
        &self,
        _node: &Value,
        area: Rect,
        children: &[Value],
        _aspect: f32,
        _layout_ctx: LayoutCtx,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect, LayoutCtx) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        if children.is_empty() {
            return vec![];
        }
        let child = &children[0];
        let child_constraints = Constraints {
            min_width: 0.0,
            max_width: area.height * 2.0,
            min_height: 0.0,
            max_height: area.height,
            aspect: 1.0,
        };
        let child_size = measure_child(child, child_constraints).unwrap_or(Size {
            width: 1.0,
            height: 1.0,
        });
        let child_rect = Rect {
            row: area.row + (area.height - child_size.height) * 0.5,
            col: area.col + TEXT_PADDING_H,
            width: child_size.width,
            height: child_size.height,
        };
        vec![build_child(child, child_rect, LayoutCtx::default())]
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        _local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);
        let cursor_pos = closest_char_index_for_text_input(node, local_col, font_size, 10.0);
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let mut state = get_state(node.widget_id);
                state.cursor_pos = cursor_pos;
                state.selection_anchor = Some(cursor_pos);
                state.selecting = true;
                set_state(node.widget_id, state);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let mut state = get_state(node.widget_id);
                if state.selection_anchor.is_none() {
                    state.selection_anchor = Some(state.cursor_pos);
                }
                state.cursor_pos = cursor_pos;
                state.selecting = true;
                set_state(node.widget_id, state);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let mut state = get_state(node.widget_id);
                state.cursor_pos = cursor_pos;
                state.selecting = false;
                if selection_range(&state).is_none() {
                    state.selection_anchor = None;
                }
                set_state(node.widget_id, state);
            }
            _ => {}
        }
        MouseEventOutcome::Consume
    }

    fn key_event(&self, node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
        text_entry_key_event(node, key, false, None)
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(ref value) = event else {
            return None;
        };
        if matches!(value, Value::Nil) {
            return None;
        }
        let Value::String(new_text) = value else {
            return None;
        };
        let callback = node
            .props
            .get("on-change")
            .filter(|v| !matches!(v, Value::Nil | Value::Bool(false)))
            .cloned()?;
        Some(EventOutput {
            callback,
            args: vec![Value::String(new_text.clone())],
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let text = get_text(props);
        let placeholder = get_placeholder(props);
        let display = if text.is_empty() { &placeholder } else { &text };

        let fg = crate::backend::Color {
            r: if text.is_empty() { 0.4 } else { 0.9 },
            g: if text.is_empty() { 0.4 } else { 0.9 },
            b: if text.is_empty() { 0.45 } else { 0.92 },
            a: 1.0,
        };
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16 + 1;
        let max_col = rect.col.round() as u16 + rect.width.round() as u16;
        for (i, ch) in display.chars().enumerate() {
            let c = col_start + i as u16;
            if c >= max_col {
                break;
            }
            buf.set(row, c, styled_cell(ch, fg, None));
        }
    }

    fn renders_own_focus(&self) -> bool {
        true
    }

    fn captures_drag(&self) -> bool {
        true
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(super::ROUNDED_RECT_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let text = get_text(&node.props);
        let placeholder = get_placeholder(&node.props);
        let state = get_state(node.widget_id);
        let is_focused = viewport.focused_widget_id == Some(node.widget_id);

        let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);

        let bg_transparent = matches!(
            node.props.get("bg"),
            Some(Value::Keyword(k)) if k == "transparent"
        );

        let theme_bg = crate::theme::BG();
        let bg_color = resolve_named_color(
            &node.props,
            "bg-color",
            Color {
                r: (theme_bg.r + 0.09).min(1.0),
                g: (theme_bg.g + 0.10).min(1.0),
                b: (theme_bg.b + 0.11).min(1.0),
                a: 1.0,
            },
        );
        let text_color = resolve_named_color(&node.props, "text-color", crate::theme::FG());
        let placeholder_color =
            resolve_named_color(&node.props, "placeholder-color", crate::theme::FG_MUTED());
        let ring_color = resolve_named_color(&node.props, "ring-color", crate::theme::ACCENT());
        let cursor_color = resolve_named_color(&node.props, "cursor-color", crate::theme::CURSOR());
        let selection_color = Color {
            r: ring_color.r,
            g: ring_color.g,
            b: ring_color.b,
            a: 0.35,
        };

        let mut prims = Vec::new();

        // ── Focus ring (larger blue rounded rect behind background) ──
        if is_focused && !bg_transparent {
            // Cells are typically taller than wide; scale horizontal margin
            // so the ring looks the same thickness on all sides.
            let ring_v = RING_WIDTH;
            let ring_h = RING_WIDTH * viewport.cell_h / viewport.cell_w;
            let ring_rect = Rect {
                row: node.rect.row - ring_v,
                col: node.rect.col - ring_h,
                width: node.rect.width + ring_h * 2.0,
                height: node.rect.height + ring_v * 2.0,
            };
            let (ndc_min, ndc_max) = ndc_bounds(ring_rect, viewport);
            let px_w = ring_rect.width * viewport.cell_w;
            let px_h = ring_rect.height * viewport.cell_h;
            prims.push(MetalPrimitive::WidgetInstance {
                widget_type: "text-input".to_string(),
                instance: WidgetInstance {
                    ndc_min,
                    ndc_max,
                    value_t: 0.0,
                    orientation: 0.0,
                    itime: viewport.time_seconds,
                    uniform_a: [0.0; 4],
                    uniform_b: [0.0; 4],
                    uniform_c: [0.0; 4],
                    uniform_d: [0.0; 4],
                    color_a: [ring_color.r, ring_color.g, ring_color.b, ring_color.a],
                    color_b: [0.0; 4],
                    color_c: [0.0; 4],
                    color_d: [0.0; 4],
                    corner_radius: 0.0,
                    pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                },
                is_background: true,
            });
        }

        // ── Background rounded rect ──
        if !bg_transparent {
            let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
            let px_w = node.rect.width * viewport.cell_w;
            let px_h = node.rect.height * viewport.cell_h;
            prims.push(MetalPrimitive::WidgetInstance {
                widget_type: "text-input".to_string(),
                instance: WidgetInstance {
                    ndc_min,
                    ndc_max,
                    value_t: 0.0,
                    orientation: 0.0,
                    itime: viewport.time_seconds,
                    uniform_a: [0.0; 4],
                    uniform_b: [0.0; 4],
                    uniform_c: [0.0; 4],
                    uniform_d: [0.0; 4],
                    color_a: [bg_color.r, bg_color.g, bg_color.b, bg_color.a],
                    color_b: [0.0; 4],
                    color_c: [0.0; 4],
                    color_d: [0.0; 4],
                    corner_radius: 0.0,
                    pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                },
                is_background: true,
            });
        }

        // ── Text position ──
        let icon_width = if !node.children.is_empty() {
            node.children[0].rect.width + ICON_GAP
        } else {
            0.0
        };
        let text_col = node.rect.col + TEXT_PADDING_H + icon_width;
        let text_row = node.rect.row + (node.rect.height - 1.0) * 0.5;
        let transparent = Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };

        // ── Display text or placeholder ──
        let (display_text, display_color) = if text.is_empty() {
            (placeholder.clone(), placeholder_color)
        } else {
            (text.clone(), text_color)
        };

        if !text.is_empty()
            && let Some((sel_start, sel_end)) = selection_range(&state)
        {
            let x0 = text_col
                + text_range_width_from_char_cache(&text, font_size, 0, sel_start, viewport.cell_w);
            let selection_width = text_range_width_from_char_cache(
                &text,
                font_size,
                sel_start,
                sel_end,
                viewport.cell_w,
            );
            if selection_width > 0.0 {
                prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
                    rect: Rect {
                        row: node.rect.row + TEXT_PADDING_V,
                        col: x0,
                        width: selection_width,
                        height: node.rect.height - TEXT_PADDING_V * 2.0,
                    },
                    color: selection_color,
                }));
            }
        }

        if !display_text.is_empty() {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: text_row,
                    col: text_col,
                    align_width: 0.0,
                    h_align: 0.0,
                    text: display_text,
                    font_size,
                    scale: 1.0,
                    fg: display_color,
                    bg: transparent,
                },
            ));
        }

        // ── Cursor ──
        if is_focused {
            let cursor_pos = state.cursor_pos.min(text.chars().count());
            let cursor_x_offset =
                cursor_x_from_char_cache(&text, font_size, cursor_pos, viewport.cell_w);

            let cursor_col = text_col + cursor_x_offset;
            let cursor_rect = Rect {
                row: node.rect.row + TEXT_PADDING_V + 0.05,
                col: cursor_col,
                width: 0.08,
                height: node.rect.height - TEXT_PADDING_V * 2.0 - 0.1,
            };
            prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: cursor_rect,
                color: cursor_color,
            }));
        }

        prims
    }
}

impl WidgetDefinition for TextboxWidget {
    fn names(&self) -> &'static [&'static str] {
        &["textbox"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &[
            "value",
            "placeholder",
            "width",
            "height",
            "font-size",
            "min-lines",
            "max-lines",
        ]
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &[
            "value", "placeholder", "width", "height", "font-size", "min-lines", "max-lines",
            "text-color", "placeholder-color", "bg", "bg-color", "cursor-color", "ring-color",
            "on-change",
        ]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or_else(|| {
                if constraints.max_width < f32::MAX {
                    constraints.max_width
                } else {
                    32.0
                }
            });
        let font_size = get_prop_num(node, "font-size")
            .map(f64_to_f32)
            .unwrap_or(ctx.inherited_font_size);
        let text = get_prop_str(node, "value")
            .or_else(|| get_prop_str(node, "placeholder"))
            .unwrap_or_default();
        cache_char_widths(text.clone(), font_size, ctx);
        if let Some(height) = get_prop_num(node, "height").map(f64_to_f32) {
            return Some(Size { width, height });
        }

        let content_width = (width - TEXT_PADDING_H * 2.0).max(1.0);
        let line_count =
            wrap_text_to_measured_lines(&text, content_width, font_size, ctx.cell_w).len();
        let min_lines = get_prop_num(node, "min-lines")
            .map(|value| value.max(1.0) as usize)
            .unwrap_or(2);
        let max_lines = get_prop_num(node, "max-lines")
            .map(|value| value.max(1.0) as usize)
            .unwrap_or(6);
        let visible_lines = line_count.clamp(min_lines, max_lines);
        let height = TEXT_PADDING_V * 2.0 + visible_lines as f32 * TEXTBOX_LINE_HEIGHT;
        Some(Size { width, height })
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);
        #[cfg(target_os = "macos")]
        let cursor_pos =
            closest_char_index_for_textbox(node, local_col, local_row, font_size, 10.0)
                .min(get_text(&node.props).chars().count());
        #[cfg(not(target_os = "macos"))]
        let cursor_pos = {
            let text = get_text(&node.props);
            let max_chars = max_line_chars(node.rect.width, font_size, 10.0, TEXT_PADDING_H);
            let row = ((local_row - node.rect.row - TEXT_PADDING_V) / TEXTBOX_LINE_HEIGHT)
                .floor()
                .max(0.0) as usize;
            let col = ((local_col - node.rect.col - TEXT_PADDING_H) / 0.6)
                .round()
                .max(0.0) as usize;
            (row * max_chars + col).min(text.chars().count())
        };
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let mut state = get_state(node.widget_id);
                state.cursor_pos = cursor_pos;
                state.selection_anchor = Some(cursor_pos);
                state.selecting = true;
                set_state(node.widget_id, state);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let mut state = get_state(node.widget_id);
                if state.selection_anchor.is_none() {
                    state.selection_anchor = Some(state.cursor_pos);
                }
                state.cursor_pos = cursor_pos;
                state.selecting = true;
                set_state(node.widget_id, state);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let mut state = get_state(node.widget_id);
                state.cursor_pos = cursor_pos;
                state.selecting = false;
                if selection_range(&state).is_none() {
                    state.selection_anchor = None;
                }
                set_state(node.widget_id, state);
            }
            _ => {}
        }
        MouseEventOutcome::Consume
    }

    fn key_event(&self, node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
        let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);
        let max_chars = max_line_chars(node.rect.width, font_size, 10.0, TEXT_PADDING_H);
        text_entry_key_event(node, key, true, Some(max_chars))
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        TEXT_INPUT_WIDGET.handle_event(node, event)
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let text = get_text(props);
        let placeholder = get_placeholder(props);
        let display = if text.is_empty() { &placeholder } else { &text };
        let font_size = get_f32_prop(props, "font-size", DEFAULT_FONT_SIZE);
        let max_chars = max_line_chars(rect.width, font_size, 10.0, TEXT_PADDING_H);
        let fg = crate::backend::Color {
            r: if text.is_empty() { 0.4 } else { 0.9 },
            g: if text.is_empty() { 0.4 } else { 0.9 },
            b: if text.is_empty() { 0.45 } else { 0.92 },
            a: 1.0,
        };
        let row_start = rect.row.round() as u16;
        let col_start = rect.col.round() as u16 + 1;
        let max_rows = rect.height.floor().max(1.0) as usize;
        for (line_idx, line) in wrap_text_to_lines(display, max_chars)
            .into_iter()
            .take(max_rows)
            .enumerate()
        {
            let row = row_start + line_idx as u16;
            for (i, ch) in line.chars().enumerate() {
                let c = col_start + i as u16;
                if c >= rect.col.round() as u16 + rect.width.round() as u16 {
                    break;
                }
                buf.set(row, c, styled_cell(ch, fg, None));
            }
        }
    }

    fn renders_own_focus(&self) -> bool {
        true
    }

    fn captures_drag(&self) -> bool {
        true
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(super::ROUNDED_RECT_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let text = get_text(&node.props);
        let placeholder = get_placeholder(&node.props);
        let state = get_state(node.widget_id);
        let is_focused = viewport.focused_widget_id == Some(node.widget_id);
        let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);
        let bg_transparent = matches!(
            node.props.get("bg"),
            Some(Value::Keyword(k)) if k == "transparent"
        );

        let theme_bg = crate::theme::BG();
        let bg_color = resolve_named_color(
            &node.props,
            "bg-color",
            Color {
                r: (theme_bg.r + 0.09).min(1.0),
                g: (theme_bg.g + 0.10).min(1.0),
                b: (theme_bg.b + 0.11).min(1.0),
                a: 1.0,
            },
        );
        let text_color = resolve_named_color(&node.props, "text-color", crate::theme::FG());
        let placeholder_color =
            resolve_named_color(&node.props, "placeholder-color", crate::theme::FG_MUTED());
        let ring_color = resolve_named_color(&node.props, "ring-color", crate::theme::ACCENT());
        let cursor_color = resolve_named_color(&node.props, "cursor-color", crate::theme::CURSOR());
        let selection_color = Color {
            r: ring_color.r,
            g: ring_color.g,
            b: ring_color.b,
            a: 0.35,
        };

        let mut prims = Vec::new();
        if is_focused && !bg_transparent {
            let ring_v = RING_WIDTH;
            let ring_h = RING_WIDTH * viewport.cell_h / viewport.cell_w;
            let ring_rect = Rect {
                row: node.rect.row - ring_v,
                col: node.rect.col - ring_h,
                width: node.rect.width + ring_h * 2.0,
                height: node.rect.height + ring_v * 2.0,
            };
            let (ndc_min, ndc_max) = ndc_bounds(ring_rect, viewport);
            let px_w = ring_rect.width * viewport.cell_w;
            let px_h = ring_rect.height * viewport.cell_h;
            prims.push(MetalPrimitive::WidgetInstance {
                widget_type: "textbox".to_string(),
                instance: WidgetInstance {
                    ndc_min,
                    ndc_max,
                    value_t: 0.0,
                    orientation: 0.0,
                    itime: viewport.time_seconds,
                    uniform_a: [0.0; 4],
                    uniform_b: [0.0; 4],
                    uniform_c: [0.0; 4],
                    uniform_d: [0.0; 4],
                    color_a: [ring_color.r, ring_color.g, ring_color.b, ring_color.a],
                    color_b: [0.0; 4],
                    color_c: [0.0; 4],
                    color_d: [0.0; 4],
                    corner_radius: 0.0,
                    pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                },
                is_background: true,
            });
        }

        if !bg_transparent {
            let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
            let px_w = node.rect.width * viewport.cell_w;
            let px_h = node.rect.height * viewport.cell_h;
            prims.push(MetalPrimitive::WidgetInstance {
                widget_type: "textbox".to_string(),
                instance: WidgetInstance {
                    ndc_min,
                    ndc_max,
                    value_t: 0.0,
                    orientation: 0.0,
                    itime: viewport.time_seconds,
                    uniform_a: [0.0; 4],
                    uniform_b: [0.0; 4],
                    uniform_c: [0.0; 4],
                    uniform_d: [0.0; 4],
                    color_a: [bg_color.r, bg_color.g, bg_color.b, bg_color.a],
                    color_b: [0.0; 4],
                    color_c: [0.0; 4],
                    color_d: [0.0; 4],
                    corner_radius: 0.0,
                    pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                },
                is_background: true,
            });
        }

        let content_width = (node.rect.width - TEXT_PADDING_H * 2.0).max(1.0);
        let (display_text, display_color) = if text.is_empty() {
            (placeholder.clone(), placeholder_color)
        } else {
            (text.clone(), text_color)
        };
        let transparent = Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };
        let line_height = TEXTBOX_LINE_HEIGHT;
        let max_lines = ((node.rect.height - TEXT_PADDING_V * 2.0) / line_height)
            .floor()
            .max(1.0) as usize;
        let wrapped_lines =
            wrap_text_to_measured_lines(&display_text, content_width, font_size, viewport.cell_w);
        let cursor_pos = state.cursor_pos.min(text.chars().count());
        let cursor_lines =
            wrap_text_to_measured_lines(&text, content_width, font_size, viewport.cell_w);
        let cursor_row = measured_cursor_line(&cursor_lines, cursor_pos);
        let first_visible_line = if is_focused && cursor_row >= max_lines {
            cursor_row + 1 - max_lines
        } else {
            0
        };
        if !text.is_empty()
            && let Some((sel_start, sel_end)) = selection_range(&state)
        {
            for (line_idx, line) in wrapped_lines
                .iter()
                .skip(first_visible_line)
                .take(max_lines)
                .enumerate()
            {
                let start = sel_start.max(line.start);
                let end = sel_end.min(line.end);
                if start >= end {
                    continue;
                }
                let x0 = node.rect.col
                    + TEXT_PADDING_H
                    + text_range_width_from_char_cache(
                        &text,
                        font_size,
                        line.start,
                        start,
                        viewport.cell_w.max(1.0),
                    );
                let width = text_range_width_from_char_cache(
                    &text,
                    font_size,
                    start,
                    end,
                    viewport.cell_w.max(1.0),
                );
                if width > 0.0 {
                    prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
                        rect: Rect {
                            row: node.rect.row + TEXT_PADDING_V + line_idx as f32 * line_height,
                            col: x0,
                            width,
                            height: line_height,
                        },
                        color: selection_color,
                    }));
                }
            }
        }

        for (line_idx, line) in wrapped_lines
            .iter()
            .skip(first_visible_line)
            .take(max_lines)
            .enumerate()
        {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: node.rect.row + TEXT_PADDING_V + line_idx as f32 * line_height,
                    col: node.rect.col + TEXT_PADDING_H,
                    align_width: 0.0,
                    h_align: 0.0,
                    text: line.text.clone(),
                    font_size,
                    scale: 1.0,
                    fg: display_color,
                    bg: transparent,
                },
            ));
        }

        if is_focused {
            let visible_cursor_row = cursor_row.saturating_sub(first_visible_line);
            let line_start = cursor_lines
                .get(cursor_row)
                .map(|line| line.start)
                .unwrap_or(0);
            let cursor_col = node.rect.col
                + TEXT_PADDING_H
                + text_range_width_from_char_cache(
                    &text,
                    font_size,
                    line_start,
                    cursor_pos,
                    viewport.cell_w.max(1.0),
                );
            let cursor_rect = Rect {
                row: node.rect.row
                    + TEXT_PADDING_V
                    + visible_cursor_row as f32 * line_height
                    + 0.05,
                col: cursor_col,
                width: 0.08,
                height: (line_height - 0.1).max(0.2),
            };
            prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: cursor_rect,
                color: cursor_color,
            }));
        }

        prims
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textbox_wraps_long_words_and_keeps_explicit_breaks() {
        assert_eq!(
            wrap_text_to_lines("abc defghij\nz", 5),
            vec![
                "abc ".to_string(),
                "defgh".to_string(),
                "ij".to_string(),
                "z".to_string()
            ]
        );
    }

    #[test]
    fn textbox_cursor_tracks_wrapped_rows() {
        assert_eq!(cursor_line_col("abc def", 7, 4), (1, 3));
    }

    #[test]
    fn measured_textbox_wraps_at_cached_widths() {
        let text = "wide narrow";
        CHAR_WIDTH_CACHE.with(|cache| {
            cache.borrow_mut().insert(
                (12.0f32.to_bits(), text.to_string()),
                vec![2.0, 2.0, 2.0, 2.0, 0.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
            );
        });
        let lines = wrap_text_to_measured_lines(text, 5.0, 12.0, 10.0);
        assert_eq!(
            lines,
            vec![
                WrappedLine {
                    start: 0,
                    end: 2,
                    text: "wi".to_string()
                },
                WrappedLine {
                    start: 2,
                    end: 5,
                    text: "de ".to_string()
                },
                WrappedLine {
                    start: 5,
                    end: 10,
                    text: "narro".to_string()
                },
                WrappedLine {
                    start: 10,
                    end: 11,
                    text: "w".to_string()
                },
            ]
        );
    }

    #[test]
    fn inserting_text_replaces_selection_and_moves_cursor() {
        let mut state = TextInputState {
            cursor_pos: 5,
            selection_anchor: Some(1),
            selecting: false,
        };
        let new = insert_text_edit("abcdef", &mut state, "X");
        assert_eq!(new, "aXf");
        assert_eq!(state.cursor_pos, 2);
        assert_eq!(selection_range(&state), None);
    }

    #[test]
    fn delete_removes_selection_before_single_character() {
        let mut state = TextInputState {
            cursor_pos: 2,
            selection_anchor: Some(5),
            selecting: false,
        };
        let new = delete_edit("abcdef", &mut state).expect("delete selection");
        assert_eq!(new, "abf");
        assert_eq!(state.cursor_pos, 2);
        assert_eq!(selection_range(&state), None);
    }
}
