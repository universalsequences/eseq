use std::cmp::Ordering;
use std::path::PathBuf;

use crate::backend::Color;
use crate::editor::ViewMode;
use crate::host::BufferId;
use crate::mode::BufferMode;
use crate::text::sexp_range_at_cursor;
use crate::vm::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct BufferTextStyle {
    pub line: Option<usize>,
    pub current_line: bool,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub full_line: bool,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
}

pub struct Buffer {
    pub id: BufferId,
    pub name: String,
    pub path: Option<PathBuf>,
    pub mode: BufferMode,
    /// Text contents as lines. Empty buffer starts as vec![""]
    pub lines: Vec<String>,
    /// Cursor position (row, col), 0-indexed. Col is measured in characters.
    pub cursor: (usize, usize),
    pub dirty: bool,
    pub read_only: bool,
    /// First visible line index (for scrolling).
    pub scroll_top: usize,
    pub revision: u64,
    /// Per-buffer widget tree (for modes that render widgets).
    pub widget_tree: Option<Value>,
    pub widget_tree_source: Option<BufferId>,
    pub view_mode: ViewMode,
    pub text_styles: Vec<BufferTextStyle>,
}

impl Buffer {
    pub fn new(id: BufferId, name: impl Into<String>) -> Self {
        Buffer {
            id,
            name: name.into(),
            path: None,
            mode: BufferMode::ESeqLisp,
            lines: vec![String::new()],
            cursor: (0, 0),
            dirty: false,
            read_only: false,
            scroll_top: 0,
            revision: 0,
            widget_tree: None,
            widget_tree_source: None,
            view_mode: ViewMode::Both,
            text_styles: Vec::new(),
        }
    }

    pub fn from_text(id: BufferId, name: impl Into<String>, text: &str) -> Self {
        let mut buffer = Self::new(id, name);
        buffer.set_text(text);
        buffer.dirty = false;
        buffer
    }

