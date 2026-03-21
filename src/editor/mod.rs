mod commands;
mod minibuffer;
mod natives;
mod widget_focus;
mod widget_interaction;

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::buffer::Buffer;
use crate::host::{BufferId, CompileKind, HostCommand, HostEvent};
use crate::layout::Rect;
use crate::mode::{
    BufferMode, CompletionItem, CompletionMatch, TokenSpan, completion_match, highlight_line,
};
use crate::runtime::Runtime;
use crate::text::{innermost_sexp_range_at_cursor, sexp_at_cursor};
use crate::tile::{HighlightCache, SplitDir, TileId, TileLeaf, TileNode, split_ratio_for_point};
use crate::vm::{Value, format_lisp_value};
use commands::key_str;
use natives::register_editor_natives;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Both,
    UiOnly,
    TextOnly,
}

impl ViewMode {
    pub fn cycle(self) -> Self {
        match self {
            ViewMode::Both => ViewMode::UiOnly,
            ViewMode::UiOnly => ViewMode::TextOnly,
            ViewMode::TextOnly => ViewMode::Both,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Both => "both",
            ViewMode::UiOnly => "ui",
            ViewMode::TextOnly => "text",
        }
    }
}

#[derive(Default, Clone)]
pub struct EditorConfig {
    pub init_source: Option<String>,
}

#[derive(Debug)]
pub enum EditorError {
    Io(std::io::Error),
    Message(String),
}

impl From<std::io::Error> for EditorError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorExit {
    Cancelled,
    Closed,
    SavedAndClosed,
}

type LispBindings = HashMap<String, String>;

struct SavePrompt {
    input: String,
    quit_after_save: bool,
}

#[derive(Debug, Clone)]
enum MinibufferMode {
    Mx {
        input: String,
        candidates: Vec<String>,
        selected: usize,
    },
    SwitchBuffer {
        input: String,
        candidates: Vec<String>,
        selected: usize,
    },
}

#[derive(Debug, Clone)]
pub struct CompletionState {
    pub start_col: usize,
    pub items: Vec<CompletionItem>,
    pub selected: usize,
    pub scroll: usize,
}

#[derive(Debug, Clone)]
pub struct SExpFlash {
    pub buffer_id: BufferId,
    pub range: ((usize, usize), (usize, usize)),
    pub expires_at: Instant,
}

// HighlightCache, CachedHitGrid, WidgetGesture, WidgetClick are in tile.rs

#[derive(Debug, Clone, Copy)]
pub struct Mark {
    pub buffer_id: BufferId,
    pub cursor: (usize, usize),
}

#[derive(Debug, Clone)]
pub struct MajorMode {
    pub name: String,
    pub read_only: bool,
    pub keybindings: HashMap<String, String>,
    pub on_enter: Option<String>,
}

pub struct Editor {
    pub buffers: Vec<Buffer>,
    pub tile_root: TileNode,
    pub active_tile: TileId,
    next_tile_id: TileId,
    pub minibuffer: Option<String>,

    pending_key: Option<KeyEvent>,
    builtins: HashMap<KeyEvent, String>,
    default_lisp_bindings: LispBindings,
    lisp_bindings: LispBindings,
    runtime: Runtime,
    needs_redraw: bool,
    should_quit: bool,
    last_exit: EditorExit,
    next_buffer_id: BufferId,
    save_prompt: Option<SavePrompt>,
    completion: Option<CompletionState>,
    last_mouse_precise: Option<(f32, f32)>,
    active_tile_resize_drag: Option<TileResizeDrag>,
    eval_flash: Option<SExpFlash>,
    mark: Option<Mark>,
    kill_ring: Vec<String>,
    minibuffer_input: Option<MinibufferMode>,
    mode_registry: HashMap<String, MajorMode>,
    /// Cached tile rects, recomputed when tiles change or viewport resizes.
    cached_tile_rects: Vec<(TileId, Rect)>,
}

#[derive(Debug, Clone, Copy)]
struct TileResizeDrag {
    split_id: TileId,
    dir: SplitDir,
    area: Rect,
}

impl Editor {
    pub fn new(mut runtime: Runtime, config: EditorConfig) -> Self {
        register_editor_natives(&mut runtime);

        let mut editor = Editor {
            buffers: vec![Buffer::new(0, "*scratch*")],
            tile_root: TileNode::Leaf(TileLeaf::new(0, 0)),
            active_tile: 0,
            next_tile_id: 1,
            minibuffer: None,
            pending_key: None,
            builtins: HashMap::new(),
            default_lisp_bindings: HashMap::new(),
            lisp_bindings: HashMap::new(),
            runtime,
            needs_redraw: true,
            should_quit: false,
            last_exit: EditorExit::Closed,
            next_buffer_id: 1,
            save_prompt: None,
            completion: None,
            last_mouse_precise: None,
            active_tile_resize_drag: None,
            eval_flash: None,
            mark: None,
            kill_ring: vec![],
            minibuffer_input: None,
            mode_registry: HashMap::new(),
            cached_tile_rects: vec![],
        };
        editor.bind_defaults();
        editor.load_init(config.init_source.as_deref());
        editor.refresh_runtime_side_effects();
        editor.sync_runtime_context();
        editor
    }

    // ── Tile accessors ─────────────────────────────────────────────────────

    pub fn active_leaf(&self) -> &TileLeaf {
        self.tile_root
            .find_leaf(self.active_tile)
            .expect("active tile must exist")
    }

    pub fn active_leaf_mut(&mut self) -> &mut TileLeaf {
        self.tile_root
            .find_leaf_mut(self.active_tile)
            .expect("active tile must exist")
    }

    fn alloc_tile_id(&mut self) -> TileId {
        let id = self.next_tile_id;
        self.next_tile_id += 1;
        id
    }

    /// Get the buffer index for the active tile.
    pub fn active_buffer_idx(&self) -> usize {
        self.active_leaf().buffer_idx
    }

    /// Recompute cached tile rects for the given viewport.
    pub fn update_tile_rects(&mut self, total_width: u16, total_height: u16) {
        let area = Rect {
            row: 0.0,
            col: 0.0,
            width: total_width as f32,
            // Reserve 1 row for global status bar
            height: (total_height as f32 - 1.0).max(0.0),
        };
        self.cached_tile_rects = self.tile_root.compute_rects(area);
    }

    /// Find which tile contains the given screen coordinate.
    pub fn tile_at_screen(&self, col: f32, row: f32) -> Option<TileId> {
        for (tile_id, rect) in &self.cached_tile_rects {
            if col >= rect.col
                && col < rect.col + rect.width
                && row >= rect.row
                && row < rect.row + rect.height
            {
                return Some(*tile_id);
            }
        }
        None
    }

    /// All cached tile rects.
    pub fn tile_rects(&self) -> &[(TileId, Rect)] {
        &self.cached_tile_rects
    }

    /// Get the rect for a given tile ID from the cache.
    pub fn tile_rect(&self, tile_id: TileId) -> Option<Rect> {
        self.cached_tile_rects
            .iter()
            .find(|(id, _)| *id == tile_id)
            .map(|(_, rect)| *rect)
    }

