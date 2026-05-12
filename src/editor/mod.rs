mod commands;
mod minibuffer;
mod natives;
mod widget_focus;
mod widget_interaction;

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};

use crate::buffer::{Buffer, debug_widget_tree_summary};
use crate::host::{BufferId, CompileKind, HostCommand, HostEvent};
use crate::layout::Rect;
use crate::mode::{
    BufferMode, CompletionItem, CompletionMatch, TokenSpan, completion_match,
    has_completion_prefix, highlight_lines,
};
use crate::runtime::Runtime;
use crate::text::{innermost_sexp_range_at_cursor, sexp_at_cursor};
use crate::tile::{SplitDir, TileId, TileLeaf, TileNode, split_ratio_for_point};
use crate::vm::{EffectTarget, PendingUiUpdate, Value, format_lisp_value};
use crate::widget_render::WidgetCursor;
use commands::key_str;
use natives::register_editor_natives;

const TILE_GAP_PX_PER_UNIT: f32 = 15.0;

fn metal_tile_content_viewport(
    rect: &Rect,
    show_status: bool,
    show_border: bool,
    border_width_px: f32,
    cell_w: f32,
    cell_h: f32,
) -> (u16, u16) {
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
    let cols = (rect.width - border_inset_px * 2.0 / cell_w.max(1.0))
        .max(0.0)
        .floor() as u16;
    let content_height =
        rect.height - border_inset_px * 2.0 / cell_h.max(1.0) - if show_status { 1.0 } else { 0.0 };
    let rows = content_height.max(0.0).floor() as u16;
    (cols.max(1), rows.max(1))
}

fn metal_tile_content_viewport_height_exact(
    rect: &Rect,
    show_status: bool,
    show_border: bool,
    border_width_px: f32,
    cell_w: f32,
    cell_h: f32,
) -> f32 {
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
    (rect.height - border_inset_px * 2.0 / cell_h.max(1.0) - if show_status { 1.0 } else { 0.0 })
        .max(0.0)
}

fn widget_only_scratch_buffer_should_show_ui(buffer: &Buffer) -> bool {
    buffer.widget_tree.is_some()
        && buffer.path.is_none()
        && buffer.name.starts_with('*')
        && buffer.name.ends_with('*')
        && buffer.text().trim().is_empty()
}

fn debug_ui_updates_enabled() -> bool {
    std::env::var("ESEQLISP_DEBUG_UI_UPDATES")
        .ok()
        .is_some_and(|value| value == "1" || value == "true")
}

