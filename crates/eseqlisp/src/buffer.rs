use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::backend::Color;
use crate::editor::ViewMode;
use crate::host::BufferId;
use crate::mode::BufferMode;
use crate::parser::{Parser, SpannedASTParser};
use crate::text::sexp_range_at_cursor;
use crate::vm::{
    INLINE_VALUE_END_BYTE_PROP, INLINE_VALUE_START_BYTE_PROP, SOURCE_END_BYTE_PROP,
    SOURCE_START_BYTE_PROP,
};
use crate::vm::{ReactiveFieldKey, Value};

pub type SourceAnchorId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceAnchor {
    pub start_byte: usize,
    pub end_byte: usize,
    pub revision: u64,
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub start_byte: usize,
    pub end_byte: usize,
    pub replacement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayBand {
    pub after_buffer_line: usize,
    pub anchor_id: SourceAnchorId,
    pub height_cells: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineColumnInsertion {
    pub anchor_id: SourceAnchorId,
    pub buffer_col: usize,
    pub width_cells: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayRow {
    Text {
        buffer_line: usize,
    },
    Band {
        anchor_id: SourceAnchorId,
        row_in_band: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayRowMap {
    rows: Vec<DisplayRow>,
    buffer_to_display: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineWidgetPlacement {
    Margin,
    Inline { width_cells: usize },
    Band { height_cells: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub struct InlineCodeWidget {
    pub anchor_id: SourceAnchorId,
    pub container_anchor_id: SourceAnchorId,
    pub value_anchor_id: Option<SourceAnchorId>,
    pub placement: InlineWidgetPlacement,
    pub widget: Value,
}

impl DisplayRowMap {
    pub fn new(buffer_line_count: usize, bands: impl IntoIterator<Item = DisplayBand>) -> Self {
        let mut bands = bands
            .into_iter()
            .filter(|band| band.height_cells > 0 && band.after_buffer_line < buffer_line_count)
            .collect::<Vec<_>>();
        bands.sort_by_key(|band| (band.after_buffer_line, band.anchor_id));

        let mut rows = Vec::with_capacity(
            buffer_line_count + bands.iter().map(|band| band.height_cells).sum::<usize>(),
        );
        let mut buffer_to_display = Vec::with_capacity(buffer_line_count);
        let mut band_index = 0usize;
        for buffer_line in 0..buffer_line_count {
            buffer_to_display.push(rows.len());
            rows.push(DisplayRow::Text { buffer_line });
            while bands
                .get(band_index)
                .is_some_and(|band| band.after_buffer_line == buffer_line)
            {
                let band = bands[band_index];
                rows.extend((0..band.height_cells).map(|row_in_band| DisplayRow::Band {
                    anchor_id: band.anchor_id,
                    row_in_band,
                }));
                band_index += 1;
            }
        }
        Self {
            rows,
            buffer_to_display,
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn row(&self, display_row: usize) -> Option<DisplayRow> {
        self.rows.get(display_row).copied()
    }

    pub fn display_row_for_buffer_line(&self, buffer_line: usize) -> Option<usize> {
        self.buffer_to_display.get(buffer_line).copied()
    }

    pub fn buffer_line_for_display_row(&self, display_row: usize) -> Option<usize> {
        match self.row(display_row)? {
            DisplayRow::Text { buffer_line } => Some(buffer_line),
            DisplayRow::Band { .. } => None,
        }
    }

    pub fn nearest_buffer_line_for_display_row(&self, display_row: usize) -> Option<usize> {
        if self.buffer_to_display.is_empty() {
            return None;
        }
        let display_row = display_row.min(self.rows.len().saturating_sub(1));
        match self.rows[display_row] {
            DisplayRow::Text { buffer_line } => Some(buffer_line),
            DisplayRow::Band { .. } => {
                self.rows[..display_row]
                    .iter()
                    .rev()
                    .find_map(|row| match row {
                        DisplayRow::Text { buffer_line } => Some(*buffer_line),
                        DisplayRow::Band { .. } => None,
                    })
            }
        }
    }

    pub fn first_display_row_for_band(&self, anchor_id: SourceAnchorId) -> Option<usize> {
        self.rows.iter().position(|row| {
            matches!(row, DisplayRow::Band { anchor_id: current, row_in_band: 0 } if *current == anchor_id)
        })
    }
}

impl TextEdit {
    pub fn new(start_byte: usize, end_byte: usize, replacement: impl Into<String>) -> Self {
        Self {
            start_byte,
            end_byte,
            replacement: replacement.into(),
        }
    }
}

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
    line_start_bytes: Vec<usize>,
    source_anchors: HashMap<SourceAnchorId, SourceAnchor>,
    inline_code_widgets: Vec<InlineCodeWidget>,
    pub inline_widget_revision: u64,
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

fn flush_identical_cell(
    a: &std::rc::Rc<std::cell::RefCell<Value>>,
    b: &std::rc::Rc<std::cell::RefCell<Value>>,
) -> bool {
    std::rc::Rc::ptr_eq(a, b) || widget_tree_flush_identical(&a.borrow(), &b.borrow())
}

/// Structural identity for pending widget-tree flushes. Matches `Value`
/// equality except that closures only compare identical when they share the
/// same chunk AND the same captured upvalue cells (pointer equality), and
/// native functions never compare identical. A pending subtree replacement
/// whose tree is flush-identical to the committed content is a no-op: the
/// committed tree, its input handlers, and its rendered output would all be
/// unchanged by applying it.
pub fn widget_tree_flush_identical(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::List(x), Value::List(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y.iter())
                    .all(|(left, right)| flush_identical_cell(left, right))
        }
        (Value::Map(x), Value::Map(y)) => {
            x.len() == y.len()
                && x.iter().all(|(key, left)| {
                    y.get(key)
                        .is_some_and(|right| flush_identical_cell(left, right))
                })
        }
        (Value::Closure(x_chunk, x_upvalues), Value::Closure(y_chunk, y_upvalues)) => {
            x_chunk == y_chunk
                && x_upvalues.len() == y_upvalues.len()
                && x_upvalues
                    .iter()
                    .zip(y_upvalues.iter())
                    .all(|(left, right)| std::rc::Rc::ptr_eq(left, right))
        }
        (Value::NativeFunction(_), _) | (_, Value::NativeFunction(_)) => false,
        _ => a == b,
    }
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
            line_start_bytes: vec![0],
            source_anchors: HashMap::new(),
            inline_code_widgets: Vec::new(),
            inline_widget_revision: 0,
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
        let old_len = self.text().len();
        self.replace_lines_from_text(text);
        self.cursor = (0, 0);
        self.scroll_top = 0;
        self.revision = self.revision.wrapping_add(1);
        self.adjust_source_anchors(&TextEdit::new(0, old_len, text), None);
        self.text_styles.clear();
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    fn replace_lines_from_text(&mut self, text: &str) {
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
        self.rebuild_line_start_bytes();
    }

    fn rebuild_line_start_bytes(&mut self) {
        self.line_start_bytes.clear();
        self.line_start_bytes.reserve(self.lines.len());
        let mut offset = 0usize;
        for (index, line) in self.lines.iter().enumerate() {
            self.line_start_bytes.push(offset);
            offset += line.len();
            if index + 1 < self.lines.len() {
                offset += 1;
            }
        }
    }

    pub fn byte_offset_at(&self, position: (usize, usize)) -> Option<usize> {
        let (row, col) = position;
        let line = self.lines.get(row)?;
        if col > Self::line_len(line) {
            return None;
        }
        Some(self.line_start_bytes.get(row).copied()? + Self::char_to_byte_idx(line, col))
    }

    pub fn position_at_byte_offset(&self, offset: usize) -> Option<(usize, usize)> {
        let text_len = self.text().len();
        if offset > text_len {
            return None;
        }
        let row = match self.line_start_bytes.binary_search(&offset) {
            Ok(row) => row,
            Err(next) => next.saturating_sub(1),
        };
        let line = self.lines.get(row)?;
        let byte_col = offset.saturating_sub(*self.line_start_bytes.get(row)?);
        if byte_col > line.len() || !line.is_char_boundary(byte_col) {
            return None;
        }
        Some((row, Self::byte_to_char_idx(line, byte_col)))
    }

    pub fn source_anchor(&self, id: SourceAnchorId) -> Option<SourceAnchor> {
        self.source_anchors.get(&id).copied()
    }

    pub fn source_anchors(&self) -> &HashMap<SourceAnchorId, SourceAnchor> {
        &self.source_anchors
    }

    pub fn inline_code_widgets(&self) -> &[InlineCodeWidget] {
        &self.inline_code_widgets
    }

    pub fn inline_widget_runtime_target(
        &self,
        anchor_id: SourceAnchorId,
    ) -> Option<(Value, String)> {
        let widget = self
            .inline_code_widgets
            .iter()
            .find(|inline| inline.anchor_id == anchor_id)?;
        let Value::Map(map) = &widget.widget else {
            return None;
        };
        let target = map
            .get("__inline-runtime-target")
            .map(|value| value.borrow().clone())?;
        let inlet = map
            .get(crate::vm::INLINE_PARENT_INLET_PROP)
            .and_then(|value| match &*value.borrow() {
                Value::String(value) => Some(value.clone()),
                _ => None,
            })?;
        Some((target, inlet))
    }

    pub fn inline_widget_runtime_bindings(&self) -> Vec<(SourceAnchorId, Value, String)> {
        self.inline_code_widgets
            .iter()
            .filter_map(|inline| {
                self.inline_widget_runtime_target(inline.anchor_id)
                    .map(|(target, inlet)| (inline.anchor_id, target, inlet))
            })
            .collect()
    }

    pub fn set_inline_widget_live_value(
        &mut self,
        anchor_id: SourceAnchorId,
        value: Value,
    ) -> bool {
        let Some(inline) = self
            .inline_code_widgets
            .iter_mut()
            .find(|inline| inline.anchor_id == anchor_id)
        else {
            return false;
        };
        let Value::Map(map) = &mut inline.widget else {
            return false;
        };
        let display_value = if matches!(map.get("__inline-toggle-numeric"), Some(marker) if matches!(&*marker.borrow(), Value::Bool(true)))
        {
            match &value {
                Value::Number(number) => Value::Bool(*number != 0.0),
                _ => value.clone(),
            }
        } else {
            value.clone()
        };
        let previous = map.get("value").map(|value| value.borrow().clone());
        let text_value = map
            .get("__inline-text-value")
            .map(|value| value.borrow().clone());
        let diverged = text_value.as_ref().is_some_and(|text| *text != value);
        let previous_diverged = matches!(
            map.get("__inline-live-diverged"),
            Some(value) if matches!(&*value.borrow(), Value::Bool(true))
        );
        if previous.as_ref() == Some(&display_value) && previous_diverged == diverged {
            return false;
        }
        map.insert(
            "value".to_string(),
            std::rc::Rc::new(std::cell::RefCell::new(display_value)),
        );
        if diverged {
            map.insert(
                "__inline-live-diverged".to_string(),
                std::rc::Rc::new(std::cell::RefCell::new(Value::Bool(true))),
            );
        } else {
            map.remove("__inline-live-diverged");
        }
        self.inline_widget_revision = self.inline_widget_revision.wrapping_add(1);
        true
    }

    pub fn normalize_inline_widget_output(&self, anchor_id: SourceAnchorId, value: Value) -> Value {
        let widget_map = self
            .inline_code_widgets
            .iter()
            .find(|inline| inline.anchor_id == anchor_id)
            .and_then(|inline| match &inline.widget {
                Value::Map(map) => Some(map),
                _ => None,
            });
        let numeric_toggle = widget_map
            .and_then(|map| map.get("__inline-toggle-numeric"))
            .is_some_and(|value| matches!(&*value.borrow(), Value::Bool(true)));
        let value = match (numeric_toggle, value) {
            (true, Value::Bool(value)) => Value::Number(if value { 1.0 } else { 0.0 }),
            (_, value) => value,
        };
        let Some(map) = widget_map else {
            return value;
        };
        let Some(step) = map.get("step").and_then(|value| match &*value.borrow() {
            Value::Number(step) if step.is_finite() && *step > 0.0 => Some(*step),
            _ => None,
        }) else {
            return value;
        };
        match value {
            Value::Number(number) => {
                let min = map
                    .get("min")
                    .and_then(|value| match &*value.borrow() {
                        Value::Number(min) if min.is_finite() => Some(*min),
                        _ => None,
                    })
                    .unwrap_or(0.0);
                Value::Number(min + ((number - min) / step).round() * step)
            }
            value => value,
        }
    }

    pub fn write_inline_widget_value(
        &mut self,
        anchor_id: SourceAnchorId,
        value: Value,
    ) -> Result<(), String> {
        let inline_index = self
            .inline_code_widgets
            .iter()
            .position(|inline| inline.anchor_id == anchor_id)
            .ok_or_else(|| format!("inline widget anchor {anchor_id} no longer exists"))?;
        let value_anchor_id = self.inline_code_widgets[inline_index]
            .value_anchor_id
            .ok_or_else(|| "inline widget has no writable literal span".to_string())?;
        let value_anchor = self
            .source_anchor(value_anchor_id)
            .ok_or_else(|| "inline widget literal anchor no longer exists".to_string())?;
        let replacement = format_inline_literal_value(
            &value,
            &self.text()[value_anchor.start_byte..value_anchor.end_byte],
            &self.inline_code_widgets[inline_index].widget,
        );
        self.apply_text_edit_for_anchor(
            TextEdit::new(value_anchor.start_byte, value_anchor.end_byte, replacement),
            Some(value_anchor_id),
        )?;
        let container_anchor_id = self.inline_code_widgets[inline_index].container_anchor_id;
        for authored_anchor_id in [anchor_id, container_anchor_id] {
            if let Some(anchor) = self.source_anchors.get_mut(&authored_anchor_id) {
                anchor.stale = false;
            }
        }
        if let Value::Map(map) = &mut self.inline_code_widgets[inline_index].widget {
            let display_value = if matches!(map.get("__inline-toggle-numeric"), Some(marker) if matches!(&*marker.borrow(), Value::Bool(true)))
            {
                match &value {
                    Value::Number(number) => Value::Bool(*number != 0.0),
                    _ => value.clone(),
                }
            } else {
                value.clone()
            };
            map.insert(
                "value".to_string(),
                std::rc::Rc::new(std::cell::RefCell::new(display_value)),
            );
            map.insert(
                "__inline-text-value".to_string(),
                std::rc::Rc::new(std::cell::RefCell::new(value)),
            );
            map.remove("__inline-live-diverged");
        }
        self.inline_widget_revision = self.inline_widget_revision.wrapping_add(1);
        Ok(())
    }

    pub fn set_inline_code_widgets(&mut self, widgets: Vec<Value>) -> Result<(), String> {
        let mut next_widgets = Vec::with_capacity(widgets.len());
        let mut anchors = Vec::with_capacity(widgets.len() * 3);
        let source = self.text();
        let tokens = Parser::new(source.clone())
            .parse_spanned()
            .map_err(|error| format!("failed to parse inline widget source: {error:?}"))?;
        let top_level_spans = SpannedASTParser::new(tokens)
            .parse()
            .map_err(|error| format!("failed to parse inline widget source AST: {error:?}"))?
            .into_iter()
            .map(|expr| {
                (
                    expr.origin.primary_span.start_byte,
                    expr.origin.primary_span.end_byte,
                )
            })
            .collect::<Vec<_>>();
        for (index, mut widget) in widgets.into_iter().enumerate() {
            let anchor_id = self
                .inline_widget_revision
                .wrapping_add(1)
                .wrapping_mul(1_000_003)
                .wrapping_add(index as u64 + 1);
            let start_byte = value_map_usize(&widget, SOURCE_START_BYTE_PROP)
                .ok_or_else(|| "inline widget is missing its source start byte".to_string())?;
            let end_byte = value_map_usize(&widget, SOURCE_END_BYTE_PROP)
                .ok_or_else(|| "inline widget is missing its source end byte".to_string())?;
            anchors.push((anchor_id, start_byte, end_byte));
            let (container_start, container_end) = top_level_spans
                .iter()
                .copied()
                .find(|(start, end)| *start <= start_byte && *end >= end_byte)
                .ok_or_else(|| {
                    format!(
                        "inline widget source {start_byte}..{end_byte} is not inside a top-level expression"
                    )
                })?;
            let container_anchor_id = anchor_id | (1_u64 << 62);
            anchors.push((container_anchor_id, container_start, container_end));

            let value_span = value_map_usize(&widget, INLINE_VALUE_START_BYTE_PROP)
                .zip(value_map_usize(&widget, INLINE_VALUE_END_BYTE_PROP));
            let value_anchor_id = value_span.map(|(start, end)| {
                let id = anchor_id | (1_u64 << 63);
                anchors.push((id, start, end));
                id
            });
            let placement = match value_map_string(&widget, "__inline-placement").as_deref() {
                Some("band") => InlineWidgetPlacement::Band {
                    height_cells: value_map_usize(&widget, "height").unwrap_or(4).max(1),
                },
                Some("inline") => InlineWidgetPlacement::Inline {
                    width_cells: value_map_usize(&widget, "__inline-width")
                        .unwrap_or(4)
                        .max(1),
                },
                _ => InlineWidgetPlacement::Margin,
            };
            if let Value::Map(map) = &mut widget {
                map.insert(
                    "__inline-anchor-id".to_string(),
                    std::rc::Rc::new(std::cell::RefCell::new(Value::Number(anchor_id as f64))),
                );
                if map.contains_key("on-change") {
                    map.insert(
                        "on-change".to_string(),
                        std::rc::Rc::new(std::cell::RefCell::new(Value::String(format!(
                            "{}:{anchor_id}",
                            crate::vm::INLINE_WRITEBACK_CALLBACK
                        )))),
                    );
                }
            }
            next_widgets.push(InlineCodeWidget {
                anchor_id,
                container_anchor_id,
                value_anchor_id,
                placement,
                widget,
            });
        }
        self.replace_source_anchors(anchors)?;
        self.inline_code_widgets = next_widgets;
        self.inline_widget_revision = self.inline_widget_revision.wrapping_add(1);
        Ok(())
    }

    pub fn inline_display_row_map(&self) -> DisplayRowMap {
        let bands = self.inline_code_widgets.iter().filter_map(|widget| {
            let height_cells = match widget.placement {
                InlineWidgetPlacement::Margin | InlineWidgetPlacement::Inline { .. } => {
                    return None;
                }
                InlineWidgetPlacement::Band { height_cells } => height_cells,
            };
            let anchor = self.source_anchor(widget.container_anchor_id)?;
            let position = self.position_at_byte_offset(anchor.end_byte)?;
            Some(DisplayBand {
                after_buffer_line: position.0,
                anchor_id: widget.anchor_id,
                height_cells,
            })
        });
        DisplayRowMap::new(self.lines.len(), bands)
    }

    pub fn inline_column_insertions(&self, buffer_line: usize) -> Vec<InlineColumnInsertion> {
        let mut insertions = self
            .inline_code_widgets
            .iter()
            .filter_map(|widget| {
                let InlineWidgetPlacement::Inline { width_cells } = widget.placement else {
                    return None;
                };
                let value_anchor = self.source_anchor(widget.value_anchor_id?)?;
                let (line, buffer_col) = self.position_at_byte_offset(value_anchor.start_byte)?;
                (line == buffer_line).then_some(InlineColumnInsertion {
                    anchor_id: widget.anchor_id,
                    buffer_col,
                    width_cells,
                })
            })
            .collect::<Vec<_>>();
        insertions.sort_by_key(|insertion| (insertion.buffer_col, insertion.anchor_id));
        insertions
    }

    pub fn display_col_for_buffer_col(&self, buffer_line: usize, buffer_col: usize) -> usize {
        buffer_col
            + self
                .inline_column_insertions(buffer_line)
                .into_iter()
                .filter(|insertion| insertion.buffer_col <= buffer_col)
                .map(|insertion| insertion.width_cells)
                .sum::<usize>()
    }

    pub fn buffer_col_for_display_col(
        &self,
        buffer_line: usize,
        display_col: usize,
    ) -> Option<usize> {
        let mut inserted = 0usize;
        for insertion in self.inline_column_insertions(buffer_line) {
            let display_start = insertion.buffer_col + inserted;
            if display_col < display_start {
                break;
            }
            if display_col < display_start + insertion.width_cells {
                return None;
            }
            inserted += insertion.width_cells;
        }
        Some(display_col.saturating_sub(inserted))
    }

    pub fn inline_widget_display_col(&self, anchor_id: SourceAnchorId) -> Option<(usize, usize)> {
        let widget = self
            .inline_code_widgets
            .iter()
            .find(|widget| widget.anchor_id == anchor_id)?;
        let value_anchor = self.source_anchor(widget.value_anchor_id?)?;
        let (buffer_line, _) = self.position_at_byte_offset(value_anchor.start_byte)?;
        let mut inserted = 0usize;
        for insertion in self.inline_column_insertions(buffer_line) {
            let display_col = insertion.buffer_col + inserted;
            if insertion.anchor_id == anchor_id {
                return Some((buffer_line, display_col));
            }
            inserted += insertion.width_cells;
        }
        None
    }

    pub fn replace_source_anchors(
        &mut self,
        anchors: impl IntoIterator<Item = (SourceAnchorId, usize, usize)>,
    ) -> Result<(), String> {
        let text = self.text();
        let mut next = HashMap::new();
        for (id, start_byte, end_byte) in anchors {
            if start_byte > end_byte
                || end_byte > text.len()
                || !text.is_char_boundary(start_byte)
                || !text.is_char_boundary(end_byte)
            {
                return Err(format!(
                    "invalid source anchor {id}: {start_byte}..{end_byte} for {} bytes",
                    text.len()
                ));
            }
            next.insert(
                id,
                SourceAnchor {
                    start_byte,
                    end_byte,
                    revision: self.revision,
                    stale: false,
                },
            );
        }
        self.source_anchors = next;
        Ok(())
    }

    pub fn apply_text_edit(&mut self, edit: TextEdit) -> Result<(), String> {
        self.apply_text_edit_for_anchor(edit, None)
    }

    pub fn apply_text_edit_for_anchor(
        &mut self,
        edit: TextEdit,
        authoring_anchor: Option<SourceAnchorId>,
    ) -> Result<(), String> {
        let mut text = self.text();
        if edit.start_byte > edit.end_byte
            || edit.end_byte > text.len()
            || !text.is_char_boundary(edit.start_byte)
            || !text.is_char_boundary(edit.end_byte)
        {
            return Err(format!(
                "invalid text edit {}..{} for {} bytes",
                edit.start_byte,
                edit.end_byte,
                text.len()
            ));
        }
        if edit.start_byte == edit.end_byte && edit.replacement.is_empty() {
            return Ok(());
        }

        let cursor_byte = self.byte_offset_at(self.cursor).unwrap_or(0);
        text.replace_range(edit.start_byte..edit.end_byte, &edit.replacement);
        let next_cursor_byte = transform_offset_after_edit(cursor_byte, &edit, true);
        self.replace_lines_from_text(&text);
        self.cursor = self
            .position_at_byte_offset(next_cursor_byte.min(text.len()))
            .unwrap_or((0, 0));
        self.revision = self.revision.wrapping_add(1);
        self.adjust_source_anchors(&edit, authoring_anchor);
        self.dirty = true;
        self.text_styles.clear();
        Ok(())
    }

    fn adjust_source_anchors(&mut self, edit: &TextEdit, authoring_anchor: Option<SourceAnchorId>) {
        let is_insertion = edit.start_byte == edit.end_byte;
        for (id, anchor) in &mut self.source_anchors {
            let overlaps = if is_insertion {
                edit.start_byte > anchor.start_byte && edit.start_byte < anchor.end_byte
            } else {
                edit.start_byte < anchor.end_byte && edit.end_byte > anchor.start_byte
            };
            let starts_after_insertion = is_insertion && edit.start_byte <= anchor.start_byte;
            let entirely_before = !is_insertion && edit.end_byte <= anchor.start_byte;

            if starts_after_insertion || entirely_before {
                anchor.start_byte = transform_offset_after_edit(anchor.start_byte, edit, true);
                anchor.end_byte = transform_offset_after_edit(anchor.end_byte, edit, true);
            } else if overlaps {
                anchor.start_byte = transform_offset_after_edit(anchor.start_byte, edit, false);
                anchor.end_byte = transform_offset_after_edit(anchor.end_byte, edit, true);
                anchor.stale = Some(*id) != authoring_anchor;
            }
            if Some(*id) == authoring_anchor {
                anchor.stale = false;
            }
            anchor.revision = self.revision;
            debug_assert!(anchor.start_byte <= anchor.end_byte);
            debug_assert!(anchor.end_byte <= self.lines.join("\n").len());
        }
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
        let cursor = self.cursor;
        let cursor_byte = self
            .byte_offset_at(cursor)
            .expect("buffer cursor must have a byte offset");
        match c {
            '\n' => {
                self.apply_text_edit(TextEdit::new(cursor_byte, cursor_byte, "\n"))
                    .expect("newline insertion must be a valid text edit");
                self.cursor = (cursor.0 + 1, 0);
            }
            '(' => {
                self.apply_text_edit(TextEdit::new(cursor_byte, cursor_byte, "()"))
                    .expect("paired delimiter insertion must be a valid text edit");
                self.cursor = (cursor.0, cursor.1 + 1);
            }
            _ => {
                self.apply_text_edit(TextEdit::new(cursor_byte, cursor_byte, c.to_string()))
                    .expect("character insertion must be a valid text edit");
                self.cursor = (cursor.0, cursor.1 + 1);
            }
        }
    }

    pub fn insert_newline_with_indent(&mut self) {
        let row = self.cursor.0;
        let col = self.cursor.1;
        let indent = lisp_indent_for_position(&self.lines, row, col);
        let start_byte = self
            .byte_offset_at((row, col))
            .expect("buffer cursor must have a byte offset");
        let suffix = &self.lines[row][Self::char_to_byte_idx(&self.lines[row], col)..];
        let trimmed_spaces = suffix.len() - suffix.trim_start_matches(' ').len();
        self.apply_text_edit(TextEdit::new(
            start_byte,
            start_byte + trimmed_spaces,
            format!("\n{}", " ".repeat(indent)),
        ))
        .expect("indented newline must be a valid text edit");
        self.cursor = (row + 1, indent);
        self.reindent_enclosing_sexp();
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
        let next_cursor_col = if self.cursor.1 <= current_indent {
            desired_indent
        } else {
            desired_indent + (self.cursor.1 - current_indent)
        };
        let start_byte = self.line_start_bytes[row];
        self.apply_text_edit(TextEdit::new(
            start_byte,
            start_byte + current_indent,
            " ".repeat(desired_indent),
        ))
        .expect("indentation must be a valid text edit");
        self.cursor = (
            row,
            next_cursor_col.min(trimmed.chars().count() + desired_indent),
        );
    }

    fn reindent_enclosing_sexp(&mut self) -> bool {
        let Some(((start_row, _), (end_row, _))) = enclosing_sexp_range(&self.lines, self.cursor)
        else {
            return false;
        };

        let cursor_row = self.cursor.0;
        let cursor_col = self.cursor.1;
        let mut edits = Vec::new();
        let mut next_cursor_col = cursor_col;

        for row in (start_row + 1)..=end_row {
            let current_line = self.lines[row].clone();
            let current_indent = current_line.chars().take_while(|ch| *ch == ' ').count();
            let desired_indent = lisp_indent_for_position(&self.lines, row, 0);
            if current_indent == desired_indent {
                continue;
            }

            if row == cursor_row {
                next_cursor_col = if cursor_col <= current_indent {
                    desired_indent
                } else {
                    desired_indent + (cursor_col - current_indent)
                };
            }
            edits.push((row, current_indent, desired_indent));
        }

        for (row, current_indent, desired_indent) in edits.iter().copied().rev() {
            let start_byte = self.line_start_bytes[row];
            self.apply_text_edit(TextEdit::new(
                start_byte,
                start_byte + current_indent,
                " ".repeat(desired_indent),
            ))
            .expect("reindentation must be a valid text edit");
        }
        if !edits.is_empty() {
            self.cursor = (cursor_row, next_cursor_col);
        }
        !edits.is_empty()
    }

    pub fn delete_char_before(&mut self) {
        if self.cursor.1 > 0 {
            let row = self.cursor.0;
            let start = (row, self.cursor.1 - 1);
            let end = self.cursor;
            let start_byte = self.byte_offset_at(start).expect("valid delete start");
            let end_byte = self.byte_offset_at(end).expect("valid delete end");
            self.apply_text_edit(TextEdit::new(start_byte, end_byte, ""))
                .expect("backspace must be a valid text edit");
            self.cursor = start;
        } else if self.cursor.0 > 0 {
            let row = self.cursor.0;
            let previous = (row - 1, Self::line_len(&self.lines[row - 1]));
            let start_byte = self.byte_offset_at(previous).expect("valid newline start");
            let end_byte = self.byte_offset_at(self.cursor).expect("valid newline end");
            self.apply_text_edit(TextEdit::new(start_byte, end_byte, ""))
                .expect("line join must be a valid text edit");
            self.cursor = previous;
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
        let start_byte = self
            .byte_offset_at((start_row, start_col))
            .expect("valid range start");
        let end_byte = self
            .byte_offset_at((end_row, end_col))
            .expect("valid range end");
        self.apply_text_edit(TextEdit::new(start_byte, end_byte, ""))
            .expect("range deletion must be a valid text edit");
        self.cursor = (start_row, start_col);
    }

    pub fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let cursor_byte = self
            .byte_offset_at(self.cursor)
            .expect("buffer cursor must have a byte offset");
        self.apply_text_edit(TextEdit::new(cursor_byte, cursor_byte, text))
            .expect("string insertion must be a valid text edit");
    }

    pub fn delete_word_before(&mut self) {
        if self.cursor.1 == 0 {
            if self.cursor.0 == 0 {
                return;
            }
            self.delete_char_before();
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
            self.delete_range((original.0, 0), original);
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
        self.delete_range((original.0, new_col), original);
    }

    pub fn delete_to_line_end(&mut self) {
        let row = self.cursor.0;
        let col = self.cursor.1;
        if col < Self::line_len(&self.lines[row]) {
            let end = (row, Self::line_len(&self.lines[row]));
            self.delete_range((row, col), end);
        } else if row + 1 < self.lines.len() {
            let start_byte = self.byte_offset_at((row, col)).expect("valid line end");
            self.apply_text_edit(TextEdit::new(start_byte, start_byte + 1, ""))
                .expect("newline deletion must be a valid text edit");
            self.cursor = (row, col);
        }
    }

    fn touch(&mut self) {
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
        self.rebuild_line_start_bytes();
        for anchor in self.source_anchors.values_mut() {
            anchor.stale = true;
            anchor.revision = self.revision;
        }
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

    /// True when replacing `subtree_root_id` with `tree` (and the given
    /// dependency list) would leave the committed snapshot bit-for-bit
    /// unchanged, so the whole flush (tree splice, re-index, layout refresh,
    /// retained repaint) can be skipped. Uses flush-identity semantics: value
    /// equality, except closures must share both chunk and captured upvalue
    /// cells (pointer equality), so a skipped flush can never leave a stale
    /// captured environment in the committed tree.
    pub fn subtree_replacement_is_noop(
        &self,
        subtree_root_id: u64,
        tree: &Value,
        reactive_dependencies: &[ReactiveFieldKey],
    ) -> bool {
        let Some(existing) = self.subtree_roots.get(&subtree_root_id) else {
            return false;
        };
        self.subtree_root_dependencies
            .get(&subtree_root_id)
            .is_some_and(|deps| deps.as_slice() == reactive_dependencies)
            && widget_tree_flush_identical(&existing.tree, tree)
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
        if is_subtree_boundary(self.subtree_root_id, self.parent_subtree_root_id)
            && let Some(root_id) = self.subtree_root_id
        {
            subtree_roots
                .entry(root_id)
                .or_insert_with(|| Arc::clone(self));
            subtree_root_dependencies
                .entry(root_id)
                .or_insert_with(|| self.reactive_dependencies.clone());
            for field in &self.reactive_dependencies {
                let roots = field_to_subtree_roots.entry(field.clone()).or_default();
                if !roots.contains(&root_id) {
                    roots.push(root_id);
                }
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

fn is_subtree_boundary(root_id: Option<u64>, parent_root_id: Option<u64>) -> bool {
    root_id.is_some() && root_id != parent_root_id
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

fn value_map_usize(value: &Value, key: &str) -> Option<usize> {
    let Value::Map(map) = value else {
        return None;
    };
    map.get(key).and_then(|value| match &*value.borrow() {
        Value::Number(number) if number.is_finite() && *number >= 0.0 => Some(*number as usize),
        Value::String(number) => number.parse().ok(),
        _ => None,
    })
}

fn value_map_string(value: &Value, key: &str) -> Option<String> {
    let Value::Map(map) = value else {
        return None;
    };
    map.get(key).and_then(|value| match &*value.borrow() {
        Value::Keyword(value) | Value::String(value) => Some(value.clone()),
        _ => None,
    })
}

fn format_inline_literal_value(value: &Value, previous: &str, widget: &Value) -> String {
    let Value::Number(number) = value else {
        return crate::vm::format_lisp_value(value);
    };
    let mut number = *number;
    if let Value::Map(map) = widget
        && let Some(step) = map.get("step").and_then(|value| match &*value.borrow() {
            Value::Number(step) if step.is_finite() && *step > 0.0 => Some(*step),
            _ => None,
        })
    {
        let min = map
            .get("min")
            .and_then(|value| match &*value.borrow() {
                Value::Number(min) if min.is_finite() => Some(*min),
                _ => None,
            })
            .unwrap_or(0.0);
        number = min + ((number - min) / step).round() * step;
    }
    if number.fract().abs() < 1.0e-9 {
        return format!("{number:.0}");
    }
    let previous_decimals = previous
        .split_once('.')
        .map(|(_, fraction)| {
            fraction
                .trim_end_matches(|ch: char| !ch.is_ascii_digit())
                .len()
        })
        .unwrap_or(0);
    let decimals = previous_decimals.max(3).min(6);
    let mut formatted = format!("{number:.decimals$}");
    while formatted.ends_with('0') {
        formatted.pop();
    }
    if formatted.ends_with('.') {
        formatted.pop();
    }
    formatted
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

fn transform_offset_after_edit(offset: usize, edit: &TextEdit, right_affinity: bool) -> usize {
    if offset < edit.start_byte {
        return offset;
    }
    if offset > edit.end_byte || (offset == edit.end_byte && edit.start_byte != edit.end_byte) {
        let removed = edit.end_byte - edit.start_byte;
        if edit.replacement.len() >= removed {
            return offset + (edit.replacement.len() - removed);
        }
        return offset.saturating_sub(removed - edit.replacement.len());
    }
    if edit.start_byte == edit.end_byte && offset == edit.start_byte {
        return if right_affinity {
            offset + edit.replacement.len()
        } else {
            offset
        };
    }
    if right_affinity {
        edit.start_byte + edit.replacement.len()
    } else {
        edit.start_byte
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

    use super::{
        Buffer, CommittedBufferUiSnapshot, DisplayBand, DisplayRow, DisplayRowMap, TextEdit,
    };
    use crate::vm::{ReactiveFieldKey, Value};

    #[test]
    fn source_anchor_tracks_edits_before_its_span_without_becoming_stale() {
        let mut buffer = Buffer::from_text(7, "*test*", "before\n(~slider 12 :min 0 :max 24)");
        let start = buffer.text().find("(~slider").unwrap();
        let end = buffer.text().len();
        buffer.replace_source_anchors([(41, start, end)]).unwrap();

        buffer.cursor = (0, 0);
        buffer.insert_str("prefix ");

        let anchor = buffer.source_anchor(41).unwrap();
        assert_eq!(anchor.start_byte, start + 7);
        assert_eq!(anchor.end_byte, end + 7);
        assert!(!anchor.stale);
    }

    #[test]
    fn source_anchor_stays_visible_but_becomes_stale_when_edited_inside() {
        let mut buffer = Buffer::from_text(7, "*test*", "(~slider 12 :min 0 :max 24)");
        buffer
            .replace_source_anchors([(41, 0, buffer.text().len())])
            .unwrap();
        let literal = buffer.text().find("12").unwrap();

        buffer
            .apply_text_edit(TextEdit::new(literal, literal + 2, "14"))
            .unwrap();

        let anchor = buffer.source_anchor(41).unwrap();
        assert_eq!(
            (anchor.start_byte, anchor.end_byte),
            (0, buffer.text().len())
        );
        assert!(anchor.stale);
    }

    #[test]
    fn anchor_owned_writeback_updates_span_without_marking_it_stale() {
        let mut buffer = Buffer::from_text(7, "*test*", "(~slider 9 :min 0 :max 24)");
        buffer
            .replace_source_anchors([(41, 0, buffer.text().len())])
            .unwrap();
        let literal = buffer.text().find('9').unwrap();

        buffer
            .apply_text_edit_for_anchor(TextEdit::new(literal, literal + 1, "12"), Some(41))
            .unwrap();

        let anchor = buffer.source_anchor(41).unwrap();
        assert_eq!(anchor.end_byte, buffer.text().len());
        assert!(!anchor.stale);
    }

    #[test]
    fn byte_position_index_handles_unicode_and_line_boundaries() {
        let buffer = Buffer::from_text(7, "*test*", "λx\nvalue");
        assert_eq!(buffer.byte_offset_at((0, 1)), Some("λ".len()));
        assert_eq!(buffer.byte_offset_at((1, 0)), Some("λx\n".len()));
        assert_eq!(buffer.position_at_byte_offset("λx\n".len()), Some((1, 0)));
        assert_eq!(buffer.position_at_byte_offset(1), None);
    }

    #[test]
    fn display_row_map_keeps_buffer_lines_honest_around_bands() {
        let map = DisplayRowMap::new(
            4,
            [DisplayBand {
                after_buffer_line: 1,
                anchor_id: 9,
                height_cells: 2,
            }],
        );

        assert_eq!(map.len(), 6);
        assert_eq!(map.display_row_for_buffer_line(0), Some(0));
        assert_eq!(map.display_row_for_buffer_line(1), Some(1));
        assert_eq!(map.display_row_for_buffer_line(2), Some(4));
        assert_eq!(map.buffer_line_for_display_row(2), None);
        assert_eq!(
            map.row(3),
            Some(DisplayRow::Band {
                anchor_id: 9,
                row_in_band: 1
            })
        );
        assert_eq!(map.nearest_buffer_line_for_display_row(3), Some(1));
        assert_eq!(map.buffer_line_for_display_row(4), Some(2));
    }

    #[test]
    fn display_row_map_orders_multiple_bands_at_the_same_anchor() {
        let map = DisplayRowMap::new(
            2,
            [
                DisplayBand {
                    after_buffer_line: 0,
                    anchor_id: 20,
                    height_cells: 1,
                },
                DisplayBand {
                    after_buffer_line: 0,
                    anchor_id: 10,
                    height_cells: 2,
                },
            ],
        );
        assert_eq!(map.display_row_for_buffer_line(1), Some(4));
        assert!(matches!(
            map.row(1),
            Some(DisplayRow::Band { anchor_id: 10, .. })
        ));
        assert!(matches!(
            map.row(3),
            Some(DisplayRow::Band { anchor_id: 20, .. })
        ));
    }

    #[test]
    fn inline_live_value_tracks_divergence_from_text_without_rewriting_source() {
        let mut buffer = Buffer::from_text(7, "*test*", "(~slider 12 :min 0 :max 24)");
        let mut widget = crate::widgets::build_widget(
            "hslider",
            vec![
                Value::Keyword("value".to_string()),
                Value::Number(12.0),
                Value::Keyword("__inline-text-value".to_string()),
                Value::Number(12.0),
                Value::Keyword(crate::vm::SOURCE_START_BYTE_PROP.to_string()),
                Value::Number(0.0),
                Value::Keyword(crate::vm::SOURCE_END_BYTE_PROP.to_string()),
                Value::Number(buffer.text().len() as f64),
                Value::Keyword(crate::vm::INLINE_VALUE_START_BYTE_PROP.to_string()),
                Value::Number(9.0),
                Value::Keyword(crate::vm::INLINE_VALUE_END_BYTE_PROP.to_string()),
                Value::Number(11.0),
            ],
        );
        if let Value::Map(map) = &mut widget {
            map.insert(
                "__inline-runtime-target".to_string(),
                Rc::new(RefCell::new(Value::Bool(true))),
            );
            map.insert(
                crate::vm::INLINE_PARENT_INLET_PROP.to_string(),
                Rc::new(RefCell::new(Value::String("limit".to_string()))),
            );
        }
        buffer.set_inline_code_widgets(vec![widget]).unwrap();
        let anchor_id = buffer.inline_code_widgets()[0].anchor_id;

        assert!(buffer.set_inline_widget_live_value(anchor_id, Value::Number(14.0)));
        assert_eq!(buffer.text(), "(~slider 12 :min 0 :max 24)");
        let Value::Map(map) = &buffer.inline_code_widgets()[0].widget else {
            panic!("widget map");
        };
        assert!(matches!(&*map["value"].borrow(), Value::Number(14.0)));
        assert!(matches!(
            &*map["__inline-live-diverged"].borrow(),
            Value::Bool(true)
        ));
    }

    #[test]
    fn inline_column_insertion_stays_before_a_growing_value_literal() {
        let mut buffer = Buffer::from_text(7, "*test*", "(~slider 12 :min 0 :max 100)");
        let value_start = buffer.text().find("12").unwrap();
        let mut widget = crate::widgets::build_widget(
            "hslider",
            vec![
                Value::Keyword("value".to_string()),
                Value::Number(12.0),
                Value::Keyword("__inline-placement".to_string()),
                Value::Keyword("inline".to_string()),
                Value::Keyword("__inline-width".to_string()),
                Value::Number(8.0),
                Value::Keyword(crate::vm::SOURCE_START_BYTE_PROP.to_string()),
                Value::Number(0.0),
                Value::Keyword(crate::vm::SOURCE_END_BYTE_PROP.to_string()),
                Value::Number(buffer.text().len() as f64),
                Value::Keyword(crate::vm::INLINE_VALUE_START_BYTE_PROP.to_string()),
                Value::Number(value_start as f64),
                Value::Keyword(crate::vm::INLINE_VALUE_END_BYTE_PROP.to_string()),
                Value::Number((value_start + 2) as f64),
            ],
        );
        if let Value::Map(map) = &mut widget {
            map.insert(
                "__inline-text-value".to_string(),
                Rc::new(RefCell::new(Value::Number(12.0))),
            );
        }
        buffer.set_inline_code_widgets(vec![widget]).unwrap();
        let anchor_id = buffer.inline_code_widgets()[0].anchor_id;

        assert_eq!(buffer.inline_widget_display_col(anchor_id), Some((0, 9)));
        assert_eq!(buffer.display_col_for_buffer_col(0, 9), 17);
        assert_eq!(buffer.buffer_col_for_display_col(0, 12), None);
        assert_eq!(buffer.buffer_col_for_display_col(0, 17), Some(9));

        buffer
            .write_inline_widget_value(anchor_id, Value::Number(100.0))
            .unwrap();
        assert!(buffer.text().contains("(~slider 100"));
        assert_eq!(buffer.inline_widget_display_col(anchor_id), Some((0, 9)));
        let inline = &buffer.inline_code_widgets()[0];
        for authored_anchor_id in [
            inline.anchor_id,
            inline.container_anchor_id,
            inline.value_anchor_id.expect("value anchor"),
        ] {
            assert!(
                !buffer
                    .source_anchor(authored_anchor_id)
                    .expect("authored anchor")
                    .stale,
                "inline writeback must keep every owned anchor fresh"
            );
        }
    }

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
