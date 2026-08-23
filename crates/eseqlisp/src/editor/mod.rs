mod commands;
mod minibuffer;
mod natives;
pub(crate) mod widget_focus;
mod widget_interaction;

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};

use crate::buffer::{Buffer, InlineWidgetPlacement, debug_widget_tree_summary};
use crate::host::{BufferId, CompileKind, HostCommand, HostEvent};
use crate::hot_reload::{ReloadReport, SourceOverlay};
use crate::layout::{LayoutNode, Rect};
use crate::mode::{
    BufferMode, CompletionItem, CompletionMatch, TokenSpan, completion_match,
    has_completion_prefix, highlight_lines,
};
use crate::runtime::Runtime;
use crate::text::{innermost_sexp_range_at_cursor, sexp_at_cursor};
use crate::tile::{
    SplitDir, TileBufferTab, TileFrameCacheKey, TileId, TileLeaf, TileNode, TileSplit,
    split_ratio_for_point, tile_body_rect, tile_tab_layouts,
};
use crate::vm::{EffectTarget, PendingUiUpdate, ReactiveFieldKey, Value, format_lisp_value};
use crate::widget_render::WidgetCursor;
use commands::key_str;
use natives::register_editor_natives;

const TILE_GAP_PX_PER_UNIT: f32 = 15.0;
const TEXT_HORIZONTAL_SCROLL_MARGIN: usize = 3;

fn tile_resize_cursor(dir: SplitDir) -> WidgetCursor {
    match dir {
        SplitDir::Vertical => WidgetCursor::EwResize,
        SplitDir::Horizontal => WidgetCursor::NsResize,
    }
}

struct EditorSubtreeReplacement {
    buffer_idx: usize,
    source_buffer_id: Option<BufferId>,
    subtree_root_id: u64,
    tree: Value,
    reactive_dependencies: Vec<ReactiveFieldKey>,
}

#[derive(Clone, Copy)]
struct InlineLayoutPlacement {
    anchor_id: u64,
    row: f32,
    col: f32,
    width: Option<f32>,
    height: Option<f32>,
    stale: bool,
}

enum PatcherSourceTabTarget {
    PatcherNode {
        node: crate::layout::LayoutNode,
        buffer_id: BufferId,
        tile_id: Option<TileId>,
    },
    SourceBuffer {
        path: String,
    },
    PatcherBuffer {
        path: String,
        buffer_id: BufferId,
        tile_id: Option<TileId>,
    },
}

struct ResolvedPatcherSourceTabTarget {
    path: String,
    patcher_buffer_id: BufferId,
    patcher_tile_id: Option<TileId>,
    source_buffer_id: Option<BufferId>,
}

fn metal_tile_content_viewport(
    rect: &Rect,
    show_status: bool,
    show_border: bool,
    border_width_px: f32,
    cell_w: f32,
    cell_h: f32,
) -> (f32, f32) {
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
    let cols = (rect.width - border_inset_px * 2.0 / cell_w.max(1.0)).max(0.0);
    let content_height =
        rect.height - border_inset_px * 2.0 / cell_h.max(1.0) - if show_status { 1.0 } else { 0.0 };
    (cols.max(1.0), content_height.max(1.0))
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

fn layout_root_matches_viewport(layout: &LayoutNode, cols: f32, rows: f32) -> bool {
    crate::layout::layout_root_matches_viewport(layout, cols, rows)
}

fn viewport_matches(stored_cols: f32, stored_rows: f32, cols: f32, rows: f32) -> bool {
    const EPSILON: f32 = 0.05;
    stored_cols > 0.0
        && stored_rows > 0.0
        && (stored_cols - cols).abs() <= EPSILON
        && (stored_rows - rows).abs() <= EPSILON
}

fn leaf_viewport_matches(leaf: &TileLeaf, cols: f32, rows: f32) -> bool {
    viewport_matches(
        leaf.widget_viewport_width,
        leaf.widget_viewport_height,
        cols,
        rows,
    )
}

fn leaf_cached_layout_matches_viewport(
    leaf: &TileLeaf,
    layout: &LayoutNode,
    cols: f32,
    rows: f32,
) -> bool {
    if leaf.widget_viewport_width > 0.0 && leaf.widget_viewport_height > 0.0 {
        leaf_viewport_matches(leaf, cols, rows)
    } else {
        layout_root_matches_viewport(layout, cols, rows)
    }
}

fn frame_viewport_matches(a: Option<crate::layout::Rect>, b: Option<crate::layout::Rect>) -> bool {
    const EPSILON: f32 = 0.05;
    match (a, b) {
        (Some(a), Some(b)) => {
            (a.row - b.row).abs() <= EPSILON
                && (a.col - b.col).abs() <= EPSILON
                && (a.width - b.width).abs() <= EPSILON
                && (a.height - b.height).abs() <= EPSILON
        }
        (None, None) => true,
        _ => false,
    }
}

fn leaf_cached_layout_matches_geometry(
    leaf: &TileLeaf,
    layout: &LayoutNode,
    cols: f32,
    rows: f32,
    frame_viewport: Option<crate::layout::Rect>,
) -> bool {
    leaf_cached_layout_matches_viewport(leaf, layout, cols, rows)
        && frame_viewport_matches(leaf.layout_frame_viewport, frame_viewport)
}

fn format_lisp_reload_report(report: &ReloadReport) -> String {
    let mut lines = Vec::new();
    lines.push(if report.success {
        "Lisp reload: success".to_string()
    } else {
        "Lisp reload: failure".to_string()
    });
    if let Some(path) = &report.requested_path {
        lines.push(format!("requested: {}", path.display()));
    }
    if let Some(path) = &report.evaluated_path {
        lines.push(format!("evaluated: {}", path.display()));
    }
    if !report.changed_symbols.is_empty() {
        lines.push(format!(
            "changed symbols: {}",
            report.changed_symbols.join(", ")
        ));
    }
    if !report.rerendered_roots.is_empty() {
        lines.push(format!(
            "rerendered roots: {}",
            report.rerendered_roots.join(", ")
        ));
    }
    if !report.diagnostics.is_empty() {
        lines.push(String::new());
        lines.push("diagnostics:".to_string());
        lines.extend(report.diagnostics.iter().cloned());
    }
    lines.join("\n")
}

fn is_inspect_mode_toggle_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('i') | KeyCode::Char('I'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && key.modifiers.contains(KeyModifiers::SHIFT)
        && !key
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SUPER)
}

fn inspect_node_source_buffer_id(node: &crate::layout::LayoutNode) -> Option<BufferId> {
    match node.props.get(crate::vm::SOURCE_BUFFER_ID_PROP) {
        Some(Value::Number(value)) if value.is_finite() && *value >= 0.0 => {
            Some(*value as BufferId)
        }
        _ => None,
    }
}

fn inspect_node_source_file_path(node: &crate::layout::LayoutNode) -> Option<PathBuf> {
    match node.props.get(crate::vm::SOURCE_MODULE_PATH_PROP) {
        Some(Value::String(path)) if !path.is_empty() => Some(PathBuf::from(path)),
        _ => None,
    }
}

fn inspect_node_source_symbol(node: &crate::layout::LayoutNode) -> Option<String> {
    match node.props.get(crate::vm::SOURCE_SYMBOL_PROP) {
        Some(Value::String(symbol)) if !symbol.is_empty() => Some(symbol.clone()),
        _ => None,
    }
}

fn inspect_node_source_number(node: &crate::layout::LayoutNode, key: &str) -> Option<usize> {
    match node.props.get(key) {
        Some(Value::Number(value)) if value.is_finite() && *value >= 0.0 => Some(*value as usize),
        _ => None,
    }
}

fn inspect_node_source_span(node: &crate::layout::LayoutNode) -> Option<(usize, usize)> {
    let start = inspect_node_source_number(node, crate::vm::SOURCE_START_BYTE_PROP)?;
    let end = inspect_node_source_number(node, crate::vm::SOURCE_END_BYTE_PROP)?;
    (end >= start).then_some((start, end))
}

fn inspect_node_source_revision(node: &crate::layout::LayoutNode) -> Option<u64> {
    match node.props.get(crate::vm::SOURCE_REVISION_PROP) {
        Some(Value::String(value)) => value.parse::<u64>().ok(),
        Some(Value::Number(value)) if value.is_finite() && *value >= 0.0 => Some(*value as u64),
        _ => None,
    }
}