fn trace_ui_invalidation_enabled() -> bool {
    std::env::var_os("ESEQLISP_TRACE_UI").is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewMode {
    Both,
    UiOnly,
    TextOnly,
}

impl ViewMode {
    pub fn toggle_primary(self) -> Self {
        match self {
            ViewMode::TextOnly => ViewMode::UiOnly,
            ViewMode::UiOnly | ViewMode::Both => ViewMode::TextOnly,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Both => "both",
            ViewMode::UiOnly => "ui",
            ViewMode::TextOnly => "text",
        }
    }

    pub fn from_label(s: &str) -> Option<Self> {
        match s {
            "ui" => Some(ViewMode::UiOnly),
            "text" => Some(ViewMode::TextOnly),
            "both" => Some(ViewMode::Both),
            _ => None,
        }
    }
}

#[derive(Default, Clone)]
pub struct EditorConfig {
    pub init_source: Option<String>,
    pub vim_mode: bool,
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
type VisibleBindingLayoutSignature = Vec<(TileId, u64, usize)>;

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
    FindFile {
        input: String,
        selected: usize,
    },
    Search {
        state: SearchState,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchDirection {
    Forward,
    Backward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SearchMatch {
    start: (usize, usize),
    end: (usize, usize),
}

#[derive(Debug, Clone)]
struct SearchState {
    input: String,
    origin: (usize, usize),
    direction: SearchDirection,
    current_match: Option<SearchMatch>,
    failed: bool,
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
struct DefinitionLocation {
    path: Option<PathBuf>,
    buffer_id: Option<BufferId>,
    cursor: (usize, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimInputMode {
    Normal,
    Insert,
}

#[derive(Debug, Clone)]
enum VimPending {
    Key(KeyEvent),
    Replace,
    Operator { op: char, count: String },
}

#[derive(Debug, Clone)]
struct TextUndoSnapshot {
    buffer_id: BufferId,
    lines: Vec<String>,
    cursor: (usize, usize),
    dirty: bool,
}

#[derive(Debug, Clone)]
pub struct MajorMode {
    pub name: String,
    pub read_only: bool,
    pub keybindings: HashMap<String, String>,
    pub on_enter: Option<String>,
    pub on_key: Option<String>,
}

pub struct Editor {
    pub buffers: Vec<Buffer>,
    pub tile_root: TileNode,
    pub active_tile: TileId,
    next_tile_id: TileId,
    pub minibuffer: Option<String>,
    minibuffer_expires_at: Option<Instant>,

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
    jump_stack: Vec<Mark>,
    active_text_drag_anchor: Option<Mark>,
    kill_ring: Vec<String>,
    minibuffer_input: Option<MinibufferMode>,
    mode_registry: HashMap<String, MajorMode>,
    vim_enabled: bool,
    vim_input_mode: VimInputMode,
    pending_vim: Option<VimPending>,
    vim_linewise_yank: Option<Vec<String>>,
    undo_stack: Vec<TextUndoSnapshot>,
    redo_stack: Vec<TextUndoSnapshot>,
    typing_undo_buffer_id: Option<BufferId>,
    /// Cached tile rects, recomputed when tiles change or viewport resizes.
    cached_tile_rects: Vec<(TileId, Rect)>,
    /// Outer margin around the tiled layout, in cell units.
    tile_outer_gap: f32,
    visible_binding_layout_signature: Option<VisibleBindingLayoutSignature>,
    widget_cursor: WidgetCursor,
    suppress_mouse_until_left_up: bool,
    pointer_drag_started_on_slider: bool,
    last_slider_drag_widget_id: Option<u64>,
    #[cfg(test)]
    test_clipboard: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct TileResizeDrag {
    split_id: TileId,
    dir: SplitDir,
    area: Rect,
}

impl Editor {
    fn trace_ui_tree_enabled_for(&self, buffer_name: &str) -> bool {
        std::env::var("ESEQLISP_TRACE_UI_TREE")
            .ok()
            .is_some_and(|filter| filter == "*" || filter == buffer_name)
    }

    fn trace_ui_tree_event_with(
        &self,
        buffer_name: &str,
        stage: &str,
        detail: impl FnOnce() -> String,
    ) {
        if self.trace_ui_tree_enabled_for(buffer_name) {
            eprintln!("[ui-tree][{buffer_name}] {stage} {}", detail());
        }
    }

    pub fn new(mut runtime: Runtime, config: EditorConfig) -> Self {
        register_editor_natives(&mut runtime);

        let mut editor = Editor {
            buffers: vec![Buffer::new(0, "*scratch*")],
            tile_root: TileNode::Leaf(TileLeaf::new(0, 0)),
            active_tile: 0,
            next_tile_id: 1,
            minibuffer: None,
            minibuffer_expires_at: None,
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
            jump_stack: vec![],
            active_text_drag_anchor: None,
            kill_ring: vec![],
            minibuffer_input: None,
            mode_registry: HashMap::new(),
            vim_enabled: config.vim_mode,
            vim_input_mode: if config.vim_mode {
                VimInputMode::Normal
            } else {
                VimInputMode::Insert
            },
            pending_vim: None,
            vim_linewise_yank: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            typing_undo_buffer_id: None,
            cached_tile_rects: vec![],
            tile_outer_gap: 0.0,
            visible_binding_layout_signature: None,
            widget_cursor: WidgetCursor::Default,
            suppress_mouse_until_left_up: false,
            pointer_drag_started_on_slider: false,
            last_slider_drag_widget_id: None,
            #[cfg(test)]
            test_clipboard: None,
        };
        editor.bind_defaults();
        editor.load_init(config.init_source.as_deref());
        editor.refresh_runtime_side_effects();
        editor.sync_runtime_context();
        editor
    }

    pub fn widget_cursor(&self) -> WidgetCursor {
        self.widget_cursor
    }

    pub fn vim_status_label(&self) -> Option<&'static str> {
        if !self.vim_applies_to_active_buffer() {
            return None;
        }
        Some(match self.vim_input_mode {
            VimInputMode::Normal => "NORMAL",
            VimInputMode::Insert => "INSERT",
        })
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
        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
        let horizontal_margin = (self.tile_outer_gap.max(0.0) * TILE_GAP_PX_PER_UNIT
            / cell_w.max(1.0))
        .min(total_width as f32 * 0.5);
        let area = Rect {
            row: 0.0,
            col: horizontal_margin,
            width: (total_width as f32 - horizontal_margin * 2.0).max(0.0),
            // Reserve 1 row for global status bar
            height: (total_height as f32 - 1.0).max(0.0),
        };
        // Enforce min-size constraints before computing rects
        Self::enforce_min_sizes_node(&mut self.tile_root, area, cell_w, cell_h);
        let old_rects = std::mem::take(&mut self.cached_tile_rects);
        self.cached_tile_rects =
            self.tile_root
                .compute_rects(area, TILE_GAP_PX_PER_UNIT, cell_w, cell_h);
        let viewport_heights: Vec<(TileId, f32)> = self
            .cached_tile_rects
            .iter()
            .filter_map(|(tile_id, rect)| {
                let leaf = self.tile_root.find_leaf(*tile_id)?;
                let buffer = self.buffers.get(leaf.buffer_idx)?;
                let show_status = leaf.show_status || buffer.view_mode != ViewMode::UiOnly;
                Some((
                    *tile_id,
                    metal_tile_content_viewport_height_exact(
                        rect,
                        show_status,
                        leaf.show_border,
                        leaf.border_width_px,
                        cell_w,
                        cell_h,
                    ),
                ))
            })
            .collect();
        for (tile_id, viewport_height) in viewport_heights {
            if let Some(leaf) = self.tile_root.find_leaf_mut(tile_id) {
                leaf.widget_viewport_height = viewport_height;
            }
        }
        // If rects changed, invalidate all inactive tile layouts so they recompute
        if old_rects != self.cached_tile_rects {
            let mut buf_indices: Vec<usize> = Vec::new();
            for id in self.tile_root.leaf_ids() {
                if id == self.active_tile {
                    continue;
                }
                if let Some(leaf) = self.tile_root.find_leaf_mut(id) {
                    leaf.cached_inactive_frame = None;
                    if !buf_indices.contains(&leaf.buffer_idx) {
                        buf_indices.push(leaf.buffer_idx);
                    }
                }
            }
            for buf_idx in buf_indices {
                self.refresh_inactive_tile_layouts_for_buffer(buf_idx);
            }
        }
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
        self.switch_active_tile_with_viewport(new_tile, None);
    }

    /// Switch the active tile and restore its widget layout for a known viewport.
    pub fn switch_active_tile_with_viewport(
        &mut self,
        new_tile: TileId,
        viewport: Option<(u16, u16)>,
    ) {
        if new_tile == self.active_tile {
            return;
        }
        let Some(target_leaf) = self.tile_root.find_leaf(new_tile) else {
            return;
        };
        let cached_layout = target_leaf.cached_layout.clone();
        let layout_revision = target_leaf.layout_revision;
        self.save_current_widget_tree();
        self.active_tile = new_tile;
        self.sync_runtime_context();
        self.restore_buffer_widget_tree_with_cached_layout(
            cached_layout,
            viewport,
            layout_revision,
        );
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
            self.refresh_inactive_tile_layouts_for_buffer(new_buffer_idx);
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

    /// Apply a declarative layout spec, replacing the entire tile tree.
    pub fn apply_layout_spec(&mut self, spec: crate::runtime::LayoutSpec) {
        use crate::buffer::Buffer as Buf;
        use crate::runtime::LayoutSpec;
        use crate::tile::TileSplit;

        let fallback_buf = self.active_leaf().buffer_idx;
        let outer_gap = match &spec {
            LayoutSpec::Rows { gap, .. } | LayoutSpec::Cols { gap, .. } => *gap,
            LayoutSpec::Buffer { .. } => 0.0,
        };

        fn build(
            spec: LayoutSpec,
            bufs: &[Buf],
            fallback: usize,
            next_id: &mut TileId,
        ) -> TileNode {
            match spec {
                LayoutSpec::Buffer {
                    name,
                    hide_status,
                    borderless,
                    border_width_px,
                    border_radius_px,
                    background_color,
                    background_color_name,
                    min_width,
                    min_height,
                    max_width,
                    max_height,
                } => {
                    let buf_idx = bufs.iter().position(|b| b.name == name).unwrap_or(fallback);
                    let id = *next_id;
                    *next_id += 1;
                    let mut leaf = TileLeaf::new(id, buf_idx);
                    leaf.show_status = !hide_status;
                    leaf.show_border = !borderless;
                    leaf.border_width_px = border_width_px;
                    leaf.border_radius_px = border_radius_px;
                    leaf.background_color = background_color;
                    leaf.background_color_name = background_color_name;
                    leaf.min_width = min_width;
                    leaf.min_height = min_height;
                    leaf.max_width = max_width;
                    leaf.max_height = max_height;
                    TileNode::Leaf(leaf)
                }
                LayoutSpec::Rows { gap, panes } => {
                    build_split(panes, SplitDir::Horizontal, gap, bufs, fallback, next_id)
                }
                LayoutSpec::Cols { gap, panes } => {
                    build_split(panes, SplitDir::Vertical, gap, bufs, fallback, next_id)
                }
            }
        }

        fn build_split(
            panes: Vec<(f32, LayoutSpec)>,
            dir: SplitDir,
            gap: f32,
            bufs: &[Buf],
            fallback: usize,
            next_id: &mut TileId,
        ) -> TileNode {
            assert!(!panes.is_empty());
            if panes.len() == 1 {
                return build(panes.into_iter().next().unwrap().1, bufs, fallback, next_id);
            }
            let mut iter = panes.into_iter();
            let (ratio, first_spec) = iter.next().unwrap();
            let rest: Vec<(f32, LayoutSpec)> = iter.collect();

            let child_a = build(first_spec, bufs, fallback, next_id);
            let child_b = if rest.len() == 1 {
                build(rest.into_iter().next().unwrap().1, bufs, fallback, next_id)
            } else {
                let rest_total: f32 = rest.iter().map(|(r, _)| r).sum();
                let rescaled: Vec<(f32, LayoutSpec)> = if rest_total > 0.0 {
                    rest.into_iter().map(|(r, s)| (r / rest_total, s)).collect()
                } else {
                    rest
                };
                build_split(rescaled, dir, gap, bufs, fallback, next_id)
            };

            let split_id = *next_id;
            *next_id += 1;
            TileNode::Split(TileSplit {
                id: split_id,
                dir,
                ratio,
                gap,
                a: Box::new(child_a),
                b: Box::new(child_b),
            })
        }

        let new_root = build(spec, &self.buffers, fallback_buf, &mut self.next_tile_id);
        self.tile_root = new_root;
        self.tile_outer_gap = outer_gap;
        // Enforce min-size constraints on initial ratios
        self.enforce_min_sizes_recursive();
        // Set active tile to first leaf
        let ids = self.tile_root.leaf_ids();
        if !ids.is_empty() {
            self.active_tile = ids[0];
        }
        self.sync_runtime_context();
        self.restore_buffer_widget_tree();
        self.mark_needs_redraw();
    }

    /// Walk the tile tree and clamp split ratios to respect min-width/min-height.
    fn enforce_min_sizes_recursive(&mut self) {
        let rects = self.cached_tile_rects.clone();
        if rects.is_empty() {
            return;
        }
        // Get the total area from the union of all tile rects
        let total_area = {
            let mut min_col = f32::MAX;
            let mut min_row = f32::MAX;
            let mut max_col = 0.0f32;
            let mut max_row = 0.0f32;
            for (_, r) in &rects {
                min_col = min_col.min(r.col);
                min_row = min_row.min(r.row);
                max_col = max_col.max(r.col + r.width);
                max_row = max_row.max(r.row + r.height);
            }
            Rect {
                col: min_col,
                row: min_row,
                width: max_col - min_col,
                height: max_row - min_row,
            }
        };
        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
        Self::enforce_min_sizes_node(&mut self.tile_root, total_area, cell_w, cell_h);
    }

    fn enforce_min_sizes_node(node: &mut TileNode, area: Rect, cell_w: f32, cell_h: f32) {
        let TileNode::Split(split) = node else { return };
        let total = match split.dir {
            SplitDir::Vertical => area.width,
            SplitDir::Horizontal => area.height,
        };
        if total > 0.0 {
            let a_min = match split.dir {
                SplitDir::Vertical => split.a.min_width(),
                SplitDir::Horizontal => split.a.min_height(),
            };
            let b_min = match split.dir {
                SplitDir::Vertical => split.b.min_width(),
                SplitDir::Horizontal => split.b.min_height(),
            };
            let a_max = match split.dir {
                SplitDir::Vertical => split.a.max_width(),
                SplitDir::Horizontal => split.a.max_height(),
            };
            let b_max = match split.dir {
                SplitDir::Vertical => split.b.max_width(),
                SplitDir::Horizontal => split.b.max_height(),
            };
            // Enforce minimums
            if a_min > 0.0 {
                split.ratio = split.ratio.max(a_min / total);
            }
            if b_min > 0.0 {
                split.ratio = split.ratio.min(1.0 - b_min / total);
            }
            // Enforce maximums
            if a_max < f32::MAX {
                split.ratio = split.ratio.min(a_max / total);
            }
            if b_max < f32::MAX {
                split.ratio = split.ratio.max(1.0 - b_max / total);
            }
        }
        let gap =
            crate::tile::gap_to_cells(split.dir, split.gap, TILE_GAP_PX_PER_UNIT, cell_w, cell_h);
        let (a_rect, b_rect) = crate::tile::split_rect(area, split.dir, split.ratio, gap);
        Self::enforce_min_sizes_node(&mut split.a, a_rect, cell_w, cell_h);
        Self::enforce_min_sizes_node(&mut split.b, b_rect, cell_w, cell_h);
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
        if crate::widget_render::overlay_widget_id().is_some()
            && matches!(
                mouse.kind,
                MouseEventKind::Down(MouseButton::Left)
                    | MouseEventKind::Drag(MouseButton::Left)
                    | MouseEventKind::Up(MouseButton::Left)
            )
        {
            let tile_id = self.active_tile;
            let Some((content_col, content_row, content_width, content_height)) =
                self.tile_content_area(tile_id, border_inset)
            else {
                return;
            };
            self.route_pointer_event_to_tile(tile_id, border_inset, true, |editor| {
                let (event_col, event_row) = editor
                    .tile_content_precise_event_position(
                        tile_id,
                        border_inset,
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                    )
                    .unwrap_or((precise_col, precise_row));
                editor.handle_mouse_precise(
                    mouse,
                    content_col,
                    content_row,
                    content_width,
                    content_height,
                    event_col,
                    event_row,
                );
            });
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.suppress_mouse_until_left_up = true;
            }
            return;
        }

        if self.suppress_mouse_until_left_up {
            if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
                self.suppress_mouse_until_left_up = false;
            }
            if matches!(
                mouse.kind,
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
            ) {
                return;
            }
            self.suppress_mouse_until_left_up = false;
        }

        if self.handle_tile_resize_drag(mouse, precise_col, precise_row) {
            return;
        }

        let screen_col = precise_col;
        let screen_row = precise_row;

        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.begin_tile_resize_drag(precise_col, precise_row, border_inset)
        {
            return;
        }

        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some(tile_id) =
                self.status_toggle_tile_at_screen(mouse.column as f32, mouse.row as f32)
        {
            self.route_pointer_event_to_tile(tile_id, border_inset, true, |editor| {
                editor.toggle_active_buffer_view_mode_from_indicator();
            });
            return;
        }

        // During drag/mouseup, if the active tile has an ongoing widget gesture,
        // keep routing to that tile so the gesture isn't broken by crossing tile borders.
        let has_active_gesture = self.active_leaf().active_widget_gesture.is_some();
        let force_active = has_active_gesture
            && matches!(
                mouse.kind,
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
            );

        // Find which tile this mouse event targets
        let target_tile = if force_active {
            Some(self.active_tile)
        } else {
            self.tile_at_screen(screen_col, screen_row)
        };
        if let Some(tile_id) = target_tile {
            if matches!(mouse.kind, MouseEventKind::Moved) && tile_id != self.active_tile {
                if let Some((content_col, content_row, _, _)) =
                    self.tile_content_area(tile_id, border_inset)
                {
                    let (event_col, event_row) = self
                        .tile_content_precise_event_position(
                            tile_id,
                            border_inset,
                            content_col,
                            content_row,
                            precise_col,
                            precise_row,
                        )
                        .unwrap_or((precise_col, precise_row));
                    self.update_sdf_hover_for_inactive_tile(
                        tile_id,
                        content_col,
                        content_row,
                        event_col,
                        event_row,
                    );
                }
                return;
            }

            let persist_selection = matches!(
                mouse.kind,
                MouseEventKind::Down(MouseButton::Left)
                    | MouseEventKind::Drag(MouseButton::Left)
                    | MouseEventKind::Up(MouseButton::Left)
                    | MouseEventKind::ScrollUp
                    | MouseEventKind::ScrollDown
                    | MouseEventKind::ScrollLeft
                    | MouseEventKind::ScrollRight
            );
            // Clicking a tile clears widget focus on all other tiles.
            if persist_selection
                && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                && tile_id != self.active_tile
            {
                self.tile_root.clear_focus_except(tile_id);
            }
            self.route_pointer_event_to_tile(tile_id, border_inset, persist_selection, |editor| {
                let Some((content_col, content_row, content_width, content_height)) =
                    editor.tile_content_area(tile_id, border_inset)
                else {
                    return;
                };
                let (event_col, event_row) = editor
                    .tile_content_precise_event_position(
                        tile_id,
                        border_inset,
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                    )
                    .unwrap_or((precise_col, precise_row));

                editor.handle_mouse_precise(
                    mouse,
                    content_col,
                    content_row,
                    content_width,
                    content_height,
                    event_col,
                    event_row,
                );
            });
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
        self.route_pointer_event_to_tile(tile_id, border_inset, true, |editor| {
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
        self.route_pointer_event_to_tile(tile_id, border_inset, true, |editor| {
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
        let show_status = self.tile_effective_show_status(tile_id)?;
        let (border_col, border_row) = self.tile_content_border_insets(tile_id, border_inset);

        let content_col_f = rect.col + border_col;
        let content_row_f = rect.row + border_row;
        let content_width_f = (rect.width - border_col * 2.0).max(0.0);
        let content_height_f = if show_status {
            (rect.height - border_row * 2.0 - 1.0).max(0.0)
        } else {
            (rect.height - border_row * 2.0).max(0.0)
        };

        if border_inset == 0 {
            Some((
                content_col_f.floor() as u16,
                content_row_f.floor() as u16,
                content_width_f.floor() as u16,
                content_height_f.floor() as u16,
            ))
        } else {
            Some((
                content_col_f.round() as u16,
                content_row_f.round() as u16,
                content_width_f.round() as u16,
                content_height_f.round() as u16,
            ))
        }
    }

    fn tile_content_precise_event_position(
        &self,
        tile_id: TileId,
        border_inset: u16,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> Option<(f32, f32)> {
        let rect = self.tile_rect(tile_id)?;
        let (border_col, border_row) = self.tile_content_border_insets(tile_id, border_inset);
        let content_col_f = rect.col + border_col;
        let content_row_f = rect.row + border_row;
        Some((
            precise_col - content_col_f + content_col as f32,
            precise_row - content_row_f + content_row as f32,
        ))
    }

    fn tile_content_border_insets(&self, tile_id: TileId, border_inset: u16) -> (f32, f32) {
        if border_inset != 0 {
            let border = border_inset as f32;
            return (border, border);
        }
        let Some(leaf) = self.tile_root.find_leaf(tile_id) else {
            return (0.0, 0.0);
        };
        if !leaf.show_border {
            return (0.0, 0.0);
        }
        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
        if cell_w <= 1.0 || cell_h <= 1.0 {
            return (0.0, 0.0);
        }
        (
            leaf.border_width_px.max(0.0) / cell_w.max(1.0),
            leaf.border_width_px.max(0.0) / cell_h.max(1.0),
        )
    }

    pub(crate) fn tile_effective_show_status(&self, tile_id: TileId) -> Option<bool> {
        let leaf = self.tile_root.find_leaf(tile_id)?;
        let buffer = self.buffers.get(leaf.buffer_idx)?;
        Some(leaf.show_status || buffer.view_mode != ViewMode::UiOnly)
    }

    fn status_toggle_tile_at_screen(&self, precise_col: f32, precise_row: f32) -> Option<TileId> {
        self.cached_tile_rects.iter().find_map(|(tile_id, _)| {
            self.tile_status_toggle_hit(*tile_id, precise_col, precise_row)
                .then_some(*tile_id)
        })
    }

    fn tile_status_toggle_hit(&self, tile_id: TileId, precise_col: f32, precise_row: f32) -> bool {
        const STATUS_TOGGLE_WIDTH: f32 = 4.0;

        let Some(rect) = self.tile_rect(tile_id) else {
            return false;
        };
        let Some(leaf) = self.tile_root.find_leaf(tile_id) else {
            return false;
        };
        if !self.tile_effective_show_status(tile_id).unwrap_or(false) {
            return false;
        }
        let ui_available = if tile_id == self.active_tile {
            self.active_buffer_has_ui()
        } else {
            let buffer = &self.buffers[leaf.buffer_idx];
            buffer.widget_tree.is_some() || leaf.cached_layout.is_some()
        };
        if !ui_available {
            return false;
        }

        let status_row = rect.row + rect.height - 1.0;
        precise_row >= status_row
            && precise_row < status_row + 1.0
            && precise_col >= rect.col
            && precise_col < rect.col + STATUS_TOGGLE_WIDTH
    }

    fn route_pointer_event_to_tile<R>(
        &mut self,
        tile_id: TileId,
        border_inset: u16,
        persist_selection: bool,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous_tile = self.active_tile;
        let switched = tile_id != previous_tile;
        let previous_viewport = self
            .tile_content_area(previous_tile, border_inset)
            .map(|(_, _, width, height)| (width, height));

        let target_viewport = self
            .tile_content_area(tile_id, border_inset)
            .map(|(_, _, width, height)| (width, height));

        if switched {
            self.switch_active_tile_with_viewport(tile_id, target_viewport);
        } else if let Some((width, height)) = target_viewport {
            self.set_layout_viewport(width, height);
        }

        let result = f(self);

        if switched && !persist_selection && self.tile_root.find_leaf(previous_tile).is_some() {
            self.switch_active_tile_with_viewport(previous_tile, previous_viewport);
        }

        result
    }

    fn toggle_active_buffer_view_mode(&mut self) {
        let next_mode = self.active_buffer().view_mode.toggle_primary();
        self.set_active_buffer_view_mode(next_mode);
    }

    fn toggle_active_buffer_view_mode_from_indicator(&mut self) {
        let next_mode = match self.active_buffer().view_mode {
            ViewMode::Both => ViewMode::UiOnly,
            other => other.toggle_primary(),
        };
        self.set_active_buffer_view_mode(next_mode);
    }

    fn set_active_buffer_view_mode(&mut self, mode: ViewMode) {
        let ui_available = self.active_buffer_has_ui();
        if matches!(mode, ViewMode::UiOnly | ViewMode::Both) && !ui_available {
            self.show_transient_message("No UI in this buffer");
            return;
        }

        self.active_buffer_mut().view_mode = mode;
        self.show_transient_message(format!("view: {}", mode.label()));
        if mode == ViewMode::UiOnly {
            self.active_buffer_mut().scroll_top = 0;
            self.active_leaf_mut().widget_scroll_top = 0.0;
        }
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
        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
        let Some(hit) = self.tile_root.hit_test_split_divider(
            root_area,
            precise_col,
            precise_row,
            tolerance,
            TILE_GAP_PX_PER_UNIT,
            cell_w,
            cell_h,
        ) else {
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
            let mut ratio = split_ratio_for_point(area, dir, precise_col, precise_row);
            // Enforce minimum sizes from layout spec
            let total = match dir {
                SplitDir::Vertical => area.width,
                SplitDir::Horizontal => area.height,
            };
            if total > 0.0 {
                let a_min = match dir {
                    SplitDir::Vertical => split.a.min_width(),
                    SplitDir::Horizontal => split.a.min_height(),
                };
                let b_min = match dir {
                    SplitDir::Vertical => split.b.min_width(),
                    SplitDir::Horizontal => split.b.min_height(),
                };
                let a_max = match dir {
                    SplitDir::Vertical => split.a.max_width(),
                    SplitDir::Horizontal => split.a.max_height(),
                };
                let b_max = match dir {
                    SplitDir::Vertical => split.b.max_width(),
                    SplitDir::Horizontal => split.b.max_height(),
                };
                if a_min > 0.0 {
                    ratio = ratio.max(a_min / total);
                }
                if b_min > 0.0 {
                    ratio = ratio.min(1.0 - b_min / total);
                }
                if a_max < f32::MAX {
                    ratio = ratio.min(a_max / total);
                }
                if b_max < f32::MAX {
                    ratio = ratio.max(1.0 - b_max / total);
                }
            }
            split.ratio = ratio;
            // Recompute layouts for all inactive tiles with their new dimensions
            let mut buf_indices: Vec<usize> = Vec::new();
            for id in self.tile_root.leaf_ids() {
                if id == self.active_tile {
                    continue;
                }
                if let Some(leaf) = self.tile_root.find_leaf_mut(id) {
                    leaf.cached_inactive_frame = None;
                    if !buf_indices.contains(&leaf.buffer_idx) {
                        buf_indices.push(leaf.buffer_idx);
                    }
                }
            }
            for buf_idx in buf_indices {
                self.refresh_inactive_tile_layouts_for_buffer(buf_idx);
            }
            self.mark_needs_redraw();
        }
    }

    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw || self.runtime.has_dirty_widget_ids()
    }

    pub fn clear_needs_redraw(&mut self) {
        self.needs_redraw = false;
    }

    pub fn mark_needs_redraw(&mut self) {
        self.needs_redraw = true;
    }

    pub fn show_transient_message(&mut self, message: impl Into<String>) {
        self.minibuffer = Some(message.into());
        self.minibuffer_expires_at = Some(Instant::now() + Duration::from_secs(1));
        self.mark_needs_redraw();
    }

    pub fn show_sticky_message(&mut self, message: impl Into<String>) {
        self.minibuffer = Some(message.into());
        self.minibuffer_expires_at = None;
        self.mark_needs_redraw();
    }

    pub fn clear_minibuffer_message(&mut self) {
        self.minibuffer = None;
        self.minibuffer_expires_at = None;
    }

    pub fn update_timers(&mut self) {
        if self
            .minibuffer_expires_at
            .is_some_and(|expires_at| Instant::now() >= expires_at)
        {
            self.clear_minibuffer_message();
            self.mark_needs_redraw();
        }

        if self
            .eval_flash
            .as_ref()
            .is_some_and(|flash| flash.expires_at < Instant::now())
        {
            self.eval_flash = None;
            self.mark_needs_redraw();
        }
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

    pub fn trace_completion_enabled() -> bool {
        std::env::var("ESEQLISP_TRACE_COMPLETION")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
    }

    pub fn completion_debug_summary(&self, stage: &str) -> String {
        let buffer = self.active_buffer();
        let cursor_row = buffer.cursor.0;
        let cursor_col = buffer.cursor.1;
        let line = buffer
            .lines
            .get(cursor_row)
            .map(String::as_str)
            .unwrap_or("");
        let prefix = debug_symbol_prefix(line, cursor_col)
            .map(|(start, prefix)| format!("prefix='{prefix}' start_col={start}"))
            .unwrap_or_else(|| "prefix=<none>".to_string());
        let completion = self
            .completion
            .as_ref()
            .map(|state| {
                let selected = state
                    .items
                    .get(state.selected)
                    .map(|item| item.label.as_str())
                    .unwrap_or("<out-of-range>");
                format!(
                    "state=some items={} selected={} selected_label='{selected}' scroll={} start_col={}",
                    state.items.len(),
                    state.selected,
                    state.scroll,
                    state.start_col
                )
            })
            .unwrap_or_else(|| "state=none".to_string());
        format!(
            "[completion][{stage}] editor_ptr={:p} runtime_ptr={:p} buffer='{}' id={} mode={:?} cursor=({}, {}) save_prompt={} minibuffer={} runtime_symbols_rev={} {} {} line='{}'",
            self,
            &self.runtime,
            buffer.name,
            buffer.id,
            buffer.mode,
            cursor_row,
            cursor_col,
            self.save_prompt.is_some(),
            self.minibuffer.is_some(),
            self.runtime.symbol_revision(),
            prefix,
            completion,
            line.escape_debug()
        )
    }

    pub fn trace_completion(&self, stage: &str) {
        if Self::trace_completion_enabled() {
            eprintln!("{}", self.completion_debug_summary(stage));
        }
    }

    pub fn active_highlight_spans_for_visible(
        &mut self,
        scroll_top: usize,
        viewport_height: usize,
    ) -> Vec<Vec<TokenSpan>> {
        let buf_idx = self.active_buffer_idx();
        let symbols = self.runtime.completion_symbols();
        let buffer = &self.buffers[buf_idx];
        let visible = scroll_top.min(buffer.lines.len())
            ..(scroll_top + viewport_height).min(buffer.lines.len());
        highlight_lines(&buffer.mode, buffer.lines[visible].iter(), &symbols, buffer)
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

    pub(super) fn exit_search_mode_if_active(&mut self) -> bool {
        if !matches!(self.minibuffer_input, Some(MinibufferMode::Search { .. })) {
            return false;
        }
        self.minibuffer_input = None;
        self.minibuffer = None;
        self.mark_needs_redraw();
        true
    }

    fn start_search(&mut self, direction: SearchDirection) {
        self.completion = None;
        self.minibuffer = None;
        self.minibuffer_input = Some(MinibufferMode::Search {
            state: SearchState {
                input: String::new(),
                origin: self.active_buffer().cursor,
                direction,
                current_match: None,
                failed: false,
            },
        });
    }

    fn apply_search_state(&mut self, state: &mut SearchState) {
        if state.input.is_empty() {
            self.active_buffer_mut().cursor = state.origin;
            state.current_match = None;
            state.failed = false;
            self.sync_text_horizontal_scroll_to_viewport();
            self.sync_runtime_context();
            return;
        }

        let result = match state.direction {
            SearchDirection::Forward => self
                .active_buffer()
                .find_forward(&state.input, state.origin),
            SearchDirection::Backward => self
                .active_buffer()
                .find_backward(&state.input, state.origin),
        };

        if let Some(start) = result {
            let end = (start.0, start.1 + state.input.chars().count());
            self.active_buffer_mut().cursor = start;
            state.current_match = Some(SearchMatch { start, end });
            state.failed = false;
            self.sync_text_horizontal_scroll_to_viewport();
            self.sync_runtime_context();
        } else {
            self.active_buffer_mut().cursor = state.origin;
            state.current_match = None;
            state.failed = true;
            self.sync_text_horizontal_scroll_to_viewport();
            self.sync_runtime_context();
        }
    }

    fn repeat_search(&mut self, state: &mut SearchState, direction: SearchDirection) {
        state.direction = direction;
        if state.input.is_empty() {
            return;
        }

        let start = match direction {
            SearchDirection::Forward => state
                .current_match
                .map(|found| found.end)
                .unwrap_or(state.origin),
            SearchDirection::Backward => state
                .current_match
                .map(|found| found.start)
                .unwrap_or(state.origin),
        };
        let result = match direction {
            SearchDirection::Forward => self.active_buffer().find_forward(&state.input, start),
            SearchDirection::Backward => self.active_buffer().find_backward(&state.input, start),
        };

        if let Some(found) = result {
            let end = (found.0, found.1 + state.input.chars().count());
            self.active_buffer_mut().cursor = found;
            state.current_match = Some(SearchMatch { start: found, end });
            state.failed = false;
            self.sync_text_horizontal_scroll_to_viewport();
            self.sync_runtime_context();
        } else {
            state.failed = true;
        }
    }

    fn text_page_height(&self) -> usize {
        self.runtime.layout_rows().max(1) as usize
    }

    fn move_page_forward(&mut self) {
        let page_height = self.text_page_height();
        let buffer = self.active_buffer_mut();
        let max_row = buffer.lines.len().saturating_sub(1);
        buffer.cursor.0 = (buffer.cursor.0 + page_height).min(max_row);
        buffer.cursor.1 = buffer
            .cursor
            .1
            .min(buffer.lines[buffer.cursor.0].chars().count());
        let max_scroll = buffer.lines.len().saturating_sub(page_height);
        buffer.scroll_top = (buffer.scroll_top + page_height).min(max_scroll);
        buffer.adjust_scroll(page_height);
    }

    fn move_page_backward(&mut self) {
        let page_height = self.text_page_height();
        let buffer = self.active_buffer_mut();
        buffer.cursor.0 = buffer.cursor.0.saturating_sub(page_height);
        buffer.cursor.1 = buffer
            .cursor
            .1
            .min(buffer.lines[buffer.cursor.0].chars().count());
        buffer.scroll_top = buffer.scroll_top.saturating_sub(page_height);
        buffer.adjust_scroll(page_height);
    }

    fn recenter_cursor(&mut self) {
        let page_height = self.text_page_height();
        let buffer = self.active_buffer_mut();
        let max_scroll = buffer.lines.len().saturating_sub(page_height);
        let desired_top = buffer.cursor.0.saturating_sub(page_height / 2);
        buffer.scroll_top = desired_top.min(max_scroll);
    }

    fn goto_definition(&mut self) {
        self.completion = None;
        let Some(symbol) = self.symbol_under_cursor() else {
            self.show_transient_message("No symbol at cursor");
            return;
        };

        let Some(location) = self.find_definition(&symbol) else {
            self.show_transient_message(format!("Definition not found: {symbol}"));
            return;
        };

        let current = Mark {
            buffer_id: self.active_buffer().id,
            cursor: self.active_buffer().cursor,
        };

        let navigated = if let Some(buffer_id) = location.buffer_id {
            self.set_active_buffer(buffer_id);
            true
        } else if let Some(path) = location.path.as_ref() {
            self.open_definition_path(path)
        } else {
            false
        };

        if !navigated {
            self.show_transient_message(format!("Definition not found: {symbol}"));
            return;
        }

        self.jump_stack.push(current);
        self.active_buffer_mut().cursor = location.cursor;
        self.sync_text_horizontal_scroll_to_viewport();
        self.sync_runtime_context();
        self.show_transient_message(format!("Definition: {symbol}"));
    }

    fn pop_definition_mark(&mut self) {
        let Some(mark) = self.jump_stack.pop() else {
            self.show_transient_message("Definition mark stack is empty");
            return;
        };
        self.set_active_buffer(mark.buffer_id);
        self.active_buffer_mut().cursor = mark.cursor;
        self.sync_text_horizontal_scroll_to_viewport();
        self.sync_runtime_context();
        self.show_transient_message("Popped definition mark");
    }

    fn open_definition_path(&mut self, path: &PathBuf) -> bool {
        if let Some(existing) = self
            .buffers
            .iter()
            .find(|buffer| buffer.path.as_ref() == Some(path))
            .map(|buffer| buffer.id)
        {
            self.set_active_buffer(existing);
            return true;
        }
        self.open_file_buffer(path.clone()).is_ok()
    }

    fn symbol_under_cursor(&self) -> Option<String> {
        let buffer = self.active_buffer();
        let line = buffer.lines.get(buffer.cursor.0)?;
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            return None;
        }

        let mut idx = buffer.cursor.1.min(chars.len());
        if idx == chars.len() || !is_symbol_char(chars[idx]) {
            if idx == 0 || !is_symbol_char(chars[idx.saturating_sub(1)]) {
                return None;
            }
            idx = idx.saturating_sub(1);
        }

        let mut start = idx;
        while start > 0 && is_symbol_char(chars[start - 1]) {
            start -= 1;
        }
        let mut end = idx + 1;
        while end < chars.len() && is_symbol_char(chars[end]) {
            end += 1;
        }

        Some(chars[start..end].iter().collect())
    }

    fn find_definition(&self, symbol: &str) -> Option<DefinitionLocation> {
        for buffer in self
            .buffers
            .iter()
            .filter(|buffer| buffer.id == self.active_buffer().id)
            .chain(
                self.buffers
                    .iter()
                    .filter(|buffer| buffer.id != self.active_buffer().id),
            )
        {
            if let Some(cursor) = find_definition_in_text(&buffer.text(), symbol) {
                return Some(DefinitionLocation {
                    path: buffer.path.clone(),
                    buffer_id: Some(buffer.id),
                    cursor,
                });
            }
        }

        let mut seen_paths = HashSet::new();
        for buffer in &self.buffers {
            if let Some(path) = &buffer.path {
                seen_paths.insert(path.clone());
            }
        }

        for path in self.definition_search_paths() {
            if seen_paths.contains(&path) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(cursor) = find_definition_in_text(&text, symbol) {
                return Some(DefinitionLocation {
                    path: Some(path),
                    buffer_id: None,
                    cursor,
                });
            }
        }

        None
    }

    fn definition_search_paths(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let root = self
            .active_buffer()
            .path
            .as_ref()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        collect_lisp_files(&root, &mut out);
        out.sort();
        out
    }

    pub fn widget_scroll_top(&self) -> f32 {
        self.active_leaf().widget_scroll_top
    }

    pub fn widget_scroll_left(&self) -> f32 {
        self.active_leaf().widget_scroll_left
    }

    pub fn reset_widget_scroll_left(&mut self) {
        self.active_leaf_mut().widget_scroll_left = 0.0;
        self.mark_needs_redraw();
    }

    pub fn reset_widget_scroll_top(&mut self) {
        self.active_leaf_mut().widget_scroll_top = 0.0;
        self.mark_needs_redraw();
    }

    pub fn reset_widget_scroll_for_buffer_named(&mut self, name: &str) {
        let Some(buffer_idx) = self.buffers.iter().position(|buffer| buffer.name == name) else {
            return;
        };
        let mut changed = false;
        for tile_id in self.tile_root.leaf_ids() {
            let Some(leaf) = self.tile_root.find_leaf_mut(tile_id) else {
                continue;
            };
            if leaf.buffer_idx != buffer_idx {
                continue;
            }
            if leaf.widget_scroll_top != 0.0 || leaf.widget_scroll_left != 0.0 {
                leaf.widget_scroll_top = 0.0;
                leaf.widget_scroll_left = 0.0;
                changed = true;
            }
        }
        if changed {
            self.mark_needs_redraw();
        }
    }

    /// Combined vertical scroll: widget scroll + text scroll.
    pub fn total_scroll_top(&self) -> f32 {
        let text_scroll = if self.active_buffer().view_mode == ViewMode::UiOnly {
            0.0
        } else {
            self.active_buffer().scroll_top as f32
        };
        self.widget_scroll_top() + text_scroll
    }

    /// Whether widget viewport scrolling should be smooth (sub-cell).
    /// True when widgets are visible (UiOnly or Both mode with a widget layout).
    pub fn is_ui_scroll_mode(&self) -> bool {
        let mode = self.active_buffer().view_mode;
        mode != ViewMode::TextOnly && self.runtime.current_layout.is_some()
    }

    pub(super) fn max_widget_vertical_scroll(&self) -> f32 {
        const SCROLL_SLOP_ROWS: f32 = 0.5;
        let viewport_rows = self.active_leaf().widget_viewport_height.max(0.0);
        let viewport_rows = if viewport_rows > 0.0 {
            viewport_rows
        } else {
            self.runtime.layout_rows() as f32
        };
        let overflow = self
            .runtime
            .current_layout
            .as_ref()
            .map(|layout| max_layout_bottom(layout.as_ref()) - viewport_rows)
            .unwrap_or(0.0);
        if overflow <= SCROLL_SLOP_ROWS {
            0.0
        } else {
            overflow.max(0.0)
        }
    }

    pub fn clamp_widget_scroll_offsets(&mut self) {
        let max_v = self.max_widget_vertical_scroll();
        let max_h = {
            let vp = self.runtime.layout_cols() as f32;
            let aspect = self.runtime.layout_aspect();
            self.runtime
                .current_layout
                .as_ref()
                .map(|layout| (crate::ui::hit::max_extent(layout, aspect).0 as f32 - vp).max(0.0))
                .unwrap_or(0.0)
        };
        let leaf = self.active_leaf_mut();
        leaf.widget_scroll_top = leaf.widget_scroll_top.clamp(0.0, max_v);
        leaf.widget_scroll_left = leaf.widget_scroll_left.clamp(0.0, max_h);
    }

    /// Apply smooth (sub-cell) scroll deltas to the widget viewport.
    /// `delta_cells_x` and `delta_cells_y` are in cell units (fractional).
    pub fn apply_smooth_widget_scroll(&mut self, delta_cells_x: f32, delta_cells_y: f32) {
        let max_v = self.max_widget_vertical_scroll();

        let max_h = {
            let vp = self.runtime.layout_cols() as f32;
            let aspect = self.runtime.layout_aspect();
            self.runtime
                .current_layout
                .as_ref()
                .map(|l| (crate::ui::hit::max_extent(l, aspect).0 as f32 - vp).max(0.0))
                .unwrap_or(0.0)
        };

        let leaf = self.active_leaf_mut();
        leaf.widget_scroll_top = (leaf.widget_scroll_top - delta_cells_y).clamp(0.0, max_v);
        leaf.widget_scroll_left = (leaf.widget_scroll_left - delta_cells_x).clamp(0.0, max_h);
        crate::widget_render::sdf_widget::clear_sdf_hit_states_except(None);
        self.mark_needs_redraw();
    }

    pub fn focused_widget_id(&self) -> Option<u64> {
        self.active_leaf().focused_widget_id
    }

    pub fn widget_layout(&self) -> Option<Arc<crate::layout::LayoutNode>> {
        // For active tile, runtime holds the authoritative layout.
        // For other tiles, their cached_layout is used directly.
        self.runtime.current_layout.clone()
    }

    pub fn active_buffer_has_ui(&self) -> bool {
        self.active_buffer().widget_tree.is_some()
            || self.runtime.current_widget_tree().is_some()
            || self.widget_layout().is_some()
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

    pub fn sync_text_horizontal_scroll_to_viewport(&mut self) {
        if self.active_buffer().view_mode != ViewMode::UiOnly {
            let viewport_height = self.runtime.layout_rows() as usize;
            self.active_buffer_mut().adjust_scroll(viewport_height);
        }
        self.sync_text_horizontal_scroll(self.runtime.layout_cols());
    }

    pub fn set_layout_aspect(&mut self, aspect: f32) {
        let old_aspect = self.runtime.layout_aspect();
        self.runtime.set_layout_aspect(aspect);
        if (old_aspect - aspect).abs() >= f32::EPSILON {
            self.sync_layout_to_active_leaf();
            self.refresh_all_inactive_tile_layouts();
        }
    }

    pub fn layout_aspect(&self) -> f32 {
        self.runtime.layout_aspect()
    }

    pub fn layout_cell_dims(&self) -> (f32, f32) {
        self.runtime.layout_cell_dims()
    }

    pub fn set_text_measurer(
        &mut self,
        measurer: Box<dyn crate::layout::TextMeasurer>,
        cell_w: f32,
        cell_h: f32,
    ) {
        let old_dims = self.runtime.layout_cell_dims();
        self.runtime.set_text_measurer(measurer, cell_w, cell_h);
        if (old_dims.0 - cell_w).abs() >= f32::EPSILON
            || (old_dims.1 - cell_h).abs() >= f32::EPSILON
        {
            self.sync_layout_to_active_leaf();
            self.refresh_all_inactive_tile_layouts();
        }
    }

    /// Sync the Runtime's current layout to the active tile leaf's cached_layout.
    /// Call this after any operation that may change the layout (eval, widget tree, etc.)
    pub fn sync_layout_to_active_leaf(&mut self) {
        let layout = self.runtime.current_layout.clone();
        let revision = self.runtime.layout_revision();
        let leaf = self.active_leaf_mut();
        leaf.cached_layout = layout;
        leaf.layout_revision = revision;
        self.remap_focused_widget_after_layout_change();
        self.sync_reactive_bindings_for_visible_layouts();
    }

    pub fn sync_reactive_bindings_for_visible_layouts(&mut self) {
        let visible_layouts = self
            .tile_root
            .leaf_ids()
            .into_iter()
            .filter_map(|id| {
                let leaf = self.tile_root.find_leaf(id)?;
                let layout = leaf.cached_layout.clone()?;
                Some((id, leaf.layout_revision, layout))
            })
            .collect::<Vec<_>>();
        let signature = visible_layouts
            .iter()
            .map(|(id, revision, layout)| (*id, *revision, Arc::as_ptr(layout) as usize))
            .collect::<Vec<_>>();
        if self
            .visible_binding_layout_signature
            .as_ref()
            .is_some_and(|cached| cached == &signature)
        {
            return;
        }
        self.runtime.replace_widget_bindings_from_layouts(
            visible_layouts.iter().map(|(_, _, layout)| layout.as_ref()),
        );
        self.visible_binding_layout_signature = Some(signature);
    }

    fn refresh_all_inactive_tile_layouts(&mut self) {
        let mut buf_indices: Vec<usize> = Vec::new();
        for id in self.tile_root.leaf_ids() {
            if id == self.active_tile {
                continue;
            }
            if let Some(leaf) = self.tile_root.find_leaf_mut(id) {
                leaf.cached_layout = None;
                leaf.dirty_widget_ids.clear();
                leaf.cached_inactive_frame = None;
                if !buf_indices.contains(&leaf.buffer_idx) {
                    buf_indices.push(leaf.buffer_idx);
                }
            }
        }
        for buf_idx in buf_indices {
            self.refresh_inactive_tile_layouts_for_buffer(buf_idx);
        }
        self.sync_reactive_bindings_for_visible_layouts();
        self.mark_needs_redraw();
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

    pub fn upsert_scratch_buffer(&mut self, name: &str, text: &str) -> BufferId {
        if let Some(idx) = self.buffers.iter().position(|buffer| buffer.name == name) {
            let id = self.buffers[idx].id;
            self.buffers[idx].set_text(text);
            id
        } else {
            let id = self.alloc_buffer_id();
            let mut buffer = Buffer::new(id, name);
            buffer.set_text(text);
            self.buffers.push(buffer);
            id
        }
    }

    fn ensure_scratch_buffer_named(&mut self, name: &str) -> usize {
        if let Some(idx) = self.buffers.iter().position(|buffer| buffer.name == name) {
            return idx;
        }
        let id = self.alloc_buffer_id();
        self.buffers.push(Buffer::new(id, name));
        self.buffers.len() - 1
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

    /// Create a file-backed buffer without switching the active tile to it.
    /// Returns the buffer name (filename) for later use with `swap_buffer_in_tile_showing`.
    pub fn create_file_buffer(
        &mut self,
        path: impl Into<PathBuf>,
        mode: BufferMode,
    ) -> Result<String, EditorError> {
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
        self.buffers.push(buffer);
        Ok(name)
    }

    /// Switch the buffer shown in the tile currently displaying `current_name`
    /// to the buffer named `new_name`. Does not change which tile is active.
    pub fn swap_buffer_in_tile_showing(&mut self, current_name: &str, new_name: &str) -> bool {
        let current_idx = self.buffers.iter().position(|b| b.name == current_name);
        let new_idx = self.buffers.iter().position(|b| b.name == new_name);
        if let (Some(cur), Some(new)) = (current_idx, new_idx) {
            if let Some(leaf) = self.tile_root.find_leaf_by_buffer_idx_mut(cur) {
                leaf.buffer_idx = new;
                if widget_only_scratch_buffer_should_show_ui(&self.buffers[new]) {
                    self.buffers[new].view_mode = ViewMode::UiOnly;
                }
                // Invalidate all cached rendering state so the new buffer
                // renders immediately instead of showing the old widget tree.
                leaf.cached_layout = None;
                leaf.cached_inactive_frame = None;
                leaf.hit_grid_cache = None;
                leaf.highlight_cache = None;
                leaf.widget_scroll_top = 0.0;
                leaf.widget_scroll_left = 0.0;
                self.refresh_inactive_tile_layouts_for_buffer(new);
                return true;
            }
        }
        false
    }

    /// Read the text content of a buffer by name.
    pub fn read_buffer_text(&self, name: &str) -> Option<String> {
        self.buffers
            .iter()
            .find(|b| b.name == name)
            .map(|b| b.lines.join("\n"))
    }

    /// Remove a buffer by name. Returns true if found and removed.
    pub fn remove_buffer_by_name(&mut self, name: &str) -> bool {
        if let Some(idx) = self.buffers.iter().position(|b| b.name == name) {
            self.buffers.remove(idx);
            // Fix up any tile leaf buffer indices that pointed past the removed slot
            Self::fix_leaf_indices(&mut self.tile_root, idx);
            true
        } else {
            false
        }
    }

    fn fix_leaf_indices(node: &mut crate::tile::TileNode, removed_idx: usize) {
        match node {
            crate::tile::TileNode::Leaf(leaf) => {
                if leaf.buffer_idx > removed_idx {
                    leaf.buffer_idx -= 1;
                }
            }
            crate::tile::TileNode::Split(s) => {
                Self::fix_leaf_indices(&mut s.a, removed_idx);
                Self::fix_leaf_indices(&mut s.b, removed_idx);
            }
        }
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
        self.sync_runtime_source_context();
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
        if key.kind != KeyEventKind::Press {
            return;
        }

        self.mark_needs_redraw();
        if !self.key_starts_text_insert(key) {
            self.finish_typing_undo_group();
        }

        if self.handle_save_prompt_key(key) {
            return;
        }

        if self.handle_minibuffer_key(key) {
            return;
        }

        if self.handle_selection_escape(key) {
            return;
        }

        if self.handle_vim_insert_escape(key) {
            return;
        }

        if self.handle_vim_redo_key(key) {
            return;
        }

        if self.handle_completion_key(key) {
            return;
        }

        if let Some(prefix) = self.pending_key.take() {
            let chord = format!("{} {}", key_str(prefix), key_str(key));
            if let Some(handler) = self.lisp_bindings.get(&chord).cloned() {
                if self.builtins.values().any(|cmd| cmd == &handler) {
                    self.run_command(&handler);
                } else {
                    self.call_lisp_handler(&handler);
                }
            }
            return;
        }

        // Focused widget keys take priority over global bindings
        // (so Enter/arrows work in number-pickers, dropdowns, etc.)
        if self.handle_focused_widget_key(key) {
            return;
        }

        if self.handle_focus_key(key) {
            return;
        }

        // Check direct keybinding before treating as chord prefix.
        // This allows e.g. "ESC" to fire even when "ESC ." chords exist.
        {
            let ks = key_str(key);
            if let Some(handler) = self.lisp_bindings.get(&ks).cloned() {
                if self.builtins.values().any(|cmd| cmd == &handler) {
                    self.run_command(&handler);
                } else {
                    self.call_lisp_handler(&handler);
                }
                return;
            }
        }

        if self.binding_has_prefix(&key_str(key)) {
            self.pending_key = Some(key);
            return;
        }

        if self.handle_vim_normal_key(key) {
            return;
        }

        if self.handle_mode_input_key(key) {
            return;
        }

        // Check mode-specific keybindings
        {
            let ks = key_str(key);
            let mode = &self.active_buffer().mode;
            if let BufferMode::Named(mode_name) = mode {
                if let Some(handler) = self
                    .mode_registry
                    .get(mode_name)
                    .and_then(|mode| mode.keybindings.get(&ks))
                    .cloned()
                {
                    self.call_lisp_handler(&handler);
                    return;
                }
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
                self.record_typing_undo_snapshot();
                self.active_buffer_mut().insert_char(c);
                self.sync_text_horizontal_scroll_to_viewport();
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
                self.record_undo_snapshot();
                self.active_buffer_mut().insert_newline_with_indent();
                self.sync_text_horizontal_scroll_to_viewport();
                self.sync_runtime_context();
            }
            _ => {}
        }
    }

    fn vim_applies_to_active_buffer(&self) -> bool {
        self.vim_enabled
            && self.active_buffer().view_mode != ViewMode::UiOnly
            && matches!(
                self.active_buffer().mode,
                BufferMode::ESeqLisp | BufferMode::DGenLisp
            )
    }

    fn key_starts_text_insert(&self, key: KeyEvent) -> bool {
        if !matches!(
            key.code,
            KeyCode::Char(_)
                if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT
        ) {
            return false;
        }
        if self.save_prompt.is_some() || self.minibuffer_input.is_some() {
            return false;
        }
        if self.active_buffer().read_only || self.active_buffer().view_mode == ViewMode::UiOnly {
            return false;
        }
        !self.vim_applies_to_active_buffer() || self.vim_input_mode == VimInputMode::Insert
    }

    fn handle_selection_escape(&mut self, key: KeyEvent) -> bool {
        if key.code != KeyCode::Esc
            || key.modifiers != KeyModifiers::NONE
            || self.active_region_range().is_none()
        {
            return false;
        }
        self.completion = None;
        self.clear_mark();
        self.mark_needs_redraw();
        true
    }

    fn handle_vim_redo_key(&mut self, key: KeyEvent) -> bool {
        if !self.vim_applies_to_active_buffer()
            || self.vim_input_mode != VimInputMode::Normal
            || key.code != KeyCode::Char('r')
            || !key.modifiers.contains(KeyModifiers::CONTROL)
        {
            return false;
        }
        self.redo_text();
        true
    }

    fn handle_vim_insert_escape(&mut self, key: KeyEvent) -> bool {
        if !self.vim_applies_to_active_buffer()
            || self.vim_input_mode != VimInputMode::Insert
            || key.modifiers != KeyModifiers::NONE
            || key.code != KeyCode::Esc
        {
            return false;
        }

        self.completion = None;
        self.pending_vim = None;
        self.vim_input_mode = VimInputMode::Normal;
        self.minibuffer = Some("-- NORMAL --".to_string());
        true
    }

    fn handle_vim_normal_key(&mut self, key: KeyEvent) -> bool {
        if !self.vim_applies_to_active_buffer()
            || self.vim_input_mode != VimInputMode::Normal
            || !(key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT)
        {
            return false;
        }

        if let Some(pending) = self.pending_vim.take() {
            return self.handle_pending_vim_key(pending, key);
        }

        match key.code {
            KeyCode::Char('u') => {
                self.undo_text();
                true
            }
            KeyCode::Char('i') => self.enter_vim_insert_mode(),
            KeyCode::Char('a') => {
                let row = self.active_buffer().cursor.0;
                let col = self.active_buffer().cursor.1;
                let line_len = self.active_buffer().lines[row].chars().count();
                if col < line_len {
                    self.active_buffer_mut().move_right();
                }
                self.enter_vim_insert_mode()
            }
            KeyCode::Char('I') => {
                let row = self.active_buffer().cursor.0;
                let first_nonblank = self.active_buffer().lines[row]
                    .chars()
                    .take_while(|ch| ch.is_whitespace())
                    .count();
                self.active_buffer_mut().cursor.1 = first_nonblank;
                self.enter_vim_insert_mode()
            }
            KeyCode::Char('A') => {
                self.active_buffer_mut().move_to_line_end();
                self.enter_vim_insert_mode()
            }
            KeyCode::Char('o') => {
                if self.guard_read_only() {
                    return true;
                }
                self.record_undo_snapshot();
                self.active_buffer_mut().move_to_line_end();
                self.active_buffer_mut().insert_newline_with_indent();
                self.sync_runtime_context();
                self.enter_vim_insert_mode()
            }
            KeyCode::Char('O') => {
                if self.guard_read_only() {
                    return true;
                }
                self.record_undo_snapshot();
                self.insert_line_above_for_vim();
                self.sync_runtime_context();
                self.enter_vim_insert_mode()
            }
            KeyCode::Char('h') => self.vim_run_motion("move-left"),
            KeyCode::Char('j') => self.vim_run_motion("move-down"),
            KeyCode::Char('k') => self.vim_run_motion("move-up"),
            KeyCode::Char('l') => self.vim_run_motion("move-right"),
            KeyCode::Left => self.vim_run_motion("move-left"),
            KeyCode::Down => self.vim_run_motion("move-down"),
            KeyCode::Up => self.vim_run_motion("move-up"),
            KeyCode::Right => self.vim_run_motion("move-right"),
            KeyCode::Char('w') => {
                self.vim_move_word_forward();
                true
            }
            KeyCode::Char('b') => self.vim_run_motion("move-word-left"),
            KeyCode::Char('0') => self.vim_run_motion("move-line-start"),
            KeyCode::Char('$') => self.vim_run_motion("move-line-end"),
            KeyCode::Char('G') => self.vim_run_motion("move-buffer-end"),
            KeyCode::Tab => {
                self.accept_or_open_completion();
                true
            }
            KeyCode::Char('g') => {
                self.pending_vim = Some(VimPending::Key(key));
                true
            }
            KeyCode::Char('d') if self.active_region_range().is_some() => {
                self.vim_delete_selection();
                true
            }
            KeyCode::Char('y') if self.active_region_range().is_some() => {
                self.vim_yank_selection();
                true
            }
            KeyCode::Char('d') | KeyCode::Char('y') => {
                let op = match key.code {
                    KeyCode::Char(op) => op,
                    _ => unreachable!(),
                };
                self.pending_vim = Some(VimPending::Operator {
                    op,
                    count: String::new(),
                });
                true
            }
            KeyCode::Char('r') => {
                self.pending_vim = Some(VimPending::Replace);
                true
            }
            KeyCode::Char('x') => {
                self.vim_delete_char_under_cursor();
                true
            }
            KeyCode::Char('p') => {
                self.vim_paste_after();
                true
            }
            KeyCode::Esc => true,
            KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace | KeyCode::Delete => true,
            _ => false,
        }
    }

    fn handle_pending_vim_key(&mut self, pending: VimPending, key: KeyEvent) -> bool {
        match pending {
            VimPending::Key(prefix) => match (prefix.code, key.code) {
                (KeyCode::Char('g'), KeyCode::Char('g')) => {
                    self.active_buffer_mut().cursor = (0, 0);
                    self.sync_text_horizontal_scroll_to_viewport();
                    true
                }
                _ => true,
            },
            VimPending::Replace => {
                if let KeyCode::Char(c) = key.code {
                    self.vim_replace_char_under_cursor(c);
                }
                true
            }
            VimPending::Operator { op, mut count } => {
                if let KeyCode::Char(c) = key.code
                    && c.is_ascii_digit()
                {
                    if !(count.is_empty() && c == '0') {
                        count.push(c);
                    }
                    self.pending_vim = Some(VimPending::Operator { op, count });
                    return true;
                }

                let count = count.parse::<usize>().unwrap_or(1).max(1);
                match (op, key.code) {
                    ('d', KeyCode::Char('d')) => {
                        self.vim_delete_lines(count);
                        true
                    }
                    ('y', KeyCode::Char('y')) => {
                        self.vim_yank_lines(count);
                        true
                    }
                    ('d', KeyCode::Char('w')) => {
                        self.vim_delete_words(count);
                        true
                    }
                    ('y', KeyCode::Char('w')) => {
                        self.vim_yank_words(count);
                        true
                    }
                    _ => true,
                }
            }
        }
    }

    fn enter_vim_insert_mode(&mut self) -> bool {
        self.pending_vim = None;
        self.vim_input_mode = VimInputMode::Insert;
        self.minibuffer = Some("-- INSERT --".to_string());
        true
    }

    fn vim_run_motion(&mut self, cmd: &str) -> bool {
        self.run_command(cmd);
        true
    }

    fn vim_move_word_forward(&mut self) {
        let (row, col) = self.vim_word_forward_position(self.active_buffer().cursor, 1);
        self.active_buffer_mut().cursor = (row, col);
        self.sync_text_horizontal_scroll_to_viewport();
    }

    fn vim_word_forward_position(&self, start: (usize, usize), count: usize) -> (usize, usize) {
        let (mut row, mut col) = start;
        for _ in 0..count {
            let next = self.vim_word_forward_position_once((row, col));
            if next == (row, col) {
                break;
            }
            (row, col) = next;
        }
        (row, col)
    }

    fn vim_word_forward_position_once(&self, start: (usize, usize)) -> (usize, usize) {
        let (mut row, mut col) = start;
        loop {
            let line = &self.active_buffer().lines[row];
            let chars = line.chars().collect::<Vec<_>>();
            let len = chars.len();

            while col < len && !chars[col].is_whitespace() {
                col += 1;
            }
            while col < len && chars[col].is_whitespace() {
                col += 1;
            }

            if col < len || row + 1 >= self.active_buffer().lines.len() {
                return (row, col.min(len));
            }

            row += 1;
            col = 0;
        }
    }

    fn vim_line_range(&self, count: usize) -> (usize, usize) {
        let start = self.active_buffer().cursor.0;
        let end_exclusive = (start + count).min(self.active_buffer().lines.len());
        (start, end_exclusive.max(start + 1))
    }

    fn vim_yank_lines(&mut self, count: usize) {
        let (start, end) = self.vim_line_range(count);
        let lines = self.active_buffer().lines[start..end].to_vec();
        self.kill_ring.push(lines.join("\n"));
        self.vim_linewise_yank = Some(lines);
        self.minibuffer = Some(format!(
            "Yanked {} line{}",
            end - start,
            if end - start == 1 { "" } else { "s" }
        ));
    }

    fn vim_yank_selection(&mut self) {
        if self.copy_active_region() {
            self.vim_linewise_yank = None;
            self.clear_mark();
            self.minibuffer = Some("Yanked selection".to_string());
        }
    }

    fn vim_delete_selection(&mut self) {
        if self.guard_read_only() {
            return;
        }
        if self.active_region_range().is_none() {
            return;
        }
        self.record_undo_snapshot();
        if self.kill_active_region() {
            self.vim_linewise_yank = None;
            self.sync_text_horizontal_scroll_to_viewport();
            self.sync_runtime_context();
            self.refresh_completion();
        }
    }

    fn vim_delete_lines(&mut self, count: usize) {
        if self.guard_read_only() {
            return;
        }
        let (start, end) = self.vim_line_range(count);
        let removed = self.active_buffer().lines[start..end].to_vec();
        self.record_undo_snapshot();
        self.kill_ring.push(removed.join("\n"));
        self.vim_linewise_yank = Some(removed);

        let buffer = self.active_buffer_mut();
        if start == 0 && end == buffer.lines.len() {
            buffer.lines = vec![String::new()];
            buffer.cursor = (0, 0);
        } else {
            buffer.lines.drain(start..end);
            let next_row = start.min(buffer.lines.len().saturating_sub(1));
            let next_col = buffer.cursor.1.min(buffer.lines[next_row].chars().count());
            buffer.cursor = (next_row, next_col);
        }
        buffer.dirty = true;
        buffer.revision = buffer.revision.wrapping_add(1);
        buffer.text_styles.clear();
        self.sync_runtime_context();
        self.refresh_completion();
    }

    fn vim_yank_words(&mut self, count: usize) {
        let start = self.active_buffer().cursor;
        let end = self.vim_word_forward_position(start, count);
        if end == start {
            return;
        }
        let text = self.active_buffer().slice_range(start, end);
        self.kill_ring.push(text);
        self.vim_linewise_yank = None;
        self.minibuffer = Some(format!(
            "Yanked {} word{}",
            count,
            if count == 1 { "" } else { "s" }
        ));
    }

    fn vim_delete_words(&mut self, count: usize) {
        if self.guard_read_only() {
            return;
        }
        let start = self.active_buffer().cursor;
        let end = self.vim_word_forward_position(start, count);
        if end == start {
            return;
        }
        self.record_undo_snapshot();
        let text = self.active_buffer().slice_range(start, end);
        self.kill_ring.push(text);
        self.vim_linewise_yank = None;
        self.active_buffer_mut().delete_range(start, end);
        self.sync_text_horizontal_scroll_to_viewport();
        self.sync_runtime_context();
        self.refresh_completion();
    }

    fn insert_line_above_for_vim(&mut self) {
        let row = self.active_buffer().cursor.0;
        let indent = self.active_buffer().lines[row]
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .collect::<String>();
        let buffer = self.active_buffer_mut();
        buffer.lines.insert(row, indent.clone());
        buffer.cursor = (row, indent.chars().count());
        buffer.dirty = true;
        buffer.revision = buffer.revision.wrapping_add(1);
    }

    fn vim_delete_char_under_cursor(&mut self) {
        if self.guard_read_only() {
            return;
        }
        let row = self.active_buffer().cursor.0;
        let col = self.active_buffer().cursor.1;
        let line_len = self.active_buffer().lines[row].chars().count();
        if col >= line_len {
            return;
        }
        self.record_undo_snapshot();
        self.active_buffer_mut()
            .delete_range((row, col), (row, col + 1));
        self.sync_runtime_context();
        self.refresh_completion();
    }

    fn vim_replace_char_under_cursor(&mut self, c: char) {
        if self.guard_read_only() {
            return;
        }
        let row = self.active_buffer().cursor.0;
        let col = self.active_buffer().cursor.1;
        let line_len = self.active_buffer().lines[row].chars().count();
        if col >= line_len {
            return;
        }
        self.record_undo_snapshot();
        {
            let buffer = self.active_buffer_mut();
            buffer.delete_range((row, col), (row, col + 1));
            buffer.insert_char(c);
            buffer.cursor = (row, col);
        }
        self.sync_runtime_context();
        self.refresh_completion();
    }

    fn vim_paste_after(&mut self) {
        if self.guard_read_only() {
            return;
        }
        if let Some(lines) = self.vim_linewise_yank.clone() {
            self.record_undo_snapshot();
            let row = self.active_buffer().cursor.0;
            let insert_row = row + 1;
            let buffer = self.active_buffer_mut();
            for (offset, line) in lines.iter().cloned().enumerate() {
                buffer.lines.insert(insert_row + offset, line);
            }
            buffer.cursor = (insert_row, 0);
            buffer.dirty = true;
            buffer.revision = buffer.revision.wrapping_add(1);
            buffer.text_styles.clear();
            self.sync_runtime_context();
            return;
        }

        if let Some(text) = self.kill_ring.last().cloned() {
            self.record_undo_snapshot();
            self.active_buffer_mut().move_right();
            self.active_buffer_mut().insert_str(&text);
            self.sync_runtime_context();
        }
    }

    fn current_text_snapshot(&self) -> TextUndoSnapshot {
        let buffer = self.active_buffer();
        TextUndoSnapshot {
            buffer_id: buffer.id,
            lines: buffer.lines.clone(),
            cursor: buffer.cursor,
            dirty: buffer.dirty,
        }
    }

    fn record_undo_snapshot(&mut self) {
        let snapshot = self.current_text_snapshot();
        if self.undo_stack.last().is_some_and(|last| {
            last.buffer_id == snapshot.buffer_id && last.lines == snapshot.lines
        }) {
            return;
        }
        self.undo_stack.push(snapshot);
        const MAX_UNDO: usize = 256;
        if self.undo_stack.len() > MAX_UNDO {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn record_typing_undo_snapshot(&mut self) {
        let buffer_id = self.active_buffer().id;
        if self.typing_undo_buffer_id == Some(buffer_id) {
            return;
        }
        self.record_undo_snapshot();
        self.typing_undo_buffer_id = Some(buffer_id);
    }

    fn finish_typing_undo_group(&mut self) {
        self.typing_undo_buffer_id = None;
    }

    fn restore_text_snapshot(&mut self, snapshot: TextUndoSnapshot) {
        let Some(buffer_idx) = self
            .buffers
            .iter()
            .position(|buffer| buffer.id == snapshot.buffer_id)
        else {
            return;
        };
        let buffer = &mut self.buffers[buffer_idx];
        buffer.lines = snapshot.lines;
        if buffer.lines.is_empty() {
            buffer.lines.push(String::new());
        }
        let row = snapshot.cursor.0.min(buffer.lines.len().saturating_sub(1));
        let col = snapshot.cursor.1.min(buffer.lines[row].chars().count());
        buffer.cursor = (row, col);
        buffer.dirty = snapshot.dirty;
        buffer.revision = buffer.revision.wrapping_add(1);
        buffer.text_styles.clear();
        self.finish_typing_undo_group();
        if self.active_buffer_idx() == buffer_idx {
            self.sync_text_horizontal_scroll_to_viewport();
        }
        self.sync_runtime_context();
        self.refresh_completion();
        self.mark_needs_redraw();
    }

    fn undo_text(&mut self) {
        let Some(snapshot) = self.undo_stack.pop() else {
            self.minibuffer = Some("No undo".to_string());
            return;
        };
        self.redo_stack.push(self.current_text_snapshot());
        self.restore_text_snapshot(snapshot);
        self.minibuffer = Some("Undo".to_string());
    }

    fn redo_text(&mut self) {
        let Some(snapshot) = self.redo_stack.pop() else {
            self.minibuffer = Some("No redo".to_string());
            return;
        };
        self.undo_stack.push(self.current_text_snapshot());
        self.restore_text_snapshot(snapshot);
        self.minibuffer = Some("Redo".to_string());
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
                self.pointer_drag_started_on_slider = false;
                self.last_slider_drag_widget_id = None;
                if widgets_visible {
                    if crate::widget_render::overlay_widget_id().is_some() {
                        let local_col = precise_col - content_col as f32;
                        let local_row = precise_row - content_row as f32;
                        if crate::widget_render::overlay_contains(local_col, local_row)
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
                    }
                    self.update_sdf_hover(content_col, content_row, precise_col, precise_row, true);
                    // Try click-to-activate on focusable widgets first
                    if self.try_click_focusable_widget(
                        precise_col,
                        precise_row,
                        content_col,
                        content_row,
                    ) {
                        return;
                    }
                    let pressed_widget = self.widget_node_at_screen(
                        precise_col,
                        precise_row,
                        content_col,
                        content_row,
                    );
                    if pressed_widget.as_ref().is_some_and(|node| {
                        matches!(node.widget_type.as_str(), "hslider" | "vslider" | "slider")
                    }) {
                        self.pointer_drag_started_on_slider = true;
                        self.last_slider_drag_widget_id =
                            pressed_widget.as_ref().map(|node| node.widget_id);
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
                        precise_col,
                        precise_row,
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
                        let is_box_pointer = matches!(
                            gesture.gesture_data.as_ref(),
                            Some(crate::vm::Value::String(s)) if s == "box-pointer"
                        );
                        if is_box_pointer {
                            // Box pointer gestures keep mouse-up tied to the original box, but
                            // drag should still hit-test across boxes (step drag-select).
                            self.try_handle_widget_drag_segment(
                                mouse,
                                content_col,
                                content_row,
                                previous,
                                (precise_col, precise_row),
                            );
                        } else if let Some(output) = self.dispatch_gesture_widget_mouse_event(
                            gesture,
                            mouse.kind,
                            content_col,
                            content_row,
                            precise_col,
                            precise_row,
                            mouse.modifiers,
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
                if text_visible {
                    self.handle_text_drag(
                        mouse,
                        content_col,
                        content_row,
                        content_width,
                        content_height,
                        precise_col,
                        precise_row,
                    );
                }
                self.last_mouse_precise = Some((precise_col, precise_row));
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if text_visible {
                    self.finish_text_drag();
                }
                if widgets_visible {
                    self.update_sdf_hover(
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                        false,
                    );
                    if let Some(gesture) = self.active_leaf_mut().active_widget_gesture.take() {
                        let output = self.dispatch_gesture_widget_mouse_event(
                            gesture,
                            mouse.kind,
                            content_col,
                            content_row,
                            precise_col,
                            precise_row,
                            mouse.modifiers,
                        );
                        let _ = self.apply_widget_output(output);
                    } else {
                        let _ = self.try_handle_widget_mouse_precise(
                            mouse,
                            content_col,
                            content_row,
                            precise_col,
                            precise_row,
                        );
                    }
                }
                self.last_mouse_precise = None;
                self.pointer_drag_started_on_slider = false;
                self.last_slider_drag_widget_id = None;
            }
            MouseEventKind::Moved => {
                // Update dropdown hover when overlay is open
                if let Some(overlay_id) = crate::widget_render::overlay_widget_id() {
                    if let Some((local_col, local_row)) =
                        crate::ui::hit::to_local(precise_col, precise_row, content_col, content_row)
                    {
                        let _local_col = local_col;
                        if crate::widget_render::dropdown::hover_overlay(overlay_id, local_row) {
                            self.mark_needs_redraw();
                        }
                    }
                }
                if widgets_visible {
                    self.update_widget_cursor(content_col, content_row, precise_col, precise_row);
                    self.update_sdf_hover(
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                        false,
                    );
                    let _ = self.try_handle_widget_mouse_precise(
                        mouse,
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                    );
                } else {
                    self.widget_cursor = WidgetCursor::Default;
                }
            }
            MouseEventKind::ScrollUp => {
                self.exit_search_mode_if_active();
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
                    leaf.widget_scroll_top = (leaf.widget_scroll_top - 3.0).max(0.0);
                    self.mark_needs_redraw();
                } else if self.active_buffer().read_only && self.has_focusable_widgets() {
                    self.navigate_focus(KeyCode::Up);
                } else {
                    let buffer = self.active_buffer_mut();
                    if buffer.scroll_top > 0 {
                        buffer.scroll_top = buffer.scroll_top.saturating_sub(3);
                    }
                    self.mark_needs_redraw();
                }
            }
            MouseEventKind::ScrollDown => {
                self.exit_search_mode_if_active();
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
                    let max_scroll = self.max_widget_vertical_scroll();
                    let leaf = self.active_leaf_mut();
                    leaf.widget_scroll_top = (leaf.widget_scroll_top + 3.0).min(max_scroll);
                    self.mark_needs_redraw();
                } else if self.active_buffer().read_only && self.has_focusable_widgets() {
                    self.navigate_focus(KeyCode::Down);
                } else {
                    let buffer = self.active_buffer_mut();
                    let max_scroll = buffer.lines.len().saturating_sub(1);
                    buffer.scroll_top = (buffer.scroll_top + 3).min(max_scroll);
                    self.mark_needs_redraw();
                }
            }
            MouseEventKind::ScrollLeft => {
                self.exit_search_mode_if_active();
                if self.active_buffer().view_mode == ViewMode::TextOnly {
                    let leaf = self.active_leaf_mut();
                    leaf.widget_scroll_left = (leaf.widget_scroll_left - 3.0).max(0.0);
                    crate::widget_render::sdf_widget::clear_sdf_hit_states_except(None);
                    self.mark_needs_redraw();
                    return;
                }
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
                        leaf.widget_scroll_left = (leaf.widget_scroll_left - 3.0).max(0.0);
                        crate::widget_render::sdf_widget::clear_sdf_hit_states_except(None);
                        self.mark_needs_redraw();
                    }
                }
            }
            MouseEventKind::ScrollRight => {
                self.exit_search_mode_if_active();
                if let Some(max_scroll) = self.max_text_horizontal_scroll(content_width) {
                    let leaf = self.active_leaf_mut();
                    leaf.widget_scroll_left =
                        (leaf.widget_scroll_left + 3.0).min(max_scroll as f32);
                    crate::widget_render::sdf_widget::clear_sdf_hit_states_except(None);
                    self.mark_needs_redraw();
                    return;
                }
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
                            (leaf.widget_scroll_left + 3.0).min(max_scroll as f32);
                        crate::widget_render::sdf_widget::clear_sdf_hit_states_except(None);
                        self.mark_needs_redraw();
                    }
                }
            }
            _ => {}
        }
    }

    fn max_text_horizontal_scroll(&self, viewport_width: u16) -> Option<u16> {
        if self.active_buffer().view_mode == ViewMode::UiOnly {
            return None;
        }
        let line_width = self
            .active_buffer()
            .lines
            .get(self.active_buffer().cursor.0)
            .map(|line| line.chars().count())
            .unwrap_or(0);
        let max_scroll = line_width.saturating_sub(viewport_width as usize) as u16;
        (max_scroll > 0).then_some(max_scroll)
    }

    pub fn sync_text_horizontal_scroll(&mut self, viewport_width: u16) {
        let Some(max_scroll) = self.max_text_horizontal_scroll(viewport_width) else {
            if self.active_buffer().view_mode != ViewMode::UiOnly {
                self.active_leaf_mut().widget_scroll_left = 0.0;
            }
            return;
        };

        let cursor_col = self.active_buffer().cursor.1;
        let viewport_width = viewport_width as usize;
        let leaf = self.active_leaf_mut();
        let scroll_left = leaf.widget_scroll_left.floor() as usize;

        let next_scroll = if cursor_col < scroll_left {
            cursor_col
        } else if cursor_col >= scroll_left + viewport_width {
            cursor_col.saturating_sub(viewport_width.saturating_sub(1))
        } else {
            scroll_left
        };

        leaf.widget_scroll_left = (next_scroll.min(max_scroll as usize) as f32).floor();
    }

    /// Maximum horizontal scroll: how far right content extends past the viewport.
    ///
    /// For widget layouts, we look at each direct child of the root layout node
    /// individually. Each child's right edge (col + width) in the constrained
    /// layout tells us how far that row extends. We take the max across all
    /// direct children. This avoids counting deep descendant overflow (e.g. an
    /// h-stack with 10 clipped boxes) while still detecting when a single row
    /// like a grid genuinely extends past the viewport.
    fn max_horizontal_scroll(&self, viewport_width: u16) -> u16 {
        let vp = viewport_width as usize;
        let view_mode = self.active_buffer().view_mode;

        // Widget layout extent: max right edge of root's direct children.
        let layout_width =
            if view_mode != ViewMode::TextOnly && self.active_buffer().widget_tree.is_some() {
                // Use bounded extent: only count nodes whose left edge starts
                // within the viewport. This prevents h-stacks with many clipped
                // children (e.g. 10 effect boxes) from inflating the scroll range,
                // while still detecting legitimate overflow (e.g. a grid whose
                // visible cells extend past the viewport).
                let aspect = self.runtime.layout_aspect();
                self.runtime
                    .current_layout
                    .as_ref()
                    .map(|l| crate::ui::hit::max_extent_bounded(l, aspect, vp as f32).0 as usize)
                    .unwrap_or(0)
            } else {
                0
            };

        // Text line width (only when text is visible).
        let max_line = if view_mode == ViewMode::UiOnly {
            0
        } else {
            self.active_buffer()
                .lines
                .iter()
                .map(|l| l.chars().count())
                .max()
                .unwrap_or(0)
        };

        let content_width = layout_width.max(max_line);
        let result = content_width.saturating_sub(vp) as u16;
        eprintln!(
            "[MAX-H-SCROLL] view_mode={view_mode:?} layout_width={layout_width} max_line={max_line} vp={vp} result={result}"
        );
        result
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
            self.show_transient_message(status);
        }
    }

    fn call_lisp_handler(&mut self, fn_name: &str) {
        self.call_lisp_handler_with_args(fn_name, &[]);
    }

    fn call_lisp_handler_with_args(&mut self, fn_name: &str, args: &[Value]) {
        if fn_name == "eval-sexp" || fn_name == "eval-buffer-command" {
            self.eval_preview_handler(fn_name);
            return;
        }
        if fn_name == "find-file" {
            self.run_command("find-file");
            return;
        }
        self.sync_runtime_source_context();
        self.clear_minibuffer_message();
        let rendered_args = args
            .iter()
            .map(format_lisp_value)
            .collect::<Vec<_>>()
            .join(" ");
        let code = if rendered_args.is_empty() {
            format!("({fn_name})")
        } else {
            format!("({fn_name} {rendered_args})")
        };
        match self.runtime.eval_str(&code) {
            Ok(Some(result)) => self.show_transient_message(format_value_for_minibuffer(&result)),
            Ok(None) => self.show_transient_message("No result"),
            Err(e) => self.show_transient_message(format!("Error: {e:?}")),
        }
        if let Some(status) = self.runtime.take_status_message() {
            self.show_transient_message(status);
        }
        self.refresh_runtime_side_effects();
        self.sync_runtime_context();
        self.completion = None;
    }

    fn handle_mode_input_key(&mut self, key: KeyEvent) -> bool {
        let Some(handler) = (match &self.active_buffer().mode {
            BufferMode::Named(mode_name) => self
                .mode_registry
                .get(mode_name)
                .and_then(|mode| mode.on_key.clone()),
            _ => None,
        }) else {
            return false;
        };

        let text = match key.code {
            KeyCode::Char(c)
                if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT =>
            {
                Value::String(c.to_string())
            }
            _ => Value::Bool(false),
        };
        let args = vec![Value::String(key_str(key)), text];
        self.sync_runtime_source_context();
        self.clear_minibuffer_message();
        let code = format!(
            "({} {} {})",
            handler,
            format_lisp_value(&args[0]),
            format_lisp_value(&args[1])
        );
        let handled = match self.runtime.eval_str(&code) {
            Ok(Some(Value::Bool(handled))) => handled,
            Ok(Some(Value::Nil)) | Ok(None) => false,
            Ok(Some(_)) => true,
            Err(e) => {
                self.show_transient_message(format!("Error: {e:?}"));
                true
            }
        };
        if let Some(status) = self.runtime.take_status_message() {
            self.show_transient_message(status);
        }
        self.refresh_runtime_side_effects();
        self.sync_runtime_context();
        handled
    }

    fn eval_preview_handler(&mut self, fn_name: &str) {
        if fn_name == "eval-sexp" {
            self.start_eval_flash();
        }
        self.sync_runtime_source_context();
        self.clear_minibuffer_message();

        let source = match fn_name {
            "eval-sexp" => {
                let buffer = self.active_buffer();
                sexp_at_cursor(&buffer.lines, buffer.cursor).unwrap_or_default()
            }
            "eval-buffer-command" => self.active_buffer().text(),
            _ => String::new(),
        };

        if source.trim().is_empty() {
            self.show_transient_message("No s-expression at cursor");
            self.completion = None;
            return;
        }

        match self.runtime.eval_str(&source) {
            Ok(Some(result)) => self.show_transient_message(format_value_for_minibuffer(&result)),
            Ok(None) => self.show_transient_message("No result"),
            Err(e) => self.show_transient_message(format!("Error: {e:?}")),
        }
        if let Some(status) = self.runtime.take_status_message() {
            self.show_transient_message(status);
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

    fn sync_runtime_source_context(&mut self) {
        self.sync_runtime_context();
        let active = self.active_buffer();
        let text = active.text();
        let sexp = sexp_at_cursor(&active.lines, active.cursor);
        let mut shared = self.runtime.shared.borrow_mut();
        shared.current_buffer_text = text;
        shared.current_sexp = sexp;
    }

    fn apply_widget_tree_to_buffer(
        &mut self,
        buffer_idx: usize,
        source_buffer_id: Option<BufferId>,
        tree: Option<Value>,
    ) {
        let is_active = self.active_buffer_idx() == buffer_idx;
        {
            let buffer = &mut self.buffers[buffer_idx];
            buffer.set_widget_tree(tree.as_ref().map(Value::deep_clone), source_buffer_id);
            if tree.is_some() {
                buffer.view_mode = ViewMode::UiOnly;
            }
        }
        if is_active {
            let buffer_id = self.buffers[buffer_idx].id as u64;
            self.runtime.set_widget_id_offset(buffer_id * 100_000);
            match tree {
                Some(tree) => {
                    self.runtime.set_widget_tree(tree);
                    self.auto_focus_first_widget();
                }
                None => self.clear_widget_focus(),
            }
        }
        self.refresh_inactive_tile_layouts_for_buffer(buffer_idx);
    }

    fn refresh_inactive_tile_layouts_for_buffer(&mut self, buffer_idx: usize) {
        let tree = self.buffers[buffer_idx].widget_tree.clone();
        let buffer_id = self.buffers[buffer_idx].id as u64;
        let tile_ids = self.tile_root.leaf_ids();
        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
        // Collect tile viewports first to avoid borrow issues
        let tiles_to_update: Vec<(TileId, u16, u16)> = tile_ids
            .into_iter()
            .filter(|id| *id != self.active_tile)
            .filter_map(|id| {
                let leaf = self.tile_root.find_leaf(id)?;
                if leaf.buffer_idx != buffer_idx {
                    return None;
                }
                // Find this tile's rect to get its actual viewport size
                let rect = self
                    .cached_tile_rects
                    .iter()
                    .find(|(tid, _)| *tid == id)
                    .map(|(_, r)| r);
                let show_status = self
                    .tile_effective_show_status(id)
                    .unwrap_or(leaf.show_status);
                let (cols, rows) = match rect {
                    Some(r) => metal_tile_content_viewport(
                        r,
                        show_status,
                        leaf.show_border,
                        leaf.border_width_px,
                        cell_w,
                        cell_h,
                    ),
                    None => (self.runtime.layout_cols(), self.runtime.layout_rows()),
                };
                Some((id, cols, rows))
            })
            .collect();

        for (tile_id, cols, rows) in tiles_to_update {
            let existing_layout = self
                .tile_root
                .find_leaf(tile_id)
                .and_then(|leaf| leaf.cached_layout.clone());
            let reused_layout_and_dirty = tree.as_ref().and_then(|tree| {
                let existing = existing_layout.as_ref()?;
                let mut dirty_widget_ids = Vec::new();
                crate::layout::reuse_layout_node(existing, tree, &mut dirty_widget_ids)
                    .map(|layout| (std::sync::Arc::new(layout), dirty_widget_ids))
            });
            let (layout, dirty_widget_ids) =
                if let Some((layout, dirty_widget_ids)) = reused_layout_and_dirty {
                    (Some(layout), dirty_widget_ids)
                } else {
                    let layout = tree.as_ref().and_then(|tree| {
                        self.runtime
                            .layout_snapshot_for_tree_with_viewport_and_offset(
                                tree,
                                Some((cols, rows)),
                                buffer_id * 100_000,
                            )
                    });
                    (layout, Vec::new())
                };
            if let Some(leaf) = self.tile_root.find_leaf_mut(tile_id) {
                leaf.cached_layout = layout;
                leaf.dirty_widget_ids = dirty_widget_ids;
                leaf.layout_revision = leaf.layout_revision.wrapping_add(1);
                leaf.cached_inactive_frame = None;
            }
        }
        self.sync_reactive_bindings_for_visible_layouts();
        self.mark_needs_redraw();
    }

    pub fn refresh_visible_layouts_for_buffer_named(&mut self, name: &str) {
        let Some(buffer_idx) = self.buffers.iter().position(|buffer| buffer.name == name) else {
            return;
        };
        if buffer_idx == self.active_buffer_idx() {
            self.sync_layout_to_active_leaf();
            self.mark_needs_redraw();
        } else {
            self.refresh_inactive_tile_layouts_for_buffer(buffer_idx);
        }
    }

    fn refresh_inactive_tile_layouts_for_buffer_subtrees(
        &mut self,
        buffer_idx: usize,
        subtree_roots: &[u64],
    ) {
        if subtree_roots.is_empty() {
            return;
        }
        let tree = self.buffers[buffer_idx].widget_tree.clone();
        let Some(tree) = tree.as_ref() else {
            self.refresh_inactive_tile_layouts_for_buffer(buffer_idx);
            return;
        };
        let buffer_id = self.buffers[buffer_idx].id as u64;
        let tile_ids = self.tile_root.leaf_ids();
        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
        let tiles_to_update: Vec<(TileId, u16, u16)> = tile_ids
            .into_iter()
            .filter(|id| *id != self.active_tile)
            .filter_map(|id| {
                let leaf = self.tile_root.find_leaf(id)?;
                if leaf.buffer_idx != buffer_idx {
                    return None;
                }
                let rect = self
                    .cached_tile_rects
                    .iter()
                    .find(|(tid, _)| *tid == id)
                    .map(|(_, r)| r);
                let show_status = self
                    .tile_effective_show_status(id)
                    .unwrap_or(leaf.show_status);
                let (cols, rows) = match rect {
                    Some(r) => metal_tile_content_viewport(
                        r,
                        show_status,
                        leaf.show_border,
                        leaf.border_width_px,
                        cell_w,
                        cell_h,
                    ),
                    None => (self.runtime.layout_cols(), self.runtime.layout_rows()),
                };
                Some((id, cols, rows))
            })
            .collect();

        for (tile_id, cols, rows) in tiles_to_update {
            let buffer_name = self.buffers[buffer_idx].name.clone();
            let existing_layout = self
                .tile_root
                .find_leaf(tile_id)
                .and_then(|leaf| leaf.cached_layout.clone());
            let mut dirty_widget_ids = Vec::new();
            let mut reuse_mode = "targeted";
            let mut miss_reason = None::<String>;
            let reusable_existing_layout = existing_layout;
            let mut layout = reusable_existing_layout.clone();
            let mut targeted = layout.is_some();
            let subtree_paths = layout
                .as_ref()
                .map(|layout| crate::layout::subtree_root_paths(layout.as_ref()));
            for subtree_root_id in subtree_roots {
                let Some(existing) = layout.as_ref() else {
                    targeted = false;
                    miss_reason.get_or_insert_with(|| "missing-layout".to_string());
                    break;
                };
                let Some(child_path) = subtree_paths
                    .as_ref()
                    .and_then(|paths| paths.get(subtree_root_id))
                else {
                    targeted = false;
                    miss_reason = Some(format!("missing-subtree-path:{subtree_root_id}"));
                    break;
                };
                let updated = match crate::layout::reuse_layout_node_for_subtree_path_result(
                    existing.as_ref(),
                    tree,
                    child_path,
                    &mut dirty_widget_ids,
                ) {
                    Ok(updated) => updated,
                    Err(reason) => {
                        targeted = false;
                        miss_reason = Some(format!("subtree:{subtree_root_id}:{reason}"));
                        break;
                    }
                };
                layout = Some(std::sync::Arc::new(updated));
            }
            let (layout, dirty_widget_ids) = if targeted {
                (layout, dirty_widget_ids)
            } else {
                let mut dirty_widget_ids = Vec::new();
                let reused_layout = reusable_existing_layout.as_ref().and_then(|existing| {
                    match crate::layout::reuse_layout_node(
                        existing.as_ref(),
                        tree,
                        &mut dirty_widget_ids,
                    ) {
                        Some(layout) => {
                            reuse_mode = "whole-tree";
                            Some(std::sync::Arc::new(layout))
                        }
                        None => {
                            if miss_reason.is_none() {
                                miss_reason = crate::layout::reuse_layout_failure_reason(
                                    existing.as_ref(),
                                    tree,
                                );
                            }
                            None
                        }
                    }
                });
                if let Some(layout) = reused_layout {
                    (Some(layout), dirty_widget_ids)
                } else {
                    reuse_mode = "full";
                    let layout = self
                        .runtime
                        .layout_snapshot_for_tree_with_viewport_and_offset(
                            tree,
                            Some((cols, rows)),
                            buffer_id * 100_000,
                        );
                    (layout, Vec::new())
                }
            };
            if trace_ui_invalidation_enabled() && reuse_mode != "targeted" {
                eprintln!(
                    "[ui-trace][inactive-layout] buffer={buffer_name} tile={tile_id} mode={reuse_mode} roots={} viewport={}x{} reason={}",
                    subtree_roots.len(),
                    cols,
                    rows,
                    miss_reason.as_deref().unwrap_or("-"),
                );
            }
            if let Some(leaf) = self.tile_root.find_leaf_mut(tile_id) {
                leaf.cached_layout = layout;
                leaf.dirty_widget_ids = dirty_widget_ids;
                leaf.layout_revision = leaf.layout_revision.wrapping_add(1);
                leaf.cached_inactive_frame = None;
            }
        }
        self.sync_reactive_bindings_for_visible_layouts();
        self.mark_needs_redraw();
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
        self.record_undo_snapshot();
        let buffer = self.active_buffer_mut();
        let row = buffer.cursor.0;
        let end_col = buffer.cursor.1.min(buffer.lines[row].len());
        buffer.lines[row].replace_range(completion.start_col..end_col, &item.label);
        buffer.cursor.1 = completion.start_col + item.label.len();
        buffer.dirty = true;
        self.completion = None;
        self.sync_runtime_context();
    }

    fn accept_or_open_completion(&mut self) {
        self.minibuffer = None;
        self.refresh_completion();
        if self.completion.is_some() {
            self.accept_completion();
        } else {
            self.record_undo_snapshot();
            self.active_buffer_mut().indent_current_line();
            self.sync_runtime_context();
        }
    }

    fn refresh_completion(&mut self) {
        if self.save_prompt.is_some() {
            self.completion = None;
            self.trace_completion("refresh:save-prompt");
            return;
        }
        if !has_completion_prefix(self.active_buffer()) {
            self.completion = None;
            self.trace_completion("refresh:no-prefix");
            return;
        }
        let symbols = self.runtime.completion_symbols();
        let metadata = self.runtime.completion_metadata();
        if Self::trace_completion_enabled() {
            eprintln!(
                "{} runtime_symbols={} metadata={}",
                self.completion_debug_summary("refresh:before"),
                symbols.len(),
                metadata.len()
            );
        }
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
        if Self::trace_completion_enabled() {
            eprintln!(
                "{} runtime_symbols={} metadata={}",
                self.completion_debug_summary("refresh:after"),
                symbols.len(),
                metadata.len()
            );
        }
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

    pub fn refresh_runtime_side_effects(&mut self) {
        self.lisp_bindings = self.default_lisp_bindings.clone();
        self.lisp_bindings.extend(self.runtime.lisp_bindings());

        if let Some(read_only) = self.runtime.take_pending_set_read_only() {
            self.active_buffer_mut().read_only = read_only;
        }

        // Process mode definitions
        for (name, read_only, on_enter, on_key) in self.runtime.take_pending_mode_defs() {
            self.mode_registry.insert(
                name.clone(),
                MajorMode {
                    name,
                    read_only,
                    keybindings: HashMap::new(),
                    on_enter,
                    on_key,
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
                    self.show_transient_message(format!("Opened {path}"));
                    self.clear_widget_focus();
                }
                Err(e) => self.show_transient_message(format!("Error: {e:?}")),
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

        for source_buffer_id in self.runtime.take_pending_cleared_effect_sources() {
            let target_indices = self
                .buffers
                .iter()
                .enumerate()
                .filter_map(|(idx, buffer)| {
                    (buffer.widget_tree_source == source_buffer_id).then_some(idx)
                })
                .collect::<Vec<_>>();
            for idx in target_indices {
                if self.active_buffer_idx() == idx {
                    let buffer = &mut self.buffers[idx];
                    buffer.set_widget_tree(None, None);
                } else {
                    self.apply_widget_tree_to_buffer(idx, None, None);
                }
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
                self.sync_runtime_source_context();
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
            buffer.text_styles.clear();
        }

        if let Some(styles) = self.runtime.take_pending_set_buffer_styles() {
            self.active_buffer_mut().text_styles = styles;
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
            crate::widget_render::clear_overlay();
            match tree {
                Value::Nil | Value::Bool(false) => {
                    let buffer = self.active_buffer_mut();
                    buffer.set_widget_tree(None, None);
                    buffer.view_mode = ViewMode::TextOnly;
                    self.runtime.clear_layout_effects();
                    self.clear_focused_widget();
                }
                tree => {
                    let source_id = self.active_buffer().id;
                    self.runtime
                        .set_widget_id_offset(source_id as u64 * 100_000);
                    let buffer = self.active_buffer_mut();
                    buffer.set_widget_tree(Some(tree.deep_clone()), Some(source_id));
                    buffer.view_mode = ViewMode::UiOnly;
                    self.runtime.set_widget_tree(tree);
                    self.auto_focus_first_widget();
                }
            }
        }

        let mut inactive_buffers_to_refresh: HashMap<usize, Option<Vec<u64>>> = HashMap::new();
        for pending in self.runtime.take_pending_buffer_widget_trees() {
            match pending {
                PendingUiUpdate::FullTree(pending) => {
                    let buffer_idx = match pending.target {
                        EffectTarget::BufferId(Some(id)) => {
                            let Some(idx) = self.buffers.iter().position(|buffer| buffer.id == id)
                            else {
                                continue;
                            };
                            idx
                        }
                        EffectTarget::BufferId(None) => self.active_buffer_idx(),
                        EffectTarget::BufferName(ref name) => {
                            self.ensure_scratch_buffer_named(name)
                        }
                    };
                    let buffer_name = self.buffers[buffer_idx].name.clone();
                    if debug_ui_updates_enabled() {
                        eprintln!(
                            "[ui-update full] target={} active={} incoming={}",
                            buffer_name,
                            self.active_buffer_idx() == buffer_idx,
                            debug_widget_tree_summary(Some(&pending.tree)),
                        );
                    }
                    self.trace_ui_tree_event_with(&buffer_name, "pending-full", || {
                        format!(
                            "incoming={} before={}",
                            debug_widget_tree_summary(Some(&pending.tree)),
                            debug_widget_tree_summary(
                                self.buffers[buffer_idx].widget_tree.as_ref()
                            ),
                        )
                    });
                    let is_active = self.active_buffer_idx() == buffer_idx;
                    let upgraded_subtree_root = self.buffers[buffer_idx]
                        .committed_ui_snapshot
                        .as_ref()
                        .and_then(|snapshot| {
                            snapshot.matching_non_root_subtree_root_id_for_tree(&pending.tree)
                        });
                    let upgraded_subtree = upgraded_subtree_root.is_some_and(|subtree_root_id| {
                        self.buffers[buffer_idx].replace_widget_subtree(
                            subtree_root_id,
                            pending.tree.deep_clone(),
                            pending.source_buffer_id,
                            pending.reactive_dependencies.clone(),
                        )
                    });
                    if upgraded_subtree {
                        if debug_ui_updates_enabled() {
                            eprintln!(
                                "[ui-update full->subtree] target={} root={:?}",
                                buffer_name, upgraded_subtree_root
                            );
                        }
                        self.buffers[buffer_idx].view_mode = ViewMode::UiOnly;
                        if is_active {
                            crate::widget_render::clear_overlay();
                            let _ = self
                                .runtime
                                .try_upgrade_full_tree_to_current_subtree(&pending);
                            self.auto_focus_first_widget();
                        } else {
                            inactive_buffers_to_refresh
                                .entry(buffer_idx)
                                .or_insert_with(|| Some(Vec::new()))
                                .as_mut()
                                .map(|roots| roots.push(upgraded_subtree_root.unwrap()));
                        }
                    } else if is_active {
                        if debug_ui_updates_enabled() {
                            eprintln!("[ui-update full-active] target={buffer_name}");
                        }
                        crate::widget_render::clear_overlay();
                        self.apply_widget_tree_to_buffer(
                            buffer_idx,
                            pending.source_buffer_id,
                            Some(pending.tree),
                        );
                    } else {
                        if debug_ui_updates_enabled() {
                            eprintln!("[ui-update full-inactive] target={buffer_name}");
                        }
                        {
                            let buffer = &mut self.buffers[buffer_idx];
                            buffer.set_widget_tree(Some(pending.tree), pending.source_buffer_id);
                            buffer.view_mode = ViewMode::UiOnly;
                        }
                        inactive_buffers_to_refresh.insert(buffer_idx, None);
                    }
                    self.trace_ui_tree_event_with(&buffer_name, "applied-full", || {
                        format!(
                            "after={}",
                            debug_widget_tree_summary(
                                self.buffers[buffer_idx].widget_tree.as_ref()
                            )
                        )
                    });
                }
                PendingUiUpdate::ReplaceSubtree {
                    source_buffer_id,
                    target,
                    subtree_root_id,
                    tree,
                    reactive_dependencies,
                } => {
                    let buffer_idx = match target {
                        EffectTarget::BufferId(Some(id)) => {
                            let Some(idx) = self.buffers.iter().position(|buffer| buffer.id == id)
                            else {
                                continue;
                            };
                            idx
                        }
                        EffectTarget::BufferId(None) => self.active_buffer_idx(),
                        EffectTarget::BufferName(name) => self.ensure_scratch_buffer_named(&name),
                    };
                    let buffer_name = self.buffers[buffer_idx].name.clone();
                    if debug_ui_updates_enabled() {
                        eprintln!(
                            "[ui-update subtree] target={} active={} root={} incoming={}",
                            buffer_name,
                            self.active_buffer_idx() == buffer_idx,
                            subtree_root_id,
                            debug_widget_tree_summary(Some(&tree)),
                        );
                    }
                    self.trace_ui_tree_event_with(&buffer_name, "pending-subtree", || {
                        format!(
                            "root={subtree_root_id} incoming={} before={}",
                            debug_widget_tree_summary(Some(&tree)),
                            debug_widget_tree_summary(
                                self.buffers[buffer_idx].widget_tree.as_ref()
                            ),
                        )
                    });
                    let is_active = self.active_buffer_idx() == buffer_idx;
                    let replaced = {
                        let buffer = &mut self.buffers[buffer_idx];
                        let replaced = buffer.replace_widget_subtree(
                            subtree_root_id,
                            tree.deep_clone(),
                            source_buffer_id,
                            reactive_dependencies.clone(),
                        );
                        if replaced {
                            buffer.view_mode = ViewMode::UiOnly;
                        }
                        replaced
                    };
                    if replaced {
                        if is_active {
                            crate::widget_render::clear_overlay();
                            let _ = self.runtime.replace_current_subtree(
                                subtree_root_id,
                                tree,
                                reactive_dependencies,
                            );
                            self.auto_focus_first_widget();
                        } else {
                            inactive_buffers_to_refresh
                                .entry(buffer_idx)
                                .or_insert_with(|| Some(Vec::new()))
                                .as_mut()
                                .map(|roots| roots.push(subtree_root_id));
                        }
                    }
                    self.trace_ui_tree_event_with(
                        &buffer_name,
                        if replaced {
                            "applied-subtree"
                        } else {
                            "missed-subtree"
                        },
                        || {
                            format!(
                                "root={subtree_root_id} after={}",
                                debug_widget_tree_summary(
                                    self.buffers[buffer_idx].widget_tree.as_ref()
                                )
                            )
                        },
                    );
                }
            }
        }
        for (buffer_idx, subtree_roots) in inactive_buffers_to_refresh {
            match subtree_roots {
                Some(subtree_roots) => {
                    self.refresh_inactive_tile_layouts_for_buffer_subtrees(
                        buffer_idx,
                        &subtree_roots,
                    );
                }
                None => self.refresh_inactive_tile_layouts_for_buffer(buffer_idx),
            }
        }

        // Process set-buffer-mode-for (after buffer creation so targets exist)
        for (buf_name, mode_name) in self.runtime.take_pending_set_mode_for() {
            let mode_def = self.mode_registry.get(&mode_name).cloned();
            if let Some(buf) = self.buffers.iter_mut().find(|b| b.name == buf_name) {
                buf.mode = BufferMode::Named(mode_name.clone());
                if let Some(mode_def) = &mode_def {
                    buf.read_only = mode_def.read_only;
                }
            }
        }

        if let Some(path) = self.runtime.take_pending_save_as() {
            match self.active_buffer_mut().save_as(path) {
                Ok(path) => self.show_transient_message(format!("Saved {}", path.display())),
                Err(error) => self.show_transient_message(format!("Error: {error}")),
            }
        } else if self.runtime.take_pending_save() {
            match self.save_active_buffer() {
                Ok(path) => self.show_transient_message(format!("Saved {}", path.display())),
                Err(error) => self.show_transient_message(format!("Error: {error:?}")),
            }
        } else if self.runtime.take_pending_load() {
            match self.load_active_buffer() {
                Ok(path) => {
                    self.completion = None;
                    self.show_transient_message(format!("Loaded {}", path.display()));
                }
                Err(error) => self.show_transient_message(format!("Error: {error:?}")),
            }
        }

        // ── Create scratch buffers (without switching) ──────────────────────
        for (name, text) in self.runtime.take_pending_scratch_buffers() {
            if let Some(buf) = self.buffers.iter_mut().find(|b| b.name == name) {
                buf.set_text(&text);
            } else {
                let id = self.alloc_buffer_id();
                let mut buffer = Buffer::new(id, &name);
                buffer.set_text(&text);
                self.buffers.push(buffer);
            }
        }

        // ── Process tiling operations (in order enqueued) ─────────────────────
        for op in self.runtime.take_pending_tile_ops() {
            match op {
                crate::runtime::TileOp::SplitRight(buf_name) => {
                    let new_buf_idx = match buf_name {
                        Some(ref name) if !name.is_empty() => self
                            .buffers
                            .iter()
                            .position(|b| b.name == *name)
                            .unwrap_or_else(|| self.find_or_create_scratch_buffer()),
                        _ => self.find_or_create_scratch_buffer(),
                    };
                    self.split_active_tile(SplitDir::Vertical, new_buf_idx);
                }
                crate::runtime::TileOp::SplitBelow(buf_name) => {
                    let new_buf_idx = match buf_name {
                        Some(ref name) if !name.is_empty() => self
                            .buffers
                            .iter()
                            .position(|b| b.name == *name)
                            .unwrap_or_else(|| self.find_or_create_scratch_buffer()),
                        _ => self.find_or_create_scratch_buffer(),
                    };
                    self.split_active_tile(SplitDir::Horizontal, new_buf_idx);
                }
                crate::runtime::TileOp::DeleteWindow => {
                    if !self.delete_active_tile() {
                        self.minibuffer = Some("Cannot delete the only window".to_string());
                    }
                }
                crate::runtime::TileOp::DeleteOtherWindows => {
                    self.delete_other_tiles();
                }
                crate::runtime::TileOp::OtherWindow => {
                    self.cycle_active_tile();
                }
                crate::runtime::TileOp::SetLayout(spec) => {
                    self.apply_layout_spec(spec);
                }
                crate::runtime::TileOp::SetWindowBuffer(name) => {
                    if let Some(idx) = self.buffers.iter().position(|b| b.name == name) {
                        self.save_current_widget_tree();
                        self.active_leaf_mut().buffer_idx = idx;
                        self.sync_runtime_context();
                        self.restore_buffer_widget_tree();
                        self.refresh_inactive_tile_layouts_for_buffer(idx);
                    } else {
                        self.minibuffer = Some(format!("No buffer named '{name}'"));
                    }
                }
                crate::runtime::TileOp::SetWindowBufferFor { current, new_name } => {
                    if !self.swap_buffer_in_tile_showing(&current, &new_name) {
                        self.minibuffer =
                            Some(format!("Could not swap '{current}' → '{new_name}'"));
                    }
                }
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

        if let Some(theme_map) = self.runtime.take_pending_apply_theme() {
            self.runtime.apply_theme_map(theme_map);
            crate::widget_render::bump_widget_state_generation();
            self.mark_needs_redraw();
        }

        if self.runtime.take_pending_cycle_view_mode() {
            self.toggle_active_buffer_view_mode();
        }

        if let Some(mode_str) = self.runtime.take_pending_set_view_mode() {
            if let Some(mode) = ViewMode::from_label(&mode_str) {
                self.set_active_buffer_view_mode(mode);
            } else {
                self.show_transient_message(format!("Unknown view mode: {mode_str}"));
            }
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
        self.active_text_drag_anchor = None;
    }

    fn clear_text_drag_anchor(&mut self) {
        self.active_text_drag_anchor = None;
    }

    fn apply_widget_output(&mut self, output: Option<crate::widget_render::EventOutput>) -> bool {
        let Some(output) = output else {
            return false;
        };
        self.sync_runtime_source_context();
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

    fn delete_active_region(&mut self) -> bool {
        let Some((start, end)) = self.active_region_range() else {
            return false;
        };
        self.active_buffer_mut().delete_range(start, end);
        self.clear_mark();
        true
    }

    fn copy_active_region_to_clipboard(&mut self) -> bool {
        let Some((start, end)) = self.active_region_range() else {
            return false;
        };
        let text = self.active_buffer().slice_range(start, end);
        match self.write_system_clipboard(&text) {
            Ok(()) => {
                self.kill_ring.push(text);
                self.minibuffer = None;
                true
            }
            Err(error) => {
                self.minibuffer = Some(format!("Clipboard error: {error}"));
                false
            }
        }
    }

    fn paste_from_system_clipboard(&mut self) {
        let text = match self.read_system_clipboard() {
            Ok(text) => text,
            Err(error) => {
                self.minibuffer = Some(format!("Clipboard error: {error}"));
                return;
            }
        };
        if text.is_empty() {
            return;
        }
        self.delete_active_region();
        self.active_buffer_mut().insert_str(&text);
        self.sync_text_horizontal_scroll_to_viewport();
        self.sync_runtime_context();
        self.refresh_completion();
    }

    fn write_system_clipboard(&mut self, text: &str) -> Result<(), String> {
        #[cfg(test)]
        if self.test_clipboard.is_some() {
            self.test_clipboard = Some(text.to_string());
            return Ok(());
        }

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

    fn read_system_clipboard(&mut self) -> Result<String, String> {
        #[cfg(test)]
        if let Some(text) = &self.test_clipboard {
            return Ok(text.clone());
        }

        let output = Command::new("pbpaste")
            .output()
            .map_err(|error| format!("failed to start pbpaste: {error}"))?;
        if !output.status.success() {
            return Err(format!("pbpaste exited with {}", output.status));
        }
        String::from_utf8(output.stdout)
            .map_err(|error| format!("clipboard did not contain UTF-8 text: {error}"))
    }

    #[cfg(test)]
    fn set_test_clipboard(&mut self, text: impl Into<String>) {
        self.test_clipboard = Some(text.into());
    }

    #[cfg(test)]
    fn test_clipboard(&self) -> Option<&str> {
        self.test_clipboard.as_deref()
    }

    fn guard_read_only(&mut self) -> bool {
        if self.active_buffer().view_mode == ViewMode::UiOnly {
            // Widget-only buffers silently ignore text insertion attempts.
            true
        } else if self.active_buffer().read_only {
            self.minibuffer = Some("Buffer is read-only".to_string());
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

fn normalize_region(a: (usize, usize), b: (usize, usize)) -> ((usize, usize), (usize, usize)) {
    if a < b { (a, b) } else { (b, a) }
}

fn is_symbol_char(ch: char) -> bool {
    !ch.is_whitespace()
        && !matches!(
            ch,
            '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | '`' | ',' | ';'
        )
}

fn collect_lisp_files(root: &std::path::Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_lisp_files(&path, out);
            continue;
        }
        if path.extension().is_some_and(|ext| ext == "lisp") {
            out.push(path);
        }
    }
}

fn find_definition_in_text(text: &str, symbol: &str) -> Option<(usize, usize)> {
    let targets = ["def", "defmacro", "defwidget", "defmode"];
    let bytes = text.as_bytes();
    let mut idx = 0usize;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;

    while idx < bytes.len() {
        let ch = bytes[idx] as char;
        if in_comment {
            if ch == '\n' {
                in_comment = false;
            }
            idx += 1;
            continue;
        }
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
            ';' => {
                in_comment = true;
                idx += 1;
            }
            '"' => {
                in_string = true;
                idx += 1;
            }
            '(' => {
                let form_depth = depth;
                depth += 1;
                idx += 1;
                if form_depth != 0 {
                    continue;
                }

                let mut cursor = skip_ws_and_comments(text, idx);
                let head_start = cursor;
                cursor = advance_symbol(text, cursor);
                if cursor == head_start {
                    continue;
                }
                let head = &text[head_start..cursor];
                if !targets.contains(&head) {
                    continue;
                }

                cursor = skip_ws_and_comments(text, cursor);
                let name_start = cursor;
                cursor = advance_symbol(text, cursor);
                if cursor == name_start {
                    continue;
                }
                if &text[name_start..cursor] == symbol {
                    return Some(offset_to_position(text, name_start));
                }
                idx = cursor;
            }
            ')' => {
                depth = depth.saturating_sub(1);
                idx += 1;
            }
            _ => idx += 1,
        }
    }

    None
}

fn skip_ws_and_comments(text: &str, mut idx: usize) -> usize {
    let bytes = text.as_bytes();
    while idx < bytes.len() {
        match bytes[idx] as char {
            ch if ch.is_whitespace() => idx += 1,
            ';' => {
                while idx < bytes.len() && bytes[idx] as char != '\n' {
                    idx += 1;
                }
            }
            _ => break,
        }
    }
    idx
}

fn advance_symbol(text: &str, mut idx: usize) -> usize {
    while idx < text.len() {
        let ch = text[idx..].chars().next().unwrap();
        if !is_symbol_char(ch) {
            break;
        }
        idx += ch.len_utf8();
    }
    idx
}

fn offset_to_position(text: &str, offset: usize) -> (usize, usize) {
    let mut row = 0usize;
    let mut col = 0usize;
    for ch in text[..offset.min(text.len())].chars() {
        if ch == '\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (row, col)
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

fn debug_symbol_prefix(line: &str, cursor_col: usize) -> Option<(usize, String)> {
    if cursor_col == 0 {
        return None;
    }
    let bytes = line.as_bytes();
    let cursor_col = cursor_col.min(bytes.len());
    let mut start = cursor_col;
    while start > 0 && debug_is_symbol_byte(bytes[start - 1]) {
        start -= 1;
    }
    if start == cursor_col {
        return None;
    }
    let prefix = line[start..cursor_col].to_ascii_lowercase();
    if prefix.is_empty() {
        return None;
    }
    Some((start, prefix))
}

fn debug_is_symbol_byte(byte: u8) -> bool {
    let ch = byte as char;
    !ch.is_whitespace()
        && !matches!(
            ch,
            '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | ';' | '#'
        )
}

fn max_layout_bottom(node: &crate::layout::LayoutNode) -> f32 {
    if node.widget_type == "scroll" {
        return node.rect.row + node.rect.height;
    }
    node.children
        .iter()
        .fold(node.rect.row + node.rect.height, |bottom, child| {
            bottom.max(max_layout_bottom(child))
        })
}

#[cfg(test)]
mod tests {
    use super::{Editor, EditorConfig, VimInputMode, key_str};
    use crate::host::HostCommand;
    use crate::mode::BufferMode;
    use crate::runtime::Runtime;
    use crate::tile::SplitDir;
    use crate::vm::Value;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
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
