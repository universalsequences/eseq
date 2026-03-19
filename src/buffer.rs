use std::cmp::Ordering;
use std::path::PathBuf;

use crate::host::BufferId;
use crate::mode::BufferMode;
use crate::text::sexp_range_at_cursor;

pub struct Buffer {
    pub id: BufferId,
    pub name: String,
    pub path: Option<PathBuf>,
    pub mode: BufferMode,
    /// Text contents as lines. Empty buffer starts as vec![""]
    pub lines: Vec<String>,
    /// Cursor position (row, col), 0-indexed. Col can be 0..=line.len().
    pub cursor: (usize, usize),
    pub dirty: bool,
    /// First visible line index (for scrolling).
    pub scroll_top: usize,
    pub revision: u64,
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
            scroll_top: 0,
            revision: 0,
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
        if self.cursor.0 < self.scroll_top {
            self.scroll_top = self.cursor.0;
        }

        if self.cursor.0 >= self.scroll_top + viewport_height {
            self.scroll_top = self.cursor.0 - viewport_height + 1;
        }
    }

    pub fn insert_char(&mut self, c: char) {
        match c {
            '\n' => {
                let new_line = self.lines[self.cursor.0].split_off(self.cursor.1);
                self.lines.insert(self.cursor.0 + 1, new_line);
                self.cursor = (self.cursor.0 + 1, 0);
            }
            '(' => {
                self.lines[self.cursor.0].insert(self.cursor.1, ')');
                self.lines[self.cursor.0].insert(self.cursor.1, '(');
                self.cursor.1 += 1;
            }
            _ => {
                self.lines[self.cursor.0].insert(self.cursor.1, c);
                self.cursor.1 += 1;
            }
        }
        self.touch();
    }

    pub fn insert_newline_with_indent(&mut self) {
        let row = self.cursor.0;
        let col = self.cursor.1;
        let indent = lisp_indent_for_position(&self.lines, row, col);
        let new_line = self.lines[row].split_off(col);
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
            self.lines[self.cursor.0].remove(self.cursor.1 - 1);
            self.cursor.1 -= 1;
            self.touch();
        } else if self.cursor.0 > 0 {
            let line = self.lines.remove(self.cursor.0);
            let prev_len = self.lines[self.cursor.0 - 1].len();
            self.lines[self.cursor.0 - 1].push_str(&line);
            self.cursor = (self.cursor.0 - 1, prev_len);
            self.touch();
        }
    }

    pub fn slice_range(&self, start: (usize, usize), end: (usize, usize)) -> String {
        let ((start_row, start_col), (end_row, end_col)) = normalize_range(start, end);
        if start_row == end_row {
            return self.lines[start_row][start_col..end_col].to_string();
        }

        let mut out = String::new();
        out.push_str(&self.lines[start_row][start_col..]);
        out.push('\n');
        for row in (start_row + 1)..end_row {
            out.push_str(&self.lines[row]);
            out.push('\n');
        }
        out.push_str(&self.lines[end_row][..end_col]);
        out
    }

    pub fn delete_range(&mut self, start: (usize, usize), end: (usize, usize)) {
        let ((start_row, start_col), (end_row, end_col)) = normalize_range(start, end);
        if start_row == end_row {
            self.lines[start_row].drain(start_col..end_col);
        } else {
            let suffix = self.lines[end_row][end_col..].to_string();
            self.lines[start_row].truncate(start_col);
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
        let suffix = self.lines[row].split_off(col);
        let parts = text.split('\n').collect::<Vec<_>>();
        if parts.len() == 1 {
            self.lines[row].push_str(parts[0]);
            self.lines[row].push_str(&suffix);
            self.cursor.1 += parts[0].len();
        } else {
            self.lines[row].push_str(parts[0]);
            for (idx, part) in parts.iter().enumerate().skip(1) {
                let insert_row = row + idx;
                self.lines.insert(insert_row, (*part).to_string());
            }
            let last_row = row + parts.len() - 1;
            self.lines[last_row].push_str(&suffix);
            self.cursor = (last_row, parts.last().map(|part| part.len()).unwrap_or(0));
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
            let prev_len = self.lines[prev_row].len();
            self.lines[prev_row].push_str(&line);
            self.cursor = (prev_row, prev_len);
            self.touch();
            return;
        }

        let original = self.cursor;
        let line = &self.lines[original.0];
        let mut delete_start = original.1;

        while delete_start > 0 {
            let ch = line[..delete_start].chars().next_back().unwrap();
            if !ch.is_whitespace() {
                break;
            }
            delete_start -= ch.len_utf8();
        }

        if delete_start == 0 {
            self.lines[original.0].drain(0..original.1);
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

        self.lines[original.0].drain(delete_start..original.1);
        self.cursor = (original.0, delete_start);
        self.touch();
    }

    pub fn delete_to_line_end(&mut self) {
        let row = self.cursor.0;
        let col = self.cursor.1;
        if col < self.lines[row].len() {
            self.lines[row].truncate(col);
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
            self.cursor.1 = self.lines[self.cursor.0].len();
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor.1 < self.lines[self.cursor.0].len() {
            self.cursor.1 += 1;
        } else if self.cursor.0 < self.lines.len() - 1 {
            self.cursor.0 += 1;
            self.cursor.1 = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor.0 > 0 {
            self.cursor.0 -= 1;
            self.cursor.1 = self.cursor.1.min(self.lines[self.cursor.0].len());
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor.0 < self.lines.len() - 1 {
            self.cursor.0 += 1;
            self.cursor.1 = self.cursor.1.min(self.lines[self.cursor.0].len());
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
            self.cursor.1 = line.len() - 1;
        }
    }

    pub fn move_to_line_start(&mut self) {
        self.cursor.1 = 0;
    }

    pub fn move_to_line_end(&mut self) {
        self.cursor.1 = self.lines[self.cursor.0].len();
    }

    pub fn move_word_left(&mut self) {
        let line = &self.lines[self.cursor.0];
        if self.cursor.1 == 0 {
            if self.cursor.0 > 0 {
                self.cursor.0 -= 1;
                self.cursor.1 = self.lines[self.cursor.0].len();
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