fn inspect_node_prop_string(node: &crate::layout::LayoutNode, key: &str) -> Option<String> {
    match node.props.get(key) {
        Some(Value::String(value) | Value::Keyword(value)) if !value.is_empty() => {
            Some(value.clone())
        }
        Some(Value::Number(value)) if value.is_finite() => Some(value.to_string()),
        Some(Value::Bool(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn inspect_node_debug_label(node: &crate::layout::LayoutNode) -> String {
    let detail = node
        .props
        .get("debug-name")
        .and_then(|value| match value {
            Value::String(value) | Value::Keyword(value) => Some(value.as_str()),
            _ => None,
        })
        .or(node.stable_key.as_deref());
    match detail {
        Some(detail) => format!("{} {detail}", node.widget_type),
        None => node.widget_type.clone(),
    }
}

fn inspect_node_has_source_identity(node: &crate::layout::LayoutNode) -> bool {
    inspect_node_source_span(node).is_some()
        || inspect_node_prop_string(node, "debug-name").is_some()
        || inspect_node_prop_string(node, "key").is_some()
        || node.stable_key.is_some()
        || inspect_node_source_symbol(node).is_some()
}

fn inspect_debug_log(message: impl AsRef<str>) {
    eprintln!("[inspect] {}", message.as_ref());
}

fn format_cursor_for_log(cursor: (usize, usize)) -> String {
    format!("{}:{}", cursor.0 + 1, cursor.1 + 1)
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
    pub init_source_path: Option<PathBuf>,
    pub vim_mode: bool,
}

pub const DEFAULT_TEXT_ZOOM: f32 = 0.85;
pub const MIN_TEXT_ZOOM: f32 = 0.5;
pub const MAX_TEXT_ZOOM: f32 = 2.0;

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
    /// Whether a host may interpret otherwise-unhandled bare keys as live
    /// performance input while this mode owns the active buffer.
    pub live_keys: bool,
    pub keybindings: HashMap<String, String>,
    pub on_enter: Option<String>,
    pub on_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LayoutRefreshTiming {
    pub buffer_name: String,
    pub tile_id: Option<TileId>,
    pub mode: String,
    pub elapsed: Duration,
}

#[derive(Clone)]
struct RetainedTileLayout {
    buffer_id: BufferId,
    widget_tree_revision: u64,
    layout_revision: u64,
    viewport_width_bits: u32,
    viewport_height_bits: u32,
    cached_layout: Arc<LayoutNode>,
    cached_inactive_frame: Option<(TileFrameCacheKey, crate::backend::RenderFrame)>,
    focused_widget_id: Option<u64>,
    focused_widget_node: Option<LayoutNode>,
    widget_scroll_top: f32,
    widget_scroll_left: f32,
    widget_viewport_width: f32,
    widget_viewport_height: f32,
    layout_frame_viewport: Option<crate::layout::Rect>,
}

impl RetainedTileLayout {
    fn from_leaf(
        buffer: &Buffer,
        leaf: &TileLeaf,
        cached_layout: Option<Arc<LayoutNode>>,
        layout_revision: u64,
    ) -> Option<Self> {
        let cached_layout = cached_layout?;
        let viewport_width = if leaf.widget_viewport_width > 0.0 {
            leaf.widget_viewport_width
        } else {
            cached_layout.rect.width
        };
        let viewport_height = if leaf.widget_viewport_height > 0.0 {
            leaf.widget_viewport_height
        } else {
            cached_layout.rect.height
        };
        Some(Self {
            buffer_id: buffer.id,
            widget_tree_revision: buffer.widget_tree_revision,
            layout_revision,
            viewport_width_bits: viewport_width.to_bits(),
            viewport_height_bits: viewport_height.to_bits(),
            cached_layout,
            cached_inactive_frame: leaf.cached_inactive_frame.clone(),
            focused_widget_id: leaf.focused_widget_id,
            focused_widget_node: leaf.focused_widget_node.clone(),
            widget_scroll_top: leaf.widget_scroll_top,
            widget_scroll_left: leaf.widget_scroll_left,
            widget_viewport_width: viewport_width,
            widget_viewport_height: viewport_height,
            layout_frame_viewport: leaf.layout_frame_viewport,
        })
    }

    fn restore_to_leaf(&self, buffer: &Buffer, leaf: &mut TileLeaf) -> bool {
        self.restore_to_leaf_for(buffer.id, buffer.widget_tree_revision, leaf)
    }

    fn restore_to_leaf_for(
        &self,
        buffer_id: BufferId,
        widget_tree_revision: u64,
        leaf: &mut TileLeaf,
    ) -> bool {
        if self.buffer_id != buffer_id || self.widget_tree_revision != widget_tree_revision {
            return false;
        }
        leaf.cached_layout = Some(self.cached_layout.clone());
        leaf.cached_layout_widget_tree_revision = self.widget_tree_revision;
        leaf.layout_revision = self.layout_revision;
        leaf.focused_widget_id = self.focused_widget_id;
        leaf.focused_widget_node = self.focused_widget_node.clone();
        leaf.widget_scroll_top = self.widget_scroll_top;
        leaf.widget_scroll_left = self.widget_scroll_left;
        leaf.widget_viewport_width = self.widget_viewport_width;
        leaf.widget_viewport_height = self.widget_viewport_height;
        leaf.layout_frame_viewport = self.layout_frame_viewport;
        leaf.cached_inactive_frame = self.cached_inactive_frame.clone();
        true
    }

    fn matches_viewport(&self, cols: f32, rows: f32) -> bool {
        viewport_matches(
            f32::from_bits(self.viewport_width_bits),
            f32::from_bits(self.viewport_height_bits),
            cols,
            rows,
        )
    }
    fn matches_geometry(
        &self,
        cols: f32,
        rows: f32,
        frame_viewport: Option<crate::layout::Rect>,
    ) -> bool {
        self.matches_viewport(cols, rows)
            && frame_viewport_matches(self.layout_frame_viewport, frame_viewport)
    }
}

pub struct Editor {
    pub buffers: Vec<Buffer>,
    buffer_recency: Vec<BufferId>,
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
    runtime_source_context_revision: Option<RuntimeSourceContextRevision>,
    needs_redraw: bool,
    should_quit: bool,
    last_exit: EditorExit,
    next_buffer_id: BufferId,
    patcher_emitted_source_origins: HashMap<String, BufferId>,
    save_prompt: Option<SavePrompt>,
    completion: Option<CompletionState>,
    last_mouse_precise: Option<(f32, f32)>,
    active_tile_resize_drag: Option<TileResizeDrag>,
    eval_flash: Option<SExpFlash>,
    mark: Option<Mark>,
    jump_stack: Vec<Mark>,
    active_text_drag_anchor: Option<Mark>,
    /// `Some` while an open modal traps focus; the inner value is the widget
    /// focused before the modal opened (restored on close, `None` = nothing).
    modal_focus_return: Option<Option<LayoutNode>>,
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
    /// Full tiled frame size used to derive frame-local overlay geometry.
    cached_tiled_frame_size: Option<(f32, f32)>,
    /// Outer margin around the tiled layout, in cell units.
    tile_outer_gap: f32,
    remembered_split_ratios: HashMap<String, f32>,
    /// Last selected buffer for each declarative tab group. Unlike tile paths,
    /// a group's ordered buffer IDs remain stable when surrounding panes are
    /// added, removed, or temporarily replaced by another workspace.
    remembered_tab_selections: HashMap<Vec<BufferId>, BufferId>,
    retained_tile_layouts: HashMap<BufferId, Vec<RetainedTileLayout>>,
    visible_binding_layout_signature: Option<VisibleBindingLayoutSignature>,
    visible_binding_registry_revision: u64,
    /// While true, `sync_reactive_bindings_for_visible_layouts` records a
    /// pending request instead of running. `refresh_runtime_side_effects`
    /// sets this around its inactive-buffer refresh loop so the visible
    /// binding scan (which walks every visible layout) runs once per flush
    /// instead of once per refreshed buffer.
    visible_binding_sync_deferred: bool,
    visible_binding_sync_pending: bool,
    /// Per-tile extracted widget-binding entries, keyed by the layout Arc
    /// pointer + layout revision that produced them. Rebuilding the binding
    /// table only rescans tiles whose layout actually changed.
    visible_binding_tile_entries:
        HashMap<TileId, (usize, u64, Arc<Vec<(crate::vm::ReactiveBindingKey, u64)>>)>,
    widget_cursor: WidgetCursor,
    suppress_mouse_until_left_up: bool,
    active_tab_mouse_capture: Option<TileId>,
    hovered_tile_tab: Option<(TileId, usize)>,
    pointer_drag_started_on_slider: bool,
    last_slider_drag_widget_id: Option<u64>,
    pending_inline_writeback: Option<(BufferId, u64)>,
    text_zoom: f32,
    text_cell_width_scale: f32,
    text_cell_height_scale: f32,
    last_layout_refresh_timings: Vec<LayoutRefreshTiming>,
    inspect_mode: bool,
    inspect_hover_tile_id: Option<TileId>,
    inspect_hover_widget_id: Option<u64>,
    inspect_hover_status: Option<String>,
    inspect_hover_rect: Option<crate::layout::Rect>,
    inspect_source_tile_id: Option<TileId>,
    #[cfg(test)]
    test_clipboard: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeSourceContextRevision {
    buffer_id: BufferId,
    text_revision: u64,
    cursor: (usize, usize),
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

    fn trace_ui_layout_enabled_for(&self, buffer_name: &str) -> bool {
        std::env::var("ESEQLISP_TRACE_UI_LAYOUT")
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

    fn trace_ui_layout_event(
        &self,
        buffer_name: &str,
        stage: &str,
        layout: Option<&crate::layout::LayoutNode>,
    ) {
        if !self.trace_ui_layout_enabled_for(buffer_name) {
            return;
        }
        eprintln!("[ui-layout][{buffer_name}] {stage}");
        match layout {
            Some(layout) => {
                for line in crate::layout::format_layout_tree_lines(layout, 0) {
                    eprintln!("[ui-layout][{buffer_name}] {line}");
                }
            }
            None => {
                eprintln!("[ui-layout][{buffer_name}] <no layout>");
            }
        }
    }

    fn record_layout_refresh_timing(
        &mut self,
        buffer_name: &str,
        tile_id: Option<TileId>,
        mode: &str,
        elapsed: Duration,
    ) {
        self.last_layout_refresh_timings.push(LayoutRefreshTiming {
            buffer_name: buffer_name.to_string(),
            tile_id,
            mode: mode.to_string(),
            elapsed,
        });
    }

    pub fn last_layout_refresh_timings(&self) -> &[LayoutRefreshTiming] {
        &self.last_layout_refresh_timings
    }

    pub fn new(mut runtime: Runtime, config: EditorConfig) -> Self {
        register_editor_natives(&mut runtime);

        let mut editor = Editor {
            buffers: vec![Buffer::new(0, "*scratch*")],
            buffer_recency: vec![0],
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
            runtime_source_context_revision: None,
            needs_redraw: true,
            should_quit: false,
            last_exit: EditorExit::Closed,
            next_buffer_id: 1,
            patcher_emitted_source_origins: HashMap::new(),
            save_prompt: None,
            completion: None,
            last_mouse_precise: None,
            active_tile_resize_drag: None,
            eval_flash: None,
            mark: None,
            jump_stack: vec![],
            active_text_drag_anchor: None,
            modal_focus_return: None,
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
            cached_tiled_frame_size: None,
            tile_outer_gap: 0.0,
            remembered_split_ratios: HashMap::new(),
            remembered_tab_selections: HashMap::new(),
            retained_tile_layouts: HashMap::new(),
            visible_binding_layout_signature: None,
            visible_binding_registry_revision: 0,
            visible_binding_sync_deferred: false,
            visible_binding_sync_pending: false,
            visible_binding_tile_entries: HashMap::new(),
            widget_cursor: WidgetCursor::Default,
            suppress_mouse_until_left_up: false,
            active_tab_mouse_capture: None,
            hovered_tile_tab: None,
            pointer_drag_started_on_slider: false,
            last_slider_drag_widget_id: None,
            pending_inline_writeback: None,
            text_zoom: DEFAULT_TEXT_ZOOM,
            text_cell_width_scale: DEFAULT_TEXT_ZOOM,
            text_cell_height_scale: DEFAULT_TEXT_ZOOM,
            last_layout_refresh_timings: Vec::new(),
            inspect_mode: false,
            inspect_hover_tile_id: None,
            inspect_hover_widget_id: None,
            inspect_hover_status: None,
            inspect_hover_rect: None,
            inspect_source_tile_id: None,
            #[cfg(test)]
            test_clipboard: None,
        };
        editor.bind_defaults();
        editor.load_init(
            config.init_source.as_deref(),
            config.init_source_path.as_deref(),
        );
        editor.refresh_runtime_side_effects();
        editor.sync_runtime_context();
        editor
    }

    pub fn widget_cursor(&self) -> WidgetCursor {
        self.widget_cursor
    }

    pub fn text_zoom(&self) -> f32 {
        self.text_zoom
    }

    pub fn set_text_zoom(&mut self, zoom: f32) -> Result<f32, String> {
        if !zoom.is_finite() {
            return Err("text zoom must be finite".to_string());
        }
        if !(MIN_TEXT_ZOOM..=MAX_TEXT_ZOOM).contains(&zoom) {
            return Err(format!(
                "text zoom must be between {MIN_TEXT_ZOOM:.2} and {MAX_TEXT_ZOOM:.2}"
            ));
        }
        if (self.text_zoom - zoom).abs() > f32::EPSILON {
            self.text_zoom = zoom;
            // Until a graphical backend reports exact zoomed font metrics, use
            // the requested zoom as the logical text-cell scale.
            self.text_cell_width_scale = zoom;
            self.text_cell_height_scale = zoom;
            self.invalidate_text_frame_caches();
            self.mark_needs_redraw();
        }
        Ok(self.text_zoom)
    }

    pub fn set_text_cell_dimensions(
        &mut self,
        layout_cell_w: f32,
        layout_cell_h: f32,
        text_cell_w: f32,
        text_cell_h: f32,
    ) {
        if layout_cell_w <= 0.0
            || layout_cell_h <= 0.0
            || text_cell_w <= 0.0
            || text_cell_h <= 0.0
            || !layout_cell_w.is_finite()
            || !layout_cell_h.is_finite()
            || !text_cell_w.is_finite()
            || !text_cell_h.is_finite()
        {
            return;
        }
        let next_width = text_cell_w / layout_cell_w;
        let next_height = text_cell_h / layout_cell_h;
        if (self.text_cell_width_scale - next_width).abs() > 0.001
            || (self.text_cell_height_scale - next_height).abs() > 0.001
        {
            self.text_cell_width_scale = next_width;
            self.text_cell_height_scale = next_height;
            self.invalidate_text_frame_caches();
            self.mark_needs_redraw();
        }
    }

    pub(crate) fn text_cell_scales_for_buffer(&self, buffer: &Buffer) -> (f32, f32) {
        if buffer.view_mode != ViewMode::UiOnly {
            (
                self.text_cell_width_scale.max(0.001),
                self.text_cell_height_scale.max(0.001),
            )
        } else {
            (1.0, 1.0)
        }
    }

    fn invalidate_text_frame_caches(&mut self) {
        for tile_id in self.tile_root.leaf_ids() {
            if let Some(leaf) = self.tile_root.find_leaf_mut(tile_id) {
                leaf.cached_inactive_frame = None;
            }
        }
    }

    pub fn active_vim_input_mode(&self) -> Option<VimInputMode> {
        self.vim_applies_to_active_buffer()
            .then_some(self.vim_input_mode)
    }

    pub fn vim_status_label(&self) -> Option<&'static str> {
        Some(match self.active_vim_input_mode()? {
            VimInputMode::Normal => "NORMAL",
            VimInputMode::Insert => "INSERT",
        })
    }

    pub(crate) fn pending_key_prompt(&self) -> Option<String> {
        self.pending_key.map(|key| format!("{} -", key_str(key)))
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

    fn remember_visible_tile_layouts(&mut self) {
        let active_tile = self.active_tile;
        let runtime_layout = self.runtime.current_layout.clone();
        let runtime_layout_revision = self.runtime.layout_revision();
        let retained = self
            .tile_root
            .leaf_ids()
            .into_iter()
            .filter_map(|tile_id| {
                let leaf = self.tile_root.find_leaf(tile_id)?;
                let buffer = self.buffers.get(leaf.buffer_idx)?;
                let cached_layout = if tile_id == active_tile {
                    runtime_layout
                        .clone()
                        .or_else(|| leaf.cached_layout.clone())
                } else {
                    leaf.cached_layout.clone()
                };
                let layout_revision = if tile_id == active_tile && runtime_layout.is_some() {
                    runtime_layout_revision
                } else {
                    leaf.layout_revision
                };
                RetainedTileLayout::from_leaf(buffer, leaf, cached_layout, layout_revision)
            })
            .collect::<Vec<_>>();
        for layout in retained {
            self.remember_retained_tile_layout(layout);
        }
        self.prune_retained_tile_layouts();
    }

    fn remember_retained_tile_layout(&mut self, retained: RetainedTileLayout) {
        const MAX_RETAINED_LAYOUTS_PER_BUFFER: usize = 4;
        let entries = self
            .retained_tile_layouts
            .entry(retained.buffer_id)
            .or_default();
        if let Some(existing) = entries.iter_mut().find(|existing| {
            existing.widget_tree_revision == retained.widget_tree_revision
                && existing.viewport_width_bits == retained.viewport_width_bits
                && existing.viewport_height_bits == retained.viewport_height_bits
                && frame_viewport_matches(
                    existing.layout_frame_viewport,
                    retained.layout_frame_viewport,
                )
        }) {
            *existing = retained;
            return;
        }
        entries.push(retained);
        if entries.len() > MAX_RETAINED_LAYOUTS_PER_BUFFER {
            entries.remove(0);
        }
    }

    fn prune_retained_tile_layouts(&mut self) {
        self.retained_tile_layouts.retain(|buffer_id, retained| {
            let Some(buffer) = self.buffers.iter().find(|buffer| buffer.id == *buffer_id) else {
                return false;
            };
            retained.retain(|layout| layout.widget_tree_revision == buffer.widget_tree_revision);
            !retained.is_empty()
        });
    }

    fn retained_tile_layout_for_geometry(
        &self,
        buffer: &Buffer,
        cols: f32,
        rows: f32,
        frame_viewport: Option<crate::layout::Rect>,
    ) -> Option<RetainedTileLayout> {
        self.retained_tile_layouts
            .get(&buffer.id)?
            .iter()
            .rev()
            .find(|retained| {
                retained.widget_tree_revision == buffer.widget_tree_revision
                    && retained.matches_geometry(cols, rows, frame_viewport)
            })
            .cloned()
    }

    fn record_buffer_access_by_idx(&mut self, buffer_idx: usize) {
        let Some(buffer) = self.buffers.get(buffer_idx) else {
            return;
        };
        let id = buffer.id;
        self.buffer_recency.retain(|existing| *existing != id);
        self.buffer_recency.insert(0, id);
    }

    fn track_new_buffer(&mut self, id: BufferId, active: bool) {
        self.buffer_recency.retain(|existing| *existing != id);
        if active {
            self.buffer_recency.insert(0, id);
        } else {
            self.buffer_recency.push(id);
        }
    }

    fn buffer_names_by_recency(&mut self) -> Vec<String> {
        let ids: std::collections::HashSet<BufferId> =
            self.buffers.iter().map(|buffer| buffer.id).collect();
        self.buffer_recency.retain(|id| ids.contains(id));

        for buffer in &self.buffers {
            if !self.buffer_recency.contains(&buffer.id) {
                self.buffer_recency.push(buffer.id);
            }
        }

        self.buffer_recency
            .iter()
            .filter_map(|id| {
                self.buffers
                    .iter()
                    .find(|buffer| buffer.id == *id)
                    .map(|buffer| buffer.name.clone())
            })
            .collect()
    }

    /// Recompute cached tile rects for the given viewport.
    pub fn update_tile_rects(&mut self, total_width: u16, total_height: u16) {
        self.cached_tiled_frame_size = Some((total_width as f32, total_height as f32));
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
        let viewport_sizes: Vec<(TileId, f32, f32)> = self
            .cached_tile_rects
            .iter()
            .filter_map(|(tile_id, rect)| {
                let leaf = self.tile_root.find_leaf(*tile_id)?;
                let buffer = self.buffers.get(leaf.buffer_idx)?;
                let show_status = self
                    .tile_effective_show_status(*tile_id)
                    .unwrap_or(leaf.show_status || buffer.view_mode != ViewMode::UiOnly);
                let body_rect = tile_body_rect(*rect, !leaf.tabs.is_empty());
                let (viewport_width, _) = metal_tile_content_viewport(
                    &body_rect,
                    show_status,
                    leaf.show_border,
                    leaf.border_width_px,
                    cell_w,
                    cell_h,
                );
                let viewport_height = metal_tile_content_viewport_height_exact(
                    &body_rect,
                    show_status,
                    leaf.show_border,
                    leaf.border_width_px,
                    cell_w,
                    cell_h,
                );
                Some((*tile_id, viewport_width, viewport_height))
            })
            .collect();
        // If rects changed, invalidate only inactive layouts whose cached root
        // no longer matches the tile's content viewport.
        if old_rects != self.cached_tile_rects {
            let inactive_layout_validity = self
                .tile_root
                .leaf_ids()
                .into_iter()
                .filter(|id| *id != self.active_tile)
                .filter_map(|id| {
                    let frame_viewport = self.tile_layout_frame_viewport(id)?;
                    let leaf = self.tile_root.find_leaf(id)?;
                    let rect = self
                        .cached_tile_rects
                        .iter()
                        .find(|(tile_id, _)| *tile_id == id)
                        .map(|(_, rect)| *rect)?;
                    let buffer = self.buffers.get(leaf.buffer_idx)?;
                    let show_status = self
                        .tile_effective_show_status(id)
                        .unwrap_or(leaf.show_status || buffer.view_mode != ViewMode::UiOnly);
                    let (cols, rows) = metal_tile_content_viewport(
                        &tile_body_rect(rect, !leaf.tabs.is_empty()),
                        show_status,
                        leaf.show_border,
                        leaf.border_width_px,
                        cell_w,
                        cell_h,
                    );
                    let valid = leaf.cached_layout.as_ref().is_some_and(|layout| {
                        leaf.cached_layout_widget_tree_revision == buffer.widget_tree_revision
                            && leaf_cached_layout_matches_geometry(
                                leaf,
                                layout,
                                cols,
                                rows,
                                Some(frame_viewport),
                            )
                    });
                    let retained = if valid {
                        None
                    } else {
                        self.retained_tile_layout_for_geometry(
                            buffer,
                            cols,
                            rows,
                            Some(frame_viewport),
                        )
                    };
                    Some((
                        id,
                        leaf.buffer_idx,
                        buffer.id,
                        buffer.widget_tree_revision,
                        valid,
                        retained,
                    ))
                })
                .collect::<Vec<_>>();

            let mut buf_indices: Vec<usize> = Vec::new();
            for (id, buffer_idx, buffer_id, widget_tree_revision, valid, retained) in
                inactive_layout_validity
            {
                if valid {
                    continue;
                }
                if let Some(leaf) = self.tile_root.find_leaf_mut(id) {
                    if let Some(retained) = retained {
                        retained.restore_to_leaf_for(buffer_id, widget_tree_revision, leaf);
                        continue;
                    }
                    leaf.cached_layout = None;
                    leaf.cached_layout_widget_tree_revision = 0;
                    leaf.layout_frame_viewport = None;
                    leaf.dirty_widget_ids.clear();
                    leaf.cached_inactive_frame = None;
                    if !buf_indices.contains(&buffer_idx) {
                        buf_indices.push(buffer_idx);
                    }
                }
            }
            for buf_idx in buf_indices {
                self.refresh_inactive_tile_layouts_for_buffer(buf_idx);
            }
        }
        for (tile_id, viewport_width, viewport_height) in viewport_sizes {
            if let Some(leaf) = self.tile_root.find_leaf_mut(tile_id) {
                leaf.widget_viewport_width = viewport_width;
                leaf.widget_viewport_height = viewport_height;
            }
        }
    }

    /// Full-frame viewport expressed in a tile's local content coordinates.
    /// This is the geometry frame-anchored widgets must be laid out against,
    /// regardless of whether their owner tile is currently active.
    pub(crate) fn tile_layout_frame_viewport(
        &self,
        tile_id: TileId,
    ) -> Option<crate::layout::Rect> {
        let (frame_width, frame_height) = self.cached_tiled_frame_size?;
        let leaf = self.tile_root.find_leaf(tile_id)?;
        let tile_rect = self
            .cached_tile_rects
            .iter()
            .find(|(id, _)| *id == tile_id)
            .map(|(_, rect)| *rect)?;
        let body_rect = tile_body_rect(tile_rect, !leaf.tabs.is_empty());
        let (border_col, border_row) = self.tile_content_border_insets(tile_id, 0);
        Some(crate::layout::Rect {
            row: -(body_rect.row + border_row),
            col: -(body_rect.col + border_col),
            width: frame_width,
            height: frame_height,
        })
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

    pub fn tile_body_rect(&self, tile_id: TileId) -> Option<Rect> {
        let rect = self.tile_rect(tile_id)?;
        let leaf = self.tile_root.find_leaf(tile_id)?;
        Some(tile_body_rect(rect, !leaf.tabs.is_empty()))
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
        viewport: Option<(f32, f32)>,
    ) {
        if new_tile == self.active_tile {
            return;
        }
        let Some(target_leaf) = self.tile_root.find_leaf(new_tile) else {
            return;
        };
        let layout_frame_viewport = target_leaf.layout_frame_viewport;
        let cached_layout = target_leaf.cached_layout.as_ref().and_then(|layout| {
            viewport
                .map(|(cols, rows)| {
                    leaf_cached_layout_matches_geometry(
                        target_leaf,
                        layout,
                        cols,
                        rows,
                        layout_frame_viewport,
                    )
                })
                .unwrap_or(true)
                .then(|| layout.clone())
        });
        let layout_revision = target_leaf.layout_revision;
        let buffer_idx = target_leaf.buffer_idx;
        self.save_current_widget_tree();
        self.active_tile = new_tile;
        self.record_buffer_access_by_idx(buffer_idx);
        self.sync_runtime_context();
        self.runtime
            .set_layout_frame_viewport(layout_frame_viewport);
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

    fn split_root_right_with_buffer(&mut self, new_buffer_idx: usize) -> TileId {
        self.save_current_widget_tree();
        let split_id = self.alloc_tile_id();
        let new_tile_id = self.alloc_tile_id();
        let existing_root = std::mem::replace(
            &mut self.tile_root,
            TileNode::Leaf(TileLeaf::new(new_tile_id, new_buffer_idx)),
        );
        self.tile_root = TileNode::Split(TileSplit {
            id: split_id,
            dir: SplitDir::Vertical,
            ratio: 0.72,
            gap: 0.0,
            remember_key: None,
            a: Box::new(existing_root),
            b: Box::new(TileNode::Leaf(TileLeaf::new(new_tile_id, new_buffer_idx))),
        });
        self.switch_active_tile(new_tile_id);
        new_tile_id
    }

    fn replace_tile_buffer_and_activate(&mut self, tile_id: TileId, buffer_idx: usize) -> bool {
        if self.tile_root.find_leaf(tile_id).is_none() {
            return false;
        }
        let was_active = self.active_tile == tile_id;
        self.save_current_widget_tree();
        {
            let Some(leaf) = self.tile_root.find_leaf_mut(tile_id) else {
                return false;
            };
            leaf.buffer_idx = buffer_idx;
            leaf.selected_tab = leaf
                .tabs
                .iter()
                .position(|tab| tab.buffer_idx == buffer_idx);
            Self::invalidate_leaf_for_buffer_switch(leaf);
        }
        if was_active {
            self.record_buffer_access_by_idx(buffer_idx);
            self.sync_runtime_context();
            self.completion = None;
            self.clear_mark();
            self.restore_buffer_widget_tree();
            self.mark_needs_redraw();
        } else {
            self.switch_active_tile(tile_id);
        }
        true
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
            let buffer_idx = self.active_buffer_idx();
            self.record_buffer_access_by_idx(buffer_idx);
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

        let outer_gap = match &spec {
            LayoutSpec::Rows { gap, .. } | LayoutSpec::Cols { gap, .. } => *gap,
            LayoutSpec::Buffer { .. } => 0.0,
        };
        let previous_active_buffer_id = self
            .tile_root
            .find_leaf(self.active_tile)
            .and_then(|leaf| self.buffers.get(leaf.buffer_idx))
            .map(|buffer| buffer.id);

        fn remember_tab_selections(
            node: &TileNode,
            bufs: &[Buf],
            out: &mut HashMap<Vec<BufferId>, BufferId>,
        ) {
            match node {
                TileNode::Leaf(leaf) => {
                    let tab_buffer_ids = leaf
                        .tabs
                        .iter()
                        .map(|tab| bufs.get(tab.buffer_idx).map(|buffer| buffer.id))
                        .collect::<Option<Vec<_>>>();
                    let selected_buffer_id = leaf
                        .selected_tab
                        .and_then(|index| leaf.tabs.get(index))
                        .and_then(|tab| bufs.get(tab.buffer_idx))
                        .map(|buffer| buffer.id);
                    if let (Some(tab_buffer_ids), Some(selected_buffer_id)) =
                        (tab_buffer_ids, selected_buffer_id)
                        && !tab_buffer_ids.is_empty()
                    {
                        out.insert(tab_buffer_ids, selected_buffer_id);
                    }
                }
                TileNode::Split(split) => {
                    remember_tab_selections(&split.a, bufs, out);
                    remember_tab_selections(&split.b, bufs, out);
                }
            }
        }

        remember_tab_selections(
            &self.tile_root,
            &self.buffers,
            &mut self.remembered_tab_selections,
        );
        self.remember_visible_tile_layouts();

        fn buffer_idx_by_name(bufs: &[Buf], name: &str) -> Option<usize> {
            bufs.iter().position(|buffer| buffer.name == name)
        }

        fn build(
            spec: LayoutSpec,
            bufs: &[Buf],
            next_id: &mut TileId,
            remembered_tab_selections: &HashMap<Vec<BufferId>, BufferId>,
            retained_tile_layouts: &HashMap<BufferId, Vec<RetainedTileLayout>>,
            remembered_split_ratios: &HashMap<String, f32>,
        ) -> Result<TileNode, String> {
            match spec {
                LayoutSpec::Buffer {
                    name,
                    tabs,
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
                    collapse_threshold,
                    on_collapse,
                } => {
                    let buf_idx = buffer_idx_by_name(bufs, &name)
                        .ok_or_else(|| format!("layout references missing buffer '{name}'"))?;
                    let mut resolved_tabs = Vec::new();
                    for tab in tabs {
                        let buffer_idx =
                            buffer_idx_by_name(bufs, &tab.buffer_name).ok_or_else(|| {
                                format!(
                                    "layout tab '{}' references missing buffer '{}'",
                                    tab.label, tab.buffer_name
                                )
                            })?;
                        resolved_tabs.push(TileBufferTab {
                            label: tab.label,
                            buffer_idx,
                            on_close: tab.on_close,
                        });
                    }
                    if !resolved_tabs.is_empty()
                        && !resolved_tabs.iter().any(|tab| tab.buffer_idx == buf_idx)
                    {
                        return Err(format!(
                            "tabs for '{name}' must include the primary :buf buffer"
                        ));
                    }
                    let id = *next_id;
                    *next_id += 1;
                    let mut leaf = TileLeaf::new(id, buf_idx);
                    if !resolved_tabs.is_empty() {
                        let tab_buffer_ids = resolved_tabs
                            .iter()
                            .map(|tab| bufs[tab.buffer_idx].id)
                            .collect::<Vec<_>>();
                        let selected_tab = remembered_tab_selections
                            .get(&tab_buffer_ids)
                            .and_then(|selected_buffer_id| {
                                resolved_tabs
                                    .iter()
                                    .position(|tab| bufs[tab.buffer_idx].id == *selected_buffer_id)
                            })
                            .or_else(|| {
                                resolved_tabs
                                    .iter()
                                    .position(|tab| tab.buffer_idx == buf_idx)
                            })
                            .unwrap_or(0);
                        leaf.buffer_idx = resolved_tabs[selected_tab].buffer_idx;
                        leaf.tabs = resolved_tabs;
                        leaf.selected_tab = Some(selected_tab);
                    }
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
                    leaf.collapse_threshold = collapse_threshold;
                    leaf.on_collapse = on_collapse;
                    if let Some(buffer) = bufs.get(leaf.buffer_idx)
                        && let Some(retained) =
                            retained_tile_layouts.get(&buffer.id).and_then(|retained| {
                                retained.iter().rev().find(|retained| {
                                    retained.widget_tree_revision == buffer.widget_tree_revision
                                })
                            })
                    {
                        retained.restore_to_leaf(buffer, &mut leaf);
                    }
                    Ok(TileNode::Leaf(leaf))
                }
                LayoutSpec::Rows {
                    gap,
                    remember,
                    panes,
                } => build_split(
                    panes,
                    SplitDir::Horizontal,
                    gap,
                    remember,
                    bufs,
                    next_id,
                    remembered_tab_selections,
                    retained_tile_layouts,
                    remembered_split_ratios,
                ),
                LayoutSpec::Cols {
                    gap,
                    remember,
                    panes,
                } => build_split(
                    panes,
                    SplitDir::Vertical,
                    gap,
                    remember,
                    bufs,
                    next_id,
                    remembered_tab_selections,
                    retained_tile_layouts,
                    remembered_split_ratios,
                ),
            }
        }

        fn build_split(
            panes: Vec<(f32, LayoutSpec)>,
            dir: SplitDir,
            gap: f32,
            remember: Option<String>,
            bufs: &[Buf],
            next_id: &mut TileId,
            remembered_tab_selections: &HashMap<Vec<BufferId>, BufferId>,
            retained_tile_layouts: &HashMap<BufferId, Vec<RetainedTileLayout>>,
            remembered_split_ratios: &HashMap<String, f32>,
        ) -> Result<TileNode, String> {
            assert!(!panes.is_empty());
            if panes.len() == 1 {
                return build(
                    panes.into_iter().next().unwrap().1,
                    bufs,
                    next_id,
                    remembered_tab_selections,
                    retained_tile_layouts,
                    remembered_split_ratios,
                );
            }
            let mut iter = panes.into_iter();
            let (ratio, first_spec) = iter.next().unwrap();
            let rest: Vec<(f32, LayoutSpec)> = iter.collect();

            let child_a = build(
                first_spec,
                bufs,
                next_id,
                remembered_tab_selections,
                retained_tile_layouts,
                remembered_split_ratios,
            )?;
            let child_b = if rest.len() == 1 {
                build(
                    rest.into_iter().next().unwrap().1,
                    bufs,
                    next_id,
                    remembered_tab_selections,
                    retained_tile_layouts,
                    remembered_split_ratios,
                )?
            } else {
                let rest_total: f32 = rest.iter().map(|(r, _)| r).sum();
                let rescaled: Vec<(f32, LayoutSpec)> = if rest_total > 0.0 {
                    rest.into_iter().map(|(r, s)| (r / rest_total, s)).collect()
                } else {
                    rest
                };
                build_split(
                    rescaled,
                    dir,
                    gap,
                    None,
                    bufs,
                    next_id,
                    remembered_tab_selections,
                    retained_tile_layouts,
                    remembered_split_ratios,
                )?
            };

            let split_id = *next_id;
            *next_id += 1;
            let ratio = remember
                .as_ref()
                .and_then(|key| remembered_split_ratios.get(key))
                .copied()
                .unwrap_or(ratio);
            Ok(TileNode::Split(TileSplit {
                id: split_id,
                dir,
                ratio,
                gap,
                remember_key: remember,
                a: Box::new(child_a),
                b: Box::new(child_b),
            }))
        }

        let new_root = match build(
            spec,
            &self.buffers,
            &mut self.next_tile_id,
            &self.remembered_tab_selections,
            &self.retained_tile_layouts,
            &self.remembered_split_ratios,
        ) {
            Ok(root) => root,
            Err(error) => {
                self.minibuffer = Some(format!("set-layout: {error}"));
                self.mark_needs_redraw();
                return;
            }
        };
        self.tile_root = new_root;
        self.prune_retained_tile_layouts();
        self.tile_outer_gap = outer_gap;
        // Enforce min-size constraints on initial ratios
        self.enforce_min_sizes_recursive();
        // Preserve the active buffer when the rebuilt layout still contains it.
        let ids = self.tile_root.leaf_ids();
        let active_tile = previous_active_buffer_id
            .and_then(|buffer_id| {
                ids.iter().copied().find(|id| {
                    self.tile_root
                        .find_leaf(*id)
                        .and_then(|leaf| self.buffers.get(leaf.buffer_idx))
                        .is_some_and(|buffer| buffer.id == buffer_id)
                })
            })
            .or_else(|| ids.first().copied());
        if let Some(active_tile) = active_tile {
            self.active_tile = active_tile;
            let buffer_idx = self.active_buffer_idx();
            self.record_buffer_access_by_idx(buffer_idx);
        }
        self.sync_runtime_context();
        let viewport = (
            self.runtime.layout_cols_exact(),
            self.runtime.layout_rows_exact(),
        );
        let retained = {
            let buffer = self.active_buffer();
            self.retained_tile_layout_for_geometry(
                buffer,
                viewport.0,
                viewport.1,
                self.runtime.layout_frame_viewport(),
            )
        };
        if let Some(retained) = retained {
            self.restore_buffer_widget_tree_with_cached_layout(
                Some(retained.cached_layout),
                Some(viewport),
                retained.layout_revision,
            );
        } else {
            let (cached_layout, layout_revision) = {
                let leaf = self.active_leaf();
                let cached_layout = leaf.cached_layout.as_ref().and_then(|layout| {
                    leaf_cached_layout_matches_geometry(
                        leaf,
                        layout,
                        viewport.0,
                        viewport.1,
                        self.runtime.layout_frame_viewport(),
                    )
                    .then(|| layout.clone())
                });
                (cached_layout, leaf.layout_revision)
            };
            self.restore_buffer_widget_tree_with_cached_layout(
                cached_layout,
                Some(viewport),
                layout_revision,
            );
        }
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

    pub fn switch_active_tile_to_buffer_named(&mut self, buffer_name: &str) -> bool {
        let Some(buffer_idx) = self
            .buffers
            .iter()
            .position(|buffer| buffer.name == buffer_name)
        else {
            return false;
        };
        let Some(tile_id) = self
            .tile_root
            .find_leaf_by_buffer_idx(buffer_idx)
            .map(|leaf| leaf.id)
        else {
            return false;
        };
        self.switch_active_tile(tile_id);
        true
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
        // Inspect mode outranks the overlay intercept: inspecting a modal's
        // widgets needs the raw hover/click, and the inspect path does its
        // own modal-subtree hit-testing. Without an overlay the inspect
        // check keeps its usual place further down the chain.
        if self.inspect_mode
            && crate::widget_render::overlay_widget_id().is_some()
            && self.handle_tiled_inspect_mouse_precise(
                mouse,
                precise_col,
                precise_row,
                border_inset,
            )
        {
            return;
        }

        if let Some(entry) = crate::widget_render::topmost_overlay()
            && matches!(
                mouse.kind,
                MouseEventKind::Moved
                    | MouseEventKind::Down(MouseButton::Left)
                    | MouseEventKind::Drag(MouseButton::Left)
                    | MouseEventKind::Up(MouseButton::Left)
                    | MouseEventKind::Down(MouseButton::Right)
                    | MouseEventKind::Up(MouseButton::Right)
            )
        {
            let tile_id = self.overlay_owner_tile(entry).unwrap_or(self.active_tile);
            let Some((content_col, content_row, content_width, content_height)) =
                self.tile_content_area(tile_id, border_inset)
            else {
                return;
            };
            if matches!(mouse.kind, MouseEventKind::Moved) && tile_id != self.active_tile {
                self.update_inactive_overlay_hover(
                    entry,
                    tile_id,
                    content_col,
                    content_row,
                    precise_col,
                    precise_row,
                    border_inset,
                );
                return;
            }
            self.route_event_to_tile(tile_id, border_inset, false, |editor| {
                // Switching the active runtime clears render-derived overlay
                // state. This entry is the event's captured routing target
                // and remains valid for the duration of this dispatch.
                crate::widget_render::push_overlay(entry);
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
            if self.overlay_entry_is_open_in_tile(entry, tile_id) {
                crate::widget_render::push_overlay(entry);
            } else {
                crate::widget_render::remove_overlay(entry.widget_id);
            }
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
                MouseEventKind::Moved
                    | MouseEventKind::Drag(MouseButton::Left)
                    | MouseEventKind::Up(MouseButton::Left)
            ) {
                return;
            }
        }

        if self.handle_tile_resize_drag(mouse, precise_col, precise_row) {
            return;
        }

        if self.handle_tiled_inspect_mouse_precise(mouse, precise_col, precise_row, border_inset) {
            return;
        }

        if matches!(mouse.kind, MouseEventKind::Moved)
            && self.update_tile_resize_hover_cursor(precise_col, precise_row, border_inset)
        {
            if self.hovered_tile_tab.take().is_some() {
                self.mark_needs_redraw();
            }
            return;
        }
        if matches!(mouse.kind, MouseEventKind::Moved) {
            let _ = self.update_tile_tab_hover(precise_col, precise_row);
        }

        if let Some(tile_id) = self.active_tab_mouse_capture
            && matches!(
                mouse.kind,
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
            )
        {
            if self.tile_root.find_leaf(tile_id).is_some() && self.active_tile != tile_id {
                let viewport = self.tile_content_layout_viewport(tile_id, border_inset);
                self.switch_active_tile_with_viewport(tile_id, viewport);
            }
            if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
                self.active_tab_mouse_capture = None;
            }
            return;
        }

        if matches!(
            mouse.kind,
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
        ) && self.route_event_to_tile(self.active_tile, border_inset, true, |editor| {
            editor.active_layout_has_pending_patch_drag()
        }) {
            let tile_id = self.active_tile;
            self.route_event_to_tile(tile_id, border_inset, true, |editor| {
                let Some((content_col, content_row, _, _)) =
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
                editor.handle_active_patch_drag_mouse(
                    mouse,
                    content_col,
                    content_row,
                    event_col,
                    event_row,
                );
            });
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
            self.route_event_to_tile(tile_id, border_inset, true, |editor| {
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

        if let Some(_gesture) = self.active_widget_drag_gesture()
            && matches!(
                mouse.kind,
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
            )
        {
            let started_drag = match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.active_widget_drag_gesture_for_drag(screen_col, screen_row)
                }
                MouseEventKind::Up(MouseButton::Left) => self.active_widget_drag_gesture_for_drop(),
                _ => None,
            };
            let target_tile = self
                .tile_at_screen(screen_col, screen_row)
                .unwrap_or(self.active_tile);
            let mut dropped = false;
            self.route_event_to_tile(target_tile, border_inset, false, |editor| {
                let Some((content_col, content_row, _, _)) =
                    editor.tile_content_area(target_tile, border_inset)
                else {
                    return;
                };
                let (event_col, event_row) = editor
                    .tile_content_precise_event_position(
                        target_tile,
                        border_inset,
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                    )
                    .unwrap_or((precise_col, precise_row));

                match mouse.kind {
                    MouseEventKind::Drag(MouseButton::Left) => {
                        if let Some(started_drag) = started_drag.as_ref() {
                            let _ = editor.update_widget_drop_hover(
                                started_drag,
                                content_col,
                                content_row,
                                event_col,
                                event_row,
                            );
                        } else {
                            crate::widget_render::set_drop_hover_target(None);
                            editor.widget_cursor = WidgetCursor::Default;
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        if let Some(started_drag) = started_drag.as_ref() {
                            let output = editor.dispatch_widget_drop_event(
                                started_drag,
                                content_col,
                                content_row,
                                event_col,
                                event_row,
                            );
                            dropped = editor.apply_widget_output(output);
                        }
                        crate::widget_render::set_drop_hover_target(None);
                        editor.widget_cursor = WidgetCursor::Default;
                    }
                    _ => {}
                }
            });
            if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
                self.active_leaf_mut().active_widget_gesture = None;
                self.last_mouse_precise = None;
                crate::widget_render::set_drop_hover_target(None);
                self.widget_cursor = WidgetCursor::Default;
            }
            if dropped
                || matches!(
                    mouse.kind,
                    MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
                )
            {
                return;
            }
        }

        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some((tile_id, tab_index)) =
                self.tile_tab_close_hit_at_screen(precise_col, precise_row)
        {
            self.tile_root.clear_focus_except(tile_id);
            let _ = self.invoke_tile_tab_close(tile_id, tab_index);
            return;
        }

        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some((tile_id, tab_index)) =
                self.tile_tab_hit_at_screen(precise_col, precise_row)
        {
            self.tile_root.clear_focus_except(tile_id);
            self.select_tile_tab(tile_id, tab_index, border_inset);
            self.active_tab_mouse_capture = Some(tile_id);
            return;
        }

        // Find which tile this mouse event targets
        let target_tile = if force_active {
            Some(self.active_tile)
        } else {
            self.tile_at_screen(screen_col, screen_row)
        };
        if let Some(tile_id) = target_tile {
            if matches!(mouse.kind, MouseEventKind::Moved) && tile_id != self.active_tile {
                self.widget_cursor = WidgetCursor::Default;
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
            self.route_event_to_tile(tile_id, border_inset, persist_selection, |editor| {
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
        } else if matches!(mouse.kind, MouseEventKind::Moved) {
            self.widget_cursor = WidgetCursor::Default;
        }
    }

    fn handle_tiled_inspect_mouse_precise(
        &mut self,
        mouse: MouseEvent,
        precise_col: f32,
        precise_row: f32,
        border_inset: u16,
    ) -> bool {
        if !self.inspect_mode {
            return false;
        }
        match mouse.kind {
            MouseEventKind::Moved | MouseEventKind::Down(MouseButton::Left) => {}
            MouseEventKind::Down(MouseButton::Right) | MouseEventKind::Up(MouseButton::Right) => {
                self.toggle_inspect_mode();
                return true;
            }
            _ => return false,
        }
        // With a modal open, inspect the modal's own tile — the panel can
        // hang over neighbouring tiles whose layouts don't contain it.
        let Some(tile_id) = self
            .modal_owner_tile()
            .or_else(|| self.tile_at_screen(precise_col, precise_row))
        else {
            if self.inspect_hover_tile_id.take().is_some()
                || self.inspect_hover_widget_id.take().is_some()
            {
                self.inspect_hover_status = None;
                self.inspect_hover_rect = None;
                self.show_sticky_message("Inspect mode: no widget");
                self.mark_needs_redraw();
            }
            return true;
        };
        let Some((content_col, content_row, _, _)) = self.tile_content_area(tile_id, border_inset)
        else {
            return true;
        };
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
        match mouse.kind {
            MouseEventKind::Moved => {
                self.update_widget_inspect_hover(
                    tile_id,
                    content_col,
                    content_row,
                    event_col,
                    event_row,
                );
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_widget_inspect_click(
                    tile_id,
                    content_col,
                    content_row,
                    event_col,
                    event_row,
                );
            }
            _ => {}
        }
        true
    }

    /// Resolve a render-derived overlay entry back to the visible tile whose
    /// layout owns it. Inspect can deliberately make a source tile active
    /// while the modal remains mounted in an arrangement/step tile, so active
    /// tile identity is not overlay ownership.
    fn overlay_owner_tile(&self, entry: crate::widget_render::OverlayEntry) -> Option<TileId> {
        let owns_entry = |layout: &crate::layout::LayoutNode| {
            widget_focus::find_node_by_id(layout, entry.widget_id).is_some()
                || (entry.kind == crate::widget_render::OverlayKind::Modal
                    && widget_focus::find_open_modal_node(layout).is_some())
        };
        if self
            .runtime
            .current_layout
            .as_deref()
            .is_some_and(owns_entry)
        {
            return Some(self.active_tile);
        }
        self.tile_root
            .leaf_ids()
            .into_iter()
            .filter(|tile_id| *tile_id != self.active_tile)
            .find(|tile_id| {
                self.tile_root
                    .find_leaf(*tile_id)
                    .and_then(|leaf| leaf.cached_layout.as_deref())
                    .is_some_and(owns_entry)
            })
    }

    fn modal_owner_tile(&self) -> Option<TileId> {
        if self
            .runtime
            .current_layout
            .as_deref()
            .and_then(widget_focus::find_open_modal_node)
            .is_some()
        {
            return Some(self.active_tile);
        }
        self.tile_root
            .leaf_ids()
            .into_iter()
            .filter(|tile_id| *tile_id != self.active_tile)
            .find(|tile_id| {
                self.tile_root
                    .find_leaf(*tile_id)
                    .and_then(|leaf| leaf.cached_layout.as_deref())
                    .and_then(widget_focus::find_open_modal_node)
                    .is_some()
            })
    }

    /// True when any visible tile owns an open modal. This is the app-level
    /// keyboard shortcut gate: modal input remains exclusive even when source
    /// inspection temporarily makes another tile active.
    pub fn modal_is_open(&self) -> bool {
        self.modal_owner_tile().is_some()
    }

    /// Route keyboard input exclusively to the open modal. Returns `false`
    /// only when no visible tile owns one.
    fn handle_open_modal_key(&mut self, key: KeyEvent) -> bool {
        let Some(owner_tile) = self.modal_owner_tile() else {
            return false;
        };
        // Switching to an inactive owner restores that tile's widget tree and
        // clears render-derived overlay registration. Preserve the keyboard
        // target across the temporary switch; widget state remains canonical.
        let topmost_overlay = crate::widget_render::topmost_overlay();

        // A chord begun before the modal opened must not resume after it
        // closes; the modal establishes a fresh keyboard context.
        self.pending_key = None;
        self.route_event_to_tile(owner_tile, 0, false, |editor| {
            editor.sync_modal_focus_state();

            if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
                // A dropdown inside the modal is the topmost keyboard target
                // and closes first. The next Escape reaches the modal itself.
                if let Some(entry) = topmost_overlay
                    && entry.kind == crate::widget_render::OverlayKind::Dropdown
                {
                    if !editor.handle_focused_widget_key(key) {
                        crate::widget_render::dropdown::close_dropdown(entry.widget_id);
                        crate::widget_render::remove_overlay(entry.widget_id);
                        editor.mark_needs_redraw();
                    }
                    return;
                }

                // Same rule one level down the modal family: a context menu
                // opened inside a modal is the topmost panel and closes
                // first, so resolve the innermost open panel rather than the
                // first one in tree order (which is the outer modal).
                let panel_id = editor
                    .runtime
                    .current_layout
                    .as_deref()
                    .and_then(topmost_open_overlay_panel)
                    .map(|panel| panel.widget_id);
                if let Some(panel_id) = panel_id {
                    editor.fire_modal_on_close(panel_id);
                }
                return;
            }

            // Native widget editing/navigation gets first refusal, followed
            // only by an explicit :on-focus-key callback on that control.
            // Do not call the editor's generic focus navigation here: its
            // edge behavior scrolls the owning tile, which is under the modal.
            // Anything the modal focus does not define is swallowed.
            if !editor.handle_focused_widget_key(key) {
                let _ = editor.dispatch_focus_key(key);
            }
        });
        true
    }

    fn modal_gesture_tile(&self) -> Option<TileId> {
        self.modal_owner_tile()
    }

    fn overlay_entry_is_open_in_tile(
        &self,
        entry: crate::widget_render::OverlayEntry,
        tile_id: TileId,
    ) -> bool {
        let layout = if tile_id == self.active_tile {
            self.runtime.current_layout.as_deref()
        } else {
            self.tile_root
                .find_leaf(tile_id)
                .and_then(|leaf| leaf.cached_layout.as_deref())
        };
        match entry.kind {
            crate::widget_render::OverlayKind::Modal => layout
                .and_then(widget_focus::find_open_modal_node)
                .is_some(),
            crate::widget_render::OverlayKind::Dropdown => {
                crate::widget_render::dropdown::is_dropdown_open(entry.widget_id)
                    && layout.is_some_and(|layout| {
                        widget_focus::find_node_by_id(layout, entry.widget_id).is_some()
                    })
            }
        }
    }

    fn update_inactive_overlay_hover(
        &mut self,
        entry: crate::widget_render::OverlayEntry,
        tile_id: TileId,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
        border_inset: u16,
    ) {
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
        let local_col = event_col - content_col as f32;
        let local_row = event_row - content_row as f32;

        if entry.kind == crate::widget_render::OverlayKind::Dropdown {
            if crate::widget_render::dropdown::hover_overlay(entry.widget_id, local_row) {
                self.mark_needs_redraw();
            }
            return;
        }

        let hovered = self.tile_root.find_leaf(tile_id).and_then(|leaf| {
            let layout = leaf.cached_layout.as_deref()?;
            let modal = widget_focus::find_node_by_id(layout, entry.widget_id)
                .or_else(|| widget_focus::find_open_modal_node(layout).cloned())?;
            let layout_col = local_col + leaf.widget_scroll_left;
            let layout_row = local_row + leaf.widget_scroll_top;
            crate::ui::layout::hit_test_layout(&modal, layout_row, layout_col)
                .map(|node| node.widget_id)
        });
        if crate::widget_render::set_pointer_hover_widget(hovered) {
            self.mark_needs_redraw();
        }
        self.widget_cursor = WidgetCursor::Default;
    }

    pub fn handle_tiled_touchpad_magnify(
        &mut self,
        precise_col: f32,
        precise_row: f32,
        border_inset: u16,
        delta: f64,
    ) {
        let modal_entry = crate::widget_render::topmost_overlay()
            .filter(|entry| entry.kind == crate::widget_render::OverlayKind::Modal);
        let Some(tile_id) = self
            .modal_gesture_tile()
            .or_else(|| self.tile_at_screen(precise_col, precise_row))
        else {
            return;
        };
        self.route_event_to_tile(tile_id, border_inset, modal_entry.is_none(), |editor| {
            if let Some(entry) = modal_entry {
                crate::widget_render::push_overlay(entry);
            }
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
        if let Some(entry) = modal_entry
            && self.overlay_entry_is_open_in_tile(entry, tile_id)
        {
            crate::widget_render::push_overlay(entry);
        }
    }

    pub fn handle_tiled_touchpad_scroll(
        &mut self,
        precise_col: f32,
        precise_row: f32,
        border_inset: u16,
        delta_x: f32,
        delta_y: f32,
    ) -> bool {
        let modal_entry = crate::widget_render::topmost_overlay()
            .filter(|entry| entry.kind == crate::widget_render::OverlayKind::Modal);
        let modal_open = modal_entry.is_some();
        let Some(tile_id) = self
            .modal_gesture_tile()
            .or_else(|| self.tile_at_screen(precise_col, precise_row))
        else {
            return false;
        };
        let consumed =
            self.route_event_to_tile(tile_id, border_inset, modal_entry.is_none(), |editor| {
                if let Some(entry) = modal_entry {
                    crate::widget_render::push_overlay(entry);
                }
                let Some((content_col, content_row, _, _)) =
                    editor.tile_content_area(tile_id, border_inset)
                else {
                    // With a modal open the gesture is trapped either way: a
                    // false here would let the caller scroll the tile leaf
                    // behind the panel (see collect_modal_overlay's coherence
                    // note).
                    return modal_open;
                };
                editor.handle_touchpad_scroll(
                    content_col,
                    content_row,
                    precise_col,
                    precise_row,
                    delta_x,
                    delta_y,
                )
            });
        if let Some(entry) = modal_entry
            && self.overlay_entry_is_open_in_tile(entry, tile_id)
        {
            crate::widget_render::push_overlay(entry);
        }
        consumed
    }

    fn tile_content_area(
        &self,
        tile_id: TileId,
        border_inset: u16,
    ) -> Option<(u16, u16, u16, u16)> {
        let rect = self.tile_body_rect(tile_id)?;
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

    fn tile_content_layout_viewport(
        &self,
        tile_id: TileId,
        border_inset: u16,
    ) -> Option<(f32, f32)> {
        let rect = self.tile_body_rect(tile_id)?;
        if border_inset != 0 {
            return self
                .tile_content_area(tile_id, border_inset)
                .map(|(_, _, width, height)| (width as f32, height as f32));
        }

        let leaf = self.tile_root.find_leaf(tile_id)?;
        let show_status = self.tile_effective_show_status(tile_id)?;
        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
        Some(metal_tile_content_viewport(
            &rect,
            show_status,
            leaf.show_border,
            leaf.border_width_px,
            cell_w,
            cell_h,
        ))
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
        let rect = self.tile_body_rect(tile_id)?;
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
        Some(
            leaf.show_status
                || buffer.view_mode != ViewMode::UiOnly
                || (self.inspect_mode && self.inspect_hover_tile_id == Some(tile_id))
                || (self.inspect_mode
                    && self.inspect_hover_tile_id.is_none()
                    && tile_id == self.active_tile)
                || (tile_id == self.active_tile && self.active_status_input_is_pending()),
        )
    }

    fn active_status_input_is_pending(&self) -> bool {
        self.pending_key.is_some() || self.minibuffer_input.is_some() || self.save_prompt.is_some()
    }

    fn status_toggle_tile_at_screen(&self, precise_col: f32, precise_row: f32) -> Option<TileId> {
        self.cached_tile_rects.iter().find_map(|(tile_id, _)| {
            self.tile_status_toggle_hit(*tile_id, precise_col, precise_row)
                .then_some(*tile_id)
        })
    }

    fn tile_tab_hit_at_screen(
        &self,
        precise_col: f32,
        precise_row: f32,
    ) -> Option<(TileId, usize)> {
        self.tile_tab_hit_at_screen_with_layout(precise_col, precise_row)
            .map(|(tile_id, index, _)| (tile_id, index))
    }

    fn tile_tab_hit_at_screen_with_layout(
        &self,
        precise_col: f32,
        precise_row: f32,
    ) -> Option<(TileId, usize, crate::tile::TileTabLayout)> {
        self.cached_tile_rects
            .iter()
            .rev()
            .find_map(|(tile_id, rect)| {
                let leaf = self.tile_root.find_leaf(*tile_id)?;
                tile_tab_layouts(*rect, &leaf.tabs, leaf.selected_tab)
                    .into_iter()
                    .find(|tab| {
                        precise_col >= tab.rect.col
                            && precise_col < tab.rect.col + tab.rect.width
                            && precise_row >= tab.rect.row
                            && precise_row < tab.rect.row + tab.rect.height
                    })
                    .map(|tab| (*tile_id, tab.index, tab))
            })
    }

    fn tile_tab_close_hit_at_screen(
        &self,
        precise_col: f32,
        precise_row: f32,
    ) -> Option<(TileId, usize)> {
        self.tile_tab_hit_at_screen_with_layout(precise_col, precise_row)
            .and_then(|(tile_id, tab_index, tab)| {
                let close_rect = tab.close_rect?;
                let leaf = self.tile_root.find_leaf(tile_id)?;
                leaf.tabs.get(tab_index)?.on_close.as_ref()?;
                (precise_col >= close_rect.col
                    && precise_col < close_rect.col + close_rect.width
                    && precise_row >= close_rect.row
                    && precise_row < close_rect.row + close_rect.height)
                    .then_some((tile_id, tab_index))
            })
    }

    fn update_tile_tab_hover(&mut self, precise_col: f32, precise_row: f32) -> bool {
        let hovered = self
            .tile_tab_hit_at_screen(precise_col, precise_row)
            .and_then(|(tile_id, tab_index)| {
                let leaf = self.tile_root.find_leaf(tile_id)?;
                leaf.tabs
                    .get(tab_index)?
                    .on_close
                    .as_ref()
                    .map(|_| (tile_id, tab_index))
            });
        if self.hovered_tile_tab == hovered {
            return false;
        }
        self.hovered_tile_tab = hovered;
        self.mark_needs_redraw();
        true
    }

    pub(crate) fn hovered_tab_for_tile(&self, tile_id: TileId) -> Option<usize> {
        self.hovered_tile_tab
            .and_then(|(hovered_tile_id, tab_index)| {
                (hovered_tile_id == tile_id).then_some(tab_index)
            })
    }

    fn invoke_tile_tab_close(&mut self, tile_id: TileId, tab_index: usize) -> bool {
        let Some((callback, buffer_name)) = self.tile_root.find_leaf(tile_id).and_then(|leaf| {
            let tab = leaf.tabs.get(tab_index)?;
            let callback = tab.on_close.clone()?;
            let buffer_name = self.buffers.get(tab.buffer_idx)?.name.clone();
            Some((callback, buffer_name))
        }) else {
            return false;
        };

        self.apply_widget_output(Some(crate::widget_render::EventOutput {
            callback,
            args: vec![Value::String(buffer_name), Value::Number(tab_index as f64)],
        }))
    }

    fn invalidate_leaf_for_buffer_switch(leaf: &mut TileLeaf) {
        leaf.cached_layout = None;
        leaf.cached_layout_widget_tree_revision = 0;
        leaf.layout_frame_viewport = None;
        leaf.cached_inactive_frame = None;
        leaf.hit_grid_cache = None;
        leaf.highlight_cache = None;
        leaf.widget_scroll_top = 0.0;
        leaf.widget_scroll_left = 0.0;
        leaf.dirty_widget_ids.clear();
    }

    fn select_tile_tab(&mut self, tile_id: TileId, tab_index: usize, border_inset: u16) -> bool {
        let Some(new_buffer_idx) = self
            .tile_root
            .find_leaf(tile_id)
            .and_then(|leaf| leaf.tabs.get(tab_index))
            .map(|tab| tab.buffer_idx)
        else {
            return false;
        };
        if tile_id != self.active_tile {
            let viewport = self.tile_content_layout_viewport(tile_id, border_inset);
            self.switch_active_tile_with_viewport(tile_id, viewport);
        }
        if self.active_buffer_idx() != new_buffer_idx {
            self.save_current_widget_tree();
            {
                let leaf = self.active_leaf_mut();
                leaf.buffer_idx = new_buffer_idx;
                leaf.selected_tab = Some(tab_index);
                Self::invalidate_leaf_for_buffer_switch(leaf);
            }
            if widget_only_scratch_buffer_should_show_ui(&self.buffers[new_buffer_idx]) {
                self.buffers[new_buffer_idx].view_mode = ViewMode::UiOnly;
            }
            self.record_buffer_access_by_idx(new_buffer_idx);
            self.sync_runtime_context();
            self.restore_buffer_widget_tree();
            self.refresh_inactive_tile_layouts_for_buffer(new_buffer_idx);
        } else {
            self.active_leaf_mut().selected_tab = Some(tab_index);
        }
        self.mark_needs_redraw();
        true
    }

    fn tile_status_toggle_hit(&self, tile_id: TileId, precise_col: f32, precise_row: f32) -> bool {
        const STATUS_TOGGLE_WIDTH: f32 = 4.0;

        let Some(rect) = self.tile_body_rect(tile_id) else {
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

    fn route_event_to_tile<R>(
        &mut self,
        tile_id: TileId,
        border_inset: u16,
        persist_selection: bool,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous_tile = self.active_tile;
        let switched = tile_id != previous_tile;
        let previous_viewport = self.tile_content_layout_viewport(previous_tile, border_inset);

        let target_viewport = self.tile_content_layout_viewport(tile_id, border_inset);

        if switched {
            self.switch_active_tile_with_viewport(tile_id, target_viewport);
        } else if let Some((width, height)) = target_viewport {
            self.set_layout_viewport_exact(width, height);
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
        self.widget_cursor = tile_resize_cursor(hit.dir);
        let _ =
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
                self.widget_cursor = tile_resize_cursor(drag.dir);
                let collapse_callback = self.update_tile_split_ratio(
                    drag.split_id,
                    drag.dir,
                    drag.area,
                    precise_col,
                    precise_row,
                );
                if let Some(callback) = collapse_callback {
                    self.active_tile_resize_drag = None;
                    self.last_mouse_precise = None;
                    self.widget_cursor = WidgetCursor::Default;
                    self.suppress_mouse_until_left_up = true;
                    self.apply_widget_output(Some(crate::widget_render::EventOutput {
                        callback,
                        args: Vec::new(),
                    }));
                }
                true
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let collapse_callback = self.update_tile_split_ratio(
                    drag.split_id,
                    drag.dir,
                    drag.area,
                    precise_col,
                    precise_row,
                );
                self.active_tile_resize_drag = None;
                self.last_mouse_precise = None;
                self.widget_cursor = WidgetCursor::Default;
                if let Some(callback) = collapse_callback {
                    self.apply_widget_output(Some(crate::widget_render::EventOutput {
                        callback,
                        args: Vec::new(),
                    }));
                }
                true
            }
            _ => true,
        }
    }

    fn update_tile_resize_hover_cursor(
        &mut self,
        precise_col: f32,
        precise_row: f32,
        border_inset: u16,
    ) -> bool {
        let Some(hit) = self.tile_resize_hit_at_screen(precise_col, precise_row, border_inset)
        else {
            return false;
        };
        self.widget_cursor = tile_resize_cursor(hit.dir);
        true
    }

    fn tile_resize_hit_at_screen(
        &self,
        precise_col: f32,
        precise_row: f32,
        border_inset: u16,
    ) -> Option<crate::tile::SplitDividerHit> {
        let root_area = self.tile_root_rect()?;
        let tolerance = if border_inset == 0 { 0.5 } else { 1.0 };
        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
        self.tile_root.hit_test_split_divider(
            root_area,
            precise_col,
            precise_row,
            tolerance,
            TILE_GAP_PX_PER_UNIT,
            cell_w,
            cell_h,
        )
    }

    fn update_tile_split_ratio(
        &mut self,
        split_id: TileId,
        dir: SplitDir,
        area: Rect,
        precise_col: f32,
        precise_row: f32,
    ) -> Option<Value> {
        if let Some(split) = self.tile_root.find_split_mut(split_id) {
            let mut ratio = split_ratio_for_point(area, dir, precise_col, precise_row);
            // Enforce minimum sizes from layout spec
            let total = match dir {
                SplitDir::Vertical => area.width,
                SplitDir::Horizontal => area.height,
            };
            let collapse_callback = if total > 0.0 {
                let collapse_for = |node: &TileNode, attempted_size: f32| {
                    let TileNode::Leaf(leaf) = node else {
                        return None;
                    };
                    let threshold = leaf.collapse_threshold?;
                    let minimum = match dir {
                        SplitDir::Vertical => leaf.min_width?,
                        SplitDir::Horizontal => leaf.min_height?,
                    };
                    (attempted_size <= minimum * (1.0 - threshold))
                        .then(|| leaf.on_collapse.clone())
                        .flatten()
                };
                collapse_for(&split.a, total * ratio)
                    .or_else(|| collapse_for(&split.b, total * (1.0 - ratio)))
            } else {
                None
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
            let remembered = split
                .remember_key
                .clone()
                .map(|remember_key| (remember_key, ratio));
            if let Some((remember_key, ratio)) = remembered {
                self.remembered_split_ratios.insert(remember_key, ratio);
            }
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
            collapse_callback
        } else {
            None
        }
    }

    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw
            || self.runtime.has_dirty_widget_ids()
            || crate::widget_render::scroll::has_dirty_scroll_keys()
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
        let display_map = self.active_buffer().inline_display_row_map();
        let current_display = display_map
            .display_row_for_buffer_line(self.active_buffer().cursor.0)
            .unwrap_or(self.active_buffer().cursor.0);
        let target_display =
            (current_display + page_height).min(display_map.len().saturating_sub(1));
        let target_line = display_map
            .nearest_buffer_line_for_display_row(target_display)
            .unwrap_or(self.active_buffer().cursor.0);
        let buffer = self.active_buffer_mut();
        buffer.cursor.0 = target_line;
        buffer.cursor.1 = buffer
            .cursor
            .1
            .min(buffer.lines[buffer.cursor.0].chars().count());
        let max_scroll = display_map.len().saturating_sub(page_height);
        buffer.scroll_top = (buffer.scroll_top + page_height).min(max_scroll);
    }

    fn move_page_backward(&mut self) {
        let page_height = self.text_page_height();
        let display_map = self.active_buffer().inline_display_row_map();
        let current_display = display_map
            .display_row_for_buffer_line(self.active_buffer().cursor.0)
            .unwrap_or(self.active_buffer().cursor.0);
        let target_line = display_map
            .nearest_buffer_line_for_display_row(current_display.saturating_sub(page_height))
            .unwrap_or(0);
        let buffer = self.active_buffer_mut();
        buffer.cursor.0 = target_line;
        buffer.cursor.1 = buffer
            .cursor
            .1
            .min(buffer.lines[buffer.cursor.0].chars().count());
        buffer.scroll_top = buffer.scroll_top.saturating_sub(page_height);
    }

    fn recenter_cursor(&mut self) {
        let page_height = self.text_page_height();
        let display_map = self.active_buffer().inline_display_row_map();
        let cursor_display = display_map
            .display_row_for_buffer_line(self.active_buffer().cursor.0)
            .unwrap_or(self.active_buffer().cursor.0);
        let buffer = self.active_buffer_mut();
        let max_scroll = display_map.len().saturating_sub(page_height);
        let desired_top = cursor_display.saturating_sub(page_height / 2);
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
        if self.active_buffer().inline_code_widgets().is_empty() {
            self.active_leaf().widget_scroll_top
        } else {
            0.0
        }
    }

    pub fn widget_scroll_left(&self) -> f32 {
        self.active_leaf().widget_scroll_left
    }

    pub(super) fn widget_layout_scroll_left(&self) -> f32 {
        let scroll_left = self.active_leaf().widget_scroll_left;
        if self.active_buffer().inline_code_widgets().is_empty() {
            scroll_left
        } else {
            let (text_cell_width_scale, _) = self.text_cell_scales_for_buffer(self.active_buffer());
            scroll_left * text_cell_width_scale
        }
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

    pub fn ensure_widget_stable_key_visible(&mut self, stable_key: &str, margin_rows: f32) -> bool {
        let Some((target_row, target_height)) = self
            .runtime
            .current_layout
            .as_ref()
            .and_then(|layout| find_layout_node_by_stable_key(layout, stable_key))
            .map(|node| (node.rect.row, node.rect.height))
        else {
            return false;
        };
        let viewport_height = self.active_leaf().widget_viewport_height.max(0.0);
        let viewport_height = if viewport_height > 0.0 {
            viewport_height
        } else {
            self.runtime.layout_rows_exact()
        };
        if viewport_height <= 0.0 {
            return false;
        }

        let margin = margin_rows.max(0.0).min(viewport_height * 0.5);
        let target_top = target_row.floor();
        let target_bottom = (target_row + target_height).ceil();
        let current_scroll = self.active_leaf().widget_scroll_top;
        let desired_scroll = if target_top < current_scroll + margin {
            target_top - margin
        } else if target_bottom > current_scroll + viewport_height - margin {
            target_bottom - viewport_height + margin
        } else {
            current_scroll
        }
        .clamp(0.0, self.max_widget_vertical_scroll());

        if (desired_scroll - current_scroll).abs() <= f32::EPSILON {
            return false;
        }

        self.active_leaf_mut().widget_scroll_top = desired_scroll;
        self.mark_needs_redraw();
        true
    }

    pub fn ensure_widget_stable_key_visible_in_buffer_named(
        &mut self,
        buffer_name: &str,
        stable_key: &str,
        margin_rows: f32,
    ) -> bool {
        let Some(buffer_idx) = self
            .buffers
            .iter()
            .position(|buffer| buffer.name == buffer_name)
        else {
            return false;
        };
        if buffer_idx == self.active_buffer_idx() {
            return self.ensure_widget_stable_key_visible(stable_key, margin_rows);
        }

        let mut changed = false;
        for tile_id in self.tile_root.leaf_ids() {
            let Some(leaf) = self.tile_root.find_leaf_mut(tile_id) else {
                continue;
            };
            if leaf.buffer_idx != buffer_idx {
                continue;
            }
            let Some(layout) = leaf.cached_layout.as_ref() else {
                continue;
            };
            let Some((target_row, target_height)) =
                find_layout_node_by_stable_key(layout, stable_key)
                    .map(|node| (node.rect.row, node.rect.height))
            else {
                continue;
            };
            let viewport_height = leaf.widget_viewport_height.max(0.0);
            if viewport_height <= 0.0 {
                continue;
            }

            let margin = margin_rows.max(0.0).min(viewport_height * 0.5);
            let target_top = target_row.floor();
            let target_bottom = (target_row + target_height).ceil();
            let current_scroll = leaf.widget_scroll_top;
            let max_scroll = {
                const SCROLL_SLOP_ROWS: f32 = 0.5;
                let overflow = max_layout_bottom(layout.as_ref()) - viewport_height;
                if overflow <= SCROLL_SLOP_ROWS {
                    0.0
                } else {
                    overflow.max(0.0)
                }
            };
            let desired_scroll = if target_top < current_scroll + margin {
                target_top - margin
            } else if target_bottom > current_scroll + viewport_height - margin {
                target_bottom - viewport_height + margin
            } else {
                current_scroll
            }
            .clamp(0.0, max_scroll);

            if (desired_scroll - current_scroll).abs() > f32::EPSILON {
                leaf.widget_scroll_top = desired_scroll;
                changed = true;
            }
        }
        if changed {
            self.mark_needs_redraw();
        }
        changed
    }

    pub fn visible_buffer_layout_contains_stable_key(
        &self,
        buffer_name: &str,
        stable_key: &str,
    ) -> bool {
        let Some(buffer_idx) = self
            .buffers
            .iter()
            .position(|buffer| buffer.name == buffer_name)
        else {
            return false;
        };
        if buffer_idx == self.active_buffer_idx() {
            return self
                .runtime
                .current_layout
                .as_ref()
                .and_then(|layout| find_layout_node_by_stable_key(layout, stable_key))
                .is_some();
        }
        self.tile_root.leaf_ids().into_iter().any(|tile_id| {
            self.tile_root
                .find_leaf(tile_id)
                .filter(|leaf| leaf.buffer_idx == buffer_idx)
                .and_then(|leaf| leaf.cached_layout.as_ref())
                .and_then(|layout| find_layout_node_by_stable_key(layout, stable_key))
                .is_some()
        })
    }

    /// Combined vertical scroll: widget scroll + text scroll.
    pub fn total_scroll_top(&self) -> f32 {
        let text_scroll = if self.active_buffer().view_mode == ViewMode::UiOnly {
            0.0
        } else if !self.active_buffer().inline_code_widgets().is_empty() {
            let (_, text_cell_height_scale) =
                self.text_cell_scales_for_buffer(self.active_buffer());
            self.active_buffer().scroll_top as f32 * text_cell_height_scale
        } else {
            self.active_buffer().scroll_top as f32
        };
        self.widget_scroll_top() + text_scroll
    }

    /// Whether widget viewport scrolling should be smooth (sub-cell).
    /// True when widgets are visible (UiOnly or Both mode with a widget layout).
    pub fn is_ui_scroll_mode(&self) -> bool {
        let mode = self.active_buffer().view_mode;
        mode != ViewMode::TextOnly
            && self.active_buffer().inline_code_widgets().is_empty()
            && self.runtime.current_layout.is_some()
    }

    pub(super) fn max_widget_vertical_scroll(&self) -> f32 {
        const SCROLL_SLOP_ROWS: f32 = 0.5;
        let viewport_rows = self.active_leaf().widget_viewport_height.max(0.0);
        let viewport_rows = if viewport_rows > 0.0 {
            viewport_rows
        } else {
            self.runtime.layout_rows_exact()
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

    fn widget_scroll_limits(&mut self) -> (f32, f32) {
        let viewport_width = self.runtime.layout_cols_exact();
        let viewport_height = self.active_leaf().widget_viewport_height.max(0.0);
        let viewport_height = if viewport_height > 0.0 {
            viewport_height
        } else {
            self.runtime.layout_rows_exact()
        };
        let aspect = self.runtime.layout_aspect();
        let revision = self.runtime.layout_revision();
        let cache_key = (
            revision,
            viewport_width.to_bits(),
            viewport_height.to_bits(),
            aspect.to_bits(),
        );
        let cached = self
            .active_leaf()
            .widget_scroll_limits_cache
            .filter(|entry| (entry.0, entry.1, entry.2, entry.3) == cache_key)
            .map(|entry| (entry.4, entry.5));
        cached.unwrap_or_else(|| {
            const SCROLL_SLOP_ROWS: f32 = 0.5;
            let (max_v, max_h) = self
                .runtime
                .current_layout
                .as_ref()
                .map(|layout| {
                    let vertical_overflow = max_layout_bottom(layout.as_ref()) - viewport_height;
                    let max_v = if vertical_overflow <= SCROLL_SLOP_ROWS {
                        0.0
                    } else {
                        vertical_overflow.max(0.0)
                    };
                    let max_h = (crate::ui::hit::max_extent_exact(layout, aspect).0
                        - viewport_width)
                        .max(0.0);
                    (max_v, max_h)
                })
                .unwrap_or((0.0, 0.0));
            self.active_leaf_mut().widget_scroll_limits_cache = Some((
                cache_key.0,
                cache_key.1,
                cache_key.2,
                cache_key.3,
                max_v,
                max_h,
            ));
            (max_v, max_h)
        })
    }

    fn clamp_widget_vertical_scroll_offset_to(&mut self, max_v: f32) {
        let has_inline_widgets = !self.active_buffer().inline_code_widgets().is_empty();
        let leaf = self.active_leaf_mut();
        leaf.widget_scroll_top = if has_inline_widgets {
            0.0
        } else {
            leaf.widget_scroll_top.clamp(0.0, max_v)
        };
    }

    pub fn clamp_widget_scroll_offsets(&mut self) -> (f32, f32) {
        let (max_v, max_h) = self.widget_scroll_limits();
        self.clamp_widget_vertical_scroll_offset_to(max_v);
        let leaf = self.active_leaf_mut();
        leaf.widget_scroll_left = leaf.widget_scroll_left.clamp(0.0, max_h);
        (max_v, max_h)
    }

    pub(super) fn clamp_widget_vertical_scroll_offset(&mut self) -> (f32, f32) {
        let (max_v, max_h) = self.widget_scroll_limits();
        self.clamp_widget_vertical_scroll_offset_to(max_v);
        (max_v, max_h)
    }

    /// Apply smooth (sub-cell) scroll deltas to the widget viewport.
    /// `delta_cells_x` and `delta_cells_y` are in cell units (fractional).
    pub fn apply_smooth_widget_scroll(&mut self, delta_cells_x: f32, delta_cells_y: f32) {
        // Tile scroll is trapped while a modal overlay is open — the modal's
        // frame-anchored overlay geometry is only coherent with the layout
        // when the leaf offset does not move under it (collect_modal_overlay).
        if crate::widget_render::topmost_overlay()
            .is_some_and(|entry| entry.kind == crate::widget_render::OverlayKind::Modal)
        {
            return;
        }
        if !self.active_buffer().inline_code_widgets().is_empty() {
            self.active_leaf_mut().widget_scroll_top = 0.0;
            return;
        }
        let max_v = self.max_widget_vertical_scroll();

        let max_h = {
            let vp = self.runtime.layout_cols_exact();
            let aspect = self.runtime.layout_aspect();
            self.runtime
                .current_layout
                .as_ref()
                .map(|l| (crate::ui::hit::max_extent_exact(l, aspect).0 - vp).max(0.0))
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

    pub(crate) fn position_inline_widget_layout(&mut self, viewport_width: f32) {
        let buffer = self.active_buffer();
        if buffer.inline_code_widgets().is_empty() {
            return;
        }
        let (text_cell_width_scale, text_cell_height_scale) =
            self.text_cell_scales_for_buffer(buffer);
        let (placements, root_height) = inline_layout_placements(
            buffer,
            viewport_width,
            text_cell_width_scale,
            text_cell_height_scale,
        );
        self.runtime.position_current_layout(|layout| {
            position_inline_layout_nodes(layout, &placements, viewport_width, root_height)
        });
    }

    pub(crate) fn refresh_inline_widget_runtime_values(&mut self) {
        let mut visible_buffer_indices = Vec::new();
        for tile_id in self.tile_root.leaf_ids() {
            let Some(buffer_idx) = self.tile_root.find_leaf(tile_id).map(|leaf| leaf.buffer_idx)
            else {
                continue;
            };
            if !visible_buffer_indices.contains(&buffer_idx) {
                visible_buffer_indices.push(buffer_idx);
            }
        }

        let active_buffer_idx = self.active_buffer_idx();
        let mut changed_inactive_buffers = Vec::new();
        for buffer_idx in visible_buffer_indices {
            let bindings = self.buffers[buffer_idx].inline_widget_runtime_bindings();
            if bindings.is_empty() {
                continue;
            }
            let mut updates = Vec::new();
            for (anchor_id, target, inlet) in bindings {
                if let Ok(Some(value)) = self.runtime.invoke(
                    target,
                    vec![
                        Value::Keyword("__inline-read".to_string()),
                        Value::Keyword(inlet),
                    ],
                ) && !matches!(value, Value::Nil)
                {
                    updates.push((anchor_id, value));
                }
            }
            let mut changed = false;
            for (anchor_id, value) in updates {
                changed |= self.buffers[buffer_idx]
                    .set_inline_widget_live_value(anchor_id, value);
            }
            if !changed {
                continue;
            }
            let buffer_id = self.buffers[buffer_idx].id;
            if let Some(tree) = inline_widget_tree(&self.buffers[buffer_idx]) {
                self.buffers[buffer_idx]
                    .set_widget_tree(Some(tree.deep_clone()), Some(buffer_id));
                if buffer_idx == active_buffer_idx {
                    self.runtime.set_widget_tree(tree);
                } else {
                    changed_inactive_buffers.push(buffer_idx);
                }
            }
        }
        for buffer_idx in changed_inactive_buffers {
            self.refresh_inactive_tile_layouts_for_buffer(buffer_idx);
        }
    }

    pub fn visible_widget_layouts(&self) -> Vec<Arc<crate::layout::LayoutNode>> {
        let active_buffer_idx = self.active_buffer_idx();
        self.tile_root
            .leaf_ids()
            .into_iter()
            .filter_map(|tile_id| self.tile_root.find_leaf(tile_id))
            .filter_map(|leaf| {
                if leaf.buffer_idx == active_buffer_idx {
                    self.runtime.current_layout.clone()
                } else {
                    leaf.cached_layout.clone()
                }
            })
            .collect()
    }

    pub fn active_buffer_has_ui(&self) -> bool {
        self.active_buffer().widget_tree.is_some()
            || self.runtime.current_widget_tree().is_some()
            || self.widget_layout().is_some()
    }

    pub fn widget_layout_revision(&self) -> u64 {
        self.runtime.layout_revision()
    }

    pub fn patcher_source_tab_available(&self) -> bool {
        self.patcher_source_tab_target().is_some()
    }

    pub fn take_dirty_widget_ids(&mut self) -> Vec<u64> {
        let mut ids = self.runtime.take_dirty_widget_ids();
        let scroll_keys = crate::widget_render::scroll::take_dirty_scroll_keys();
        if !scroll_keys.is_empty() {
            if let Some(layout) = self.runtime.current_layout.as_deref() {
                collect_scroll_widget_ids_for_keys(layout, &scroll_keys, &mut ids);
            }
            for tile_id in self.tile_root.leaf_ids() {
                if let Some(layout) = self
                    .tile_root
                    .find_leaf(tile_id)
                    .and_then(|leaf| leaf.cached_layout.as_deref())
                {
                    collect_scroll_widget_ids_for_keys(layout, &scroll_keys, &mut ids);
                }
            }
        }
        ids
    }

    pub fn set_layout_viewport(&mut self, cols: u16, rows: u16) {
        self.runtime.set_layout_viewport(cols, rows);
        self.position_inline_widget_layout(cols as f32);
        self.sync_layout_to_active_leaf();
    }

    /// Whole-window viewport in the active tile's local cell coordinates,
    /// consumed by frame-anchored widgets (modal). Set by the tiled frame
    /// builder before laying out the active tile; `None` for single-tile
    /// paths (the tile root area is the frame).
    pub fn set_layout_frame_viewport(&mut self, frame_viewport: Option<crate::layout::Rect>) {
        self.runtime.set_layout_frame_viewport(frame_viewport);
    }

    /// Publish the active tile's content-scroll offsets to the layout engine
    /// so frame-anchored widgets can express the frame viewport in the same
    /// content space their own rects (and pointer anchors) live in.
    pub fn sync_layout_content_scroll(&mut self) {
        let content_scroll = (self.widget_layout_scroll_left(), self.total_scroll_top());
        self.runtime.set_layout_content_scroll(content_scroll);
    }

    pub fn set_layout_viewport_exact(&mut self, cols: f32, rows: f32) {
        let cols = cols.max(1.0);
        let rows = rows.max(1.0);
        let runtime_viewport_matches = self.runtime.current_layout.is_some()
            && viewport_matches(
                self.runtime.layout_cols_exact(),
                self.runtime.layout_rows_exact(),
                cols,
                rows,
            );
        let retained = (!runtime_viewport_matches)
            .then(|| {
                let buffer = self.active_buffer();
                self.retained_tile_layout_for_geometry(
                    buffer,
                    cols,
                    rows,
                    self.runtime.layout_frame_viewport(),
                )
            })
            .flatten();
        if let Some(retained) = retained {
            self.restore_buffer_widget_tree_with_cached_layout(
                Some(retained.cached_layout),
                Some((cols, rows)),
                retained.layout_revision,
            );
            self.position_inline_widget_layout(cols);
            self.sync_layout_to_active_leaf();
            return;
        }
        self.runtime.set_layout_viewport_exact(cols, rows);
        self.position_inline_widget_layout(cols);
        self.sync_layout_to_active_leaf();
    }

    pub fn sync_text_horizontal_scroll_to_viewport(&mut self) {
        if self.active_buffer().view_mode != ViewMode::UiOnly {
            let viewport_height = self.runtime.layout_rows() as usize;
            let display_map = self.active_buffer().inline_display_row_map();
            let cursor_display = display_map
                .display_row_for_buffer_line(self.active_buffer().cursor.0)
                .unwrap_or(self.active_buffer().cursor.0);
            let max_scroll = display_map.len().saturating_sub(viewport_height);
            let buffer = self.active_buffer_mut();
            buffer.scroll_top = buffer.scroll_top.min(max_scroll);
            if cursor_display < buffer.scroll_top {
                buffer.scroll_top = cursor_display;
            } else if cursor_display >= buffer.scroll_top + viewport_height {
                buffer.scroll_top = cursor_display - viewport_height + 1;
            }
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
        let buffer_name = self.active_buffer().name.clone();
        let layout = self.runtime.current_layout.clone();
        let revision = self.runtime.layout_revision();
        let widget_tree_revision = self.active_buffer().widget_tree_revision;
        let viewport_width = self.runtime.layout_cols_exact();
        let viewport_height = self.runtime.layout_rows_exact();
        let layout_frame_viewport = self.runtime.layout_frame_viewport();
        let already_synced = {
            let leaf = self.active_leaf();
            leaf.layout_revision == revision
                && leaf.cached_layout_widget_tree_revision == widget_tree_revision
                && leaf.widget_viewport_width.to_bits() == viewport_width.to_bits()
                && leaf.widget_viewport_height.to_bits() == viewport_height.to_bits()
                && frame_viewport_matches(leaf.layout_frame_viewport, layout_frame_viewport)
                && match (&leaf.cached_layout, &layout) {
                    (Some(cached), Some(current)) => Arc::ptr_eq(cached, current),
                    (None, None) => true,
                    _ => false,
                }
        };
        if already_synced {
            return;
        }
        self.trace_ui_layout_event(&buffer_name, "active-sync", layout.as_deref());
        let leaf = self.active_leaf_mut();
        leaf.cached_layout = layout;
        leaf.cached_layout_widget_tree_revision = if leaf.cached_layout.is_some() {
            widget_tree_revision
        } else {
            0
        };
        leaf.layout_revision = revision;
        leaf.widget_viewport_width = viewport_width;
        leaf.widget_viewport_height = viewport_height;
        leaf.layout_frame_viewport = layout_frame_viewport;
        self.remap_focused_widget_after_layout_change();
        self.sync_reactive_bindings_for_visible_layouts();
    }

    pub fn sync_reactive_bindings_for_visible_layouts(&mut self) {
        if self.visible_binding_sync_deferred {
            self.visible_binding_sync_pending = true;
            return;
        }
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
        let registry_revision = self.runtime.widget_bindings_revision();
        if self
            .visible_binding_layout_signature
            .as_ref()
            .is_some_and(|cached| cached == &signature)
            && self.visible_binding_registry_revision == registry_revision
        {
            return;
        }
        // The binding table stays valid across syncs as long as nobody else
        // replaced it: `visible_binding_registry_revision` is the registry
        // revision this editor last published. When it still matches, only the
        // tiles whose layout changed need to be diffed into the table; when it
        // does not (some other path rebuilt the bindings), the table has to be
        // rebuilt from every visible tile's entries.
        let can_apply_delta = self.visible_binding_layout_signature.is_some()
            && self.visible_binding_registry_revision == registry_revision;
        // Rescan only tiles whose layout changed; unchanged tiles reuse their
        // cached entry list (keyed like `signature`: layout Arc + revision).
        let mut entry_lists = Vec::with_capacity(visible_layouts.len());
        let mut removed_entries = Vec::<Arc<Vec<(crate::vm::ReactiveBindingKey, u64)>>>::new();
        let mut added_entries = Vec::<Arc<Vec<(crate::vm::ReactiveBindingKey, u64)>>>::new();
        for (tile_id, revision, layout) in &visible_layouts {
            let layout_ptr = Arc::as_ptr(layout) as usize;
            let entries = match self.visible_binding_tile_entries.get(tile_id) {
                Some((cached_ptr, cached_revision, cached_entries))
                    if *cached_ptr == layout_ptr && *cached_revision == *revision =>
                {
                    cached_entries.clone()
                }
                previous => {
                    if let Some((_, _, previous_entries)) = previous {
                        removed_entries.push(previous_entries.clone());
                    }
                    let mut out = Vec::new();
                    crate::reactive::ReactiveRegistry::collect_widget_binding_entries(
                        layout.as_ref(),
                        &mut out,
                    );
                    let entries = Arc::new(out);
                    added_entries.push(entries.clone());
                    self.visible_binding_tile_entries
                        .insert(*tile_id, (layout_ptr, *revision, entries.clone()));
                    entries
                }
            };
            entry_lists.push(entries);
        }
        self.visible_binding_tile_entries.retain(|tile_id, entry| {
            let still_visible = visible_layouts.iter().any(|(id, _, _)| id == tile_id);
            if !still_visible {
                removed_entries.push(entry.2.clone());
            }
            still_visible
        });
        if can_apply_delta {
            self.runtime.update_widget_bindings_with_tile_delta(
                removed_entries.iter().map(|entries| entries.as_slice()),
                added_entries.iter().map(|entries| entries.as_slice()),
            );
        } else {
            self.runtime.replace_widget_bindings_from_entry_lists(
                entry_lists.iter().map(|entries| entries.as_slice()),
            );
        }
        self.visible_binding_layout_signature = Some(signature);
        self.visible_binding_registry_revision = self.runtime.widget_bindings_revision();
    }

    pub fn visible_widgets_animating(&self) -> bool {
        let leaf_ids = self.tile_root.leaf_ids();
        if std::env::var_os("ESEQ_DEBUG_ANIMATING_WIDGETS").is_some() {
            static LAST_ACTIVE_WIDGETS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
            let mut active = leaf_ids
                .iter()
                .filter_map(|id| self.tile_root.find_leaf(*id))
                .filter_map(|leaf| leaf.cached_layout.as_ref())
                .flat_map(|layout| crate::widget_render::active_animation_widgets(layout))
                .map(|(widget_id, widget_type)| format!("{widget_type}#{widget_id}"))
                .collect::<Vec<_>>();
            active.sort();
            active.dedup();
            let mut previous = LAST_ACTIVE_WIDGETS
                .get_or_init(|| Mutex::new(Vec::new()))
                .lock()
                .unwrap();
            if *previous != active {
                eprintln!("[animation-frames] active={active:?}");
                *previous = active;
            }
        }
        leaf_ids.into_iter().any(|id| {
            self.tile_root
                .find_leaf(id)
                .and_then(|leaf| leaf.cached_layout.as_ref())
                .is_some_and(|layout| crate::widget_render::layout_wants_animation_frames(layout))
        })
    }

    fn refresh_all_inactive_tile_layouts(&mut self) {
        let mut buf_indices: Vec<usize> = Vec::new();
        for id in self.tile_root.leaf_ids() {
            if id == self.active_tile {
                continue;
            }
            if let Some(leaf) = self.tile_root.find_leaf_mut(id) {
                leaf.cached_layout = None;
                leaf.cached_layout_widget_tree_revision = 0;
                leaf.layout_frame_viewport = None;
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
        let new_idx = self.buffers.len() - 1;
        {
            let leaf = self.active_leaf_mut();
            leaf.buffer_idx = new_idx;
            leaf.selected_tab = None;
            Self::invalidate_leaf_for_buffer_switch(leaf);
        }
        self.track_new_buffer(id, true);
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
            self.track_new_buffer(id, false);
            id
        }
    }

    fn upsert_read_only_scratch_buffer_with_mode(
        &mut self,
        name: &str,
        text: &str,
        mode: BufferMode,
    ) -> BufferId {
        if let Some(idx) = self.buffers.iter().position(|buffer| buffer.name == name) {
            let id = self.buffers[idx].id;
            let cursor = self.buffers[idx].cursor;
            let scroll_top = self.buffers[idx].scroll_top;
            self.buffers[idx].set_text(text);
            self.buffers[idx].set_mode(mode);
            self.buffers[idx].read_only = true;
            self.buffers[idx].dirty = false;
            let cursor_row = cursor
                .0
                .min(self.buffers[idx].lines.len().saturating_sub(1));
            let cursor_col = cursor
                .1
                .min(self.buffers[idx].lines[cursor_row].chars().count());
            self.buffers[idx].cursor = (cursor_row, cursor_col);
            self.buffers[idx].scroll_top =
                scroll_top.min(self.buffers[idx].lines.len().saturating_sub(1));
            id
        } else {
            let id = self.alloc_buffer_id();
            let mut buffer = Buffer::new(id, name);
            buffer.set_text(text);
            buffer.set_mode(mode);
            buffer.read_only = true;
            buffer.dirty = false;
            self.buffers.push(buffer);
            self.track_new_buffer(id, false);
            id
        }
    }

    pub fn create_scratch_buffer(&mut self, name: &str, text: &str, mode: BufferMode) -> BufferId {
        let id = self.alloc_buffer_id();
        let mut buffer = Buffer::new(id, name);
        buffer.set_text(text);
        buffer.set_mode(mode);
        self.buffers.push(buffer);
        self.track_new_buffer(id, false);
        id
    }

    pub fn set_buffer_view_mode_by_name(&mut self, name: &str, view_mode: ViewMode) -> bool {
        let Some(buffer) = self.buffers.iter_mut().find(|buffer| buffer.name == name) else {
            return false;
        };
        buffer.view_mode = view_mode;
        true
    }

    fn ensure_scratch_buffer_named(&mut self, name: &str) -> usize {
        if let Some(idx) = self.buffers.iter().position(|buffer| buffer.name == name) {
            return idx;
        }
        let id = self.alloc_buffer_id();
        self.buffers.push(Buffer::new(id, name));
        self.track_new_buffer(id, false);
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
        let new_idx = self.buffers.len() - 1;
        {
            let leaf = self.active_leaf_mut();
            leaf.buffer_idx = new_idx;
            leaf.selected_tab = None;
            Self::invalidate_leaf_for_buffer_switch(leaf);
        }
        self.track_new_buffer(id, true);
        self.sync_runtime_context();
        self.completion = None;
        self.clear_mark();
        self.clear_widget_focus();
        Ok(id)
    }

    fn upsert_inactive_file_buffer_with_mode(
        &mut self,
        path: PathBuf,
        mode: BufferMode,
    ) -> Result<BufferId, EditorError> {
        if let Some(existing) = self
            .buffers
            .iter()
            .find(|buffer| buffer.path.as_ref() == Some(&path))
            .map(|buffer| buffer.id)
        {
            return Ok(existing);
        }

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
        self.buffers.push(buffer);
        self.track_new_buffer(id, false);
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
        let new_idx = self.buffers.len() - 1;
        {
            let leaf = self.active_leaf_mut();
            leaf.buffer_idx = new_idx;
            leaf.selected_tab = None;
            Self::invalidate_leaf_for_buffer_switch(leaf);
        }
        self.track_new_buffer(id, true);
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
        self.track_new_buffer(id, false);
        Ok(name)
    }

    /// Switch the buffer shown in the tile currently displaying `current_name`
    /// to the buffer named `new_name`. Does not change which tile is active.
    pub fn swap_buffer_in_tile_showing(&mut self, current_name: &str, new_name: &str) -> bool {
        let current_idx = self.buffers.iter().position(|b| b.name == current_name);
        let new_idx = self.buffers.iter().position(|b| b.name == new_name);
        if let (Some(cur), Some(new)) = (current_idx, new_idx) {
            let active_tile = self.active_tile;
            let swapping_active_tile = self
                .tile_root
                .find_leaf(active_tile)
                .is_some_and(|leaf| leaf.buffer_idx == cur);
            if swapping_active_tile {
                self.save_current_widget_tree();
            }
            if let Some(leaf) = self.tile_root.find_leaf_by_buffer_idx_mut(cur) {
                leaf.buffer_idx = new;
                leaf.selected_tab = leaf.tabs.iter().position(|tab| tab.buffer_idx == new);
                if widget_only_scratch_buffer_should_show_ui(&self.buffers[new]) {
                    self.buffers[new].view_mode = ViewMode::UiOnly;
                }
                // Invalidate all cached rendering state so the new buffer
                // renders immediately instead of showing the old widget tree.
                Self::invalidate_leaf_for_buffer_switch(leaf);
                if swapping_active_tile {
                    self.record_buffer_access_by_idx(new);
                    self.sync_runtime_context();
                    self.restore_buffer_widget_tree();
                } else {
                    self.refresh_inactive_tile_layouts_for_buffer(new);
                }
                self.mark_needs_redraw();
                return true;
            }
        }
        false
    }

    pub fn set_tabs_in_tile_showing(
        &mut self,
        current_name: &str,
        tabs: Vec<crate::runtime::LayoutTabSpec>,
    ) -> Result<bool, String> {
        let Some(current_idx) = self.buffers.iter().position(|b| b.name == current_name) else {
            return Ok(false);
        };
        if self
            .tile_root
            .find_leaf_by_buffer_idx(current_idx)
            .is_none()
        {
            return Ok(false);
        }

        let mut resolved_tabs = Vec::with_capacity(tabs.len());
        for tab in tabs {
            let Some(buffer_idx) = self
                .buffers
                .iter()
                .position(|buffer| buffer.name == tab.buffer_name)
            else {
                return Err(format!(
                    "tab '{}' references missing buffer '{}'",
                    tab.label, tab.buffer_name
                ));
            };
            resolved_tabs.push(TileBufferTab {
                label: tab.label,
                buffer_idx,
                on_close: tab.on_close,
            });
        }
        if !resolved_tabs
            .iter()
            .any(|tab| tab.buffer_idx == current_idx)
        {
            return Err(format!(
                "tabs for '{current_name}' must include the current buffer"
            ));
        }

        if let Some(leaf) = self.tile_root.find_leaf_by_buffer_idx_mut(current_idx) {
            leaf.tabs = resolved_tabs;
            leaf.selected_tab = leaf
                .tabs
                .iter()
                .position(|tab| tab.buffer_idx == current_idx);
            leaf.cached_inactive_frame = None;
            leaf.layout_revision = leaf.layout_revision.wrapping_add(1);
        }

        if current_idx == self.active_buffer_idx() {
            self.sync_layout_to_active_leaf();
            self.remap_focused_widget_after_layout_change();
            self.mark_needs_redraw();
        } else {
            self.refresh_inactive_tile_layouts_for_buffer(current_idx);
        }
        Ok(true)
    }

    /// Read the text content of a buffer by name.
    pub fn read_buffer_text(&self, name: &str) -> Option<String> {
        self.buffers
            .iter()
            .find(|b| b.name == name)
            .map(|b| b.lines.join("\n"))
    }

    pub fn upsert_patcher_emitted_source_buffer(
        &mut self,
        patcher_buffer_name: &str,
        patcher_path: &std::path::Path,
        emitted_source: &str,
    ) -> Result<String, String> {
        let Some(patcher_buffer_id) = self
            .buffers
            .iter()
            .find(|buffer| buffer.name == patcher_buffer_name)
            .map(|buffer| buffer.id)
        else {
            return Err(format!("No patcher buffer named '{patcher_buffer_name}'"));
        };
        let path = patcher_path.to_string_lossy().to_string();
        let emitted_buffer_name = crate::widget_render::patcher::emitted_source_buffer_name(&path);
        self.upsert_read_only_scratch_buffer_with_mode(
            &emitted_buffer_name,
            emitted_source,
            BufferMode::DGenLisp,
        );
        self.patcher_emitted_source_origins
            .insert(path, patcher_buffer_id);
        Ok(emitted_buffer_name)
    }

    /// Remove a buffer by name. Returns true if found and removed.
    pub fn remove_buffer_by_name(&mut self, name: &str) -> bool {
        if let Some(idx) = self.buffers.iter().position(|b| b.name == name) {
            let removed_id = self.buffers[idx].id;
            self.buffers.remove(idx);
            self.buffer_recency.retain(|id| *id != removed_id);
            self.patcher_emitted_source_origins
                .retain(|_, buffer_id| *buffer_id != removed_id);
            self.remembered_tab_selections
                .retain(|tab_buffer_ids, selected_buffer_id| {
                    *selected_buffer_id != removed_id && !tab_buffer_ids.contains(&removed_id)
                });
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
                for tab in &mut leaf.tabs {
                    if tab.buffer_idx > removed_idx {
                        tab.buffer_idx -= 1;
                    }
                }
                leaf.selected_tab = leaf
                    .selected_tab
                    .filter(|index| leaf.tabs.get(*index).is_some());
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
            {
                let leaf = self.active_leaf_mut();
                leaf.buffer_idx = index;
                leaf.selected_tab = leaf.tabs.iter().position(|tab| tab.buffer_idx == index);
                Self::invalidate_leaf_for_buffer_switch(leaf);
            }
            self.record_buffer_access_by_idx(index);
            self.mark_needs_redraw();
            self.sync_runtime_context();
            self.completion = None;
            self.clear_mark();
            self.restore_buffer_widget_tree();
        }
    }

    pub fn inspect_mode_enabled(&self) -> bool {
        self.inspect_mode
    }

    /// The widget currently highlighted by inspect-mode hover (test hook).
    pub fn inspect_hovered_widget_id(&self) -> Option<u64> {
        self.inspect_hover_widget_id
    }

    pub fn toggle_inspect_mode(&mut self) {
        self.inspect_mode = !self.inspect_mode;
        self.inspect_hover_tile_id = None;
        self.inspect_hover_widget_id = None;
        self.inspect_hover_status = None;
        self.inspect_hover_rect = None;
        if self.inspect_mode {
            self.show_sticky_message("Inspect mode: hover widgets, click to open source");
        } else {
            self.show_transient_message("Inspect mode off");
        }
        self.widget_cursor = WidgetCursor::Default;
        self.active_leaf_mut().active_widget_gesture = None;
        crate::widget_render::set_drop_hover_target(None);
        self.mark_needs_redraw();
    }

    fn exit_inspect_mode(&mut self) {
        if !self.inspect_mode {
            return;
        }
        self.inspect_mode = false;
        self.inspect_hover_tile_id = None;
        self.inspect_hover_widget_id = None;
        self.inspect_hover_status = None;
        self.inspect_hover_rect = None;
        self.widget_cursor = WidgetCursor::Default;
        self.active_leaf_mut().active_widget_gesture = None;
        crate::widget_render::set_drop_hover_target(None);
        self.show_transient_message("Inspect mode off");
    }

    fn update_widget_inspect_hover(
        &mut self,
        tile_id: TileId,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> bool {
        let Some(node) = self.inspect_widget_node_at_tile(
            tile_id,
            content_col,
            content_row,
            precise_col,
            precise_row,
        ) else {
            if self.inspect_hover_widget_id.take().is_some()
                || self.inspect_hover_tile_id.take().is_some()
            {
                self.inspect_hover_status = None;
                self.inspect_hover_rect = None;
                self.show_transient_message("Inspect mode: no widget");
                self.mark_needs_redraw();
            }
            return false;
        };
        if self.inspect_hover_tile_id == Some(tile_id)
            && self.inspect_hover_widget_id == Some(node.widget_id)
        {
            if self.inspect_hover_rect != Some(node.rect) {
                self.inspect_hover_rect = Some(node.rect);
                self.mark_needs_redraw();
            }
            return true;
        }
        self.inspect_hover_tile_id = Some(tile_id);
        self.inspect_hover_widget_id = Some(node.widget_id);
        self.inspect_hover_rect = Some(node.rect);
        let status = self.inspect_status_for_node(&node);
        self.inspect_hover_status = Some(status.clone());
        self.mark_needs_redraw();
        true
    }

    fn handle_widget_inspect_click(
        &mut self,
        tile_id: TileId,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> bool {
        let Some(node) = self.inspect_widget_node_at_tile(
            tile_id,
            content_col,
            content_row,
            precise_col,
            precise_row,
        ) else {
            self.show_sticky_message("Inspect mode: no widget source at pointer");
            return true;
        };
        self.inspect_hover_tile_id = Some(tile_id);
        self.inspect_hover_widget_id = Some(node.widget_id);
        self.inspect_hover_rect = Some(node.rect);
        match self.open_source_for_inspected_node(&node) {
            Ok(true) => {
                self.inspect_mode = false;
                self.inspect_hover_tile_id = None;
                self.inspect_hover_widget_id = None;
                self.inspect_hover_rect = None;
                true
            }
            Ok(false) => {
                self.show_sticky_message(format!(
                    "Inspect mode: {} has no source metadata",
                    inspect_node_debug_label(&node)
                ));
                true
            }
            Err(error) => {
                self.show_sticky_message(format!("Inspect mode: {error:?}"));
                true
            }
        }
    }

    pub(crate) fn tile_inspect_status_message(&self, tile_id: TileId) -> Option<&str> {
        (self.inspect_mode && self.inspect_hover_tile_id == Some(tile_id))
            .then_some(self.inspect_hover_status.as_deref())
            .flatten()
    }

    pub(crate) fn tile_inspect_overlay_rect(&self, tile_id: TileId) -> Option<crate::layout::Rect> {
        if !self.inspect_mode || self.inspect_hover_tile_id != Some(tile_id) {
            return None;
        }
        let rect = self.inspect_hover_rect?;
        (rect.row.is_finite()
            && rect.col.is_finite()
            && rect.width.is_finite()
            && rect.height.is_finite()
            && rect.width > 0.0
            && rect.height > 0.0)
            .then_some(rect)
    }

    fn inspect_widget_node_at_tile(
        &self,
        tile_id: TileId,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> Option<crate::layout::LayoutNode> {
        let leaf = self.tile_root.find_leaf(tile_id)?;
        let buffer = self.buffers.get(leaf.buffer_idx)?;
        if buffer.view_mode == ViewMode::TextOnly {
            return None;
        }
        let layout = if tile_id == self.active_tile {
            self.runtime.current_layout.as_ref()
        } else {
            leaf.cached_layout.as_ref()
        }?;
        let modal = widget_focus::find_open_modal_node(layout);
        // The modal panel may legitimately extend above/left of its tile's
        // content origin (frame-anchored); the normal content-area
        // conversion rejects those points.
        let (local_col, local_row) = if modal.is_some() {
            (
                precise_col - content_col as f32,
                precise_row - content_row as f32,
            )
        } else {
            crate::ui::hit::to_local(precise_col, precise_row, content_col, content_row)?
        };
        let (text_width_scale, text_height_scale) = self.text_cell_scales_for_buffer(buffer);
        let has_inline_widgets = !buffer.inline_code_widgets().is_empty();
        let layout_scroll_left = if has_inline_widgets {
            leaf.widget_scroll_left * text_width_scale
        } else {
            leaf.widget_scroll_left
        };
        let text_scroll_top = if buffer.view_mode == ViewMode::UiOnly {
            0.0
        } else if has_inline_widgets {
            buffer.scroll_top as f32 * text_height_scale
        } else {
            buffer.scroll_top as f32
        };
        let layout_col = local_col + layout_scroll_left;
        let layout_row = local_row + leaf.widget_scroll_top + text_scroll_top;
        // While a modal is open, inspect only its subtree — hits must never
        // fall through the panel to the widgets underneath.
        let (node, scroll_dy) = match modal {
            Some(modal) => inspect_hit_test_layout(modal, layout_row, layout_col)?,
            None => inspect_hit_test_layout(layout, layout_row, layout_col)?,
        };
        let mut node = node.clone();
        node.rect.row -= scroll_dy;
        Some(node)
    }

    fn inspect_status_for_node(&self, node: &crate::layout::LayoutNode) -> String {
        let source = inspect_node_source_file_path(node)
            .map(|path| path.display().to_string())
            .or_else(|| {
                inspect_node_source_buffer_id(node).and_then(|id| {
                    self.buffers
                        .iter()
                        .find(|buffer| buffer.id == id)
                        .map(|buffer| buffer.name.clone())
                })
            })
            .unwrap_or_else(|| "source unavailable".to_string());
        let definition = inspect_node_source_symbol(node)
            .map(|symbol| format!(" def {symbol}"))
            .unwrap_or_default();
        format!(
            "Inspect: {} @ {:.1},{:.1} {:.1}x{:.1} -> {}{}",
            inspect_node_debug_label(node),
            node.rect.col,
            node.rect.row,
            node.rect.width,
            node.rect.height,
            source,
            definition
        )
    }

    fn open_source_for_inspected_node(
        &mut self,
        node: &crate::layout::LayoutNode,
    ) -> Result<bool, EditorError> {
        inspect_debug_log(format!(
            "click widget={} stable_key={:?} debug_name={:?} source_buffer_id={:?} source_file={:?} source_symbol={:?} source_span={:?} source_revision={:?}",
            node.widget_type,
            node.stable_key,
            inspect_node_prop_string(node, "debug-name"),
            inspect_node_source_buffer_id(node),
            inspect_node_source_file_path(node),
            inspect_node_source_symbol(node),
            inspect_node_source_span(node),
            inspect_node_source_revision(node)
        ));
        let source_file_path = inspect_node_source_file_path(node);
        let source_buffer_id = if let Some(path) = source_file_path.clone() {
            inspect_debug_log(format!("opening source module path {}", path.display()));
            Some(self.upsert_inactive_file_buffer_with_mode(path, BufferMode::ESeqLisp)?)
        } else if let Some(id) = inspect_node_source_buffer_id(node) {
            inspect_debug_log(format!("using source buffer id {id} from widget metadata"));
            Some(id)
        } else {
            inspect_debug_log("no source buffer or source module metadata; cannot open source");
            None
        };
        let Some(mut source_buffer_id) = source_buffer_id else {
            return Ok(false);
        };
        let Some(mut source_buffer_idx) = self.buffer_idx_for_id(source_buffer_id) else {
            inspect_debug_log(format!(
                "source buffer id {source_buffer_id} was not found in editor buffers"
            ));
            return Ok(false);
        };
        let mut opened_stale_snapshot = false;
        if let (Some(path), Some(revision), Some(_span)) = (
            source_file_path.as_deref(),
            inspect_node_source_revision(node),
            inspect_node_source_span(node),
        ) {
            if let Some(snapshot_text) = self.runtime.evaluated_source_text(path, revision) {
                let current_text = self.buffers[source_buffer_idx].text();
                if current_text != snapshot_text {
                    let name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| format!("*inspect source: {name} @ {revision:x}*"))
                        .unwrap_or_else(|| format!("*inspect source @ {revision:x}*"));
                    let snapshot_buffer_id = self.upsert_read_only_scratch_buffer_with_mode(
                        &name,
                        &snapshot_text,
                        BufferMode::ESeqLisp,
                    );
                    if let Some(idx) = self.buffer_idx_for_id(snapshot_buffer_id) {
                        inspect_debug_log(format!(
                            "rendered source revision is stale in visible file; opening evaluated read-only snapshot {name}"
                        ));
                        source_buffer_id = snapshot_buffer_id;
                        source_buffer_idx = idx;
                        opened_stale_snapshot = true;
                    }
                }
            } else {
                inspect_debug_log(format!(
                    "source revision {revision} has no evaluated snapshot; using current buffer text"
                ));
            }
        }
        self.buffers[source_buffer_idx].view_mode = ViewMode::TextOnly;

        if let Some(tile_id) = self.visible_tile_for_buffer_id(source_buffer_id) {
            inspect_debug_log(format!(
                "source buffer id {source_buffer_id} already visible in tile {tile_id:?}; switching"
            ));
            self.switch_active_tile(tile_id);
            self.inspect_source_tile_id = Some(tile_id);
        } else if let Some(tile_id) = self
            .inspect_source_tile_id
            .filter(|tile_id| self.tile_root.find_leaf(*tile_id).is_some())
        {
            inspect_debug_log(format!(
                "source buffer id {source_buffer_id} not visible; reusing inspect source tile {tile_id:?}"
            ));
            self.replace_tile_buffer_and_activate(tile_id, source_buffer_idx);
        } else {
            inspect_debug_log(format!(
                "source buffer id {source_buffer_id} not visible; splitting root to the right"
            ));
            let source_tile = self.split_root_right_with_buffer(source_buffer_idx);
            self.inspect_source_tile_id = Some(source_tile);
            inspect_debug_log(format!(
                "new root-right source tile {source_tile:?}; switching"
            ));
        }
        let source_span = inspect_node_source_span(node);
        let source_symbol = inspect_node_source_symbol(node);
        let source_text = self.active_buffer().text();
        let resolved_span_cursor = source_span.and_then(|(start, end)| {
            if end > source_text.len()
                || !source_text.is_char_boundary(start)
                || !source_text.is_char_boundary(end)
            {
                inspect_debug_log(format!(
                    "source span {start}..{end} is outside the opened source snapshot ({} bytes)",
                    source_text.len()
                ));
                None
            } else {
                Some(offset_to_position(&source_text, start))
            }
        });
        let (resolved_widget_form, resolved_definition) = if resolved_span_cursor.is_some() {
            (None, None)
        } else {
            inspect_debug_log("legacy fallback: no usable parser source span metadata");
            let widget_form = find_widget_form_in_text(&source_text, node).or_else(|| {
                source_symbol.as_deref().and_then(|symbol| {
                    find_unique_widget_form_in_definition(&source_text, symbol, node)
                })
            });
            let definition = source_symbol
                .as_deref()
                .and_then(|symbol| find_definition_in_text(&source_text, symbol));
            (widget_form, definition)
        };
        let resolved_cursor = resolved_span_cursor
            .or(resolved_widget_form)
            .or(resolved_definition);
        if let Some(cursor) = resolved_cursor {
            if source_span.is_some() && resolved_span_cursor.is_some() {
                inspect_debug_log(format!(
                    "resolved parser source span at {}",
                    format_cursor_for_log(cursor)
                ));
            }
            if let Some(widget_cursor) = resolved_widget_form {
                inspect_debug_log(format!(
                    "resolved exact widget form at {}",
                    format_cursor_for_log(widget_cursor)
                ));
            } else if let (Some(symbol), Some(def_cursor)) =
                (source_symbol.as_deref(), resolved_definition)
            {
                inspect_debug_log(format!(
                    "falling back to producer definition {symbol} at {}",
                    format_cursor_for_log(def_cursor)
                ));
            }
            self.active_buffer_mut().cursor = cursor;
            self.sync_text_horizontal_scroll_to_viewport();
        } else {
            inspect_debug_log(
                "source opened without exact cursor; no source span, widget form, or definition match",
            );
        }
        let destination_buffer = self.active_buffer();
        inspect_debug_log(format!(
            "opened buffer={} path={} cursor={}",
            destination_buffer.name,
            destination_buffer
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<scratch>".to_string()),
            format_cursor_for_log(destination_buffer.cursor)
        ));
        self.sync_runtime_context();
        let destination = match (
            resolved_span_cursor,
            resolved_widget_form,
            source_symbol.as_deref(),
            resolved_definition,
            opened_stale_snapshot,
        ) {
            (Some(_), _, _, _, true) => {
                format!(
                    "stale source snapshot for {}",
                    inspect_node_debug_label(node)
                )
            }
            (Some(_), _, _, _, false) => format!("source span {}", inspect_node_debug_label(node)),
            (None, Some(_), _, _, _) => format!("widget form {}", inspect_node_debug_label(node)),
            (None, None, Some(symbol), Some(_), _) => format!("definition {symbol}"),
            (None, None, Some(symbol), None, _) => {
                format!("source for {symbol} (definition not found)")
            }
            (None, None, None, _, _) => "source (exact location metadata unavailable)".to_string(),
        };
        self.show_sticky_message(format!(
            "Inspect: opened {destination} for {}",
            inspect_node_debug_label(node)
        ));
        Ok(true)
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

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
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

    /// Return the named command bound to `key` in the active buffer's major
    /// mode. Hosts use this to implement hold gestures because `handle_key`
    /// deliberately dispatches press events only; the mode keymap remains the
    /// single user-customizable source of truth for the gesture's key.
    pub fn active_mode_keybinding(&self, key: KeyEvent) -> Option<&str> {
        self.mode_keybinding(&self.active_buffer().mode, key)
    }

    /// Return the binding from a named buffer's major mode without changing
    /// the active tile. Application-wide hold gestures can therefore use one
    /// declarative keymap even after a transport/mixer click moves focus to a
    /// different tile.
    pub fn buffer_mode_keybinding(&self, buffer_name: &str, key: KeyEvent) -> Option<&str> {
        let buffer = self.buffers.iter().find(|buffer| buffer.name == buffer_name)?;
        self.mode_keybinding(&buffer.mode, key)
    }

    /// Whether the active major mode explicitly permits host live-keyboard
    /// shortcuts. Modes opt in so ordinary source and special text modes keep
    /// ownership of their bare keys by default.
    pub fn active_mode_accepts_live_keys(&self) -> bool {
        let BufferMode::Named(mode_name) = &self.active_buffer().mode else {
            return false;
        };
        self.mode_registry
            .get(mode_name)
            .is_some_and(|mode| mode.live_keys)
    }

    fn mode_keybinding(&self, mode: &BufferMode, key: KeyEvent) -> Option<&str> {
        let BufferMode::Named(mode_name) = mode else {
            return None;
        };
        self.mode_registry
            .get(mode_name)
            .and_then(|mode| mode.keybindings.get(&key_str(key)))
            .map(String::as_str)
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }

        self.mark_needs_redraw();
        if !self.key_starts_text_insert(key) {
            self.finish_typing_undo_group();
        }

        // Inspect mode outranks the modal keyboard boundary, same as it
        // outranks the overlay intercept for hit-testing: the toggle chord and
        // the Esc that exits inspect must work while inspecting a modal.
        if is_inspect_mode_toggle_key(key) {
            self.toggle_inspect_mode();
            return;
        }

        if self.inspect_mode && key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
            self.exit_inspect_mode();
            return;
        }

        // Active prompts keep keyboard priority over an open modal: a save
        // prompt or minibuffer already on screen must still receive its
        // keystrokes (y/n, prompt text) even if a modal is open.
        if self.handle_save_prompt_key(key) {
            return;
        }

        if self.handle_minibuffer_key(key) {
            return;
        }

        // A modal is a keyboard boundary. Route the key through the tile that
        // owns the modal (which may be inactive while inspected source is
        // open), and never let an unhandled modal key reach editor/global
        // bindings. Escape and focused modal widgets are handled inside.
        if self.handle_open_modal_key(key) {
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

        if self.handle_patcher_source_tab(key) {
            return;
        }

        if self.handle_completion_key(key) {
            return;
        }

        if let Some(prefix) = self.pending_key.take() {
            let chord = format!("{} {}", key_str(prefix), key_str(key));
            if let Some(handler) = self.lisp_bindings.get(&chord).cloned() {
                // Module-qualified handlers resolve against the binding
                // module first, then fall back to the flat name (spec §5).
                let handler = self.runtime.resolve_handler_name(&handler).to_string();
                if self.builtins.values().any(|cmd| cmd == &handler) {
                    self.run_command(&handler);
                } else {
                    self.call_lisp_handler(&handler);
                }
            }
            return;
        }

        // Escape is both a local cancellation key and a commonly bound global
        // command. When a widget has focus, let it cancel any in-progress edit,
        // then blur it and run the direct Escape binding in the same keypress.
        // Minibuffers, regions, Vim insert mode, and other modal contexts have
        // already had the opportunity to consume Escape above.
        if key.code == KeyCode::Esc
            && key.modifiers == KeyModifiers::NONE
            && self.active_leaf().focused_widget_id.is_some()
        {
            let _ = self.handle_focused_widget_key(key);
            self.clear_focused_widget();
            self.mark_needs_redraw();
            let _ = self.run_direct_lisp_binding("ESC");
            return;
        }

        // Focused widget keys take priority over global bindings
        // (so Enter/arrows work in number-pickers, dropdowns, etc.)
        if self.handle_focused_widget_key(key) {
            return;
        }

        if self.handle_visible_patcher_selected_cable_shortcut(key) {
            return;
        }

        if self.handle_visible_patcher_agentic_shortcut(key) {
            return;
        }

        if self.handle_focus_key(key) {
            return;
        }

        // A named mode's on-key handler outranks global Escape bindings and
        // the "ESC ." chord table: modal list modes (buffer-list, etc.) use
        // Escape to dismiss themselves, which must win over e.g. the global
        // selection-clearing binding.
        if key.code == KeyCode::Esc
            && key.modifiers == KeyModifiers::NONE
            && self.handle_mode_input_key(key)
        {
            return;
        }

        // Check direct keybinding before treating as chord prefix.
        // This allows e.g. "ESC" to fire even when "ESC ." chords exist.
        // In Vim insert mode, literal characters and plain Tab retain their
        // editing/completion meaning. Application bindings for those keys are
        // normal-mode commands.
        let vim_insert_literal = key.modifiers == KeyModifiers::NONE
            && matches!(key.code, KeyCode::Char(_) | KeyCode::Tab)
            && self.active_vim_input_mode() == Some(VimInputMode::Insert);
        if !vim_insert_literal && self.run_direct_lisp_binding(&key_str(key)) {
            return;
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
                    if self.call_lisp_handler(&handler) {
                        return;
                    }
                    self.clear_minibuffer_message();
                }
            }
        }

        if let Some(cmd) = self.builtins.get(&key).cloned() {
            self.run_command(&cmd);
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

    fn run_direct_lisp_binding(&mut self, key: &str) -> bool {
        let Some(handler) = self.lisp_bindings.get(key).cloned() else {
            return false;
        };
        // Module-qualified handlers: binding module first, then flat.
        let handler = self.runtime.resolve_handler_name(&handler).to_string();
        if self.builtins.values().any(|command| command == &handler) {
            self.run_command(&handler);
            true
        } else if self.call_lisp_handler(&handler) {
            true
        } else {
            self.clear_minibuffer_message();
            false
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
                (KeyCode::Char('g'), KeyCode::Char('d')) => {
                    self.goto_definition();
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
                    ('d', KeyCode::Char('$')) => {
                        self.vim_delete_to_line_end(count);
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
        let original_col = self.active_buffer().cursor.1;
        self.record_undo_snapshot();
        self.kill_ring.push(removed.join("\n"));
        self.vim_linewise_yank = Some(removed);

        let buffer = self.active_buffer_mut();
        if start == 0 && end == buffer.lines.len() {
            buffer.set_text("");
        } else {
            let (range_start, range_end) = if end < buffer.lines.len() {
                ((start, 0), (end, 0))
            } else {
                let previous_row = start.saturating_sub(1);
                (
                    (previous_row, buffer.lines[previous_row].chars().count()),
                    (end - 1, buffer.lines[end - 1].chars().count()),
                )
            };
            buffer.delete_range(range_start, range_end);
            let next_row = start.min(buffer.lines.len().saturating_sub(1));
            let next_col = original_col.min(buffer.lines[next_row].chars().count());
            buffer.cursor = (next_row, next_col);
        }
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

    fn vim_delete_to_line_end(&mut self, count: usize) {
        if self.guard_read_only() {
            return;
        }
        let start = self.active_buffer().cursor;
        let end_row = start
            .0
            .saturating_add(count.saturating_sub(1))
            .min(self.active_buffer().lines.len().saturating_sub(1));
        let end = (
            end_row,
            self.active_buffer().lines[end_row].chars().count(),
        );
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
        buffer.cursor = (row, 0);
        buffer.insert_str(&format!("{indent}\n"));
        buffer.cursor = (row, indent.chars().count());
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
            let insertion = format!("{}\n", lines.join("\n"));
            buffer.cursor = (insert_row.min(buffer.lines.len().saturating_sub(1)), 0);
            if insert_row >= buffer.lines.len() {
                buffer.cursor = (row, buffer.lines[row].chars().count());
                buffer.insert_str(&format!("\n{}", lines.join("\n")));
            } else {
                buffer.insert_str(&insertion);
            }
            buffer.cursor = (insert_row, 0);
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
        let scroll_top = buffer.scroll_top;
        buffer.set_text(&snapshot.lines.join("\n"));
        // Undo restores document state, not navigation state. Keep the current
        // viewport and let the normal visibility sync move it only when the
        // restored cursor actually falls outside the visible rows.
        buffer.scroll_top = scroll_top;
        let row = snapshot.cursor.0.min(buffer.lines.len().saturating_sub(1));
        let col = snapshot.cursor.1.min(buffer.lines[row].chars().count());
        buffer.cursor = (row, col);
        buffer.dirty = snapshot.dirty;
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

        if self.inspect_mode && widgets_visible {
            match mouse.kind {
                MouseEventKind::Moved => {
                    self.update_widget_inspect_hover(
                        self.active_tile,
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                    );
                    return;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    self.handle_widget_inspect_click(
                        self.active_tile,
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                    );
                    return;
                }
                MouseEventKind::Down(MouseButton::Right)
                | MouseEventKind::Up(MouseButton::Right) => {
                    self.toggle_inspect_mode();
                    return;
                }
                _ => {}
            }
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Right) => {
                if !widgets_visible {
                    return;
                }
                // While an overlay is up, a right-click inside the topmost
                // modal-family panel routes to its subtree (the intercept in
                // try_handle_widget_mouse_precise); outside, it dismisses the
                // topmost entry and is consumed, exactly like a left-click.
                if let Some(entry) = crate::widget_render::topmost_overlay() {
                    let local_col = precise_col - content_col as f32;
                    let local_row = precise_row - content_row as f32;
                    if !crate::widget_render::overlay_contains(local_col, local_row) {
                        self.dismiss_overlay_entry(entry);
                        self.mark_needs_redraw();
                    } else if entry.kind == crate::widget_render::OverlayKind::Modal {
                        let _ = self.try_handle_widget_mouse_precise(
                            mouse,
                            content_col,
                            content_row,
                            precise_col,
                            precise_row,
                        );
                    }
                    return;
                }
                let _ = self.try_handle_widget_mouse_precise(
                    mouse,
                    content_col,
                    content_row,
                    precise_col,
                    precise_row,
                );
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.last_mouse_precise = Some((precise_col, precise_row));
                self.active_leaf_mut().active_widget_gesture = None;
                crate::widget_render::set_drop_hover_target(None);
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
                    self.begin_widget_gesture(
                        content_col,
                        content_row,
                        precise_col,
                        precise_row,
                        mouse.modifiers,
                    );
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
                        let started_drag =
                            self.active_widget_drag_gesture_for_drag(precise_col, precise_row);
                        if let Some(started_drag) = started_drag.as_ref() {
                            let _ = self.update_widget_drop_hover(
                                started_drag,
                                content_col,
                                content_row,
                                precise_col,
                                precise_row,
                            );
                        } else if self.active_widget_drag_gesture().is_some() {
                            crate::widget_render::set_drop_hover_target(None);
                            self.widget_cursor = WidgetCursor::Default;
                            self.last_mouse_precise = Some((precise_col, precise_row));
                            return;
                        }
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
                        } else {
                            let gen_before = crate::widget_render::widget_state_generation();
                            if let Some(output) = self.dispatch_gesture_widget_mouse_event(
                                gesture,
                                mouse.kind,
                                content_col,
                                content_row,
                                precise_col,
                                precise_row,
                                mouse.modifiers,
                            ) {
                                let _ = self.apply_widget_output(Some(output));
                            } else if crate::widget_render::widget_state_generation() != gen_before
                            {
                                self.mark_needs_redraw();
                            }
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
                        let is_widget_drag =
                            crate::editor::widget_interaction::widget_gesture_is_drag(&gesture);
                        let output = if is_widget_drag && gesture.drag_active {
                            self.dispatch_widget_drop_event(
                                &gesture,
                                content_col,
                                content_row,
                                precise_col,
                                precise_row,
                            )
                        } else {
                            None
                        }
                        .or_else(|| {
                            if is_widget_drag {
                                None
                            } else {
                                self.dispatch_gesture_widget_mouse_event(
                                    gesture,
                                    mouse.kind,
                                    content_col,
                                    content_row,
                                    precise_col,
                                    precise_row,
                                    mouse.modifiers,
                                )
                            }
                        });
                        let _ = self.apply_widget_output(output);
                        crate::widget_render::set_drop_hover_target(None);
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
                crate::widget_render::set_drop_hover_target(None);
                self.pointer_drag_started_on_slider = false;
                self.last_slider_drag_widget_id = None;
                self.finish_inline_widget_writeback();
            }
            MouseEventKind::Moved => {
                // Update dropdown hover when overlay is open
                if let Some(overlay_id) = crate::widget_render::overlay_widget_id() {
                    let local_row = precise_row - content_row as f32;
                    if crate::widget_render::dropdown::hover_overlay(overlay_id, local_row) {
                        self.mark_needs_redraw();
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
                    if crate::widget_render::set_pointer_hover_widget(None) {
                        self.mark_needs_redraw();
                    }
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
                    let max_scroll = buffer.inline_display_row_map().len().saturating_sub(1);
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
                let text_viewport_width = self.text_viewport_width_for_layout_width(content_width);
                if let Some(max_scroll) = self.max_text_horizontal_scroll(text_viewport_width) {
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
            .map(|line| {
                line.chars().count()
                    + self
                        .active_buffer()
                        .inline_column_insertions(self.active_buffer().cursor.0)
                        .iter()
                        .map(|insertion| insertion.width_cells)
                        .sum::<usize>()
            })
            .unwrap_or(0);
        let max_scroll = line_width.saturating_sub(viewport_width as usize) as u16;
        (max_scroll > 0).then_some(max_scroll)
    }

    fn text_viewport_width_for_layout_width(&self, viewport_width: u16) -> u16 {
        let (text_cell_width_scale, _) = self.text_cell_scales_for_buffer(self.active_buffer());
        ((viewport_width as f32 / text_cell_width_scale.max(0.001))
            .floor()
            .max(1.0)) as u16
    }

    pub fn sync_text_horizontal_scroll(&mut self, viewport_width: u16) {
        let Some(max_scroll) = self.max_text_horizontal_scroll(viewport_width) else {
            if self.active_buffer().view_mode != ViewMode::UiOnly {
                self.active_leaf_mut().widget_scroll_left = 0.0;
            }
            return;
        };

        let cursor_col = self.active_buffer().display_col_for_buffer_col(
            self.active_buffer().cursor.0,
            self.active_buffer().cursor.1,
        );
        let viewport_width = viewport_width as usize;
        let leaf = self.active_leaf_mut();
        let scroll_left = leaf.widget_scroll_left.floor() as usize;
        let scroll_margin = TEXT_HORIZONTAL_SCROLL_MARGIN.min(viewport_width.saturating_sub(1) / 2);
        let left_margin_edge = scroll_left.saturating_add(scroll_margin);
        let right_margin_edge =
            scroll_left.saturating_add(viewport_width.saturating_sub(scroll_margin));

        let next_scroll = if cursor_col < left_margin_edge {
            cursor_col.saturating_sub(scroll_margin)
        } else if cursor_col >= right_margin_edge {
            cursor_col
                .saturating_add(scroll_margin)
                .saturating_add(1)
                .saturating_sub(viewport_width)
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
        let layout_overflow = if view_mode != ViewMode::TextOnly
            && self.active_buffer().widget_tree.is_some()
        {
            // Use bounded extent: only count nodes whose left edge starts
            // within the viewport. This prevents h-stacks with many clipped
            // children (e.g. 10 effect boxes) from inflating the scroll range,
            // while still detecting legitimate overflow (e.g. a grid whose
            // visible cells extend past the viewport).
            let aspect = self.runtime.layout_aspect();
            let layout_vp = self.runtime.layout_cols_exact();
            self.runtime
                .current_layout
                .as_ref()
                .map(|l| {
                    (crate::ui::hit::max_extent_bounded_exact(l, aspect, layout_vp).0 - layout_vp)
                        .max(0.0)
                })
                .unwrap_or(0.0)
        } else {
            0.0
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

        let text_overflow = max_line.saturating_sub(vp) as f32;
        layout_overflow.max(text_overflow).ceil() as u16
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

    pub fn snapshot_file_backed_sources(&self) -> Vec<SourceOverlay> {
        self.buffers
            .iter()
            .filter_map(|buffer| {
                let path = buffer.path.clone()?;
                Some(SourceOverlay {
                    path,
                    text: buffer.text(),
                    dirty: buffer.dirty,
                    revision: buffer.revision,
                })
            })
            .collect()
    }

    pub fn process_lisp_reload_report(&mut self, report: ReloadReport) {
        let diagnostics_text = format_lisp_reload_report(&report);
        if !report.success || !report.diagnostics.is_empty() {
            let idx = self.ensure_scratch_buffer_named("*lisp-reload*");
            let buffer = &mut self.buffers[idx];
            buffer.set_text(&diagnostics_text);
            buffer.read_only = true;
            buffer.dirty = false;
            buffer.view_mode = ViewMode::TextOnly;
        }
        if report.success {
            self.show_transient_message(report.success_message());
        } else {
            self.show_transient_message(report.failure_message());
        }
        self.refresh_runtime_side_effects();
        if report.success {
            self.sync_layout_to_active_leaf();
        }
        self.sync_runtime_context();
        self.mark_needs_redraw();
    }

    // ── Internal methods ─────────────────────────────────────────────────────

    fn load_init(&mut self, override_source: Option<&str>, source_path: Option<&std::path::Path>) {
        let init_src = override_source
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| crate::factory_init_source());
        if init_src.trim().is_empty() {
            return;
        }
        let factory_path = (override_source.is_none() && source_path.is_none())
            .then(crate::factory_init_path);
        let source_path = source_path.or(factory_path.as_deref());
        if let Some(path) = source_path {
            let report = self.runtime.eval_source_transactional(
                Some(path.to_path_buf()),
                &init_src,
                Vec::new(),
            );
            if !report.success {
                self.show_transient_message(report.failure_message());
            }
        } else {
            let _ = self.runtime.eval_str(&init_src);
        }
        self.refresh_runtime_side_effects();
        if let Some(status) = self.runtime.take_status_message() {
            self.show_transient_message(status);
        }
    }

    fn call_lisp_handler(&mut self, fn_name: &str) -> bool {
        self.call_lisp_handler_with_args(fn_name, &[])
    }

    fn call_lisp_handler_with_args(&mut self, fn_name: &str, args: &[Value]) -> bool {
        // Module-qualified handler names (bind-key / mode handlers stored
        // from a declared module) fall back to the flat base name when the
        // module never defined the handler (spec §5).
        let fn_name = self.runtime.resolve_handler_name(fn_name);
        if fn_name == "eval-sexp" || fn_name == "eval-buffer-command" {
            self.eval_preview_handler(fn_name);
            return true;
        }
        if fn_name == "find-file" {
            self.run_command("find-file");
            return true;
        }
        self.sync_runtime_source_context();
        self.clear_minibuffer_message();
        let handled = match self.runtime.invoke_global(fn_name, args.to_vec()) {
            Ok(Some(Value::Bool(false))) => false,
            Ok(Some(result)) => {
                self.show_transient_message(format_value_for_minibuffer(&result));
                true
            }
            Ok(None) => {
                self.show_transient_message("No result");
                true
            }
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
        self.completion = None;
        handled
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
        // Module-qualified on-key handlers: defining module first, then flat.
        let handler = self.runtime.resolve_handler_name(&handler).to_string();
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

        if fn_name == "eval-buffer-command" {
            let buffer_id = self.active_buffer().id;
            self.evaluate_buffer_transactional(buffer_id);
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

    fn evaluate_buffer_transactional(&mut self, buffer_id: BufferId) {
        let Some(buffer_idx) = self
            .buffers
            .iter()
            .position(|buffer| buffer.id == buffer_id)
        else {
            self.show_transient_message(format!("Buffer #{buffer_id} no longer exists"));
            return;
        };
        let path = self.buffers[buffer_idx].path.clone();
        let source = self.buffers[buffer_idx].text();
        let overlays = self.snapshot_file_backed_sources();
        let transaction_id = buffer_id as u64;
        self.runtime
            .enqueue_host_command(HostCommand::AuthoringTransactionBegin {
                id: transaction_id,
                label: "Lisp authoring edit".to_string(),
            });
        let report = self
            .runtime
            .eval_source_transactional(path, &source, overlays);
        self.runtime
            .enqueue_host_command(HostCommand::AuthoringTransactionEnd {
                id: transaction_id,
                success: report.success,
            });
        if std::env::var("ESEQ_INLINE_TRACE").is_ok_and(|value| value != "0") {
            eprintln!(
                "[inline-widgets] evaluated buffer_id={buffer_id} name={:?} success={} requested_path={:?} evaluated_path={:?} diagnostics={:?}",
                self.buffers[buffer_idx].name,
                report.success,
                report.requested_path,
                report.evaluated_path,
                report.diagnostics
            );
        }
        self.process_lisp_reload_report(report);
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
        let current_buffer_id = active.id;
        let current_buffer_name = active.name.clone();
        let current_buffer_path = active.path.clone();
        let current_buffer_read_only = active.read_only;
        let current_buffer_mode = active.mode.name().to_string();
        let current_line_number = active.cursor.0 + 1;
        let current_line_text = active
            .lines
            .get(active.cursor.0)
            .cloned()
            .unwrap_or_default();
        let current_view_mode = active.view_mode.label().to_string();
        let buffer_names = self.buffer_names_by_recency();
        let mut shared = self.runtime.shared.borrow_mut();
        shared.current_buffer_id = Some(current_buffer_id);
        shared.current_buffer_name = current_buffer_name;
        shared.current_buffer_path = current_buffer_path;
        shared.current_buffer_read_only = current_buffer_read_only;
        shared.current_buffer_mode = current_buffer_mode;
        shared.current_line_number = current_line_number;
        shared.current_line_text = current_line_text;
        shared.buffer_names = buffer_names;
        shared.current_view_mode = current_view_mode;
        shared.current_text_zoom = self.text_zoom as f64;
    }

    fn sync_runtime_source_context(&mut self) {
        self.sync_runtime_context();
        let active = self.active_buffer();
        let revision = RuntimeSourceContextRevision {
            buffer_id: active.id,
            text_revision: active.revision,
            cursor: active.cursor,
        };
        let previous = self.runtime_source_context_revision;
        let text_changed = previous.is_none_or(|previous| {
            previous.buffer_id != revision.buffer_id
                || previous.text_revision != revision.text_revision
        });
        let cursor_changed =
            text_changed || previous.is_none_or(|previous| previous.cursor != revision.cursor);
        if !text_changed && !cursor_changed {
            return;
        }
        let text = text_changed.then(|| active.text());
        let sexp = cursor_changed.then(|| sexp_at_cursor(&active.lines, active.cursor));
        let mut shared = self.runtime.shared.borrow_mut();
        if let Some(text) = text {
            shared.current_buffer_text = text;
        }
        if let Some(sexp) = sexp {
            shared.current_sexp = sexp;
        }
        self.runtime_source_context_revision = Some(revision);
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
                buffer.view_mode = view_mode_for_widget_tree(buffer.widget_tree.as_ref());
            }
        }
        if is_active {
            let buffer_id = self.buffers[buffer_idx].id as u64;
            self.runtime.set_widget_id_offset(buffer_id * 100_000);
            match tree {
                Some(tree) => {
                    self.runtime.set_widget_tree(tree);
                    self.remap_focused_widget_after_layout_change();
                }
                None => self.clear_widget_focus(),
            }
        }
        self.refresh_inactive_tile_layouts_for_buffer(buffer_idx);
    }

    fn sync_active_buffer_widget_snapshot_from_runtime(&mut self) {
        let Some(runtime_generation) = self.runtime.current_committed_ui_snapshot_generation()
        else {
            return;
        };
        if self.active_buffer().committed_ui_runtime_generation == Some(runtime_generation) {
            return;
        }
        let Some(snapshot) = self.runtime.current_committed_ui_snapshot() else {
            return;
        };
        let buffer = self.active_buffer_mut();
        buffer.adopt_runtime_committed_ui_snapshot(snapshot, runtime_generation);
        buffer.view_mode = view_mode_for_widget_tree(buffer.widget_tree.as_ref());
    }

    fn restore_runtime_widget_tree_from_buffer(&mut self, buffer_idx: usize) {
        if self.active_buffer_idx() != buffer_idx {
            return;
        }
        let tree = self.buffers[buffer_idx].widget_tree.clone();
        let snapshot = self.buffers[buffer_idx].committed_ui_snapshot.clone();
        let buffer_id = self.buffers[buffer_idx].id as u64;
        match tree {
            Some(tree) => {
                if trace_ui_invalidation_enabled() {
                    eprintln!(
                        "[ui-trace][editor] restoring active runtime tree from buffer snapshot target={}",
                        self.buffers[buffer_idx].name
                    );
                }
                self.runtime.restore_widget_tree_with_cached_layout(
                    tree,
                    snapshot,
                    None,
                    None,
                    buffer_id * 100_000,
                    self.runtime.layout_revision(),
                );
            }
            None => self.clear_widget_focus(),
        }
    }

    fn refresh_inactive_tile_layouts_for_buffer(&mut self, buffer_idx: usize) {
        let tree = self.buffers[buffer_idx].widget_tree.clone();
        let buffer_name = self.buffers[buffer_idx].name.clone();
        let buffer_id = self.buffers[buffer_idx].id as u64;
        let widget_tree_revision = self.buffers[buffer_idx].widget_tree_revision;
        let tile_ids = self.tile_root.leaf_ids();
        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
        // Collect tile viewports first to avoid borrow issues
        let tiles_to_update: Vec<(TileId, f32, f32, Option<crate::layout::Rect>, bool)> = tile_ids
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
                let viewport_known = rect.is_some();
                let (cols, rows) = match rect {
                    Some(r) => metal_tile_content_viewport(
                        &tile_body_rect(*r, !leaf.tabs.is_empty()),
                        show_status,
                        leaf.show_border,
                        leaf.border_width_px,
                        cell_w,
                        cell_h,
                    ),
                    None => (
                        self.runtime.layout_cols_exact(),
                        self.runtime.layout_rows_exact(),
                    ),
                };
                let frame_viewport = self.tile_layout_frame_viewport(id);
                Some((id, cols, rows, frame_viewport, viewport_known))
            })
            .collect();

        for (tile_id, cols, rows, frame_viewport, viewport_known) in tiles_to_update {
            let layout_started = Instant::now();
            let existing_layout = self.tile_root.find_leaf(tile_id).and_then(|leaf| {
                let layout = leaf.cached_layout.clone()?;
                let current_tree = leaf.cached_layout_widget_tree_revision == widget_tree_revision;
                let current_viewport = !viewport_known
                    || leaf_cached_layout_matches_geometry(
                        leaf,
                        &layout,
                        cols,
                        rows,
                        frame_viewport,
                    );
                Some((layout, current_tree, current_viewport))
            });
            let cached_layout = existing_layout
                .as_ref()
                .filter(|(_, current_tree, current_viewport)| *current_tree && *current_viewport)
                .map(|(layout, _, _)| layout.clone());
            let reused_layout_and_dirty = if cached_layout.is_some() {
                None
            } else {
                tree.as_ref().and_then(|tree| {
                    let (existing, _, current_viewport) = existing_layout.as_ref()?;
                    if !current_viewport {
                        return None;
                    }
                    let mut dirty_widget_ids = Vec::new();
                    crate::layout::reuse_layout_node(existing, tree, &mut dirty_widget_ids)
                        .map(|layout| (std::sync::Arc::new(layout), dirty_widget_ids))
                })
            };
            let mut mode = "none";
            let (layout, dirty_widget_ids) = if let Some(layout) = cached_layout {
                mode = "cached";
                (Some(layout), Vec::new())
            } else if let Some((layout, dirty_widget_ids)) = reused_layout_and_dirty {
                mode = "reuse";
                (Some(layout), dirty_widget_ids)
            } else {
                if tree.is_some() {
                    mode = "full";
                }
                let layout = tree.as_ref().and_then(|tree| {
                    self.runtime
                        .layout_snapshot_for_tree_with_geometry_and_offset(
                            tree,
                            Some((cols, rows)),
                            frame_viewport,
                            buffer_id * 100_000,
                        )
                });
                (layout, Vec::new())
            };
            self.trace_ui_layout_event(
                &buffer_name,
                &format!("inactive-tile-{tile_id}"),
                layout.as_deref(),
            );
            if let Some(leaf) = self.tile_root.find_leaf_mut(tile_id) {
                leaf.cached_layout = layout;
                leaf.cached_layout_widget_tree_revision = if leaf.cached_layout.is_some() {
                    widget_tree_revision
                } else {
                    0
                };
                leaf.widget_viewport_width = cols;
                leaf.widget_viewport_height = rows;
                leaf.layout_frame_viewport = frame_viewport;
                leaf.dirty_widget_ids = dirty_widget_ids;
                leaf.layout_revision = leaf.layout_revision.wrapping_add(1);
                leaf.cached_inactive_frame = None;
            }
            self.record_layout_refresh_timing(
                &buffer_name,
                Some(tile_id),
                mode,
                layout_started.elapsed(),
            );
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
        let widget_tree_revision = self.buffers[buffer_idx].widget_tree_revision;
        let tile_ids = self.tile_root.leaf_ids();
        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
        let tiles_to_update: Vec<(TileId, f32, f32, Option<crate::layout::Rect>)> = tile_ids
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
                        &tile_body_rect(*r, !leaf.tabs.is_empty()),
                        show_status,
                        leaf.show_border,
                        leaf.border_width_px,
                        cell_w,
                        cell_h,
                    ),
                    None => (
                        self.runtime.layout_cols_exact(),
                        self.runtime.layout_rows_exact(),
                    ),
                };
                Some((id, cols, rows, self.tile_layout_frame_viewport(id)))
            })
            .collect();

        for (tile_id, cols, rows, frame_viewport) in tiles_to_update {
            let layout_started = Instant::now();
            let buffer_name = self.buffers[buffer_idx].name.clone();
            // Move the cached layout out of the tile (and drop the cached
            // inactive frame, which this refresh invalidates anyway) so this
            // update owns the only reference: the batched in-place reuse below
            // can then mutate the path nodes instead of rebuilding the tree.
            // Every exit from this iteration reassigns `leaf.cached_layout`.
            let mut layout = self.tile_root.find_leaf_mut(tile_id).and_then(|leaf| {
                leaf.cached_inactive_frame = None;
                leaf.cached_layout.take()
            });
            let mut dirty_widget_ids = Vec::new();
            let mut reuse_mode = "targeted";
            let mut miss_reason = None::<String>;
            let mut targeted = layout.is_some();
            let subtree_paths = layout
                .as_ref()
                .map(|layout| crate::layout::subtree_root_paths(layout.as_ref()));
            // Fast path: apply every subtree replacement in one traversal.
            // The per-root loop below rebuilds the full layout tree once per
            // root, so N changed subtrees cost N full-tree clones; the batched
            // reuse keeps it at one, and at zero when this tile owns its
            // layout. Any miss falls through to the per-root loop, which
            // preserves the targeted-relayout and full-relayout fallbacks (and
            // their diagnostics) unchanged.
            let mut batched_applied = false;
            if let (Some(existing), Some(paths_by_root)) = (layout.as_mut(), subtree_paths.as_ref())
            {
                let batched_paths: Option<Vec<&[usize]>> = subtree_roots
                    .iter()
                    .map(|root_id| paths_by_root.get(root_id).map(|path| path.as_slice()))
                    .collect();
                if let Some(batched_paths) = batched_paths
                    && crate::layout::reuse_layout_node_for_subtree_paths_in_place(
                        existing,
                        tree,
                        &batched_paths,
                        &mut dirty_widget_ids,
                    )
                    .is_ok()
                {
                    batched_applied = true;
                }
                if !batched_applied {
                    dirty_widget_ids.clear();
                }
            }
            for subtree_root_id in subtree_roots {
                if batched_applied {
                    break;
                }
                if layout.is_none() {
                    targeted = false;
                    miss_reason.get_or_insert_with(|| "missing-layout".to_string());
                    break;
                }
                let Some(child_path) = subtree_paths
                    .as_ref()
                    .and_then(|paths| paths.get(subtree_root_id))
                else {
                    targeted = false;
                    miss_reason = Some(format!("missing-subtree-path:{subtree_root_id}"));
                    break;
                };
                // Reuse in place first (no tree copy when this tile owns its
                // layout); the plan phase fails before touching anything, so
                // the relayout fallback still sees the untouched layout.
                let reuse_result = crate::layout::reuse_layout_node_for_subtree_paths_in_place(
                    layout.as_mut().expect("layout checked above"),
                    tree,
                    &[child_path.as_slice()],
                    &mut dirty_widget_ids,
                );
                if let Err(reuse_reason) = reuse_result {
                    reuse_mode = "targeted-relayout";
                    let relayout_result = self.runtime.relayout_subtree_for_tree_with_viewport(
                        layout.as_ref().expect("layout checked above").as_ref(),
                        tree,
                        child_path,
                        Some((cols, rows)),
                        frame_viewport,
                        &mut dirty_widget_ids,
                    );
                    match relayout_result {
                        Ok(updated) => {
                            layout = Some(std::sync::Arc::new(updated));
                        }
                        Err(relayout_reason) => {
                            targeted = false;
                            miss_reason = Some(format!(
                                "subtree:{subtree_root_id}:{reuse_reason}; partial-relayout:{relayout_reason}"
                            ));
                            break;
                        }
                    }
                }
            }
            let (layout, dirty_widget_ids) = if targeted {
                (layout, dirty_widget_ids)
            } else {
                reuse_mode = "full";
                let layout = self
                    .runtime
                    .layout_snapshot_for_tree_with_geometry_and_offset(
                        tree,
                        Some((cols, rows)),
                        frame_viewport,
                        buffer_id * 100_000,
                    );
                (layout, Vec::new())
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
                leaf.cached_layout_widget_tree_revision = if leaf.cached_layout.is_some() {
                    widget_tree_revision
                } else {
                    0
                };
                leaf.widget_viewport_width = cols;
                leaf.widget_viewport_height = rows;
                leaf.layout_frame_viewport = frame_viewport;
                leaf.dirty_widget_ids = dirty_widget_ids;
                leaf.layout_revision = leaf.layout_revision.wrapping_add(1);
                leaf.cached_inactive_frame = None;
            }
            self.record_layout_refresh_timing(
                &buffer_name,
                Some(tile_id),
                reuse_mode,
                layout_started.elapsed(),
            );
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

    fn flush_active_subtree_replacements(
        &mut self,
        replacements: &mut Vec<EditorSubtreeReplacement>,
    ) {
        if replacements.is_empty() {
            return;
        }
        while let Some(buffer_idx) = replacements
            .first()
            .map(|replacement| replacement.buffer_idx)
        {
            let mut group = Vec::new();
            let mut remaining = Vec::new();
            for replacement in replacements.drain(..) {
                if replacement.buffer_idx == buffer_idx {
                    group.push(replacement);
                } else {
                    remaining.push(replacement);
                }
            }
            *replacements = remaining;

            let common_source = group
                .first()
                .and_then(|first| {
                    group
                        .iter()
                        .all(|replacement| replacement.source_buffer_id == first.source_buffer_id)
                        .then_some(first.source_buffer_id)
                })
                .flatten();
            let batch = group
                .iter()
                .map(|replacement| {
                    (
                        replacement.subtree_root_id,
                        replacement.tree.deep_clone(),
                        replacement.reactive_dependencies.clone(),
                    )
                })
                .collect::<Vec<_>>();
            let batch_replaced = {
                let buffer = &mut self.buffers[buffer_idx];
                let replaced = buffer.replace_widget_subtrees(&batch, common_source);
                if replaced {
                    buffer.view_mode = view_mode_for_widget_tree(buffer.widget_tree.as_ref());
                }
                replaced
            };
            let mut any_replaced = batch_replaced;
            if !batch_replaced {
                for replacement in &group {
                    let replaced = {
                        let buffer = &mut self.buffers[buffer_idx];
                        let replaced = buffer.replace_widget_subtree(
                            replacement.subtree_root_id,
                            replacement.tree.deep_clone(),
                            replacement.source_buffer_id,
                            replacement.reactive_dependencies.clone(),
                        );
                        if replaced {
                            buffer.view_mode =
                                view_mode_for_widget_tree(buffer.widget_tree.as_ref());
                        }
                        replaced
                    };
                    if replaced {
                        any_replaced = true;
                    }
                }
            }
            if !any_replaced {
                continue;
            }
            if self.active_buffer_idx() == buffer_idx {
                crate::widget_render::clear_overlay();
                if let Some(tree) = self.buffers[buffer_idx].widget_tree.clone() {
                    let snapshot = self.buffers[buffer_idx].committed_ui_snapshot.clone();
                    let buffer_id = self.buffers[buffer_idx].id as u64;
                    self.runtime.adopt_current_widget_tree_snapshot(
                        tree,
                        snapshot,
                        buffer_id * 100_000,
                    );
                }
                self.remap_focused_widget_after_layout_change();
            }
        }
    }

    /// Resolve a mode reference by exact name, then by the flat base for a
    /// module reference to a vanilla mode. There is deliberately no registry
    /// scan by base name because two modules may define the same mode.
    fn resolve_mode_name(&self, name: String) -> String {
        if self.mode_registry.contains_key(&name) {
            return name;
        }
        let base = crate::modules::split_qualified(&name)
            .map(|(_, base)| base)
            .unwrap_or(name.as_str());
        if base != name.as_str() && self.mode_registry.contains_key(base) {
            return base.to_string();
        }
        name
    }

    pub fn refresh_runtime_side_effects(&mut self) {
        let scene_trace = std::env::var("ESEQ_SCENE_TRACE").is_ok_and(|v| v == "1");
        let trace_started = std::time::Instant::now();
        self.last_layout_refresh_timings.clear();
        self.lisp_bindings = self.default_lisp_bindings.clone();
        self.lisp_bindings.extend(self.runtime.lisp_bindings());

        if let Some(buffer_id) = self.runtime.take_pending_eval_buffer() {
            self.evaluate_buffer_transactional(buffer_id);
        }

        if let Some(read_only) = self.runtime.take_pending_set_read_only() {
            self.active_buffer_mut().read_only = read_only;
        }

        // Process mode definitions
        for definition in self.runtime.take_pending_mode_defs() {
            self.mode_registry.insert(
                definition.name.clone(),
                MajorMode {
                    name: definition.name,
                    read_only: definition.read_only,
                    live_keys: definition.live_keys,
                    keybindings: HashMap::new(),
                    on_enter: definition.on_enter,
                    on_key: definition.on_key,
                },
            );
        }

        // Process mode keybindings
        for (mode_name, key, handler) in self.runtime.take_pending_mode_bindings() {
            let mode_name = self.resolve_mode_name(mode_name);
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
                {
                    let leaf = self.active_leaf_mut();
                    leaf.buffer_idx = idx;
                    leaf.selected_tab = leaf.tabs.iter().position(|tab| tab.buffer_idx == idx);
                    Self::invalidate_leaf_for_buffer_switch(leaf);
                }
                self.record_buffer_access_by_idx(idx);
                self.mark_needs_redraw();
                self.sync_runtime_context();
                self.completion = None;
                self.clear_mark();
                self.restore_buffer_widget_tree();
            }
        }

        for cleared_source in self.runtime.take_pending_cleared_effect_sources() {
            let target_indices = self
                .buffers
                .iter()
                .enumerate()
                .filter_map(|(idx, buffer)| {
                    let clear_is_current = buffer
                        .committed_ui_runtime_generation
                        .is_none_or(|generation| generation <= cleared_source.runtime_generation);
                    (clear_is_current
                        && buffer.widget_tree_source == cleared_source.source_buffer_id)
                        .then_some(idx)
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
            let mode_name = self.resolve_mode_name(mode_name);
            let mode_def = self.mode_registry.get(&mode_name).cloned();
            let buffer = self.active_buffer_mut();
            buffer.mode = BufferMode::Named(mode_name.clone());
            if let Some(mode_def) = &mode_def {
                buffer.read_only = mode_def.read_only;
            }
            // Call on_enter hook
            if let Some(on_enter) = mode_def.as_ref().and_then(|m| m.on_enter.clone()) {
                self.sync_runtime_source_context();
                let on_enter = self.runtime.resolve_handler_name(&on_enter).to_string();
                let code = format!("({on_enter})");
                let _ = self.runtime.eval_str(&code);
                if let Some(status) = self.runtime.take_status_message() {
                    self.minibuffer = Some(status);
                }
            }
            self.remap_focused_widget_after_layout_change();
        }

        if let Some(text) = self.runtime.take_pending_set_text() {
            self.active_buffer_mut().set_text(&text);
        }

        for (name, text) in self.runtime.take_pending_set_text_for() {
            let buffer_idx = self.ensure_scratch_buffer_named(&name);
            self.buffers[buffer_idx].set_text(&text);
            if self.active_buffer_idx() == buffer_idx {
                self.remap_focused_widget_after_layout_change();
            }
        }

        for (name, text, separator) in self.runtime.take_pending_append_text_for() {
            let buffer_idx = self.ensure_scratch_buffer_named(&name);
            let mut next_text = self.buffers[buffer_idx].text();
            if !next_text.trim().is_empty() {
                next_text.push_str(&separator);
            }
            next_text.push_str(&text);
            self.buffers[buffer_idx].set_text(&next_text);
            if self.active_buffer_idx() == buffer_idx {
                self.remap_focused_widget_after_layout_change();
            }
        }

        for (name, lines) in self.runtime.take_pending_append_lines_for() {
            let buffer_idx = self.ensure_scratch_buffer_named(&name);
            let mut next_text = self.buffers[buffer_idx].text();
            if !next_text.trim().is_empty() {
                next_text.push_str("\n\n");
            }
            next_text.push_str(&lines.join("\n"));
            self.buffers[buffer_idx].set_text(&next_text);
            if self.active_buffer_idx() == buffer_idx {
                self.remap_focused_widget_after_layout_change();
            }
        }

        for (name, lines) in self.runtime.take_pending_remove_lines_for() {
            let Some(buffer_idx) = self.buffers.iter().position(|b| b.name == name) else {
                continue;
            };
            if lines.is_empty() {
                continue;
            }
            let mut previous_blank = true;
            let mut kept = Vec::new();
            for line in self.buffers[buffer_idx].lines.iter() {
                if lines.iter().any(|target| target == line) {
                    continue;
                }
                let blank = line.trim().is_empty();
                if blank && previous_blank {
                    continue;
                }
                kept.push(line.clone());
                previous_blank = blank;
            }
            while kept.last().is_some_and(|line| line.trim().is_empty()) {
                kept.pop();
            }
            let next_text = kept.join("\n");
            self.buffers[buffer_idx].set_text(&next_text);
            if self.active_buffer_idx() == buffer_idx {
                self.remap_focused_widget_after_layout_change();
            }
        }

        if let Some(lines) = self.runtime.take_pending_set_lines() {
            let buffer = self.active_buffer_mut();
            buffer.set_text(&lines.join("\n"));
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
                    buffer.set_widget_tree(Some(tree.clone()), Some(source_id));
                    buffer.view_mode = ViewMode::UiOnly;
                    self.runtime.set_widget_tree(tree);
                    self.remap_focused_widget_after_layout_change();
                }
            }
        }

        if scene_trace {
            eprintln!(
                "[side-effects-trace] pre-widget-trees={:.3}ms",
                trace_started.elapsed().as_secs_f64() * 1000.0
            );
        }
        let widget_trees_started = std::time::Instant::now();
        let mut inactive_buffers_to_refresh: HashMap<usize, Option<Vec<u64>>> = HashMap::new();
        let mut active_subtree_replacements = Vec::<EditorSubtreeReplacement>::new();
        let mut inactive_subtree_batches: HashMap<
            usize,
            (Option<BufferId>, Vec<(u64, Value, Vec<ReactiveFieldKey>)>),
        > = HashMap::new();
        self.sync_active_buffer_widget_snapshot_from_runtime();
        for pending in self.runtime.take_pending_buffer_widget_trees() {
            match pending {
                PendingUiUpdate::FullTree(pending) => {
                    self.flush_active_subtree_replacements(&mut active_subtree_replacements);
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
                    let upgraded_subtree_root = upgraded_subtree_root.and_then(|subtree_root_id| {
                        self.buffers[buffer_idx]
                            .replace_widget_subtree(
                                subtree_root_id,
                                pending.tree.deep_clone(),
                                pending.source_buffer_id,
                                pending.reactive_dependencies.clone(),
                            )
                            .then_some(subtree_root_id)
                    });
                    if let Some(subtree_root_id) = upgraded_subtree_root {
                        if debug_ui_updates_enabled() {
                            eprintln!(
                                "[ui-update full->subtree] target={} root={}",
                                buffer_name, subtree_root_id
                            );
                        }
                        self.buffers[buffer_idx].view_mode = view_mode_for_widget_tree(
                            self.buffers[buffer_idx].widget_tree.as_ref(),
                        );
                        if is_active {
                            crate::widget_render::clear_overlay();
                            let runtime_upgraded = self
                                .runtime
                                .try_upgrade_full_tree_to_current_subtree(&pending);
                            if !runtime_upgraded {
                                self.restore_runtime_widget_tree_from_buffer(buffer_idx);
                            }
                            self.remap_focused_widget_after_layout_change();
                        } else {
                            inactive_buffers_to_refresh
                                .entry(buffer_idx)
                                .or_insert_with(|| Some(Vec::new()))
                                .as_mut()
                                .map(|roots| roots.push(subtree_root_id));
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
                            buffer.view_mode =
                                view_mode_for_widget_tree(buffer.widget_tree.as_ref());
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
                    ..
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
                    // A rerun subtree whose rendered tree (including captured
                    // callback environments) is identical to the committed
                    // content changes nothing: skip the splice, snapshot
                    // re-index, layout refresh, and retained repaint for it.
                    if self.buffers[buffer_idx]
                        .committed_ui_snapshot
                        .as_ref()
                        .is_some_and(|snapshot| {
                            snapshot.subtree_replacement_is_noop(
                                subtree_root_id,
                                &tree,
                                &reactive_dependencies,
                            )
                        })
                    {
                        self.trace_ui_tree_event_with(&buffer_name, "noop-subtree", || {
                            format!("root={subtree_root_id}")
                        });
                        continue;
                    }
                    let is_active = self.active_buffer_idx() == buffer_idx;
                    if is_active {
                        active_subtree_replacements.push(EditorSubtreeReplacement {
                            buffer_idx,
                            source_buffer_id,
                            subtree_root_id,
                            tree,
                            reactive_dependencies,
                        });
                        continue;
                    }
                    // Pending subtree trees are freshly built by
                    // annotate_widget_tree_stable_ids, so they can be adopted
                    // without another deep clone. Batch them per buffer so the
                    // snapshot merge/re-index runs once per buffer instead of
                    // once per subtree.
                    inactive_subtree_batches
                        .entry(buffer_idx)
                        .or_insert_with(|| (source_buffer_id, Vec::new()))
                        .1
                        .push((subtree_root_id, tree, reactive_dependencies));
                }
            }
        }
        for (buffer_idx, (source_buffer_id, replacements)) in inactive_subtree_batches {
            let buffer_name = self.buffers[buffer_idx].name.clone();
            let batch_applied = {
                let buffer = &mut self.buffers[buffer_idx];
                buffer.replace_widget_subtrees(&replacements, source_buffer_id)
            };
            if batch_applied {
                self.buffers[buffer_idx].view_mode =
                    view_mode_for_widget_tree(self.buffers[buffer_idx].widget_tree.as_ref());
                if let Some(roots) = inactive_buffers_to_refresh
                    .entry(buffer_idx)
                    .or_insert_with(|| Some(Vec::new()))
                    .as_mut()
                {
                    roots.extend(replacements.iter().map(|(root_id, _, _)| *root_id));
                }
                self.trace_ui_tree_event_with(&buffer_name, "applied-subtree", || {
                    format!(
                        "roots={:?} after={}",
                        replacements
                            .iter()
                            .map(|(root_id, _, _)| *root_id)
                            .collect::<Vec<_>>(),
                        debug_widget_tree_summary(self.buffers[buffer_idx].widget_tree.as_ref())
                    )
                });
                continue;
            }
            // The batch merge is all-or-nothing; fall back to applying each
            // subtree individually so valid roots still land when one is stale.
            for (subtree_root_id, tree, reactive_dependencies) in replacements {
                let replaced = {
                    let buffer = &mut self.buffers[buffer_idx];
                    let replaced = buffer.replace_widget_subtree(
                        subtree_root_id,
                        tree,
                        source_buffer_id,
                        reactive_dependencies,
                    );
                    if replaced {
                        buffer.view_mode = view_mode_for_widget_tree(buffer.widget_tree.as_ref());
                    }
                    replaced
                };
                if replaced {
                    inactive_buffers_to_refresh
                        .entry(buffer_idx)
                        .or_insert_with(|| Some(Vec::new()))
                        .as_mut()
                        .map(|roots| roots.push(subtree_root_id));
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
        if scene_trace {
            eprintln!(
                "[side-effects-trace] pending-loop={:.3}ms",
                widget_trees_started.elapsed().as_secs_f64() * 1000.0
            );
        }
        let flush_started = std::time::Instant::now();
        self.flush_active_subtree_replacements(&mut active_subtree_replacements);

        if let Some(pending) = self.runtime.take_pending_inline_widgets()
            && let Some(buffer_idx) = self
                .buffers
                .iter()
                .position(|buffer| buffer.id == pending.source_buffer_id)
        {
            if std::env::var("ESEQ_INLINE_TRACE").is_ok_and(|value| value != "0") {
                eprintln!(
                    "[inline-widgets] consume source_buffer_id={} name={:?} count={}",
                    pending.source_buffer_id,
                    self.buffers[buffer_idx].name,
                    pending.widgets.len()
                );
            }
            let inline_result = self.buffers[buffer_idx].set_inline_code_widgets(pending.widgets);
            match inline_result {
                Ok(()) => {
                    let tree = inline_widget_tree(&self.buffers[buffer_idx]);
                    let is_active = self.active_buffer_idx() == buffer_idx;
                    if let Some(tree) = tree {
                        self.buffers[buffer_idx].set_widget_tree(
                            Some(tree.deep_clone()),
                            Some(pending.source_buffer_id),
                        );
                        self.buffers[buffer_idx].view_mode = ViewMode::Both;
                        if is_active {
                            self.active_leaf_mut().widget_scroll_top = 0.0;
                            self.runtime
                                .set_widget_id_offset(pending.source_buffer_id as u64 * 100_000);
                            self.runtime.set_widget_tree(tree);
                            self.remap_focused_widget_after_layout_change();
                        } else {
                            inactive_buffers_to_refresh.insert(buffer_idx, None);
                        }
                    } else if self.buffers[buffer_idx]
                        .widget_tree
                        .as_ref()
                        .is_some_and(widget_tree_is_inline_root)
                    {
                        self.buffers[buffer_idx].set_widget_tree(None, None);
                        if is_active {
                            self.runtime.clear_current_widget_tree();
                        }
                    }
                    if std::env::var("ESEQ_INLINE_TRACE").is_ok_and(|value| value != "0") {
                        eprintln!(
                            "[inline-widgets] applied source_buffer_id={} name={:?} registered={} view_mode={} inline_root={}",
                            pending.source_buffer_id,
                            self.buffers[buffer_idx].name,
                            self.buffers[buffer_idx].inline_code_widgets().len(),
                            self.buffers[buffer_idx].view_mode.label(),
                            self.buffers[buffer_idx]
                                .widget_tree
                                .as_ref()
                                .is_some_and(widget_tree_is_inline_root)
                        );
                    }
                }
                Err(error) => {
                    self.minibuffer = Some(format!("Inline widget registration failed: {error}"));
                }
            }
        }
        if scene_trace {
            eprintln!(
                "[side-effects-trace] active-flush={:.3}ms",
                flush_started.elapsed().as_secs_f64() * 1000.0
            );
        }
        let inactive_started = std::time::Instant::now();
        // The visible-binding scan walks every visible layout; defer it so
        // refreshing N buffers in this flush pays for one scan, not N.
        self.visible_binding_sync_deferred = true;
        for (buffer_idx, subtree_roots) in inactive_buffers_to_refresh {
            match subtree_roots {
                Some(subtree_roots) => {
                    let per_buffer = std::time::Instant::now();
                    let root_count = subtree_roots.len();
                    self.refresh_inactive_tile_layouts_for_buffer_subtrees(
                        buffer_idx,
                        &subtree_roots,
                    );
                    if scene_trace {
                        eprintln!(
                            "[side-effects-trace] inactive-subtrees buffer={} roots={} {:.3}ms",
                            self.buffers[buffer_idx].name,
                            root_count,
                            per_buffer.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                }
                None => {
                    let per_buffer = std::time::Instant::now();
                    self.refresh_inactive_tile_layouts_for_buffer(buffer_idx);
                    if scene_trace {
                        eprintln!(
                            "[side-effects-trace] inactive-full buffer={} {:.3}ms",
                            self.buffers[buffer_idx].name,
                            per_buffer.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                }
            }
        }
        self.visible_binding_sync_deferred = false;
        if std::mem::take(&mut self.visible_binding_sync_pending) {
            let bindings_started = std::time::Instant::now();
            self.sync_reactive_bindings_for_visible_layouts();
            if scene_trace {
                eprintln!(
                    "[side-effects-trace] deferred-bindings {:.3}ms",
                    bindings_started.elapsed().as_secs_f64() * 1000.0
                );
            }
        }
        if scene_trace {
            eprintln!(
                "[side-effects-trace] inactive-refresh-total={:.3}ms",
                inactive_started.elapsed().as_secs_f64() * 1000.0
            );
        }

        // Process set-buffer-mode-for (after buffer creation so targets exist)
        for (buf_name, mode_name) in self.runtime.take_pending_set_mode_for() {
            let mode_name = self.resolve_mode_name(mode_name);
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
                        if self.active_buffer_idx() == idx {
                            self.record_buffer_access_by_idx(idx);
                            continue;
                        }
                        self.save_current_widget_tree();
                        {
                            let leaf = self.active_leaf_mut();
                            leaf.buffer_idx = idx;
                            leaf.selected_tab =
                                leaf.tabs.iter().position(|tab| tab.buffer_idx == idx);
                            Self::invalidate_leaf_for_buffer_switch(leaf);
                        }
                        self.record_buffer_access_by_idx(idx);
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
                crate::runtime::TileOp::SetWindowTabsFor { current, tabs } => {
                    if let Err(error) = self.set_tabs_in_tile_showing(&current, tabs) {
                        self.minibuffer =
                            Some(format!("Could not update tabs for '{current}': {error}"));
                    }
                }
                crate::runtime::TileOp::ClearWindowTabsFor { current } => {
                    if let Some(buffer_idx) = self.buffers.iter().position(|b| b.name == current) {
                        if let Some(leaf) = self.tile_root.find_leaf_by_buffer_idx_mut(buffer_idx) {
                            leaf.tabs.clear();
                            leaf.selected_tab = None;
                            leaf.cached_inactive_frame = None;
                            leaf.layout_revision = leaf.layout_revision.wrapping_add(1);
                            self.mark_needs_redraw();
                        }
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
            let resized = self
                .tile_root
                .find_parent_split(self.active_tile)
                .map(|split| {
                    split.ratio = (split.ratio + delta as f32).clamp(0.1, 0.9);
                    split.remember_key.clone()
                });
            if let Some(remember_key) = resized {
                self.enforce_min_sizes_recursive();
                if let Some(remember_key) = remember_key
                    && let Some(split) = self.tile_root.find_parent_split(self.active_tile)
                {
                    self.remembered_split_ratios
                        .insert(remember_key, split.ratio);
                }
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

        if let Some(zoom) = self.runtime.take_pending_set_text_zoom() {
            match self.set_text_zoom(zoom as f32) {
                Ok(applied) => self.show_transient_message(format!("Text zoom: {applied:.2}")),
                Err(message) => self.show_transient_message(message),
            }
        }

        self.sync_layout_to_active_leaf();
        if scene_trace {
            eprintln!(
                "[side-effects-trace] total={:.3}ms",
                trace_started.elapsed().as_secs_f64() * 1000.0
            );
        }
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
        if let Value::String(callback) = &output.callback
            && let Some(anchor_id) = callback
                .strip_prefix(crate::vm::INLINE_WRITEBACK_CALLBACK)
                .and_then(|suffix| suffix.strip_prefix(':'))
                .and_then(|id| id.parse::<u64>().ok())
        {
            let Some(value) = output.args.first().cloned() else {
                self.minibuffer = Some("Inline widget produced no value".to_string());
                return true;
            };
            let value = self
                .active_buffer()
                .normalize_inline_widget_output(anchor_id, value);
            let buffer_id = self.active_buffer().id;
            if self.pending_inline_writeback != Some((buffer_id, anchor_id)) {
                self.record_undo_snapshot();
                self.pending_inline_writeback = Some((buffer_id, anchor_id));
            }
            let runtime_preview_error = self
                .active_buffer()
                .inline_widget_runtime_target(anchor_id)
                .and_then(|(target, inlet)| {
                    self.runtime
                        .invoke(target, vec![Value::Keyword(inlet), value.clone()])
                        .err()
                });
            match self
                .active_buffer_mut()
                .write_inline_widget_value(anchor_id, value)
            {
                Ok(()) => {
                    if let Some(tree) = inline_widget_tree(self.active_buffer()) {
                        self.active_buffer_mut()
                            .set_widget_tree(Some(tree.deep_clone()), Some(buffer_id));
                        self.runtime.set_widget_tree(tree);
                        self.position_inline_widget_layout(self.runtime.layout_cols_exact());
                        self.sync_layout_to_active_leaf();
                    }
                    self.minibuffer = runtime_preview_error
                        .map(|error| format!("Inline runtime preview failed: {error:?}"));
                }
                Err(error) => {
                    self.minibuffer = Some(format!("Inline widget write-back failed: {error}"));
                }
            }
            self.completion = None;
            self.mark_needs_redraw();
            return true;
        }
        if trace_ui_invalidation_enabled() {
            let args = output
                .args
                .iter()
                .take(3)
                .map(format_lisp_value)
                .collect::<Vec<_>>()
                .join(", ");
            let suffix = if output.args.len() > 3 { ", ..." } else { "" };
            eprintln!(
                "[ui-trace][callback] active_buffer={} callback={} args=[{}{}] arg_count={}",
                self.active_buffer().name,
                format_lisp_value(&output.callback),
                args,
                suffix,
                output.args.len()
            );
        }
        self.sync_patcher_emitted_source_buffer(&output.args);
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
        self.remap_focused_widget_after_layout_change();
        self.completion = None;
        self.mark_needs_redraw();
        true
    }

    fn finish_inline_widget_writeback(&mut self) {
        let Some((buffer_id, _)) = self.pending_inline_writeback.take() else {
            return;
        };
        self.finish_typing_undo_group();
        self.evaluate_buffer_transactional(buffer_id);
    }

    fn sync_patcher_emitted_source_buffer(&mut self, args: &[Value]) -> Option<BufferId> {
        let Value::Map(map) = args.first()? else {
            return None;
        };
        let status = map.get("status").and_then(|value| match &*value.borrow() {
            Value::Keyword(status) | Value::String(status) => Some(status.clone()),
            _ => None,
        })?;
        if status != "valid" {
            return None;
        }
        let source = map.get("source").and_then(|value| match &*value.borrow() {
            Value::String(source) => Some(source.clone()),
            _ => None,
        })?;
        let path = map.get("path").and_then(|value| match &*value.borrow() {
            Value::String(path) if !path.is_empty() => Some(path.clone()),
            _ => None,
        })?;
        let name = crate::widget_render::patcher::emitted_source_buffer_name(&path);
        let id =
            self.upsert_read_only_scratch_buffer_with_mode(&name, &source, BufferMode::DGenLisp);
        if self
            .active_buffer()
            .widget_tree
            .as_ref()
            .is_some_and(|tree| widget_tree_contains_patcher_path(tree, &path))
        {
            self.patcher_emitted_source_origins
                .insert(path, self.active_buffer().id);
        }
        Some(id)
    }

    fn handle_patcher_source_tab(&mut self, key: KeyEvent) -> bool {
        if key.code != KeyCode::Tab || key.modifiers != KeyModifiers::NONE {
            return false;
        }
        let Some(target) = self.patcher_source_tab_target() else {
            return false;
        };
        self.toggle_patcher_source_split(target);
        true
    }

    fn toggle_patcher_source_split(&mut self, target: PatcherSourceTabTarget) {
        let resolved = match self.resolve_patcher_source_tab_target(target) {
            Ok(resolved) => resolved,
            Err(error) => {
                self.minibuffer = Some(format!("Patch emitted source unavailable: {error}"));
                self.mark_needs_redraw();
                return;
            }
        };
        if let Some(source_tile_id) = self.visible_emitted_source_tile_for_path(&resolved.path) {
            if self.tile_root.leaf_count() <= 1 {
                self.set_active_buffer(resolved.patcher_buffer_id);
                return;
            }
            self.remove_tile_and_activate(source_tile_id, resolved.patcher_tile_id);
            return;
        }
        let Some(source_buffer_id) = resolved.source_buffer_id else {
            self.minibuffer = Some(format!(
                "Patch emitted source unavailable: no source buffer found for {}",
                resolved.path
            ));
            self.mark_needs_redraw();
            return;
        };
        let Some(source_buffer_idx) = self.buffer_idx_for_id(source_buffer_id) else {
            self.minibuffer = Some(format!(
                "Patch emitted source unavailable: source buffer disappeared for {}",
                resolved.path
            ));
            self.mark_needs_redraw();
            return;
        };
        let patcher_tile_id = resolved
            .patcher_tile_id
            .or_else(|| self.visible_tile_for_buffer_id(resolved.patcher_buffer_id));
        let Some(patcher_tile_id) = patcher_tile_id else {
            self.set_active_buffer(resolved.patcher_buffer_id);
            let tile_id = self.active_tile;
            self.split_active_tile(SplitDir::Vertical, source_buffer_idx);
            self.switch_active_tile(tile_id);
            return;
        };
        self.switch_active_tile(patcher_tile_id);
        self.split_active_tile(SplitDir::Vertical, source_buffer_idx);
    }

    fn resolve_patcher_source_tab_target(
        &mut self,
        target: PatcherSourceTabTarget,
    ) -> Result<ResolvedPatcherSourceTabTarget, String> {
        match target {
            PatcherSourceTabTarget::PatcherNode {
                node,
                buffer_id,
                tile_id,
            } => {
                let snapshot =
                    crate::widget_render::patcher::emitted_source_buffer_snapshot(&node)?;
                let source_buffer_id = self.upsert_read_only_scratch_buffer_with_mode(
                    &snapshot.buffer_name,
                    &snapshot.source,
                    BufferMode::DGenLisp,
                );
                self.patcher_emitted_source_origins
                    .insert(snapshot.path.clone(), buffer_id);
                Ok(ResolvedPatcherSourceTabTarget {
                    path: snapshot.path,
                    patcher_buffer_id: buffer_id,
                    patcher_tile_id: tile_id,
                    source_buffer_id: Some(source_buffer_id),
                })
            }
            PatcherSourceTabTarget::SourceBuffer { path } => {
                let patcher_buffer_id = self
                    .patcher_buffer_id_for_path(&path)
                    .ok_or_else(|| format!("No patcher buffer found for {path}"))?;
                let source_buffer_name =
                    crate::widget_render::patcher::emitted_source_buffer_name(&path);
                let source_buffer_id = self
                    .buffers
                    .iter()
                    .find(|buffer| buffer.name == source_buffer_name)
                    .map(|buffer| buffer.id);
                Ok(ResolvedPatcherSourceTabTarget {
                    path,
                    patcher_buffer_id,
                    patcher_tile_id: self.visible_tile_for_buffer_id(patcher_buffer_id),
                    source_buffer_id,
                })
            }
            PatcherSourceTabTarget::PatcherBuffer {
                path,
                buffer_id,
                tile_id,
            } => {
                let source_buffer_name =
                    crate::widget_render::patcher::emitted_source_buffer_name(&path);
                let source_buffer_id = self
                    .buffers
                    .iter()
                    .find(|buffer| buffer.name == source_buffer_name)
                    .map(|buffer| buffer.id);
                Ok(ResolvedPatcherSourceTabTarget {
                    path,
                    patcher_buffer_id: buffer_id,
                    patcher_tile_id: tile_id,
                    source_buffer_id,
                })
            }
        }
    }

    fn patcher_source_tab_target(&self) -> Option<PatcherSourceTabTarget> {
        if let Some(path) = crate::widget_render::patcher::emitted_source_path_from_buffer_name(
            &self.active_buffer().name,
        ) {
            return Some(PatcherSourceTabTarget::SourceBuffer { path });
        }
        if let Some(node) = self.focused_widget_node()
            && node.widget_type == "patcher"
            && !crate::widget_render::patcher::patcher_has_text_edit(&node)
        {
            return Some(PatcherSourceTabTarget::PatcherNode {
                node,
                buffer_id: self.active_buffer().id,
                tile_id: Some(self.active_tile),
            });
        }
        if let Some(target) = self.patcher_source_tab_target_for_tile(self.active_tile) {
            return Some(target);
        }
        self.tile_root
            .leaf_ids()
            .into_iter()
            .find_map(|tile_id| self.patcher_source_tab_target_for_tile(tile_id))
            .or_else(|| {
                let buffer = self.active_buffer();
                let path = buffer
                    .widget_tree
                    .as_ref()
                    .and_then(widget_tree_first_patcher_path)?;
                Some(PatcherSourceTabTarget::PatcherBuffer {
                    path,
                    buffer_id: buffer.id,
                    tile_id: Some(self.active_tile),
                })
            })
    }

    fn patcher_source_tab_target_for_tile(
        &self,
        tile_id: TileId,
    ) -> Option<PatcherSourceTabTarget> {
        let leaf = self.tile_root.find_leaf(tile_id)?;
        let buffer_id = self.buffers.get(leaf.buffer_idx)?.id;
        let layout = if tile_id == self.active_tile {
            self.runtime.current_layout.as_ref()
        } else {
            leaf.cached_layout.as_ref()
        }?;
        let node = find_patcher_layout_node(layout.as_ref())?;
        if crate::widget_render::patcher::patcher_has_text_edit(&node) {
            return None;
        }
        Some(PatcherSourceTabTarget::PatcherNode {
            node,
            buffer_id,
            tile_id: Some(tile_id),
        })
    }

    fn buffer_idx_for_id(&self, id: BufferId) -> Option<usize> {
        self.buffers.iter().position(|buffer| buffer.id == id)
    }

    fn visible_tile_for_buffer_id(&self, id: BufferId) -> Option<TileId> {
        self.tile_root.leaf_ids().into_iter().find(|tile_id| {
            self.tile_root
                .find_leaf(*tile_id)
                .and_then(|leaf| self.buffers.get(leaf.buffer_idx))
                .is_some_and(|buffer| buffer.id == id)
        })
    }

    fn visible_emitted_source_tile_for_path(&self, path: &str) -> Option<TileId> {
        let source_buffer_name = crate::widget_render::patcher::emitted_source_buffer_name(path);
        self.tile_root.leaf_ids().into_iter().find(|tile_id| {
            self.tile_root
                .find_leaf(*tile_id)
                .and_then(|leaf| self.buffers.get(leaf.buffer_idx))
                .is_some_and(|buffer| buffer.name == source_buffer_name)
        })
    }

    fn remove_tile_and_activate(
        &mut self,
        remove_tile_id: TileId,
        activate_tile_id: Option<TileId>,
    ) {
        if self.tile_root.leaf_count() <= 1 {
            return;
        }
        if self.tile_root.remove_leaf(remove_tile_id).is_none() {
            return;
        }
        let remaining_ids = self.tile_root.leaf_ids();
        let next_tile = activate_tile_id
            .filter(|tile_id| self.tile_root.find_leaf(*tile_id).is_some())
            .or_else(|| remaining_ids.first().copied());
        if let Some(next_tile) = next_tile {
            self.active_tile = next_tile;
            let buffer_idx = self.active_buffer_idx();
            self.record_buffer_access_by_idx(buffer_idx);
            self.sync_runtime_context();
            self.restore_buffer_widget_tree();
        }
        self.mark_needs_redraw();
    }

    fn patcher_buffer_id_for_path(&self, path: &str) -> Option<BufferId> {
        if let Some(buffer_id) = self
            .patcher_emitted_source_origins
            .get(path)
            .copied()
            .filter(|buffer_id| self.buffers.iter().any(|buffer| buffer.id == *buffer_id))
        {
            return Some(buffer_id);
        }
        self.buffers.iter().find_map(|buffer| {
            buffer
                .widget_tree
                .as_ref()
                .is_some_and(|tree| widget_tree_contains_patcher_path(tree, path))
                .then_some(buffer.id)
        })
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

/// The topmost open modal-family overlay panel (modal or context menu) in a
/// layout — the one a dismissal gesture reaches first.
///
/// Stacking order follows nesting: a context menu opened inside a modal is
/// laid out as its descendant and draws above it, so the innermost open panel
/// is on top. Among siblings the later one wins, matching draw order.
/// `widget_focus::find_open_modal_node` answers in tree order instead, which
/// returns the outer modal and would close the whole modal while its context
/// menu is still up.
fn topmost_open_overlay_panel(
    node: &crate::layout::LayoutNode,
) -> Option<&crate::layout::LayoutNode> {
    if let Some(found) = node
        .children
        .iter()
        .rev()
        .find_map(topmost_open_overlay_panel)
    {
        return Some(found);
    }
    if crate::widget_render::is_overlay_panel_widget(&node.widget_type) && !node.children.is_empty()
    {
        return Some(node);
    }
    None
}

/// Inspect hit test. Returns the hit node plus the accumulated scroll
/// offset between layout coordinates and rendered position, so callers can
/// place the hover highlight where the widget is actually drawn.
fn inspect_hit_test_layout(
    node: &crate::layout::LayoutNode,
    row: f32,
    col: f32,
) -> Option<(&crate::layout::LayoutNode, f32)> {
    inspect_hit_test_layout_impl(node, row, col, 0.0)
}

fn inspect_hit_test_layout_impl(
    node: &crate::layout::LayoutNode,
    row: f32,
    col: f32,
    scroll_dy: f32,
) -> Option<(&crate::layout::LayoutNode, f32)> {
    // An open modal-family overlay has a zero-size layout node (zero parent
    // footprint); its children are anchored to the frame viewport with real
    // rects, so descend without the containment gate.
    if crate::widget_render::is_overlay_panel_widget(&node.widget_type) {
        return node
            .children
            .iter()
            .rev()
            .find_map(|child| inspect_hit_test_layout_impl(child, row, col, scroll_dy));
    }
    if !rect_contains_point(node.rect, row, col) {
        return None;
    }
    // Scroll containers: children are hit at their rendered position
    // (mirrors hit_test_layout), and the offset is carried so hover rects
    // can be mapped back.
    let (child_row, child_dy) = if node.widget_type == "scroll" {
        let state = crate::widget_render::scroll::get_scroll_state(
            crate::widget_render::scroll::scroll_state_key(node),
        );
        (row + state.offset_y, scroll_dy + state.offset_y)
    } else {
        (row, scroll_dy)
    };
    let deepest = node
        .children
        .iter()
        .rev()
        .find_map(|child| inspect_hit_test_layout_impl(child, child_row, col, child_dy));
    match deepest {
        Some(hit) if inspect_node_has_source_identity(hit.0) => Some(hit),
        Some(_) if inspect_node_has_source_identity(node) => Some((node, scroll_dy)),
        Some(hit) => Some(hit),
        None => Some((node, scroll_dy)),
    }
}

fn rect_contains_point(rect: Rect, row: f32, col: f32) -> bool {
    row >= rect.row
        && row < rect.row + rect.height
        && col >= rect.col
        && col < rect.col + rect.width
}

fn find_widget_form_in_text(
    text: &str,
    node: &crate::layout::LayoutNode,
) -> Option<(usize, usize)> {
    let mut identities = Vec::new();
    if let Some(debug_name) = inspect_node_prop_string(node, "debug-name") {
        identities.push(("debug-name".to_string(), debug_name));
    }
    if let Some(key) = inspect_node_prop_string(node, "key").or_else(|| node.stable_key.clone()) {
        identities.push(("key".to_string(), key));
    }
    if identities.is_empty() {
        return None;
    }

    let bytes = text.as_bytes();
    let mut idx = 0usize;
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
                let form_start = idx;
                idx += 1;
                let mut cursor = skip_ws_and_comments(text, idx);
                let head_start = cursor;
                cursor = advance_symbol(text, cursor);
                if cursor == head_start || &text[head_start..cursor] != node.widget_type {
                    continue;
                }
                let Some(form_end) = find_matching_list_end(text, form_start) else {
                    continue;
                };
                if widget_form_matches_identities(&text[form_start..form_end], &identities) {
                    return Some(offset_to_position(text, form_start));
                }
                idx = form_end;
            }
            _ => idx += 1,
        }
    }

    None
}

fn find_unique_widget_form_in_definition(
    text: &str,
    symbol: &str,
    node: &crate::layout::LayoutNode,
) -> Option<(usize, usize)> {
    let (definition_start, definition_end) = find_definition_bounds_in_text(text, symbol)?;
    let mut matches =
        widget_form_positions_in_range(text, definition_start, definition_end, &node.widget_type);
    if matches.len() == 1 {
        matches.pop()
    } else {
        None
    }
}

fn find_definition_bounds_in_text(text: &str, symbol: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut idx = 0usize;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;
    let targets = ["def", "defmacro", "defwidget", "defmode"];

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
                let form_start = idx;
                depth += 1;
                idx += 1;
                if depth != 1 {
                    continue;
                }
                let mut cursor = skip_ws_and_comments(text, idx);
                let head_start = cursor;
                cursor = advance_symbol(text, cursor);
                if cursor == head_start || !targets.contains(&&text[head_start..cursor]) {
                    continue;
                }
                cursor = skip_ws_and_comments(text, cursor);
                let name_start = if text[cursor..].starts_with('(') {
                    skip_ws_and_comments(text, cursor + 1)
                } else {
                    cursor
                };
                let name_end = advance_symbol(text, name_start);
                if name_end == name_start || &text[name_start..name_end] != symbol {
                    continue;
                }
                if let Some(form_end) = find_matching_list_end(text, form_start) {
                    return Some((form_start, form_end));
                }
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

fn widget_form_positions_in_range(
    text: &str,
    start: usize,
    end: usize,
    widget_type: &str,
) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut idx = start;
    let mut positions = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;

    while idx < end && idx < bytes.len() {
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
                let form_start = idx;
                idx += 1;
                let mut cursor = skip_ws_and_comments(text, idx);
                let head_start = cursor;
                cursor = advance_symbol(text, cursor);
                if cursor != head_start && &text[head_start..cursor] == widget_type {
                    positions.push(offset_to_position(text, form_start));
                }
            }
            _ => idx += 1,
        }
    }

    positions
}

fn find_matching_list_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut idx = start;
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
            ';' => in_comment = true,
            '"' => in_string = true,
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(idx + 1);
                }
            }
            _ => {}
        }
        idx += 1;
    }

    None
}

fn widget_form_matches_identities(form: &str, identities: &[(String, String)]) -> bool {
    let Ok(tokens) = crate::parser::Parser::new(form.to_string()).parse() else {
        return false;
    };
    let mut ast = crate::parser::ASTParser::new(tokens);
    let Ok(expressions) = ast.parse() else {
        return false;
    };
    let Some(crate::parser::Expression::List(items)) = expressions.first() else {
        return false;
    };

    identities.iter().any(|(key, expected)| {
        items.windows(2).any(|pair| {
            matches!(&pair[0], crate::parser::Expression::Keyword(candidate) if candidate == key)
                && (expression_matches_source_identity(&pair[1], expected)
                    || expression_may_generate_source_identity(&pair[1], expected))
        })
    })
}

fn expression_matches_source_identity(expr: &crate::parser::Expression, expected: &str) -> bool {
    match expr {
        crate::parser::Expression::String(value)
        | crate::parser::Expression::Symbol(value)
        | crate::parser::Expression::QuoteSymbol(value) => value == expected,
        crate::parser::Expression::Keyword(value) => {
            expected == value || expected == format!(":{value}")
        }
        crate::parser::Expression::Number(value) => expected
            .parse::<f64>()
            .is_ok_and(|expected| (expected - value).abs() < f64::EPSILON),
        crate::parser::Expression::QuoteList(_)
        | crate::parser::Expression::List(_)
        | crate::parser::Expression::Quasiquote(_)
        | crate::parser::Expression::Unquote(_)
        | crate::parser::Expression::UnquoteSplicing(_) => false,
    }
}

fn expression_may_generate_source_identity(
    expr: &crate::parser::Expression,
    expected: &str,
) -> bool {
    let crate::parser::Expression::List(items) = expr else {
        return false;
    };
    let Some(crate::parser::Expression::Symbol(head)) = items.first() else {
        return false;
    };
    if head != "str" {
        return false;
    }

    let fragments = items[1..]
        .iter()
        .filter_map(|item| match item {
            crate::parser::Expression::String(value) if !value.is_empty() => Some(value.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    !fragments.is_empty() && string_contains_fragments_in_order(expected, &fragments)
}

fn string_contains_fragments_in_order(value: &str, fragments: &[&str]) -> bool {
    let mut search_start = 0usize;
    for fragment in fragments {
        let Some(relative_idx) = value[search_start..].find(fragment) else {
            return false;
        };
        search_start += relative_idx + fragment.len();
    }
    true
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

fn find_layout_node_by_stable_key<'a>(
    node: &'a crate::layout::LayoutNode,
    stable_key: &str,
) -> Option<&'a crate::layout::LayoutNode> {
    if node.stable_key.as_deref() == Some(stable_key) {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_layout_node_by_stable_key(child, stable_key))
}

/// Map dirty scroll state keys (stable ids) to the layout widget ids of the
/// scroll nodes that own them, so scroll changes ride the dirty-widget-id
/// invalidation path scoped to the owning subtree.
fn collect_scroll_widget_ids_for_keys(
    node: &crate::layout::LayoutNode,
    keys: &std::collections::HashSet<u64>,
    out: &mut Vec<u64>,
) {
    if node.widget_type == "scroll"
        && keys.contains(&crate::widget_render::scroll::scroll_state_key(node))
        && !out.contains(&node.widget_id)
    {
        out.push(node.widget_id);
    }
    for child in &node.children {
        collect_scroll_widget_ids_for_keys(child, keys, out);
    }
}

fn find_patcher_layout_node(node: &crate::layout::LayoutNode) -> Option<crate::layout::LayoutNode> {
    if node.widget_type == "patcher" {
        return Some(node.clone());
    }
    node.children.iter().find_map(find_patcher_layout_node)
}

fn widget_tree_first_patcher_path(value: &Value) -> Option<String> {
    match value {
        Value::Map(map) => {
            let is_patcher = map.get("type").is_some_and(
                |value| matches!(&*value.borrow(), Value::String(kind) if kind == "patcher"),
            );
            if is_patcher {
                for key in ["path", "file"] {
                    if let Some(path) = map.get(key).and_then(|value| match &*value.borrow() {
                        Value::String(path) | Value::Keyword(path) | Value::Symbol(path)
                            if !path.is_empty() =>
                        {
                            Some(path.clone())
                        }
                        _ => None,
                    }) {
                        return Some(path);
                    }
                }
            }
            map.values().find_map(|value| {
                let value = value.borrow();
                widget_tree_first_patcher_path(&value)
            })
        }
        Value::List(items) => items.iter().find_map(|item| {
            let item = item.borrow();
            widget_tree_first_patcher_path(&item)
        }),
        _ => None,
    }
}

fn widget_tree_contains_patcher_path(value: &Value, path: &str) -> bool {
    match value {
        Value::Map(map) => {
            let is_matching_patcher = map.get("type").is_some_and(
                |value| matches!(&*value.borrow(), Value::String(kind) if kind == "patcher"),
            ) && ["path", "file"].iter().any(|key| {
                map.get(*key).is_some_and(|value| {
                    matches!(
                        &*value.borrow(),
                        Value::String(value) | Value::Keyword(value) | Value::Symbol(value)
                            if value == path
                    )
                })
            });
            is_matching_patcher
                || map.values().any(|value| {
                    let value = value.borrow();
                    widget_tree_contains_patcher_path(&value, path)
                })
        }
        Value::List(items) => items.iter().any(|item| {
            let item = item.borrow();
            widget_tree_contains_patcher_path(&item, path)
        }),
        _ => false,
    }
}

fn inline_widget_tree(buffer: &Buffer) -> Option<Value> {
    if buffer.inline_code_widgets().is_empty() {
        return None;
    }
    let children = buffer
        .inline_code_widgets()
        .iter()
        .map(|inline| inline.widget.deep_clone())
        .collect::<Vec<_>>();
    let mut tree = crate::widgets::build_widget("v-stack", children);
    if let Value::Map(map) = &mut tree {
        map.insert(
            "__inline-root".to_string(),
            std::rc::Rc::new(std::cell::RefCell::new(Value::Bool(true))),
        );
        map.insert(
            "gap".to_string(),
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(0.0))),
        );
    }
    Some(tree)
}

fn position_inline_layout_nodes(
    node: &mut LayoutNode,
    placements: &[InlineLayoutPlacement],
    viewport_width: f32,
    root_height: f32,
) -> bool {
    let mut changed = false;
    if matches!(node.props.get("__inline-root"), Some(Value::Bool(true))) {
        if (node.rect.width - viewport_width).abs() > f32::EPSILON
            || (node.rect.height - root_height).abs() > f32::EPSILON
        {
            node.rect.width = viewport_width;
            node.rect.height = root_height;
            changed = true;
        }
    }
    let anchor_id = node
        .props
        .get("__inline-anchor-id")
        .and_then(|value| match value {
            Value::Number(number) if number.is_finite() && *number >= 0.0 => Some(*number as u64),
            _ => None,
        });
    if let Some(placement) = anchor_id.and_then(|id| {
        placements
            .iter()
            .find(|placement| placement.anchor_id == id)
    }) {
        let next = Rect {
            row: placement.row,
            col: placement.col,
            width: placement.width.unwrap_or(node.rect.width),
            height: placement.height.unwrap_or(node.rect.height),
        };
        if node.rect != next {
            node.rect = next;
            changed = true;
        }
        let was_stale = matches!(node.props.get("muted"), Some(Value::Bool(true)));
        if was_stale != placement.stale {
            if placement.stale {
                node.props.insert("muted".to_string(), Value::Bool(true));
            } else {
                node.props.remove("muted");
            }
            changed = true;
        }
    }
    for child in &mut node.children {
        changed |= position_inline_layout_nodes(child, placements, viewport_width, root_height);
    }
    changed
}

pub(crate) fn positioned_inline_layout_for_buffer(
    buffer: &Buffer,
    layout: Arc<LayoutNode>,
    viewport_width: f32,
    text_cell_width_scale: f32,
    text_cell_height_scale: f32,
) -> Arc<LayoutNode> {
    if buffer.inline_code_widgets().is_empty() {
        return layout;
    }
    let (placements, root_height) = inline_layout_placements(
        buffer,
        viewport_width,
        text_cell_width_scale,
        text_cell_height_scale,
    );
    let mut positioned = (*layout).clone();
    position_inline_layout_nodes(&mut positioned, &placements, viewport_width, root_height);
    Arc::new(positioned)
}

fn inline_layout_placements(
    buffer: &Buffer,
    viewport_width: f32,
    text_cell_width_scale: f32,
    text_cell_height_scale: f32,
) -> (Vec<InlineLayoutPlacement>, f32) {
    let display_map = buffer.inline_display_row_map();
    let mut placements = Vec::with_capacity(buffer.inline_code_widgets().len());
    for inline in buffer.inline_code_widgets() {
        let Some(anchor) = buffer.source_anchor(inline.anchor_id) else {
            continue;
        };
        let Some((buffer_line, _)) = buffer.position_at_byte_offset(anchor.end_byte) else {
            continue;
        };
        let container = buffer
            .source_anchor(inline.container_anchor_id)
            .unwrap_or(anchor);
        let container_end_line = buffer
            .position_at_byte_offset(container.end_byte)
            .map(|position| position.0)
            .unwrap_or(buffer_line);
        let visually_stale = anchor.stale
            || container.stale
            || matches!(&inline.widget, Value::Map(map) if matches!(map.get("__inline-live-diverged"), Some(value) if matches!(&*value.borrow(), Value::Bool(true))));
        placements.push(match inline.placement {
            InlineWidgetPlacement::Margin => {
                let natural_width = 16.0;
                InlineLayoutPlacement {
                    anchor_id: inline.anchor_id,
                    row: display_map
                        .display_row_for_buffer_line(buffer_line)
                        .unwrap_or(buffer_line) as f32
                        * text_cell_height_scale,
                    col: ((buffer.lines[buffer_line].chars().count() as f32 + 2.0)
                        * text_cell_width_scale)
                        .min((viewport_width - natural_width).max(0.0)),
                    width: None,
                    height: None,
                    stale: visually_stale,
                }
            }
            InlineWidgetPlacement::Inline { width_cells } => {
                let (inline_line, display_col) = buffer
                    .inline_widget_display_col(inline.anchor_id)
                    .unwrap_or((buffer_line, 0));
                InlineLayoutPlacement {
                    anchor_id: inline.anchor_id,
                    row: display_map
                        .display_row_for_buffer_line(inline_line)
                        .unwrap_or(inline_line) as f32
                        * text_cell_height_scale,
                    col: display_col as f32 * text_cell_width_scale,
                    width: Some(width_cells as f32 * text_cell_width_scale),
                    height: Some(text_cell_height_scale),
                    stale: visually_stale,
                }
            }
            InlineWidgetPlacement::Band { height_cells } => InlineLayoutPlacement {
                anchor_id: inline.anchor_id,
                row: display_map
                    .first_display_row_for_band(inline.anchor_id)
                    .unwrap_or(container_end_line + 1) as f32
                    * text_cell_height_scale,
                col: 0.0,
                width: Some(viewport_width),
                height: Some(height_cells as f32 * text_cell_height_scale),
                stale: visually_stale,
            },
        });
    }
    (
        placements,
        display_map.len() as f32 * text_cell_height_scale,
    )
}

fn widget_tree_is_inline_root(value: &Value) -> bool {
    let Value::Map(map) = value else {
        return false;
    };
    map.get("__inline-root")
        .is_some_and(|value| matches!(&*value.borrow(), Value::Bool(true)))
}

fn view_mode_for_widget_tree(tree: Option<&Value>) -> ViewMode {
    match tree {
        Some(tree) if widget_tree_is_inline_root(tree) => ViewMode::Both,
        Some(_) => ViewMode::UiOnly,
        None => ViewMode::TextOnly,
    }
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

    fn first_patcher_layout_node(
        node: &crate::layout::LayoutNode,
    ) -> Option<crate::layout::LayoutNode> {
        if node.widget_type == "patcher" {
            return Some(node.clone());
        }
        for child in &node.children {
            if let Some(found) = first_patcher_layout_node(child) {
                return Some(found);
            }
        }
        None
    }

    // Tests are included from the original file — they reference super:: helpers above.
    include!("tests.rs");
}