    pub fn from_file(id: BufferId, path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        let text = std::fs::read_to_string(&path)?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let mut buffer = Self::from_text(id, name, &text);
        buffer.path = Some(path);
        Ok(buffer)
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines = if text.is_empty() {
            vec![String::new()]
        } else {
            text.lines().map(|line| line.to_string()).collect()
        };
        if text.ends_with('\n') {
            self.lines.push(String::new());
        }
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor = (0, 0);
        self.scroll_top = 0;
        self.revision = self.revision.wrapping_add(1);
        self.text_styles.clear();
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn set_path(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        self.name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        self.path = Some(path);
    }

    pub fn set_mode(&mut self, mode: BufferMode) {
        self.mode = mode;
    }

    pub fn save(&mut self) -> std::io::Result<PathBuf> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| std::io::Error::other("buffer is not file-backed"))?;
        std::fs::write(&path, self.text())?;
        self.dirty = false;
        Ok(path)
    }

    pub fn save_as(&mut self, path: impl Into<PathBuf>) -> std::io::Result<PathBuf> {
        self.set_path(path);
        self.save()
    }

    /// Adjust scroll_top so the cursor stays within the visible viewport.
    ///
    pub fn adjust_scroll(&mut self, viewport_height: usize) {
        if viewport_height == 0 {
            return;
        }
        let max_scroll = self.lines.len().saturating_sub(viewport_height);
        self.scroll_top = self.scroll_top.min(max_scroll);

        if self.cursor.0 < self.scroll_top {
            self.scroll_top = self.cursor.0;
        }

        if self.cursor.0 >= self.scroll_top + viewport_height {
            self.scroll_top = self.cursor.0 - viewport_height + 1;
        }
    }

    pub fn insert_char(&mut self, c: char) {
        let cursor_col = Self::char_to_byte_idx(&self.lines[self.cursor.0], self.cursor.1);
        match c {
            '\n' => {
                let new_line = self.lines[self.cursor.0].split_off(cursor_col);
                self.lines.insert(self.cursor.0 + 1, new_line);
                self.cursor = (self.cursor.0 + 1, 0);
            }
            '(' => {
                self.lines[self.cursor.0].insert(cursor_col, ')');
                self.lines[self.cursor.0].insert(cursor_col, '(');
                self.cursor.1 += 1;
            }
            _ => {
                self.lines[self.cursor.0].insert(cursor_col, c);
                self.cursor.1 += 1;
            }
        }
        self.touch();
    }

    pub fn insert_newline_with_indent(&mut self) {
        let row = self.cursor.0;
        let col = self.cursor.1;
        let indent = lisp_indent_for_position(&self.lines, row, col);
        let split_at = Self::char_to_byte_idx(&self.lines[row], col);
        let new_line = self.lines[row].split_off(split_at);
        let new_line = new_line.trim_start_matches(' ').to_string();
        self.lines
            .insert(row + 1, format!("{}{}", " ".repeat(indent), new_line));
        self.cursor = (row + 1, indent);
        self.reindent_enclosing_sexp();
        self.touch();
    }

    pub fn indent_current_line(&mut self) {
        if self.reindent_enclosing_sexp() {
            return;
        }

        let row = self.cursor.0;
        let desired_indent = lisp_indent_for_position(&self.lines, row, 0);
        let current_line = self.lines[row].clone();
        let current_indent = current_line.chars().take_while(|ch| *ch == ' ').count();
        if current_indent == desired_indent {
            return;
        }

        let trimmed = current_line.trim_start_matches(' ').to_string();
        self.lines[row] = format!("{}{}", " ".repeat(desired_indent), trimmed);
        self.cursor.1 = if self.cursor.1 <= current_indent {
            desired_indent
        } else {
            desired_indent + (self.cursor.1 - current_indent)
        };
        self.touch();
    }

    fn reindent_enclosing_sexp(&mut self) -> bool {
        let Some(((start_row, _), (end_row, _))) = enclosing_sexp_range(&self.lines, self.cursor)
        else {
            return false;
        };

        let mut changed = false;
        let cursor_row = self.cursor.0;
        let cursor_col = self.cursor.1;

        for row in (start_row + 1)..=end_row {
            let current_line = self.lines[row].clone();
            let current_indent = current_line.chars().take_while(|ch| *ch == ' ').count();
            let desired_indent = lisp_indent_for_position(&self.lines, row, 0);
            if current_indent == desired_indent {
                continue;
            }

            let trimmed = current_line.trim_start_matches(' ').to_string();
            self.lines[row] = format!("{}{}", " ".repeat(desired_indent), trimmed);

            if row == cursor_row {
                self.cursor.1 = if cursor_col <= current_indent {
                    desired_indent
                } else {
                    desired_indent + (cursor_col - current_indent)
                };
            }

            changed = true;
        }

        if changed {
            self.touch();
        }
        changed
    }

    pub fn delete_char_before(&mut self) {
        if self.cursor.1 > 0 {
            let remove_at = Self::char_to_byte_idx(&self.lines[self.cursor.0], self.cursor.1 - 1);
            self.lines[self.cursor.0].remove(remove_at);
            self.cursor.1 -= 1;
            self.touch();
        } else if self.cursor.0 > 0 {
            let line = self.lines.remove(self.cursor.0);
            let prev_len = Self::line_len(&self.lines[self.cursor.0 - 1]);
            self.lines[self.cursor.0 - 1].push_str(&line);
            self.cursor = (self.cursor.0 - 1, prev_len);
            self.touch();
        }
    }

    pub fn slice_range(&self, start: (usize, usize), end: (usize, usize)) -> String {
        let ((start_row, start_col), (end_row, end_col)) = normalize_range(start, end);
        let start_idx = Self::char_to_byte_idx(&self.lines[start_row], start_col);
        let end_idx = Self::char_to_byte_idx(&self.lines[end_row], end_col);
        if start_row == end_row {
            return self.lines[start_row][start_idx..end_idx].to_string();
        }

        let mut out = String::new();
        out.push_str(&self.lines[start_row][start_idx..]);
        out.push('\n');
        for row in (start_row + 1)..end_row {
            out.push_str(&self.lines[row]);
            out.push('\n');
        }
        out.push_str(&self.lines[end_row][..end_idx]);
        out
    }

    pub fn delete_range(&mut self, start: (usize, usize), end: (usize, usize)) {
        let ((start_row, start_col), (end_row, end_col)) = normalize_range(start, end);
        let start_idx = Self::char_to_byte_idx(&self.lines[start_row], start_col);
        let end_idx = Self::char_to_byte_idx(&self.lines[end_row], end_col);
        if start_row == end_row {
            self.lines[start_row].drain(start_idx..end_idx);
        } else {
            let suffix = self.lines[end_row][end_idx..].to_string();
            self.lines[start_row].truncate(start_idx);
            self.lines[start_row].push_str(&suffix);
            self.lines.drain((start_row + 1)..=end_row);
        }
        self.cursor = (start_row, start_col);
        self.touch();
    }

    pub fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let row = self.cursor.0;
        let col = self.cursor.1;
        let split_at = Self::char_to_byte_idx(&self.lines[row], col);
        let suffix = self.lines[row].split_off(split_at);
        let parts = text.split('\n').collect::<Vec<_>>();
        if parts.len() == 1 {
            self.lines[row].push_str(parts[0]);
            self.lines[row].push_str(&suffix);
            self.cursor.1 += parts[0].chars().count();
        } else {
            self.lines[row].push_str(parts[0]);
            for (idx, part) in parts.iter().enumerate().skip(1) {
                let insert_row = row + idx;
                self.lines.insert(insert_row, (*part).to_string());
            }
            let last_row = row + parts.len() - 1;
            self.lines[last_row].push_str(&suffix);
            self.cursor = (
                last_row,
                parts.last().map(|part| part.chars().count()).unwrap_or(0),
            );
        }
        self.touch();
    }

    pub fn delete_word_before(&mut self) {
        if self.cursor.1 == 0 {
            if self.cursor.0 == 0 {
                return;
            }
            let row = self.cursor.0;
            let line = self.lines.remove(row);
            let prev_row = row - 1;
            let prev_len = Self::line_len(&self.lines[prev_row]);
            self.lines[prev_row].push_str(&line);
            self.cursor = (prev_row, prev_len);
            self.touch();
            return;
        }

        let original = self.cursor;
        let line = &self.lines[original.0];
        let original_idx = Self::char_to_byte_idx(line, original.1);
        let mut delete_start = original_idx;

        while delete_start > 0 {
            let ch = line[..delete_start].chars().next_back().unwrap();
            if !ch.is_whitespace() {
                break;
            }
            delete_start -= ch.len_utf8();
        }

        if delete_start == 0 {
            self.lines[original.0].drain(0..original_idx);
            self.cursor = (original.0, 0);
            self.touch();
            return;
        }

        let ch = line[..delete_start].chars().next_back().unwrap();
        if is_lisp_delimiter(ch) {
            delete_start -= ch.len_utf8();
        } else {
            while delete_start > 0 {
                let ch = line[..delete_start].chars().next_back().unwrap();
                if ch.is_whitespace() || is_lisp_delimiter(ch) {
                    break;
                }
                delete_start -= ch.len_utf8();
            }
        }

        let new_col = Self::byte_to_char_idx(line, delete_start);
        self.lines[original.0].drain(delete_start..original_idx);
        self.cursor = (original.0, new_col);
        self.touch();
    }

    pub fn delete_to_line_end(&mut self) {
        let row = self.cursor.0;
        let col = self.cursor.1;
        let col_idx = Self::char_to_byte_idx(&self.lines[row], col);
        if col < Self::line_len(&self.lines[row]) {
            self.lines[row].truncate(col_idx);
            self.touch();
        } else if row + 1 < self.lines.len() {
            let next = self.lines.remove(row + 1);
            self.lines[row].push_str(&next);
            self.touch();
        }
    }

    fn touch(&mut self) {
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn move_left(&mut self) {
        if self.cursor.1 > 0 {
            self.cursor.1 -= 1;
        } else if self.cursor.0 > 0 {
            self.cursor.0 -= 1;
            self.cursor.1 = Self::line_len(&self.lines[self.cursor.0]);
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor.1 < Self::line_len(&self.lines[self.cursor.0]) {
            self.cursor.1 += 1;
        } else if self.cursor.0 < self.lines.len() - 1 {
            self.cursor.0 += 1;
            self.cursor.1 = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor.0 > 0 {
            self.cursor.0 -= 1;
            self.cursor.1 = self
                .cursor
                .1
                .min(Self::line_len(&self.lines[self.cursor.0]));
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor.0 < self.lines.len() - 1 {
            self.cursor.0 += 1;
            self.cursor.1 = self
                .cursor
                .1
                .min(Self::line_len(&self.lines[self.cursor.0]));
        }
    }

    pub fn move_to_buffer_end(&mut self) {
        if self.lines.is_empty() {
            return;
        }
        self.cursor.0 = self.lines.len() - 1;
        if let Some(line) = self.lines.last()
            && !line.is_empty()
        {
            self.cursor.1 = Self::line_len(line) - 1;
        }
    }

    pub fn move_to_line_start(&mut self) {
        self.cursor.1 = 0;
    }

    pub fn move_to_line_end(&mut self) {
        self.cursor.1 = Self::line_len(&self.lines[self.cursor.0]);
    }

    pub fn move_word_left(&mut self) {
        let line = &self.lines[self.cursor.0];
        if self.cursor.1 == 0 {
            if self.cursor.0 > 0 {
                self.cursor.0 -= 1;
                self.cursor.1 = Self::line_len(&self.lines[self.cursor.0]);
                self.move_word_left();
            }
            return;
        }

        let chars: Vec<char> = line.chars().collect();
        let mut idx = self.cursor.1.min(chars.len());

        while idx > 0 && chars[idx - 1].is_whitespace() {
            idx -= 1;
        }
        while idx > 0 && !chars[idx - 1].is_whitespace() {
            idx -= 1;
        }

        self.cursor.1 = idx;
    }

    pub fn move_word_right(&mut self) {
        let line = &self.lines[self.cursor.0];
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();
        let mut idx = self.cursor.1.min(len);

        while idx < len && chars[idx].is_whitespace() {
            idx += 1;
        }
        while idx < len && !chars[idx].is_whitespace() {
            idx += 1;
        }

        if idx == len && self.cursor.0 < self.lines.len() - 1 {
            self.cursor.0 += 1;
            self.cursor.1 = 0;
            self.move_word_right();
        } else {
            self.cursor.1 = idx;
        }
    }

    pub fn find_forward(&self, needle: &str, start: (usize, usize)) -> Option<(usize, usize)> {
        if needle.is_empty() {
            return Some(start);
        }
        let haystack = self.text();
        let start_offset = self.position_to_offset(start);
        let found = haystack[start_offset..].find(needle)?;
        Some(self.offset_to_position(start_offset + found))
    }

    pub fn find_backward(&self, needle: &str, start: (usize, usize)) -> Option<(usize, usize)> {
        if needle.is_empty() {
            return Some(start);
        }
        let haystack = self.text();
        let start_offset = self.position_to_offset(start);
        let found = haystack[..start_offset].rfind(needle)?;
        Some(self.offset_to_position(found))
    }

    fn position_to_offset(&self, pos: (usize, usize)) -> usize {
        let row = pos.0.min(self.lines.len().saturating_sub(1));
        let mut offset = 0;
        for line in &self.lines[..row] {
            offset += line.len() + 1;
        }
        let line = &self.lines[row];
        offset + Self::char_to_byte_idx(line, pos.1.min(Self::line_len(line)))
    }

    fn offset_to_position(&self, offset: usize) -> (usize, usize) {
        let mut remaining = offset;
        for (row, line) in self.lines.iter().enumerate() {
            if remaining <= line.len() {
                return (row, Self::byte_to_char_idx(line, remaining));
            }
            remaining = remaining.saturating_sub(line.len() + 1);
        }

        let row = self.lines.len().saturating_sub(1);
        let col = self.lines.get(row).map(|line| Self::line_len(line)).unwrap_or(0);
        (row, col)
    }

    fn line_len(line: &str) -> usize {
        line.chars().count()
    }

    fn char_to_byte_idx(line: &str, col: usize) -> usize {
        if col == 0 {
            return 0;
        }
        line.char_indices()
            .nth(col)
            .map(|(idx, _)| idx)
            .unwrap_or_else(|| line.len())
    }

    fn byte_to_char_idx(line: &str, byte_idx: usize) -> usize {
        line[..byte_idx.min(line.len())].chars().count()
    }
}

fn normalize_range(start: (usize, usize), end: (usize, usize)) -> ((usize, usize), (usize, usize)) {
    match compare_pos(start, end) {
        Ordering::Greater => (end, start),
        _ => (start, end),
    }
}

fn compare_pos(left: (usize, usize), right: (usize, usize)) -> Ordering {
    left.0.cmp(&right.0).then(left.1.cmp(&right.1))
}

#[cfg(test)]
mod tests {
    use super::Buffer;

    #[test]
    fn move_to_line_start_sets_column_to_zero() {
        let mut buffer = Buffer::from_text(0, "*test*", "abcdef");
        buffer.cursor = (0, 4);
        buffer.move_to_line_start();
        assert_eq!(buffer.cursor, (0, 0));
    }

    #[test]
    fn move_to_line_end_sets_column_to_line_length() {
        let mut buffer = Buffer::from_text(0, "*test*", "abcdef");
        buffer.cursor = (0, 1);
        buffer.move_to_line_end();
        assert_eq!(buffer.cursor, (0, 6));
    }

    #[test]
    fn move_word_left_stops_at_previous_word_boundary() {
        let mut buffer = Buffer::from_text(0, "*test*", "abc def ghi");
        buffer.cursor = (0, 10);
        buffer.move_word_left();
        assert_eq!(buffer.cursor, (0, 8));
    }

    #[test]
    fn move_word_right_stops_at_next_word_boundary() {
        let mut buffer = Buffer::from_text(0, "*test*", "abc def ghi");
        buffer.cursor = (0, 0);
        buffer.move_word_right();
        assert_eq!(buffer.cursor, (0, 3));
    }

    #[test]
    fn delete_word_before_removes_previous_word_and_space() {
        let mut buffer = Buffer::from_text(0, "*test*", "abc def ghi");
        buffer.cursor = (0, 8);
        buffer.delete_word_before();
        assert_eq!(buffer.text(), "abc ghi");
        assert_eq!(buffer.cursor, (0, 4));
    }

    #[test]
    fn delete_word_before_joins_previous_line_when_at_line_start() {
        let mut buffer = Buffer::from_text(0, "*test*", "abc def\nghi");
        buffer.cursor = (1, 0);
        buffer.delete_word_before();
        assert_eq!(buffer.text(), "abc defghi");
        assert_eq!(buffer.cursor, (0, 7));
    }

    #[test]
    fn delete_word_before_at_line_start_only_removes_one_newline() {
        let mut buffer = Buffer::from_text(0, "*test*", "alpha\n\nhello");
        buffer.cursor = (2, 0);
        buffer.delete_word_before();
        assert_eq!(buffer.text(), "alpha\nhello");
        assert_eq!(buffer.cursor, (1, 0));
    }

    #[test]
    fn delete_word_at_end_of_buffer() {
        let mut buffer = Buffer::from_text(0, "*test*", "(+ 1 2)\n\n(def");
        buffer.cursor = (2, 4);
        buffer.delete_word_before();
        assert_eq!(buffer.text(), "(+ 1 2)\n\n(");
        assert_eq!(buffer.cursor, (2, 1));
    }

    #[test]
    fn delete_word_before_respects_lisp_symbol_boundaries() {
        let mut buffer = Buffer::from_text(0, "*test*", "(hello");
        buffer.cursor = (0, 6);
        buffer.delete_word_before();
        assert_eq!(buffer.text(), "(");
        assert_eq!(buffer.cursor, (0, 1));
    }

    #[test]
    fn delete_word_before_deletes_single_closing_paren() {
        let mut buffer = Buffer::from_text(0, "*test*", "(+ 1 1)");
        buffer.cursor = (0, 7);
        buffer.delete_word_before();
        assert_eq!(buffer.text(), "(+ 1 1");
        assert_eq!(buffer.cursor, (0, 6));
    }

    #[test]
    fn delete_to_line_end_truncates_current_line() {
        let mut buffer = Buffer::from_text(0, "*test*", "abc def ghi");
        buffer.cursor = (0, 4);
        buffer.delete_to_line_end();
        assert_eq!(buffer.text(), "abc ");
        assert_eq!(buffer.cursor, (0, 4));
    }

    #[test]
    fn delete_to_line_end_at_eol_joins_next_line() {
        let mut buffer = Buffer::from_text(0, "*test*", "abc\ndef");
        buffer.cursor = (0, 3);
        buffer.delete_to_line_end();
        assert_eq!(buffer.text(), "abcdef");
        assert_eq!(buffer.cursor, (0, 3));
    }

    #[test]
    fn insert_newline_with_indent_uses_two_spaces_per_enclosing_form() {
        let mut buffer = Buffer::from_text(0, "*test*", "(if (< (rand-int 8) 4) :4t)");
        buffer.cursor = (0, 22);
        buffer.insert_newline_with_indent();
        assert_eq!(buffer.text(), "(if (< (rand-int 8) 4)\n  :4t)");
        assert_eq!(buffer.cursor, (1, 2));
    }

    #[test]
    fn insert_newline_mid_symbol_uses_enclosing_form_depth() {
        let mut buffer = Buffer::from_text(0, "*test*", "(biquadinput)");
        buffer.cursor = (0, 7);
        buffer.insert_newline_with_indent();
        assert_eq!(buffer.text(), "(biquad\n  input)");
        assert_eq!(buffer.cursor, (1, 2));
    }

    #[test]
    fn indent_current_line_uses_two_spaces_per_enclosing_form() {
        let mut buffer = Buffer::from_text(0, "*test*", "(if test\n:4t\n  :32)");
        buffer.cursor = (1, 0);
        buffer.indent_current_line();
        assert_eq!(buffer.text(), "(if test\n  :4t\n  :32)");
        assert_eq!(buffer.cursor, (1, 2));
    }

    #[test]
    fn indent_current_line_scales_with_nested_forms() {
        let mut buffer = Buffer::from_text(0, "*test*", "(outer\n  (inner\nvalue))");
        buffer.cursor = (2, 0);
        buffer.indent_current_line();
        assert_eq!(buffer.text(), "(outer\n  (inner\n    value))");
        assert_eq!(buffer.cursor, (2, 4));
    }

    #[test]
    fn indent_current_line_reindents_whole_enclosing_sexp() {
        let mut buffer = Buffer::from_text(0, "*test*", "(outer\n(inner\nvalue))");
        buffer.cursor = (1, 0);
        buffer.indent_current_line();
        assert_eq!(buffer.text(), "(outer\n  (inner\n    value))");
        assert_eq!(buffer.cursor, (1, 2));
    }

    #[test]
    fn insert_newline_with_indent_reindents_following_lines_in_enclosing_sexp() {
        let mut buffer = Buffer::from_text(0, "*test*", "(outer (inner value))");
        buffer.cursor = (0, 6);
        buffer.insert_newline_with_indent();
        assert_eq!(buffer.text(), "(outer\n  (inner value))");
        assert_eq!(buffer.cursor, (1, 2));
    }

    #[test]
    fn slice_range_spans_multiple_lines() {
        let buffer = Buffer::from_text(0, "*test*", "abc\ndef\nghi");
        assert_eq!(buffer.slice_range((0, 1), (2, 2)), "bc\ndef\ngh");
    }

    #[test]
    fn delete_range_spans_multiple_lines() {
        let mut buffer = Buffer::from_text(0, "*test*", "abc\ndef\nghi");
        buffer.delete_range((0, 1), (2, 2));
        assert_eq!(buffer.text(), "ai");
        assert_eq!(buffer.cursor, (0, 1));
    }

    #[test]
    fn insert_str_handles_newlines() {
        let mut buffer = Buffer::from_text(0, "*test*", "abef");
        buffer.cursor = (0, 2);
        buffer.insert_str("cd\nxy");
        assert_eq!(buffer.text(), "abcd\nxyef");
        assert_eq!(buffer.cursor, (1, 2));
    }

    #[test]
    fn unicode_columns_do_not_panic_when_slicing_and_deleting() {
        let mut buffer = Buffer::from_text(0, "*test*", "a😀bc\ndéf");
        assert_eq!(buffer.slice_range((0, 1), (1, 2)), "😀bc\ndé");
        buffer.delete_range((0, 1), (1, 2));
        assert_eq!(buffer.text(), "af");
        assert_eq!(buffer.cursor, (0, 1));
    }

    #[test]
    fn unicode_insert_and_backspace_use_character_columns() {
        let mut buffer = Buffer::from_text(0, "*test*", "😀z");
        buffer.cursor = (0, 1);
        buffer.insert_char('x');
        assert_eq!(buffer.text(), "😀xz");
        assert_eq!(buffer.cursor, (0, 2));
        buffer.delete_char_before();
        assert_eq!(buffer.text(), "😀z");
        assert_eq!(buffer.cursor, (0, 1));
    }

    #[test]
    fn adjust_scroll_clamps_stale_scroll_top_after_buffer_shrinks() {
        let mut buffer = Buffer::from_text(0, "*test*", "a\nb\nc\nd\ne\nf");
        buffer.scroll_top = 5;
        buffer.cursor = (0, 0);

        buffer.set_text("a\nb");
        buffer.scroll_top = 5;
        buffer.adjust_scroll(4);

        assert_eq!(buffer.scroll_top, 0);
    }
}

fn is_lisp_delimiter(ch: char) -> bool {
    matches!(
        ch,
        '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | '`' | ','
    )
}

fn lisp_indent_for_position(lines: &[String], row: usize, col: usize) -> usize {
    let mut depth = 0usize;

    for (line_idx, line) in lines.iter().enumerate().take(row + 1) {
        let limit = if line_idx == row {
            col.min(line.len())
        } else {
            line.len()
        };
        let bytes = line.as_bytes();
        let mut idx = 0usize;
        let mut in_string = false;
        let mut escaped = false;

        while idx < limit {
            let ch = bytes[idx] as char;
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                idx += 1;
                continue;
            }

            match ch {
                ';' | '#' => break,
                '"' => in_string = true,
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
            idx += 1;
        }
    }

    depth * 2
}

fn enclosing_sexp_range(
    lines: &[String],
    cursor: (usize, usize),
) -> Option<((usize, usize), (usize, usize))> {
    if let Some(range) = sexp_range_at_cursor(lines, cursor) {
        let line = lines.get(cursor.0)?;
        if cursor.1 < line.len() && line.as_bytes()[cursor.1] as char == '(' {
            let inner_cursor = (cursor.0, cursor.1 + 1);
            return sexp_range_at_cursor(lines, inner_cursor).or(Some(range));
        }
        return Some(range);
    }
    None
}