    fn tile_root_rect(&self) -> Option<Rect> {
        let mut rects = self.cached_tile_rects.iter().map(|(_, rect)| *rect);
        let first = rects.next()?;
        let mut min_row = first.row;
        let mut min_col = first.col;
        let mut max_row = first.row + first.height;
        let mut max_col = first.col + first.width;
        for rect in rects {
            min_row = min_row.min(rect.row);
            min_col = min_col.min(rect.col);
            max_row = max_row.max(rect.row + rect.height);
            max_col = max_col.max(rect.col + rect.width);
        }
        Some(Rect {
            row: min_row,
            col: min_col,
            width: (max_col - min_col).max(0.0),
            height: (max_row - min_row).max(0.0),
        })
    }

    /// Check if a screen coordinate is on a border between tiles.
    /// Returns the parent split if found.
    #[allow(dead_code)]
    fn is_on_tile_border(&self, col: f32, row: f32) -> bool {
        if self.cached_tile_rects.len() <= 1 {
            return false;
        }
        // A position is on a border if it's at the edge of one tile
        // and the start of another (within the 1-char border zone)
        for (_, rect) in &self.cached_tile_rects {
            // Right edge of a tile (vertical split border)
            if (col - (rect.col + rect.width)).abs() < 0.5
                && row >= rect.row
                && row < rect.row + rect.height
            {
                return true;
            }
            // Bottom edge (horizontal split border)
            if (row - (rect.row + rect.height)).abs() < 0.5
                && col >= rect.col
                && col < rect.col + rect.width
            {
                return true;
            }
        }
        false
    }

    /// Switch the active tile (updates runtime context, saves/restores widget trees).
    pub fn switch_active_tile(&mut self, new_tile: TileId) {
        if new_tile == self.active_tile {
            return;
        }
        if self.tile_root.find_leaf(new_tile).is_none() {
            return;
        }
        self.save_current_widget_tree();
        self.active_tile = new_tile;
        self.sync_runtime_context();
        self.restore_buffer_widget_tree();
        self.mark_needs_redraw();
    }

    /// Split the active tile. Returns the new tile's ID, or None if no split was possible.
    pub fn split_active_tile(&mut self, dir: SplitDir, new_buffer_idx: usize) -> Option<TileId> {
        let target = self.active_tile;
        let split_id = self.alloc_tile_id();
        let new_tile_id = self.alloc_tile_id();
        if self
            .tile_root
            .split_leaf(target, split_id, new_tile_id, new_buffer_idx, dir)
        {
            self.mark_needs_redraw();
            Some(new_tile_id)
        } else {
            // Roll back IDs
            self.next_tile_id -= 2;
            None
        }
    }

    /// Delete the active tile (close window, not buffer).
    /// Returns true if successful. Cannot delete the last tile.
    pub fn delete_active_tile(&mut self) -> bool {
        if self.tile_root.leaf_count() <= 1 {
            return false;
        }
        let removed = self.tile_root.remove_leaf(self.active_tile);
        if removed.is_some() {
            // Switch to the first remaining leaf
            let ids = self.tile_root.leaf_ids();
            self.active_tile = ids[0];
            self.sync_runtime_context();
            self.restore_buffer_widget_tree();
            self.mark_needs_redraw();
            true
        } else {
            false
        }
    }

    /// Collapse all tiles to a single tile showing the active buffer.
    pub fn delete_other_tiles(&mut self) {
        if self.tile_root.leaf_count() <= 1 {
            return;
        }
        let buffer_idx = self.active_leaf().buffer_idx;
        let new_id = self.alloc_tile_id();
        self.tile_root.collapse_to_single(new_id, buffer_idx);
        self.active_tile = new_id;
        self.mark_needs_redraw();
    }

    /// Cycle active tile to the next leaf in order.
    pub fn cycle_active_tile(&mut self) {
        let ids = self.tile_root.leaf_ids();
        if ids.len() <= 1 {
            return;
        }
        let current_idx = ids
            .iter()
            .position(|id| *id == self.active_tile)
            .unwrap_or(0);
        let next_idx = (current_idx + 1) % ids.len();
        self.switch_active_tile(ids[next_idx]);
    }

