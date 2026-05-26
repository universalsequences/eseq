use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::backend::Color;
use crate::editor::ViewMode;
use crate::host::BufferId;
use crate::mode::BufferMode;
use crate::text::sexp_range_at_cursor;
use crate::vm::{ReactiveFieldKey, Value};

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

#[derive(Debug, Clone, PartialEq)]
pub struct CommittedSubtreeSnapshot {
    pub stable_widget_id: Option<u64>,
    pub subtree_root_id: Option<u64>,
    pub parent_subtree_root_id: Option<u64>,
    pub stable_key: Option<String>,
    pub widget_type: Option<String>,
    pub reactive_dependencies: Vec<ReactiveFieldKey>,
    pub tree: Value,
    pub children: Vec<Arc<CommittedSubtreeSnapshot>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommittedBufferUiSnapshot {
    pub source_buffer_id: Option<BufferId>,
    pub tree: Value,
    pub root_stable_widget_id: Option<u64>,
    pub root_subtree_root_id: Option<u64>,
    pub field_to_subtree_roots: HashMap<ReactiveFieldKey, Vec<u64>>,
    pub subtree_root_dependencies: HashMap<u64, Vec<ReactiveFieldKey>>,
    pub subtree_roots: HashMap<u64, Arc<CommittedSubtreeSnapshot>>,
    pub widgets: HashMap<u64, Arc<CommittedSubtreeSnapshot>>,
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
    pub widget_tree_revision: u64,
    pub committed_ui_snapshot: Option<CommittedBufferUiSnapshot>,
    pub committed_ui_revision: u64,
    pub committed_ui_runtime_generation: Option<u64>,
    pub view_mode: ViewMode,
    pub text_styles: Vec<BufferTextStyle>,
}

pub fn debug_widget_tree_summary(tree: Option<&Value>) -> String {
    fn value_label(value: &Value, depth: usize) -> String {
        if depth > 1 {
            return "...".to_string();
        }
        match value {
            Value::Map(map) => {
                let widget_type = map
                    .get("type")
                    .and_then(|value| match value.borrow().clone() {
                        Value::String(s) => Some(s),
                        _ => None,
                    })
                    .unwrap_or_else(|| "map".to_string());
                let text = map
                    .get("text")
                    .and_then(|value| match value.borrow().clone() {
                        Value::String(s) => Some(s),
                        _ => None,
                    });
                let child_labels = map
                    .get("children")
                    .and_then(|value| match value.borrow().clone() {
                        Value::List(children) => Some(
                            children
                                .iter()
                                .take(3)
                                .map(|child| value_label(&child.borrow().clone(), depth + 1))
                                .collect::<Vec<_>>(),
                        ),
                        _ => None,
                    })
                    .unwrap_or_default();
                let child_count = map
                    .get("children")
                    .and_then(|value| match value.borrow().clone() {
                        Value::List(children) => Some(children.len()),
                        _ => None,
                    })
                    .unwrap_or(0);
                let text_part = text
                    .map(|text| format!(" text={text:?}"))
                    .unwrap_or_default();
                if child_count > 0 {
                    format!(
                        "{widget_type}{text_part} children={child_count}[{}]",
                        child_labels.join(", ")
                    )
                } else {
                    format!("{widget_type}{text_part}")
                }
            }
            Value::List(items) => format!("list(len={})", items.len()),
            Value::String(s) => format!("string({s:?})"),
            Value::Number(n) => format!("number({n})"),
            Value::Bool(b) => format!("bool({b})"),
            Value::Nil => "nil".to_string(),
            other => format!("{other:?}"),
        }
    }

    tree.map(|value| value_label(value, 0))
        .unwrap_or_else(|| "<none>".to_string())
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
            widget_tree_revision: 0,
            committed_ui_snapshot: None,
            committed_ui_revision: 0,
            committed_ui_runtime_generation: None,
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

    pub fn set_widget_tree(&mut self, tree: Option<Value>, source: Option<BufferId>) {
        let tree_unchanged = self.widget_tree.as_ref() == tree.as_ref();
        let source_unchanged = self.widget_tree_source == source;
        if tree_unchanged && source_unchanged {
            self.set_committed_ui_snapshot(
                tree.map(|tree| CommittedBufferUiSnapshot::from_tree(tree, source, Vec::new())),
            );
            return;
        }
        self.widget_tree = tree.as_ref().map(Value::deep_clone);
        self.widget_tree_source = source;
        self.widget_tree_revision = self.widget_tree_revision.wrapping_add(1);
        self.set_committed_ui_snapshot(
            tree.map(|tree| CommittedBufferUiSnapshot::from_tree(tree, source, Vec::new())),
        );
    }

    pub fn adopt_committed_ui_snapshot(&mut self, snapshot: CommittedBufferUiSnapshot) {
        let tree_unchanged = self.widget_tree.as_ref() == Some(&snapshot.tree);
        let source_unchanged = self.widget_tree_source == snapshot.source_buffer_id;
        if !tree_unchanged || !source_unchanged {
            self.widget_tree = Some(snapshot.tree.clone());
            self.widget_tree_source = snapshot.source_buffer_id;
            self.widget_tree_revision = self.widget_tree_revision.wrapping_add(1);
        }
        self.set_committed_ui_snapshot(Some(snapshot));
    }

    pub fn adopt_runtime_committed_ui_snapshot(
        &mut self,
        snapshot: CommittedBufferUiSnapshot,
        runtime_generation: u64,
    ) {
        self.widget_tree = Some(snapshot.tree.clone());
        self.widget_tree_source = snapshot.source_buffer_id;
        self.widget_tree_revision = self.widget_tree_revision.wrapping_add(1);
        self.committed_ui_snapshot = Some(snapshot);
        self.committed_ui_revision = self.committed_ui_revision.wrapping_add(1);
        self.committed_ui_runtime_generation = Some(runtime_generation);
    }

    pub fn set_committed_ui_snapshot(&mut self, snapshot: Option<CommittedBufferUiSnapshot>) {
        let unchanged = self
            .committed_ui_snapshot
            .as_ref()
            .zip(snapshot.as_ref())
            .is_some_and(|(current, next)| current == next)
            || (self.committed_ui_snapshot.is_none() && snapshot.is_none());
        if unchanged {
            return;
        }
        self.committed_ui_snapshot = snapshot;
        self.committed_ui_revision = self.committed_ui_revision.wrapping_add(1);
        self.committed_ui_runtime_generation = None;
    }

    pub fn clear_committed_ui_snapshot(&mut self) {
        self.set_committed_ui_snapshot(None);
    }

    pub fn replace_widget_subtree(
        &mut self,
        subtree_root_id: u64,
        tree: Value,
        source: Option<BufferId>,
        reactive_dependencies: Vec<ReactiveFieldKey>,
    ) -> bool {
        let Some(snapshot) = self.committed_ui_snapshot.take() else {
            return false;
        };
        if let Some(reason) = snapshot.subtree_replace_failure_reason(subtree_root_id, &tree) {
            let _ = reason;
            self.committed_ui_snapshot = Some(snapshot);
            return false;
        }
        let Some(merged) = snapshot.replacing_subtree(subtree_root_id, tree, reactive_dependencies)
        else {
            return false;
        };
        self.widget_tree = Some(merged.tree.clone());
        self.widget_tree_source = source.or(merged.source_buffer_id);
        self.widget_tree_revision = self.widget_tree_revision.wrapping_add(1);
        self.set_committed_ui_snapshot(Some(merged));
        true
    }

    pub fn replace_widget_subtrees(
        &mut self,
        replacements: &[(u64, Value, Vec<ReactiveFieldKey>)],
        source: Option<BufferId>,
    ) -> bool {
        let Some(snapshot) = self.committed_ui_snapshot.take() else {
            return false;
        };
        let Some(merged) = snapshot.clone().replacing_subtrees(replacements) else {
            self.committed_ui_snapshot = Some(snapshot);
            return false;
        };
        self.widget_tree = Some(merged.tree.clone());
        self.widget_tree_source = source.or(merged.source_buffer_id);
        self.widget_tree_revision = self.widget_tree_revision.wrapping_add(1);
        self.set_committed_ui_snapshot(Some(merged));
        true
    }

    pub fn replace_committed_subtree(&mut self, subtree: CommittedSubtreeSnapshot) {
        let Some(root_id) = subtree.subtree_root_id else {
            return;
        };
        let replaced = self.replace_widget_subtree(
            root_id,
            subtree.tree,
            self.widget_tree_source,
            subtree.reactive_dependencies,
        );
        if !replaced {
            return;
        }
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

    /// Clamp scroll_top to the valid range for the current buffer size.
    pub fn clamp_scroll(&mut self, viewport_height: usize) {
        if viewport_height == 0 {
            return;
        }
        let max_scroll = self.lines.len().saturating_sub(viewport_height);
        self.scroll_top = self.scroll_top.min(max_scroll);
    }

    /// Adjust scroll_top so the cursor stays within the visible viewport.
    pub fn adjust_scroll(&mut self, viewport_height: usize) {
        if viewport_height == 0 {
            return;
        }
        self.clamp_scroll(viewport_height);

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
        let col = self
            .lines
            .get(row)
            .map(|line| Self::line_len(line))
            .unwrap_or(0);
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

impl CommittedBufferUiSnapshot {
    pub fn from_tree(
        tree: Value,
        source_buffer_id: Option<BufferId>,
        reactive_dependencies: Vec<ReactiveFieldKey>,
    ) -> Self {
        let root = CommittedSubtreeSnapshot::from_tree(tree.clone(), &reactive_dependencies);
        let mut subtree_roots = HashMap::new();
        let mut widgets = HashMap::new();
        let mut field_to_subtree_roots = HashMap::new();
        let mut subtree_root_dependencies = HashMap::new();
        root.collect_indexes(
            &mut subtree_roots,
            &mut widgets,
            &mut field_to_subtree_roots,
            &mut subtree_root_dependencies,
        );
        Self {
            source_buffer_id,
            tree,
            root_stable_widget_id: root.stable_widget_id,
            root_subtree_root_id: root.subtree_root_id,
            field_to_subtree_roots,
            subtree_root_dependencies,
            subtree_roots,
            widgets,
        }
    }

    pub fn subtree_roots_for_field(&self, field: &ReactiveFieldKey) -> Vec<u64> {
        self.field_to_subtree_roots
            .get(field)
            .cloned()
            .unwrap_or_default()
    }

    pub fn subtree_replace_failure_reason(
        &self,
        subtree_root_id: u64,
        replacement_tree: &Value,
    ) -> Option<&'static str> {
        if !self.subtree_roots.contains_key(&subtree_root_id) {
            return Some("unknown-root");
        }
        let Some(replacement_root_id) = root_subtree_root_id(replacement_tree) else {
            return Some("missing-root-id");
        };
        if replacement_root_id != subtree_root_id {
            return Some("root-id-mismatch");
        }
        None
    }

    pub fn dependencies_for_subtree_root(&self, root_id: u64) -> Vec<ReactiveFieldKey> {
        self.subtree_root_dependencies
            .get(&root_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn matching_non_root_subtree_root_id_for_tree(&self, tree: &Value) -> Option<u64> {
        let subtree_root_id = root_subtree_root_id(tree)?;
        (Some(subtree_root_id) != self.root_subtree_root_id
            && self.subtree_roots.contains_key(&subtree_root_id))
        .then_some(subtree_root_id)
    }

    pub fn replacing_subtree(
        self,
        subtree_root_id: u64,
        replacement_tree: Value,
        reactive_dependencies: Vec<ReactiveFieldKey>,
    ) -> Option<Self> {
        if self
            .subtree_replace_failure_reason(subtree_root_id, &replacement_tree)
            .is_some()
        {
            return None;
        }

        let merged_tree = replace_subtree_in_value(&self.tree, subtree_root_id, &replacement_tree)?;
        let mut dependency_lookup = self.subtree_root_dependencies;
        for root_id in collect_subtree_root_ids(&replacement_tree) {
            dependency_lookup.insert(root_id, reactive_dependencies.clone());
        }
        Some(Self::from_tree_with_dependency_lookup(
            merged_tree,
            self.source_buffer_id,
            &dependency_lookup,
        ))
    }

    pub fn replacing_subtrees(
        self,
        replacements: &[(u64, Value, Vec<ReactiveFieldKey>)],
    ) -> Option<Self> {
        if replacements.is_empty() {
            return Some(self);
        }

        let mut valid_replacements = Vec::<ValidatedSubtreeReplacement<'_>>::new();
        let mut stale_roots = Vec::new();
        let replacement_parent_lookup = replacements
            .iter()
            .filter_map(|(subtree_root_id, replacement_tree, _)| {
                Some((*subtree_root_id, parent_subtree_root_id(replacement_tree)?))
            })
            .collect::<HashMap<_, _>>();

        for (subtree_root_id, replacement_tree, reactive_dependencies) in replacements {
            if root_subtree_root_id(replacement_tree) != Some(*subtree_root_id) {
                return None;
            }
            match self.subtree_replace_failure_reason(*subtree_root_id, replacement_tree) {
                None => valid_replacements.push(ValidatedSubtreeReplacement {
                    root_id: *subtree_root_id,
                    tree: replacement_tree,
                    reactive_dependencies: reactive_dependencies.as_slice(),
                }),
                Some("unknown-root") => stale_roots.push(*subtree_root_id),
                Some(_) => return None,
            }
        }

        let valid_root_ids = valid_replacements
            .iter()
            .map(|replacement| replacement.root_id)
            .collect::<HashSet<_>>();
        if stale_roots.iter().any(|root_id| {
            !replacement_has_valid_replaced_ancestor(
                *root_id,
                &replacement_parent_lookup,
                &valid_root_ids,
            )
        }) {
            return None;
        }
        valid_replacements.retain(|replacement| {
            !replacement_has_valid_replaced_ancestor(
                replacement.root_id,
                &replacement_parent_lookup,
                &valid_root_ids,
            )
        });

        let replacement_lookup = valid_replacements
            .iter()
            .map(|replacement| (replacement.root_id, replacement.tree))
            .collect::<HashMap<_, _>>();
        let merged_tree = replace_subtrees_in_value(&self.tree, &replacement_lookup)?;
        let mut dependency_lookup = self.subtree_root_dependencies;
        for replacement in valid_replacements {
            for root_id in collect_subtree_root_ids(replacement.tree) {
                dependency_lookup.insert(root_id, replacement.reactive_dependencies.to_vec());
            }
        }

        Some(Self::from_tree_with_dependency_lookup(
            merged_tree,
            self.source_buffer_id,
            &dependency_lookup,
        ))
    }

    fn from_tree_with_dependency_lookup(
        tree: Value,
        source_buffer_id: Option<BufferId>,
        dependency_lookup: &HashMap<u64, Vec<ReactiveFieldKey>>,
    ) -> Self {
        let root = CommittedSubtreeSnapshot::from_tree_with_dependency_lookup(
            tree.clone(),
            &[],
            dependency_lookup,
        );
        let mut subtree_roots = HashMap::new();
        let mut widgets = HashMap::new();
        let mut field_to_subtree_roots = HashMap::new();
        let mut subtree_root_dependencies = HashMap::new();
        root.collect_indexes(
            &mut subtree_roots,
            &mut widgets,
            &mut field_to_subtree_roots,
            &mut subtree_root_dependencies,
        );
        Self {
            source_buffer_id,
            tree,
            root_stable_widget_id: root.stable_widget_id,
            root_subtree_root_id: root.subtree_root_id,
            field_to_subtree_roots,
            subtree_root_dependencies,
            subtree_roots,
            widgets,
        }
    }
}

struct ValidatedSubtreeReplacement<'a> {
    root_id: u64,
    tree: &'a Value,
    reactive_dependencies: &'a [ReactiveFieldKey],
}

fn replacement_has_valid_replaced_ancestor(
    subtree_root_id: u64,
    replacement_parent_lookup: &HashMap<u64, u64>,
    valid_root_ids: &HashSet<u64>,
) -> bool {
    let mut visited = HashSet::new();
    let mut current = subtree_root_id;
    while let Some(parent_root_id) = replacement_parent_lookup.get(&current).copied() {
        if valid_root_ids.contains(&parent_root_id) {
            return true;
        }
        if !visited.insert(current) {
            return false;
        }
        current = parent_root_id;
    }
    false
}

impl CommittedSubtreeSnapshot {
    pub fn from_tree(tree: Value, reactive_dependencies: &[ReactiveFieldKey]) -> Arc<Self> {
        Self::from_tree_with_dependency_lookup(tree, reactive_dependencies, &HashMap::new())
    }

    fn from_tree_with_dependency_lookup(
        tree: Value,
        reactive_dependencies: &[ReactiveFieldKey],
        dependency_lookup: &HashMap<u64, Vec<ReactiveFieldKey>>,
    ) -> Arc<Self> {
        let stable_widget_id = prop_u64_from_value(&tree, "__stable-widget-id");
        let subtree_root_id = prop_u64_from_value(&tree, "__subtree-root-id");
        let parent_subtree_root_id = prop_u64_from_value(&tree, "__parent-subtree-root-id");
        let stable_key = prop_string_from_value(&tree, "__stable-key");
        let widget_type = prop_widget_type_from_value(&tree);
        let subtree_dependencies = subtree_root_id
            .and_then(|root_id| dependency_lookup.get(&root_id).cloned())
            .unwrap_or_else(|| reactive_dependencies.to_vec());
        let children = value_children(&tree)
            .into_iter()
            .map(|child| {
                Self::from_tree_with_dependency_lookup(
                    child,
                    reactive_dependencies,
                    dependency_lookup,
                )
            })
            .collect();
        Arc::new(Self {
            stable_widget_id,
            subtree_root_id,
            parent_subtree_root_id,
            stable_key,
            widget_type,
            reactive_dependencies: subtree_dependencies,
            tree,
            children,
        })
    }

    fn collect_indexes(
        self: &Arc<Self>,
        subtree_roots: &mut HashMap<u64, Arc<CommittedSubtreeSnapshot>>,
        widgets: &mut HashMap<u64, Arc<CommittedSubtreeSnapshot>>,
        field_to_subtree_roots: &mut HashMap<ReactiveFieldKey, Vec<u64>>,
        subtree_root_dependencies: &mut HashMap<u64, Vec<ReactiveFieldKey>>,
    ) {
        if let Some(root_id) = self.subtree_root_id {
            subtree_roots.insert(root_id, Arc::clone(self));
            subtree_root_dependencies.insert(root_id, self.reactive_dependencies.clone());
            for field in &self.reactive_dependencies {
                field_to_subtree_roots
                    .entry(field.clone())
                    .or_default()
                    .push(root_id);
            }
        }
        if let Some(widget_id) = self.stable_widget_id {
            widgets.insert(widget_id, Arc::clone(self));
        }
        for child in &self.children {
            child.collect_indexes(
                subtree_roots,
                widgets,
                field_to_subtree_roots,
                subtree_root_dependencies,
            );
        }
    }
}

#[cfg(test)]
fn value_map(value: &Value) -> Option<HashMap<String, Value>> {
    match value {
        Value::Map(map) => Some(
            map.iter()
                .map(|(key, value)| (key.clone(), value.borrow().clone()))
                .collect(),
        ),
        _ => None,
    }
}

fn value_children(value: &Value) -> Vec<Value> {
    match value {
        Value::Map(map) => match map.get("children").map(|value| value.borrow()) {
            Some(child) => match &*child {
                Value::List(children) => children
                    .iter()
                    .map(|child| child.borrow().clone())
                    .collect(),
                _ => Vec::new(),
            },
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

pub fn root_subtree_root_id(value: &Value) -> Option<u64> {
    prop_u64_from_value(value, "__subtree-root-id")
}

fn parent_subtree_root_id(value: &Value) -> Option<u64> {
    prop_u64_from_value(value, "__parent-subtree-root-id")
}

fn collect_subtree_root_ids(value: &Value) -> Vec<u64> {
    let mut root_ids = Vec::new();
    collect_subtree_root_ids_impl(value, &mut root_ids);
    root_ids
}

fn collect_subtree_root_ids_impl(value: &Value, root_ids: &mut Vec<u64>) {
    if let Some(root_id) = root_subtree_root_id(value) {
        root_ids.push(root_id);
    }
    for child in value_children(value) {
        collect_subtree_root_ids_impl(&child, root_ids);
    }
}

fn replace_subtree_in_value(
    value: &Value,
    subtree_root_id: u64,
    replacement_tree: &Value,
) -> Option<Value> {
    let Value::Map(map) = value else {
        return None;
    };
    if prop_u64_from_map(map, "__subtree-root-id") == Some(subtree_root_id) {
        return Some(replacement_tree.deep_clone());
    }

    let children_value = map.get("children")?;
    let children_borrow = children_value.borrow();
    let Value::List(children) = &*children_borrow else {
        return None;
    };

    let mut replacements = Vec::with_capacity(children.len());
    let mut replaced_any = false;
    for child in children {
        let child_borrow = child.borrow();
        let replaced_child =
            replace_subtree_in_value(&child_borrow, subtree_root_id, replacement_tree);
        replaced_any |= replaced_child.is_some();
        replacements.push(replaced_child);
    }

    if !replaced_any {
        return None;
    }

    let next_children: Vec<std::rc::Rc<std::cell::RefCell<Value>>> = children
        .iter()
        .zip(replacements)
        .map(|(child, replacement)| {
            let child_value = replacement.unwrap_or_else(|| child.borrow().clone());
            std::rc::Rc::new(std::cell::RefCell::new(child_value))
        })
        .collect();
    drop(children_borrow);

    let mut rebuilt = HashMap::with_capacity(map.len());
    for (key, value) in map {
        let next_value = if key == "children" {
            Value::List(next_children.clone())
        } else {
            value.borrow().clone()
        };
        rebuilt.insert(
            key.clone(),
            std::rc::Rc::new(std::cell::RefCell::new(next_value)),
        );
    }
    Some(Value::Map(rebuilt))
}

fn replace_subtrees_in_value(
    value: &Value,
    replacement_lookup: &HashMap<u64, &Value>,
) -> Option<Value> {
    let Value::Map(map) = value else {
        return None;
    };
    if let Some(replacement_tree) = prop_u64_from_map(map, "__subtree-root-id")
        .and_then(|root_id| replacement_lookup.get(&root_id))
    {
        return Some((*replacement_tree).deep_clone());
    }

    let children_value = map.get("children")?;
    let children_borrow = children_value.borrow();
    let Value::List(children) = &*children_borrow else {
        return None;
    };

    let mut replaced_any = false;
    let next_children = children
        .iter()
        .map(|child| {
            let child_borrow = child.borrow();
            let child_value = if let Some(replacement) =
                replace_subtrees_in_value(&child_borrow, replacement_lookup)
            {
                replaced_any = true;
                replacement
            } else {
                child_borrow.clone()
            };
            std::rc::Rc::new(std::cell::RefCell::new(child_value))
        })
        .collect::<Vec<_>>();

    if !replaced_any {
        return None;
    }
    drop(children_borrow);

    let mut rebuilt = HashMap::with_capacity(map.len());
    for (key, value) in map {
        let next_value = if key == "children" {
            Value::List(next_children.clone())
        } else {
            value.borrow().clone()
        };
        rebuilt.insert(
            key.clone(),
            std::rc::Rc::new(std::cell::RefCell::new(next_value)),
        );
    }
    Some(Value::Map(rebuilt))
}

fn prop_u64_from_value(value: &Value, key: &str) -> Option<u64> {
    match value {
        Value::Map(map) => prop_u64_from_map(map, key),
        _ => None,
    }
}

fn prop_u64_from_map(
    map: &HashMap<String, std::rc::Rc<std::cell::RefCell<Value>>>,
    key: &str,
) -> Option<u64> {
    match map.get(key).map(|value| value.borrow()) {
        Some(value) => match &*value {
            Value::Number(n) if *n >= 0.0 && n.fract() == 0.0 => Some(*n as u64),
            _ => None,
        },
        None => None,
    }
}

fn prop_string_from_value(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Map(map) => match map.get(key).map(|value| value.borrow()) {
            Some(value) => match &*value {
                Value::String(s) => Some(s.clone()),
                _ => None,
            },
            None => None,
        },
        _ => None,
    }
}

fn prop_widget_type_from_value(value: &Value) -> Option<String> {
    match value {
        Value::Map(map) => match map.get("type").map(|value| value.borrow()) {
            Some(value) => match &*value {
                Value::Keyword(widget_type) | Value::String(widget_type) => {
                    Some(widget_type.clone())
                }
                _ => None,
            },
            None => None,
        },
        _ => None,
    }
}

#[cfg(test)]
fn prop_u64(map: &HashMap<String, Value>, key: &str) -> Option<u64> {
    match map.get(key) {
        Some(Value::Number(n)) if *n >= 0.0 && n.fract() == 0.0 => Some(*n as u64),
        _ => None,
    }
}

#[cfg(test)]
fn prop_string(map: &HashMap<String, Value>, key: &str) -> Option<String> {
    match map.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
fn prop_widget_type(map: &HashMap<String, Value>) -> Option<String> {
    match map.get("type") {
        Some(Value::Keyword(widget_type)) => Some(widget_type.clone()),
        Some(Value::String(widget_type)) => Some(widget_type.clone()),
        _ => None,
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
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::{Buffer, CommittedBufferUiSnapshot};
    use crate::vm::{ReactiveFieldKey, Value};

    fn widget(
        widget_type: &str,
        stable_widget_id: u64,
        subtree_root_id: u64,
        children: Vec<Value>,
    ) -> Value {
        let mut map = HashMap::new();
        map.insert(
            "type".to_string(),
            Rc::new(RefCell::new(Value::Keyword(widget_type.to_string()))),
        );
        map.insert(
            "__stable-widget-id".to_string(),
            Rc::new(RefCell::new(Value::Number(stable_widget_id as f64))),
        );
        map.insert(
            "__subtree-root-id".to_string(),
            Rc::new(RefCell::new(Value::Number(subtree_root_id as f64))),
        );
        map.insert(
            "children".to_string(),
            Rc::new(RefCell::new(Value::List(
                children
                    .into_iter()
                    .map(|child| Rc::new(RefCell::new(child)))
                    .collect(),
            ))),
        );
        Value::Map(map)
    }

    fn widget_with_parent(
        widget_type: &str,
        stable_widget_id: u64,
        subtree_root_id: u64,
        parent_subtree_root_id: u64,
        children: Vec<Value>,
    ) -> Value {
        let Value::Map(mut map) = widget(widget_type, stable_widget_id, subtree_root_id, children)
        else {
            unreachable!("test widget helper returns a map");
        };
        map.insert(
            "__parent-subtree-root-id".to_string(),
            Rc::new(RefCell::new(Value::Number(parent_subtree_root_id as f64))),
        );
        Value::Map(map)
    }

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

    #[test]
    fn committed_snapshot_replacing_subtree_rebuilds_tree_and_dependency_indexes() {
        let tree = widget(
            "root",
            1,
            1,
            vec![widget("left", 2, 2, vec![]), widget("right", 3, 3, vec![])],
        );
        let snapshot = CommittedBufferUiSnapshot::from_tree(
            tree,
            Some(7),
            vec![ReactiveFieldKey {
                namespace: "SEQ".to_string(),
                field: "steps".to_string(),
            }],
        );
        let merged = snapshot
            .replacing_subtree(
                2,
                widget("left-updated", 22, 2, vec![]),
                vec![ReactiveFieldKey {
                    namespace: "SEQ".to_string(),
                    field: "selected-step".to_string(),
                }],
            )
            .expect("replace subtree");

        let root_children = super::value_children(&merged.tree);
        let left_type = super::value_map(&root_children[0])
            .and_then(|map| super::prop_widget_type(&map))
            .expect("left widget type");
        let right_type = super::value_map(&root_children[1])
            .and_then(|map| super::prop_widget_type(&map))
            .expect("right widget type");
        assert_eq!(left_type, "left-updated");
        assert_eq!(right_type, "right");
        assert_eq!(
            merged.subtree_roots_for_field(&ReactiveFieldKey {
                namespace: "SEQ".to_string(),
                field: "selected-step".to_string(),
            }),
            vec![2]
        );
        assert_eq!(
            merged.subtree_roots_for_field(&ReactiveFieldKey {
                namespace: "SEQ".to_string(),
                field: "steps".to_string(),
            }),
            vec![1, 3]
        );
    }

    #[test]
    fn runtime_snapshot_adoption_records_generation_identity() {
        let tree = widget("root", 1, 1, vec![widget("knob", 2, 2, vec![])]);
        let snapshot = CommittedBufferUiSnapshot::from_tree(tree.clone(), Some(9), Vec::new());
        let mut buffer = Buffer::new(9, "*ui*");

        buffer.adopt_runtime_committed_ui_snapshot(snapshot.clone(), 42);
        assert_eq!(buffer.widget_tree_revision, 1);
        assert_eq!(buffer.committed_ui_revision, 1);
        assert_eq!(buffer.committed_ui_runtime_generation, Some(42));

        buffer.adopt_runtime_committed_ui_snapshot(snapshot.clone(), 42);
        assert_eq!(buffer.widget_tree_revision, 2);
        assert_eq!(buffer.committed_ui_revision, 2);

        buffer.adopt_runtime_committed_ui_snapshot(snapshot, 43);
        assert_eq!(buffer.widget_tree_revision, 3);
        assert_eq!(buffer.committed_ui_revision, 3);
        assert_eq!(buffer.committed_ui_runtime_generation, Some(43));
        assert_eq!(buffer.widget_tree.as_ref(), Some(&tree));
    }

    #[test]
    fn committed_snapshot_replacing_subtrees_updates_multiple_siblings() {
        let tree = widget(
            "root",
            1,
            1,
            vec![
                widget("left", 2, 2, vec![]),
                widget("middle", 3, 3, vec![]),
                widget("right", 4, 4, vec![]),
            ],
        );
        let snapshot = CommittedBufferUiSnapshot::from_tree(
            tree,
            Some(7),
            vec![ReactiveFieldKey {
                namespace: "SEQ".to_string(),
                field: "steps".to_string(),
            }],
        );
        let merged = snapshot
            .replacing_subtrees(&[
                (
                    2,
                    widget("left-updated", 22, 2, vec![]),
                    vec![ReactiveFieldKey {
                        namespace: "SEQ".to_string(),
                        field: "left".to_string(),
                    }],
                ),
                (
                    4,
                    widget("right-updated", 44, 4, vec![]),
                    vec![ReactiveFieldKey {
                        namespace: "SEQ".to_string(),
                        field: "right".to_string(),
                    }],
                ),
            ])
            .expect("replace subtrees");

        let root_children = super::value_children(&merged.tree);
        let child_types = root_children
            .iter()
            .map(|child| {
                super::value_map(child)
                    .and_then(|map| super::prop_widget_type(&map))
                    .expect("child widget type")
            })
            .collect::<Vec<_>>();
        assert_eq!(child_types, vec!["left-updated", "middle", "right-updated"]);
        assert_eq!(
            merged.subtree_roots_for_field(&ReactiveFieldKey {
                namespace: "SEQ".to_string(),
                field: "left".to_string(),
            }),
            vec![2]
        );
        assert_eq!(
            merged.subtree_roots_for_field(&ReactiveFieldKey {
                namespace: "SEQ".to_string(),
                field: "right".to_string(),
            }),
            vec![4]
        );
        assert_eq!(
            merged.subtree_roots_for_field(&ReactiveFieldKey {
                namespace: "SEQ".to_string(),
                field: "steps".to_string(),
            }),
            vec![1, 3]
        );
    }

    #[test]
    fn committed_snapshot_replacing_subtrees_ignores_stale_nested_replacement_when_parent_updates()
    {
        let tree = widget("root", 1, 1, vec![widget("panel-closed", 2, 2, vec![])]);
        let snapshot = CommittedBufferUiSnapshot::from_tree(
            tree,
            Some(7),
            vec![ReactiveFieldKey {
                namespace: "SEQ".to_string(),
                field: "steps".to_string(),
            }],
        );

        let merged = snapshot
            .replacing_subtrees(&[
                (
                    3,
                    widget_with_parent("stale-child", 33, 3, 2, vec![]),
                    vec![ReactiveFieldKey {
                        namespace: "UI".to_string(),
                        field: "stale".to_string(),
                    }],
                ),
                (
                    2,
                    widget(
                        "panel-open",
                        22,
                        2,
                        vec![widget("fresh-child", 44, 4, vec![])],
                    ),
                    vec![ReactiveFieldKey {
                        namespace: "UI".to_string(),
                        field: "panel".to_string(),
                    }],
                ),
            ])
            .expect("stale child replacement should not block valid parent replacement");

        let root_children = super::value_children(&merged.tree);
        let panel_type = super::value_map(&root_children[0])
            .and_then(|map| super::prop_widget_type(&map))
            .expect("panel widget type");
        let panel_children = super::value_children(&root_children[0]);
        let child_type = super::value_map(&panel_children[0])
            .and_then(|map| super::prop_widget_type(&map))
            .expect("fresh child widget type");
        assert_eq!(panel_type, "panel-open");
        assert_eq!(child_type, "fresh-child");
        assert_eq!(
            merged.subtree_roots_for_field(&ReactiveFieldKey {
                namespace: "UI".to_string(),
                field: "panel".to_string(),
            }),
            vec![2, 4]
        );
        assert!(
            merged
                .subtree_roots_for_field(&ReactiveFieldKey {
                    namespace: "UI".to_string(),
                    field: "stale".to_string(),
                })
                .is_empty(),
            "discarded stale subtree dependencies should not be indexed"
        );
    }

    #[test]
    fn committed_snapshot_replacing_subtrees_discards_known_child_when_parent_updates() {
        let tree = widget(
            "root",
            1,
            1,
            vec![widget(
                "panel-closed",
                2,
                2,
                vec![widget_with_parent("old-child", 3, 3, 2, vec![])],
            )],
        );
        let snapshot = CommittedBufferUiSnapshot::from_tree(
            tree,
            Some(7),
            vec![ReactiveFieldKey {
                namespace: "SEQ".to_string(),
                field: "steps".to_string(),
            }],
        );

        let merged = snapshot
            .replacing_subtrees(&[
                (
                    3,
                    widget_with_parent("stale-child", 33, 3, 2, vec![]),
                    vec![ReactiveFieldKey {
                        namespace: "UI".to_string(),
                        field: "stale-child".to_string(),
                    }],
                ),
                (
                    2,
                    widget(
                        "panel-open",
                        22,
                        2,
                        vec![widget("fresh-child", 44, 4, vec![])],
                    ),
                    vec![ReactiveFieldKey {
                        namespace: "UI".to_string(),
                        field: "panel".to_string(),
                    }],
                ),
            ])
            .expect("known child replacement should not block valid parent replacement");

        let root_children = super::value_children(&merged.tree);
        let panel_children = super::value_children(&root_children[0]);
        let child_type = super::value_map(&panel_children[0])
            .and_then(|map| super::prop_widget_type(&map))
            .expect("child widget type");
        assert_eq!(child_type, "fresh-child");
        assert!(
            merged
                .subtree_roots_for_field(&ReactiveFieldKey {
                    namespace: "UI".to_string(),
                    field: "stale-child".to_string(),
                })
                .is_empty(),
            "discarded child replacement dependencies should not be indexed"
        );
    }

    #[test]
    fn committed_snapshot_replacing_subtrees_rejects_unknown_root_without_valid_parent() {
        let tree = widget("root", 1, 1, vec![widget("panel-closed", 2, 2, vec![])]);
        let snapshot = CommittedBufferUiSnapshot::from_tree(
            tree,
            Some(7),
            vec![ReactiveFieldKey {
                namespace: "SEQ".to_string(),
                field: "steps".to_string(),
            }],
        );

        assert!(
            snapshot
                .replacing_subtrees(&[(
                    3,
                    widget_with_parent("orphaned-child", 33, 3, 99, vec![]),
                    vec![ReactiveFieldKey {
                        namespace: "UI".to_string(),
                        field: "orphan".to_string(),
                    }],
                )])
                .is_none(),
            "unknown subtree roots should only be discarded when covered by a valid parent replacement"
        );
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