    /// Handle tiled mouse event: hit-test tiles, switch active, then dispatch.
    /// `border_inset`: 1 for TUI (cell-based borders), 0 for Metal (pixel borders).
    pub fn handle_tiled_mouse_precise(
        &mut self,
        mouse: MouseEvent,
        precise_col: f32,
        precise_row: f32,
        border_inset: u16,
    ) {
        if self.handle_tile_resize_drag(mouse, precise_col, precise_row) {
            return;
        }

        let screen_col = mouse.column as f32;
        let screen_row = mouse.row as f32;

        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.begin_tile_resize_drag(precise_col, precise_row, border_inset)
        {
            return;
        }

        // Find which tile this mouse event targets
        if let Some(tile_id) = self.tile_at_screen(screen_col, screen_row) {
            self.route_pointer_event_to_tile(
                tile_id,
                matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)),
                |editor| {
                    let Some((content_col, content_row, content_width, content_height)) =
                        editor.tile_content_area(tile_id, border_inset)
                    else {
                        return;
                    };

                    editor.handle_mouse_precise(
                        mouse,
                        content_col,
                        content_row,
                        content_width,
                        content_height,
                        precise_col,
                        precise_row,
                    );
                },
            );
        }
    }

    pub fn handle_tiled_touchpad_magnify(
        &mut self,
        precise_col: f32,
        precise_row: f32,
        border_inset: u16,
        delta: f64,
    ) {
        let Some(tile_id) = self.tile_at_screen(precise_col, precise_row) else {
            return;
        };
        self.route_pointer_event_to_tile(tile_id, false, |editor| {
            let Some((content_col, content_row, _, _)) =
                editor.tile_content_area(tile_id, border_inset)
            else {
                return;
            };
            editor.handle_touchpad_magnify(
                content_col,
                content_row,
                precise_col,
                precise_row,
                delta,
            );
        });
    }

    pub fn handle_tiled_touchpad_scroll(
        &mut self,
        precise_col: f32,
        precise_row: f32,
        border_inset: u16,
        delta_x: f32,
        delta_y: f32,
    ) -> bool {
        let Some(tile_id) = self.tile_at_screen(precise_col, precise_row) else {
            return false;
        };
        self.route_pointer_event_to_tile(tile_id, false, |editor| {
            let Some((content_col, content_row, _, _)) =
                editor.tile_content_area(tile_id, border_inset)
            else {
                return false;
            };
            editor.handle_touchpad_scroll(
                content_col,
                content_row,
                precise_col,
                precise_row,
                delta_x,
                delta_y,
            )
        })
    }

    fn tile_content_area(
        &self,
        tile_id: TileId,
        border_inset: u16,
    ) -> Option<(u16, u16, u16, u16)> {
        let rect = self.tile_rect(tile_id)?;
        let border = border_inset as f32;
        let content_col = (rect.col + border).round() as u16;
        let content_row = (rect.row + border).round() as u16;
        let content_width = (rect.width - border * 2.0).max(0.0).round() as u16;
        let show_status = self.tile_root.find_leaf(tile_id)?.show_status;
        let content_height = if show_status {
            (rect.height - border * 2.0 - 1.0).max(0.0).round() as u16
        } else {
            (rect.height - border * 2.0).max(0.0).round() as u16
        };
        Some((content_col, content_row, content_width, content_height))
    }

    fn route_pointer_event_to_tile<R>(
        &mut self,
        tile_id: TileId,
        persist_selection: bool,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous_tile = self.active_tile;
        let switched = tile_id != previous_tile;

        if switched {
            self.switch_active_tile(tile_id);
        }

        let result = f(self);

        if switched && !persist_selection && self.tile_root.find_leaf(previous_tile).is_some() {
            self.switch_active_tile(previous_tile);
        }

        result
    }

    fn begin_tile_resize_drag(
        &mut self,
        precise_col: f32,
        precise_row: f32,
        border_inset: u16,
    ) -> bool {
        let Some(root_area) = self.tile_root_rect() else {
            return false;
        };
        let tolerance = if border_inset == 0 { 0.5 } else { 1.0 };
        let Some(hit) =
            self.tile_root
                .hit_test_split_divider(root_area, precise_col, precise_row, tolerance)
        else {
            return false;
        };
        self.active_tile_resize_drag = Some(TileResizeDrag {
            split_id: hit.split_id,
            dir: hit.dir,
            area: hit.area,
        });
        self.update_tile_split_ratio(hit.split_id, hit.dir, hit.area, precise_col, precise_row);
        true
    }

    fn handle_tile_resize_drag(
        &mut self,
        mouse: MouseEvent,
        precise_col: f32,
        precise_row: f32,
    ) -> bool {
        let Some(drag) = self.active_tile_resize_drag else {
            return false;
        };

        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                self.update_tile_split_ratio(
                    drag.split_id,
                    drag.dir,
                    drag.area,
                    precise_col,
                    precise_row,
                );
                true
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.update_tile_split_ratio(
                    drag.split_id,
                    drag.dir,
                    drag.area,
                    precise_col,
                    precise_row,
                );
                self.active_tile_resize_drag = None;
                self.last_mouse_precise = None;
                true
            }
            _ => true,
        }
    }

    fn update_tile_split_ratio(
        &mut self,
        split_id: TileId,
        dir: SplitDir,
        area: Rect,
        precise_col: f32,
        precise_row: f32,
    ) {
        if let Some(split) = self.tile_root.find_split_mut(split_id) {
            split.ratio = split_ratio_for_point(area, dir, precise_col, precise_row);
            self.mark_needs_redraw();
        }
    }

    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    pub fn clear_needs_redraw(&mut self) {
        self.needs_redraw = false;
    }

    pub fn mark_needs_redraw(&mut self) {
        self.needs_redraw = true;
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn clear_quit_request(&mut self) {
        self.should_quit = false;
        self.mark_needs_redraw();
    }

    pub fn prompt_text(&self) -> Option<String> {
        self.save_prompt
            .as_ref()
            .map(|prompt| format!(" Save as: {}", prompt.input))
    }

    pub fn completion_state(&self) -> Option<&CompletionState> {
        self.completion.as_ref()
    }

    pub fn active_highlight_spans(&mut self) -> Rc<Vec<Vec<TokenSpan>>> {
        let buf_idx = self.active_buffer_idx();
        let buffer = &self.buffers[buf_idx];
        let buffer_id = buffer.id;
        let buffer_revision = buffer.revision;
        let buffer_mode = buffer.mode.clone();
        let runtime_symbol_revision = self.runtime.symbol_revision();

        let leaf = self.active_leaf();
        let is_fresh = leaf.highlight_cache.as_ref().is_some_and(|cache| {
            cache.buffer_id == buffer_id
                && cache.buffer_revision == buffer_revision
                && cache.buffer_mode == buffer_mode
                && cache.runtime_symbol_revision == runtime_symbol_revision
        });

        if !is_fresh {
            let symbols = self.runtime.completion_symbols();
            let buffer = &self.buffers[buf_idx];
            let spans = buffer
                .lines
                .iter()
                .map(|line| highlight_line(&buffer.mode, line, &symbols, buffer))
                .collect();
            self.active_leaf_mut().highlight_cache = Some(HighlightCache {
                buffer_id,
                buffer_revision,
                buffer_mode,
                runtime_symbol_revision,
                spans: Rc::new(spans),
            });
        }

        Rc::clone(
            &self
                .active_leaf()
                .highlight_cache
                .as_ref()
                .expect("highlight cache")
                .spans,
        )
    }

    pub fn active_sexp_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let buffer = self.active_buffer();
        innermost_sexp_range_at_cursor(&buffer.lines, buffer.cursor)
    }

    pub fn active_eval_flash_range(&mut self) -> Option<((usize, usize), (usize, usize))> {
        let flash = self.eval_flash.as_ref()?;
        if flash.buffer_id != self.active_buffer().id {
            self.eval_flash = None;
            return None;
        }
        if flash.expires_at < Instant::now() {
            self.eval_flash = None;
            return None;
        }
        Some(flash.range)
    }

    pub fn active_region_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let mark = self.mark.as_ref()?;
        if mark.buffer_id != self.active_buffer().id {
            return None;
        }
        Some(normalize_region(mark.cursor, self.active_buffer().cursor))
    }

    pub fn active_buffer(&self) -> &Buffer {
        let idx = self.active_leaf().buffer_idx;
        &self.buffers[idx]
    }

    pub fn active_buffer_mut(&mut self) -> &mut Buffer {
        let idx = self.active_leaf().buffer_idx;
        &mut self.buffers[idx]
    }

    pub fn widget_scroll_top(&self) -> u16 {
        self.active_leaf().widget_scroll_top
    }

    pub fn widget_scroll_left(&self) -> u16 {
        self.active_leaf().widget_scroll_left
    }

    /// Combined vertical scroll: widget scroll + text scroll.
    pub fn total_scroll_top(&self) -> f32 {
        self.widget_scroll_top() as f32 + self.active_buffer().scroll_top as f32
    }

    pub fn focused_widget_id(&self) -> Option<u64> {
        self.active_leaf().focused_widget_id
    }

    pub fn widget_layout(&self) -> Option<Arc<crate::layout::LayoutNode>> {
        // For active tile, runtime holds the authoritative layout.
        // For other tiles, their cached_layout is used directly.
        self.runtime.current_layout.clone()
    }

    pub fn widget_layout_revision(&self) -> u64 {
        self.runtime.layout_revision()
    }

    pub fn take_dirty_widget_ids(&mut self) -> Vec<u64> {
        self.runtime.take_dirty_widget_ids()
    }

    pub fn set_layout_viewport(&mut self, cols: u16, rows: u16) {
        self.runtime.set_layout_viewport(cols, rows);
        self.sync_layout_to_active_leaf();
    }

    pub fn set_layout_aspect(&mut self, aspect: f32) {
        self.runtime.set_layout_aspect(aspect);
    }

    /// Sync the Runtime's current layout to the active tile leaf's cached_layout.
    /// Call this after any operation that may change the layout (eval, widget tree, etc.)
    pub fn sync_layout_to_active_leaf(&mut self) {
        let layout = self.runtime.current_layout.clone();
        let revision = self.runtime.layout_revision();
        let leaf = self.active_leaf_mut();
        leaf.cached_layout = layout;
        leaf.layout_revision = revision;
    }

    pub fn open_scratch_buffer(&mut self, name: &str, initial: &str) -> BufferId {
        self.open_scratch_buffer_with_mode(name, initial, BufferMode::ESeqLisp)
    }

    pub fn open_scratch_buffer_with_mode(
        &mut self,
        name: &str,
        initial: &str,
        mode: BufferMode,
    ) -> BufferId {
        let id = self.alloc_buffer_id();
        let mut buffer = Buffer::new(id, name);
        buffer.set_text(initial);
        buffer.set_mode(mode);
        self.save_current_widget_tree();
        self.buffers.push(buffer);
        self.active_leaf_mut().buffer_idx = self.buffers.len() - 1;
        self.sync_runtime_context();
        self.completion = None;
        self.clear_mark();
        self.clear_widget_focus();
        id
    }

    pub fn open_file_buffer(&mut self, path: impl Into<PathBuf>) -> Result<BufferId, EditorError> {
        self.open_file_buffer_with_mode(path, BufferMode::ESeqLisp)
    }

    pub fn open_file_buffer_with_mode(
        &mut self,
        path: impl Into<PathBuf>,
        mode: BufferMode,
    ) -> Result<BufferId, EditorError> {
        let path = path.into();
        let text = std::fs::read_to_string(&path)?;
        let name = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let id = self.alloc_buffer_id();
        let mut buffer = Buffer::new(id, &name);
        buffer.set_text(&text);
        buffer.set_path(path);
        buffer.set_mode(mode);
        buffer.dirty = false;
        self.save_current_widget_tree();
        self.buffers.push(buffer);
        self.active_leaf_mut().buffer_idx = self.buffers.len() - 1;
        self.sync_runtime_context();
        self.completion = None;
        self.clear_mark();
        self.clear_widget_focus();
        Ok(id)
    }

    pub fn open_or_create_file_buffer(
        &mut self,
        path: impl Into<PathBuf>,
    ) -> Result<BufferId, EditorError> {
        self.open_or_create_file_buffer_with_mode(path, BufferMode::ESeqLisp)
    }

    pub fn open_or_create_file_buffer_with_mode(
        &mut self,
        path: impl Into<PathBuf>,
        mode: BufferMode,
    ) -> Result<BufferId, EditorError> {
        let path = path.into();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(EditorError::Io(err)),
        };
        let name = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let id = self.alloc_buffer_id();
        let mut buffer = Buffer::new(id, &name);
        buffer.set_text(&text);
        buffer.set_path(path);
        buffer.set_mode(mode);
        buffer.dirty = false;
        self.save_current_widget_tree();
        self.buffers.push(buffer);
        self.active_leaf_mut().buffer_idx = self.buffers.len() - 1;
        self.sync_runtime_context();
        self.completion = None;
        self.clear_mark();
        self.clear_widget_focus();
        Ok(id)
    }

    pub fn set_active_buffer(&mut self, id: BufferId) {
        if let Some(index) = self.buffers.iter().position(|buffer| buffer.id == id) {
            self.save_current_widget_tree();
            self.active_leaf_mut().buffer_idx = index;
            self.mark_needs_redraw();
            self.sync_runtime_context();
            self.completion = None;
            self.clear_mark();
            self.restore_buffer_widget_tree();
        }
    }

    pub fn handle_host_event(&mut self, event: HostEvent) {
        let message = match event {
            HostEvent::Status(msg) => msg,
            HostEvent::Error(msg) => format!("Error: {msg}"),
            HostEvent::CommandStarted { label } => format!("{label}..."),
            HostEvent::CommandFinished {
                label,
                success,
                message,
            } => {
                let outcome = if success { "finished" } else { "failed" };
                match message {
                    Some(message) => format!("{label} {outcome}: {message}"),
                    None => format!("{label} {outcome}"),
                }
            }
            HostEvent::CompileFinished {
                kind,
                success,
                name,
                diagnostics,
            } => {
                let label = match kind {
                    CompileKind::Instrument => "instrument",
                    CompileKind::Effect => "effect",
                };
                if success {
                    match name {
                        Some(name) => format!("Compiled {label} '{name}'"),
                        None => format!("Compiled {label}"),
                    }
                } else {
                    match diagnostics {
                        Some(diag) => format!("Compile failed ({label}): {diag}"),
                        None => format!("Compile failed ({label})"),
                    }
                }
            }
            HostEvent::BufferSaved { buffer_id, path } => {
                if let Some(buffer) = self
                    .buffers
                    .iter_mut()
                    .find(|buffer| buffer.id == buffer_id)
                {
                    buffer.set_path(path.clone());
                    buffer.dirty = false;
                }
                format!("Saved {}", path.display())
            }
        };
        self.minibuffer = Some(message);
        self.mark_needs_redraw();
        self.sync_runtime_context();
        self.completion = None;
    }

    pub fn drain_host_commands(&mut self) -> Vec<HostCommand> {
        self.runtime.drain_host_commands()
    }

    pub fn runtime_mut(&mut self) -> &mut Runtime {
        &mut self.runtime
    }

    pub fn into_runtime(self) -> Runtime {
        self.runtime
    }

    pub fn run_embedded(&mut self) -> Result<EditorExit, EditorError> {
        loop {
            if event::poll(std::time::Duration::from_millis(16))? {
                match event::read()? {
                    Event::Key(key) => self.handle_key(key),
                    Event::Mouse(mouse) => {
                        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                        self.handle_mouse(
                            mouse,
                            1,
                            1,
                            cols.saturating_sub(2),
                            rows.saturating_sub(3),
                        );
                    }
                    Event::Resize(_, _) => self.mark_needs_redraw(),
                    _ => {}
                }
            }
            if self.should_quit {
                return Ok(self.last_exit);
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.mark_needs_redraw();

        if self.handle_save_prompt_key(key) {
            return;
        }

        if self.handle_minibuffer_key(key) {
            return;
        }

        if self.handle_completion_key(key) {
            return;
        }

        if let Some(prefix) = self.pending_key.take() {
            let chord = format!("{} {}", key_str(prefix), key_str(key));
            if let Some(handler) = self.lisp_bindings.get(&chord).cloned() {
                self.call_lisp_handler(&handler);
            }
            return;
        }

        if self.binding_has_prefix(&key_str(key)) {
            self.pending_key = Some(key);
            return;
        }

        if self.handle_focused_widget_key(key) {
            return;
        }

        if self.handle_focus_key(key) {
            return;
        }

        // Check mode-specific keybindings
        if let BufferMode::Named(ref mode_name) = self.active_buffer().mode {
            if let Some(handler) = self
                .mode_registry
                .get(mode_name)
                .and_then(|mode| mode.keybindings.get(&key_str(key)))
                .cloned()
            {
                self.call_lisp_handler(&handler);
                return;
            }
        }

        if let Some(cmd) = self.builtins.get(&key).cloned() {
            self.run_command(&cmd);
            return;
        }

        if let Some(handler) = self.lisp_bindings.get(&key_str(key)).cloned() {
            self.call_lisp_handler(&handler);
            return;
        }

        match key.code {
            KeyCode::Char(c)
                if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT =>
            {
                if self.guard_read_only() {
                    return;
                }
                self.minibuffer = None;
                self.clear_mark();
                self.active_buffer_mut().insert_char(c);
                self.sync_runtime_context();
                self.refresh_completion();
            }
            KeyCode::Enter => {
                if self.guard_read_only() {
                    return;
                }
                self.completion = None;
                self.minibuffer = None;
                self.clear_mark();
                self.active_buffer_mut().insert_newline_with_indent();
                self.sync_runtime_context();
            }
            _ => {}
        }
    }

    pub fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        content_width: u16,
        content_height: u16,
    ) {
        self.handle_mouse_precise(
            mouse,
            content_col,
            content_row,
            content_width,
            content_height,
            mouse.column as f32,
            mouse.row as f32,
        );
    }

    pub fn handle_mouse_precise(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        content_width: u16,
        content_height: u16,
        precise_col: f32,
        precise_row: f32,
    ) {
        let view_mode = self.active_buffer().view_mode;
        let widgets_visible = view_mode != ViewMode::TextOnly;
        let text_visible = view_mode != ViewMode::UiOnly;

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.last_mouse_precise = Some((precise_col, precise_row));
                self.active_leaf_mut().active_widget_gesture = None;
                if widgets_visible {
                    // Try click-to-activate on focusable widgets first
                    if self.try_click_focusable_widget(mouse, content_col, content_row) {
                        return;
                    }
                    if self.try_handle_widget_double_click(
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                    ) {
                        self.remember_widget_click(
                            content_col,
                            content_row,
                            precise_col,
                            precise_row,
                        );
                        return;
                    }
                    self.begin_widget_gesture(content_col, content_row, precise_col, precise_row);
                    if self.try_handle_widget_mouse_precise(
                        mouse,
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                    ) {
                        self.remember_widget_click(
                            content_col,
                            content_row,
                            precise_col,
                            precise_row,
                        );
                        return;
                    }
                    if self
                        .widget_node_at_screen(precise_col, precise_row, content_col, content_row)
                        .is_some()
                    {
                        self.remember_widget_click(
                            content_col,
                            content_row,
                            precise_col,
                            precise_row,
                        );
                        return;
                    }
                }
                if text_visible {
                    self.handle_text_click(
                        mouse,
                        content_col,
                        content_row,
                        content_width,
                        content_height,
                    );
                }
                self.active_leaf_mut().last_widget_click = None;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if widgets_visible {
                    let previous = self
                        .last_mouse_precise
                        .unwrap_or((precise_col, precise_row));
                    if let Some(gesture) = self.active_leaf().active_widget_gesture.clone() {
                        if let Some(output) = self.dispatch_gesture_widget_mouse_event(
                            gesture,
                            mouse.kind,
                            content_col,
                            content_row,
                            precise_col,
                            precise_row,
                        ) {
                            let _ = self.apply_widget_output(Some(output));
                        }
                    } else {
                        self.try_handle_widget_drag_segment(
                            mouse,
                            content_col,
                            content_row,
                            previous,
                            (precise_col, precise_row),
                        );
                    }
                }
                self.last_mouse_precise = Some((precise_col, precise_row));
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if widgets_visible {
                    if let Some(gesture) = self.active_leaf_mut().active_widget_gesture.take() {
                        let output = self.dispatch_gesture_widget_mouse_event(
                            gesture,
                            mouse.kind,
                            content_col,
                            content_row,
                            precise_col,
                            precise_row,
                        );
                        let _ = self.apply_widget_output(output);
                    }
                }
                self.last_mouse_precise = None;
            }
            MouseEventKind::ScrollUp => {
                if widgets_visible
                    && self.try_handle_widget_mouse_precise(
                        mouse,
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                    )
                {
                    return;
                }
                if !text_visible {
                    // UI-only: scroll widget viewport
                    let leaf = self.active_leaf_mut();
                    leaf.widget_scroll_top = leaf.widget_scroll_top.saturating_sub(3);
                    self.mark_needs_redraw();
                } else if self.active_buffer().read_only && self.has_focusable_widgets() {
                    self.navigate_focus(KeyCode::Up);
                } else {
                    let buffer = self.active_buffer_mut();
                    if buffer.scroll_top > 0 {
                        buffer.scroll_top = buffer.scroll_top.saturating_sub(3);
                        if !buffer.read_only {
                            buffer.cursor.0 = buffer
                                .cursor
                                .0
                                .min(buffer.scroll_top + content_height.saturating_sub(1) as usize);
                        }
                    }
                    self.mark_needs_redraw();
                }
            }
            MouseEventKind::ScrollDown => {
                if widgets_visible
                    && self.try_handle_widget_mouse_precise(
                        mouse,
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                    )
                {
                    return;
                }
                if !text_visible {
                    // UI-only: scroll widget viewport, clamped to widget bounds
                    let max_scroll = self
                        .runtime
                        .current_layout
                        .as_ref()
                        .map(|l| {
                            ((l.rect.row + l.rect.height).ceil() as u16)
                                .saturating_sub(content_height)
                        })
                        .unwrap_or(0);
                    let leaf = self.active_leaf_mut();
                    leaf.widget_scroll_top =
                        leaf.widget_scroll_top.saturating_add(3).min(max_scroll);
                    self.mark_needs_redraw();
                } else if self.active_buffer().read_only && self.has_focusable_widgets() {
                    self.navigate_focus(KeyCode::Down);
                } else {
                    let buffer = self.active_buffer_mut();
                    let max_scroll = buffer.lines.len().saturating_sub(1);
                    buffer.scroll_top = (buffer.scroll_top + 3).min(max_scroll);
                    if !buffer.read_only && buffer.cursor.0 < buffer.scroll_top {
                        buffer.cursor.0 = buffer.scroll_top;
                    }
                    self.mark_needs_redraw();
                }
            }
            MouseEventKind::ScrollLeft => {
                if !widgets_visible
                    || !self.try_handle_widget_mouse_precise(
                        mouse,
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                    )
                {
                    if widgets_visible {
                        let leaf = self.active_leaf_mut();
                        leaf.widget_scroll_left = leaf.widget_scroll_left.saturating_sub(3);
                        self.mark_needs_redraw();
                    }
                }
            }
            MouseEventKind::ScrollRight => {
                if !widgets_visible
                    || !self.try_handle_widget_mouse_precise(
                        mouse,
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                    )
                {
                    if widgets_visible {
                        let max_scroll = self.max_horizontal_scroll(content_width);
                        let leaf = self.active_leaf_mut();
                        leaf.widget_scroll_left =
                            leaf.widget_scroll_left.saturating_add(3).min(max_scroll);
                        self.mark_needs_redraw();
                    }
                }
            }
            _ => {}
        }
    }

    /// Maximum horizontal scroll: how far right content extends past the viewport.
    fn max_horizontal_scroll(&self, viewport_width: u16) -> u16 {
        let vp = viewport_width as usize;
        // Walk the layout tree to find the rightmost edge of any descendant,
        // since the root rect is clamped to viewport width by LayoutEngine.
        let layout_width = self
            .runtime
            .current_layout
            .as_ref()
            .map(|l| max_layout_right_edge(l) as usize)
            .unwrap_or(0);
        // Include text line width only if text is visible
        let max_line = if self.active_buffer().view_mode == ViewMode::UiOnly {
            0
        } else {
            self.active_buffer()
                .lines
                .iter()
                .map(|l| l.len())
                .max()
                .unwrap_or(0)
        };
        let content_width = layout_width.max(max_line);
        content_width.saturating_sub(vp) as u16
    }

    pub fn handle_touchpad_magnify(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
        delta: f64,
    ) {
        if self.active_buffer().view_mode == ViewMode::TextOnly {
            return;
        }
        self.handle_touchpad_magnify_impl(
            content_col,
            content_row,
            precise_col,
            precise_row,
            delta,
        );
    }

    pub fn handle_touchpad_scroll(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
        delta_x: f32,
        delta_y: f32,
    ) -> bool {
        if self.active_buffer().view_mode == ViewMode::TextOnly {
            return false;
        }
        self.handle_touchpad_scroll_impl(
            content_col,
            content_row,
            precise_col,
            precise_row,
            delta_x,
            delta_y,
        )
    }

    // ── Internal methods ─────────────────────────────────────────────────────

    fn load_init(&mut self, override_source: Option<&str>) {
        let init_src = override_source
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| std::fs::read_to_string("init.lisp").unwrap_or_default());
        if init_src.trim().is_empty() {
            return;
        }
        let _ = self.runtime.eval_str(&init_src);
        self.refresh_runtime_side_effects();
        if let Some(status) = self.runtime.take_status_message() {
            self.minibuffer = Some(status);
        }
    }

    fn call_lisp_handler(&mut self, fn_name: &str) {
        if fn_name == "eval-sexp" || fn_name == "eval-buffer-command" {
            self.eval_preview_handler(fn_name);
            return;
        }
        self.sync_runtime_context();
        self.minibuffer = None;
        let code = format!("({fn_name})");
        match self.runtime.eval_str(&code) {
            Ok(Some(result)) => self.minibuffer = Some(format_value_for_minibuffer(&result)),
            Ok(None) => self.minibuffer = Some("No result".to_string()),
            Err(e) => self.minibuffer = Some(format!("Error: {e:?}")),
        }
        if let Some(status) = self.runtime.take_status_message() {
            self.minibuffer = Some(status);
        }
        self.refresh_runtime_side_effects();
        self.sync_runtime_context();
        self.completion = None;
    }

    fn eval_preview_handler(&mut self, fn_name: &str) {
        if fn_name == "eval-sexp" {
            self.start_eval_flash();
        }
        self.sync_runtime_context();
        self.minibuffer = None;

        let source = match fn_name {
            "eval-sexp" => {
                let buffer = self.active_buffer();
                sexp_at_cursor(&buffer.lines, buffer.cursor).unwrap_or_default()
            }
            "eval-buffer-command" => self.active_buffer().text(),
            _ => String::new(),
        };

        if source.trim().is_empty() {
            self.minibuffer = Some("No s-expression at cursor".to_string());
            self.completion = None;
            return;
        }

        match self.runtime.eval_str(&source) {
            Ok(Some(result)) => self.minibuffer = Some(format_value_for_minibuffer(&result)),
            Ok(None) => self.minibuffer = Some("No result".to_string()),
            Err(e) => self.minibuffer = Some(format!("Error: {e:?}")),
        }
        if let Some(status) = self.runtime.take_status_message() {
            self.minibuffer = Some(status);
        }
        self.refresh_runtime_side_effects();
        self.sync_runtime_context();
        self.completion = None;
    }

    fn save_active_buffer(&mut self) -> Result<PathBuf, EditorError> {
        let path = self.active_buffer_mut().save()?;
        self.last_exit = EditorExit::SavedAndClosed;
        Ok(path)
    }

    fn load_active_buffer(&mut self) -> Result<PathBuf, EditorError> {
        let active = self.active_buffer();
        let path = active
            .path
            .clone()
            .ok_or_else(|| EditorError::Message("buffer is not file-backed".to_string()))?;
        let mode = active.mode.clone();
        let buffer = self.active_buffer_mut();
        let text = std::fs::read_to_string(&path)?;
        buffer.set_text(&text);
        buffer.set_path(path.clone());
        buffer.set_mode(mode);
        buffer.dirty = false;
        Ok(path)
    }

    fn sync_runtime_context(&mut self) {
        let active = self.active_buffer();
        let buffer_names: Vec<String> = self.buffers.iter().map(|b| b.name.clone()).collect();
        let mut shared = self.runtime.shared.borrow_mut();
        shared.current_buffer_id = Some(active.id);
        shared.current_buffer_name = active.name.clone();
        shared.current_buffer_path = active.path.clone();
        shared.current_buffer_text = active.text();
        shared.current_sexp = sexp_at_cursor(&active.lines, active.cursor);
        shared.current_buffer_read_only = active.read_only;
        shared.current_buffer_mode = active.mode.name().to_string();
        shared.current_line_number = active.cursor.0 + 1;
        shared.current_line_text = active
            .lines
            .get(active.cursor.0)
            .cloned()
            .unwrap_or_default();
        shared.buffer_names = buffer_names;
        shared.current_view_mode = active.view_mode.label().to_string();
    }

    fn handle_completion_key(&mut self, key: KeyEvent) -> bool {
        let Some(completion) = self.completion.as_mut() else {
            return false;
        };

        match key.code {
            KeyCode::Up => {
                if completion.selected > 0 {
                    completion.selected -= 1;
                }
                completion.ensure_visible();
                self.mark_needs_redraw();
                true
            }
            KeyCode::Down => {
                if completion.selected + 1 < completion.items.len() {
                    completion.selected += 1;
                }
                completion.ensure_visible();
                self.mark_needs_redraw();
                true
            }
            KeyCode::Tab | KeyCode::Enter => {
                self.accept_completion();
                true
            }
            KeyCode::Esc => {
                self.completion = None;
                self.mark_needs_redraw();
                true
            }
            _ => false,
        }
    }

    fn accept_completion(&mut self) {
        let Some(completion) = self.completion.clone() else {
            return;
        };
        let Some(item) = completion.items.get(completion.selected) else {
            return;
        };
        let buffer = self.active_buffer_mut();
        let row = buffer.cursor.0;
        let end_col = buffer.cursor.1.min(buffer.lines[row].len());
        buffer.lines[row].replace_range(completion.start_col..end_col, &item.label);
        buffer.cursor.1 = completion.start_col + item.label.len();
        buffer.dirty = true;
        self.completion = None;
        self.sync_runtime_context();
    }

    fn refresh_completion(&mut self) {
        if self.save_prompt.is_some() {
            self.completion = None;
            return;
        }
        let symbols = self.runtime.completion_symbols();
        let metadata = self.runtime.completion_metadata();
        let previous = self
            .completion
            .as_ref()
            .and_then(|state| state.items.get(state.selected))
            .map(|item| item.label.clone());
        self.completion = completion_match(
            &self.active_buffer().mode,
            self.active_buffer(),
            &symbols,
            &metadata,
        )
        .map(
            |CompletionMatch {
                 start_col, items, ..
             }| {
                let selected = previous
                    .as_ref()
                    .and_then(|label| items.iter().position(|item| item.label == *label))
                    .unwrap_or(0);
                CompletionState {
                    start_col,
                    items,
                    selected,
                    scroll: 0,
                }
            },
        )
        .map(|mut state| {
            state.ensure_visible();
            state
        });
    }

    fn alloc_buffer_id(&mut self) -> BufferId {
        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        id
    }

    fn binding_has_prefix(&self, prefix: &str) -> bool {
        self.lisp_bindings.keys().any(|binding| {
            binding
                .strip_prefix(prefix)
                .map(|rest| rest.starts_with(' '))
                .unwrap_or(false)
        })
    }

    fn needs_save_as_prompt(&self) -> bool {
        self.active_buffer()
            .path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|stem| stem == "untitled")
            .unwrap_or(true)
    }

    fn should_prompt_on_quit(&self) -> bool {
        let buffer = self.active_buffer();
        buffer.dirty && buffer.path.is_some()
    }

    fn open_save_prompt(&mut self, quit_after_save: bool) {
        let default_name = self
            .active_buffer()
            .path
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|stem| {
                let stem = stem.to_string_lossy().to_string();
                if stem == "untitled" { None } else { Some(stem) }
            })
            .unwrap_or_default();
        self.save_prompt = Some(SavePrompt {
            input: default_name,
            quit_after_save,
        });
        self.sync_runtime_context();
    }

    fn handle_save_prompt_key(&mut self, key: KeyEvent) -> bool {
        let Some(prompt) = self.save_prompt.as_mut() else {
            return false;
        };

        match key.code {
            KeyCode::Esc => {
                self.save_prompt = None;
                self.minibuffer = Some("Save cancelled".to_string());
            }
            KeyCode::Char('d') if prompt.quit_after_save => {
                self.save_prompt = None;
                self.minibuffer = Some("Discarded changes".to_string());
                self.should_quit = true;
                self.last_exit = EditorExit::Closed;
            }
            KeyCode::Char('q')
                if prompt.quit_after_save && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.save_prompt = None;
                self.minibuffer = Some("Discarded changes".to_string());
                self.should_quit = true;
                self.last_exit = EditorExit::Closed;
            }
            KeyCode::Enter => {
                let quit_after_save = prompt.quit_after_save;
                let input = prompt.input.trim().to_string();
                if input.is_empty() {
                    self.minibuffer = Some("Filename required".to_string());
                    return true;
                }
                let mut target = self
                    .active_buffer()
                    .path
                    .as_ref()
                    .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
                    .unwrap_or_default();
                let filename = if input.ends_with(".lisp") {
                    input
                } else {
                    format!("{input}.lisp")
                };
                target.push(filename);
                match self.active_buffer_mut().save_as(target) {
                    Ok(path) => {
                        self.minibuffer = Some(format!("Saved {}", path.display()));
                        self.save_prompt = None;
                        if quit_after_save {
                            self.should_quit = true;
                            self.last_exit = EditorExit::SavedAndClosed;
                        }
                    }
                    Err(error) => {
                        self.minibuffer = Some(format!("Error: {error}"));
                    }
                }
            }
            KeyCode::Backspace => {
                prompt.input.pop();
            }
            KeyCode::Char(c)
                if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT =>
            {
                prompt.input.push(c);
            }
            _ => {}
        }
        self.mark_needs_redraw();
        self.sync_runtime_context();
        true
    }

    fn refresh_runtime_side_effects(&mut self) {
        self.lisp_bindings = self.default_lisp_bindings.clone();
        self.lisp_bindings.extend(self.runtime.lisp_bindings());

        if let Some(read_only) = self.runtime.take_pending_set_read_only() {
            self.active_buffer_mut().read_only = read_only;
        }

        // Process mode definitions
        for (name, read_only, on_enter) in self.runtime.take_pending_mode_defs() {
            self.mode_registry.insert(
                name.clone(),
                MajorMode {
                    name,
                    read_only,
                    keybindings: HashMap::new(),
                    on_enter,
                },
            );
        }

        // Process mode keybindings
        for (mode_name, key, handler) in self.runtime.take_pending_mode_bindings() {
            if let Some(mode) = self.mode_registry.get_mut(&mode_name) {
                mode.keybindings.insert(key, handler);
            }
        }

        // Process buffer operations first (create/switch must happen before set-mode)
        if let Some(path) = self.runtime.take_pending_open_file() {
            match self.open_file_buffer(&path) {
                Ok(_) => {
                    self.minibuffer = Some(format!("Opened {path}"));
                    self.clear_widget_focus();
                }
                Err(e) => self.minibuffer = Some(format!("Error: {e:?}")),
            }
        }

        if let Some(name) = self.runtime.take_pending_create_buffer() {
            self.open_scratch_buffer(&name, "");
        }

        if let Some(name) = self.runtime.take_pending_switch_buffer() {
            if let Some(idx) = self.buffers.iter().position(|b| b.name == name) {
                self.save_current_widget_tree();
                self.active_leaf_mut().buffer_idx = idx;
                self.mark_needs_redraw();
                self.sync_runtime_context();
                self.completion = None;
                self.clear_mark();
                self.restore_buffer_widget_tree();
            }
        }

        // Process set-buffer-mode (after buffer creation so it targets the new buffer)
        if let Some(mode_name) = self.runtime.take_pending_set_mode() {
            let mode_def = self.mode_registry.get(&mode_name).cloned();
            let buffer = self.active_buffer_mut();
            buffer.mode = BufferMode::Named(mode_name.clone());
            if let Some(mode_def) = &mode_def {
                buffer.read_only = mode_def.read_only;
            }
            // Call on_enter hook
            if let Some(on_enter) = mode_def.as_ref().and_then(|m| m.on_enter.clone()) {
                self.sync_runtime_context();
                let code = format!("({on_enter})");
                let _ = self.runtime.eval_str(&code);
                if let Some(status) = self.runtime.take_status_message() {
                    self.minibuffer = Some(status);
                }
            }
            // Auto-focus first focusable widget if mode has them
            self.auto_focus_first_widget();
        }

        if let Some(text) = self.runtime.take_pending_set_text() {
            self.active_buffer_mut().set_text(&text);
        }

        if let Some(lines) = self.runtime.take_pending_set_lines() {
            let buffer = self.active_buffer_mut();
            buffer.lines = if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            };
            buffer.cursor = (0, 0);
            buffer.scroll_top = 0;
            buffer.revision = buffer.revision.wrapping_add(1);
        }

        if let Some(line) = self.runtime.take_pending_goto_line() {
            let buffer = self.active_buffer_mut();
            let row = line
                .saturating_sub(1)
                .min(buffer.lines.len().saturating_sub(1));
            buffer.cursor = (row, 0);
        }

        // Process widget tree rendering (stored per-buffer)
        if let Some(tree) = self.runtime.take_pending_widget_tree() {
            match tree {
                Value::Nil | Value::Bool(false) => {
                    self.active_buffer_mut().widget_tree = None;
                    self.runtime.clear_layout_effects();
                    self.active_leaf_mut().focused_widget_id = None;
                }
                tree => {
                    self.active_buffer_mut().widget_tree = Some(tree.clone());
                    self.runtime.set_widget_tree(tree);
                    self.auto_focus_first_widget();
                }
            }
        }

        if let Some(path) = self.runtime.take_pending_save_as() {
            match self.active_buffer_mut().save_as(path) {
                Ok(path) => self.minibuffer = Some(format!("Saved {}", path.display())),
                Err(error) => self.minibuffer = Some(format!("Error: {error}")),
            }
        } else if self.runtime.take_pending_save() {
            match self.save_active_buffer() {
                Ok(path) => self.minibuffer = Some(format!("Saved {}", path.display())),
                Err(error) => self.minibuffer = Some(format!("Error: {error:?}")),
            }
        } else if self.runtime.take_pending_load() {
            match self.load_active_buffer() {
                Ok(path) => self.minibuffer = Some(format!("Loaded {}", path.display())),
                Err(error) => self.minibuffer = Some(format!("Error: {error:?}")),
            }
        }
        self.completion = None;

        // ── Process tiling operations ────────────────────────────────────────
        if self.runtime.take_pending_split_right() {
            // Create a new tile showing the *scratch* buffer (or first available)
            let new_buf_idx = self.find_or_create_scratch_buffer();
            self.split_active_tile(SplitDir::Vertical, new_buf_idx);
        }

        if self.runtime.take_pending_split_below() {
            let new_buf_idx = self.find_or_create_scratch_buffer();
            self.split_active_tile(SplitDir::Horizontal, new_buf_idx);
        }

        if self.runtime.take_pending_delete_window() {
            if !self.delete_active_tile() {
                self.minibuffer = Some("Cannot delete the only window".to_string());
            }
        }

        if self.runtime.take_pending_delete_other_windows() {
            self.delete_other_tiles();
        }

        if self.runtime.take_pending_other_window() {
            self.cycle_active_tile();
        }

        if let Some(name) = self.runtime.take_pending_set_window_buffer() {
            if let Some(idx) = self.buffers.iter().position(|b| b.name == name) {
                self.save_current_widget_tree();
                self.active_leaf_mut().buffer_idx = idx;
                self.sync_runtime_context();
                self.restore_buffer_widget_tree();
            } else {
                self.minibuffer = Some(format!("No buffer named '{name}'"));
            }
        }

        if self.runtime.take_pending_window_hide_status() {
            let leaf = self.active_leaf_mut();
            leaf.show_status = !leaf.show_status;
            self.mark_needs_redraw();
        }

        if let Some(delta) = self.runtime.take_pending_resize_window() {
            if let Some(split) = self.tile_root.find_parent_split(self.active_tile) {
                split.ratio = (split.ratio + delta as f32).clamp(0.1, 0.9);
                self.mark_needs_redraw();
            }
        }

        if self.runtime.take_pending_cycle_view_mode() {
            let new_mode = self.active_buffer().view_mode.cycle();
            self.active_buffer_mut().view_mode = new_mode;
            self.minibuffer = Some(format!("view: {}", new_mode.label()));
            // Reset scroll when entering UI-only mode so text scroll doesn't
            // offset the widget viewport past its content.
            if new_mode == ViewMode::UiOnly {
                self.active_leaf_mut().widget_scroll_top = 0;
            }
            self.mark_needs_redraw();
        }

        self.sync_layout_to_active_leaf();
    }

    /// Choose which buffer to display in a newly split tile.
    /// Prefers an existing undisplayed buffer; creates a scratch if all are taken.
    fn find_or_create_scratch_buffer(&mut self) -> usize {
        let tile_buffer_idxs: Vec<usize> = self
            .tile_root
            .leaf_ids()
            .iter()
            .filter_map(|id| self.tile_root.find_leaf(*id).map(|l| l.buffer_idx))
            .collect();

        // Pick the first buffer not currently shown in any tile
        for (idx, _) in self.buffers.iter().enumerate() {
            if !tile_buffer_idxs.contains(&idx) {
                return idx;
            }
        }

        // All buffers visible — create a new scratch
        let id = self.alloc_buffer_id();
        let buffer = Buffer::new(id, "*scratch*");
        self.buffers.push(buffer);
        self.buffers.len() - 1
    }

    fn start_eval_flash(&mut self) {
        let buffer = self.active_buffer();
        let Some(range) = innermost_sexp_range_at_cursor(&buffer.lines, buffer.cursor) else {
            self.eval_flash = None;
            return;
        };
        self.eval_flash = Some(SExpFlash {
            buffer_id: buffer.id,
            range,
            expires_at: Instant::now() + Duration::from_millis(350),
        });
    }

    fn clear_mark(&mut self) {
        self.mark = None;
    }

    fn apply_widget_output(&mut self, output: Option<crate::widget_render::EventOutput>) -> bool {
        let Some(output) = output else {
            return false;
        };
        let result = self.runtime.invoke(output.callback, output.args);
        if let Some(status) = self.runtime.take_status_message() {
            self.minibuffer = Some(status);
        } else if let Err(error) = result {
            self.minibuffer = Some(format!("Error: {error:?}"));
        } else {
            self.minibuffer = None;
        }
        self.refresh_runtime_side_effects();
        self.completion = None;
        self.mark_needs_redraw();
        true
    }

    fn copy_active_region(&mut self) -> bool {
        let Some((start, end)) = self.active_region_range() else {
            return false;
        };
        let text = self.active_buffer().slice_range(start, end);
        self.kill_ring.push(text);
        true
    }

    fn kill_active_region(&mut self) -> bool {
        let Some((start, end)) = self.active_region_range() else {
            return false;
        };
        let text = self.active_buffer().slice_range(start, end);
        self.kill_ring.push(text);
        self.active_buffer_mut().delete_range(start, end);
        self.clear_mark();
        true
    }

    fn guard_read_only(&mut self) -> bool {
        if self.active_buffer().read_only {
            self.minibuffer = Some("Buffer is read-only".to_string());
            true
        } else if self.active_buffer().view_mode == ViewMode::UiOnly {
            true
        } else {
            false
        }
    }
}

impl CompletionState {
    const VISIBLE_ROWS: usize = 8;

    fn ensure_visible(&mut self) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + Self::VISIBLE_ROWS {
            self.scroll = self.selected + 1 - Self::VISIBLE_ROWS;
        }
    }
}

/// Find the rightmost edge (col + width) of any node in the layout tree.
fn max_layout_right_edge(node: &crate::layout::LayoutNode) -> u16 {
    crate::ui::hit::max_extent(node, 1.0).0
}

fn normalize_region(a: (usize, usize), b: (usize, usize)) -> ((usize, usize), (usize, usize)) {
    if a < b { (a, b) } else { (b, a) }
}

fn filter_candidates(candidates: &[String], input: &str) -> Vec<String> {
    if input.is_empty() {
        return candidates.to_vec();
    }
    let lower = input.to_ascii_lowercase();
    candidates
        .iter()
        .filter(|c| c.to_ascii_lowercase().contains(&lower))
        .cloned()
        .collect()
}

fn format_value_for_minibuffer(value: &Value) -> String {
    let mut s = format_lisp_value(value);
    if s.len() > 240 {
        s.truncate(237);
        s.push_str("...");
    }
    s
}

// ── Test helpers (used by tests via super::) ─────────────────────────────

#[cfg(test)]
fn get_map_field_number(value: &Value, key: &str) -> Option<f64> {
    let Value::Map(map) = value else {
        return None;
    };
    match &*map.get(key)?.borrow() {
        Value::Number(n) => Some(*n),
        _ => None,
    }
}

#[cfg(test)]
fn get_map_field_keyword(value: &Value, key: &str) -> Option<String> {
    let Value::Map(map) = value else {
        return None;
    };
    match &*map.get(key)?.borrow() {
        Value::Keyword(k) => Some(k.clone()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
fn get_first_list_number(value: &Value, key: &str) -> Option<f64> {
    let Value::Map(map) = value else {
        return None;
    };
    let list_val = map.get(key)?;
    let Value::List(items) = &*list_val.borrow() else {
        return None;
    };
    match &*items.first()?.borrow() {
        Value::Number(n) => Some(*n),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Editor, EditorConfig, key_str};
    use crate::host::HostCommand;
    use crate::mode::BufferMode;
    use crate::runtime::Runtime;
    use crate::tile::SplitDir;
    use crate::vm::Value;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::fs;
    use std::rc::Rc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file_path(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("eseqlisp-{name}-{unique}.lisp"))
    }

    fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    // Tests are included from the original file — they reference super:: helpers above.
    include!("tests.rs");
}
