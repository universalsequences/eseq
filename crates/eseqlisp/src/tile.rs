use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use crate::backend::Color;
use crate::host::BufferId;
use crate::layout::{LayoutNode, Rect};
use crate::mode::{BufferMode, TokenSpan};
use crate::ui::hit::HitGrid;
use crate::vm::Value;

pub type TileId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    Horizontal, // children stacked top/bottom
    Vertical,   // children stacked left/right
}

pub enum TileNode {
    Leaf(TileLeaf),
    Split(TileSplit),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileBufferTab {
    pub label: String,
    pub buffer_idx: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TileTabLayout {
    pub index: usize,
    pub label: String,
    pub rect: Rect,
    pub selected: bool,
}

pub const TILE_TAB_STRIP_HEIGHT: f32 = 1.4;
const TILE_TAB_MIN_WIDTH: f32 = 4.0;
const TILE_TAB_MAX_WIDTH: f32 = 14.0;
const TILE_TAB_LABEL_PAD: f32 = 2.0;
const TILE_TAB_GAP: f32 = 0.2;
const TILE_TAB_RIGHT_INSET: f32 = 1.0;

pub struct TileLeaf {
    pub id: TileId,
    pub buffer_idx: usize,
    pub tabs: Vec<TileBufferTab>,
    pub selected_tab: Option<usize>,
    pub show_status: bool,
    pub show_border: bool,
    /// Pixel width for Metal tile borders. TUI rendering ignores this.
    pub border_width_px: f32,
    /// Pixel radius for Metal tile borders. TUI rendering ignores this.
    pub border_radius_px: f32,
    /// Default Metal background color for this tile's buffer content.
    pub background_color: Option<Color>,
    /// Theme color name to resolve for the Metal background each frame.
    pub background_color_name: Option<String>,
    /// Minimum tile width in cells (enforced during divider drag).
    pub min_width: Option<f32>,
    /// Minimum tile height in cells (enforced during divider drag).
    pub min_height: Option<f32>,
    /// Maximum tile width in cells.
    pub max_width: Option<f32>,
    /// Maximum tile height in cells.
    pub max_height: Option<f32>,
    // Per-tile interaction state (moved from Editor)
    pub focused_widget_id: Option<u64>,
    pub focused_widget_node: Option<LayoutNode>,
    pub widget_scroll_top: f32,
    pub widget_viewport_height: f32,
    pub widget_scroll_left: f32,
    pub active_widget_gesture: Option<WidgetGesture>,
    pub last_widget_click: Option<WidgetClick>,
    pub hit_grid_cache: Option<CachedHitGrid>,
    pub highlight_cache: Option<HighlightCache>,
    // Per-tile layout cache
    pub cached_layout: Option<Arc<LayoutNode>>,
    pub dirty_widget_ids: Vec<u64>,
    pub layout_revision: u64,
    /// Cached RenderFrame for inactive tile optimization.
    /// Key is (buffer_id, buffer_revision, widget_tree_revision, layout_revision, scroll_top,
    /// viewport_width, viewport_height, exact viewport width bits, exact viewport height bits,
    /// view_mode).
    pub cached_inactive_frame: Option<(
        (
            BufferId,
            u64,
            u64,
            u64,
            usize,
            usize,
            usize,
            u32,
            u32,
            crate::editor::ViewMode,
        ),
        crate::backend::RenderFrame,
    )>,
}

pub struct TileSplit {
    pub id: TileId,
    pub dir: SplitDir,
    pub ratio: f32, // 0.0..1.0, portion for child `a`
    pub gap: f32,
    pub a: Box<TileNode>,
    pub b: Box<TileNode>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplitDividerHit {
    pub split_id: TileId,
    pub dir: SplitDir,
    pub area: Rect,
}

// Types moved from editor/mod.rs to be shared

#[derive(Debug, Clone)]
pub struct WidgetGesture {
    pub widget_id: u64,
    pub node: LayoutNode,
    pub start_precise_col: f32,
    pub start_precise_row: f32,
    pub drag_active: bool,
    pub gesture_data: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct WidgetClick {
    pub widget_id: u64,
    pub precise_col: f32,
    pub precise_row: f32,
    pub at: Instant,
}

pub struct CachedHitGrid {
    pub layout_revision: u64,
    pub scroll_top: f32,
    pub grid: HitGrid,
}

#[derive(Debug, Clone)]
pub struct HighlightCache {
    pub buffer_id: BufferId,
    pub buffer_revision: u64,
    pub buffer_mode: BufferMode,
    pub runtime_symbol_revision: u64,
    pub spans: Rc<Vec<Vec<TokenSpan>>>,
}

// ── TileLeaf constructors ────────────────────────────────────────────────

impl TileLeaf {
    pub fn new(id: TileId, buffer_idx: usize) -> Self {
        Self {
            id,
            buffer_idx,
            tabs: Vec::new(),
            selected_tab: None,
            show_status: true,
            show_border: true,
            border_width_px: 2.0,
            border_radius_px: 0.0,
            background_color: None,
            background_color_name: None,
            min_width: None,
            min_height: None,
            max_width: None,
            max_height: None,
            focused_widget_id: None,
            focused_widget_node: None,
            widget_scroll_top: 0.0,
            widget_viewport_height: 0.0,
            widget_scroll_left: 0.0,
            active_widget_gesture: None,
            last_widget_click: None,
            hit_grid_cache: None,
            highlight_cache: None,
            cached_layout: None,
            dirty_widget_ids: Vec::new(),
            layout_revision: 0,
            cached_inactive_frame: None,
        }
    }
}

pub fn tile_body_rect(rect: Rect, _has_tabs: bool) -> Rect {
    rect
}

pub fn tile_tab_layouts(
    rect: Rect,
    tabs: &[TileBufferTab],
    selected_tab: Option<usize>,
) -> Vec<TileTabLayout> {
    if tabs.is_empty() || rect.width <= 0.0 || rect.height <= 0.0 {
        return Vec::new();
    }
    let available_width = (rect.width - TILE_TAB_RIGHT_INSET * 2.0).max(0.0);
    if available_width <= 0.0 {
        return Vec::new();
    }
    let gap_total = TILE_TAB_GAP * tabs.len().saturating_sub(1) as f32;
    let available_tab_width = (available_width - gap_total).max(0.0);
    if available_tab_width <= 0.0 {
        return Vec::new();
    }

    let desired_widths = tabs
        .iter()
        .map(|tab| {
            (estimated_tab_label_width_cells(&tab.label) + TILE_TAB_LABEL_PAD)
                .clamp(TILE_TAB_MIN_WIDTH, TILE_TAB_MAX_WIDTH)
        })
        .collect::<Vec<_>>();
    let desired_total = desired_widths.iter().sum::<f32>();
    let widths = if desired_total <= available_tab_width {
        desired_widths
    } else {
        let equal_width = (available_tab_width / tabs.len() as f32).max(1.0);
        desired_widths
            .into_iter()
            .map(|width| width.min(equal_width).max(1.0))
            .collect::<Vec<_>>()
    };
    let total_width = widths.iter().sum::<f32>() + gap_total;
    let mut col = rect.col + rect.width - TILE_TAB_RIGHT_INSET - total_width;
    let selected = selected_tab.unwrap_or(usize::MAX);
    widths
        .into_iter()
        .enumerate()
        .map(|(index, width)| {
            let is_selected = index == selected;
            let layout = TileTabLayout {
                index,
                label: truncate_tab_label(&tabs[index].label, width),
                rect: Rect {
                    row: rect.row - TILE_TAB_STRIP_HEIGHT,
                    col,
                    width,
                    height: TILE_TAB_STRIP_HEIGHT,
                },
                selected: is_selected,
            };
            col += width + TILE_TAB_GAP;
            layout
        })
        .collect()
}

fn truncate_tab_label(label: &str, width: f32) -> String {
    let budget = (width - TILE_TAB_LABEL_PAD).max(0.0);
    let mut used = 0.0;
    let mut out = String::new();
    for ch in label.chars() {
        let char_width = estimated_tab_char_width_cells(ch);
        if used + char_width > budget + 0.01 && !out.is_empty() {
            break;
        }
        used += char_width;
        out.push(ch);
    }
    out
}

fn estimated_tab_label_width_cells(label: &str) -> f32 {
    label.chars().map(estimated_tab_char_width_cells).sum()
}

fn estimated_tab_char_width_cells(ch: char) -> f32 {
    match ch {
        ' ' => 0.38,
        'i' | 'l' | 'I' | '!' | '|' | '.' | ',' | ':' | ';' | '\'' | '`' => 0.36,
        'j' | 'f' | 'r' | 't' | '(' | ')' | '[' | ']' | '{' | '}' => 0.5,
        'm' | 'w' | 'M' | 'W' => 0.95,
        'A'..='Z' => 0.78,
        '0'..='9' => 0.64,
        '-' | '_' | '/' | '\\' => 0.55,
        _ => 0.64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_width_estimate_uses_proportional_character_widths() {
        let rect = Rect {
            row: 10.0,
            col: 0.0,
            width: 80.0,
            height: 20.0,
        };
        let layouts = tile_tab_layouts(
            rect,
            &[
                TileBufferTab {
                    label: "iii".to_string(),
                    buffer_idx: 0,
                },
                TileBufferTab {
                    label: "WWW".to_string(),
                    buffer_idx: 1,
                },
            ],
            Some(0),
        );

        assert_eq!(layouts.len(), 2);
        assert!(
            layouts[1].rect.width > layouts[0].rect.width,
            "wide glyph labels should reserve more tab width than narrow glyph labels"
        );
    }

    #[test]
    fn tab_width_estimate_keeps_matrix_label_untruncated() {
        let rect = Rect {
            row: 10.0,
            col: 0.0,
            width: 80.0,
            height: 20.0,
        };
        let layouts = tile_tab_layouts(
            rect,
            &[
                TileBufferTab {
                    label: "Seq".to_string(),
                    buffer_idx: 0,
                },
                TileBufferTab {
                    label: "Matrix".to_string(),
                    buffer_idx: 1,
                },
            ],
            Some(0),
        );

        assert_eq!(layouts.len(), 2);
        assert_eq!(layouts[1].label, "Matrix");
    }
}

// ── Tree operations ──────────────────────────────────────────────────────

impl TileNode {
    /// Compute the screen rect for each leaf tile given the total available area.
    pub fn compute_rects(
        &self,
        area: Rect,
        gap_px_per_unit: f32,
        cell_w: f32,
        cell_h: f32,
    ) -> Vec<(TileId, Rect)> {
        match self {
            TileNode::Leaf(leaf) => vec![(leaf.id, area)],
            TileNode::Split(s) => {
                let (a_rect, b_rect) = split_rect(
                    area,
                    s.dir,
                    s.ratio,
                    gap_to_cells(s.dir, s.gap, gap_px_per_unit, cell_w, cell_h),
                );
                let mut r = s.a.compute_rects(a_rect, gap_px_per_unit, cell_w, cell_h);
                r.extend(s.b.compute_rects(b_rect, gap_px_per_unit, cell_w, cell_h));
                r
            }
        }
    }

    /// Compute the minimum width of this subtree in cells.
    pub fn min_width(&self) -> f32 {
        match self {
            TileNode::Leaf(leaf) => leaf.min_width.unwrap_or(0.0),
            TileNode::Split(s) => match s.dir {
                SplitDir::Vertical => s.a.min_width() + s.b.min_width(),
                SplitDir::Horizontal => s.a.min_width().max(s.b.min_width()),
            },
        }
    }

    /// Compute the minimum height of this subtree in cells.
    pub fn min_height(&self) -> f32 {
        match self {
            TileNode::Leaf(leaf) => leaf.min_height.unwrap_or(0.0),
            TileNode::Split(s) => match s.dir {
                SplitDir::Horizontal => s.a.min_height() + s.b.min_height(),
                SplitDir::Vertical => s.a.min_height().max(s.b.min_height()),
            },
        }
    }

    /// Compute the maximum width of this subtree in cells.
    pub fn max_width(&self) -> f32 {
        match self {
            TileNode::Leaf(leaf) => leaf.max_width.unwrap_or(f32::MAX),
            TileNode::Split(s) => match s.dir {
                SplitDir::Vertical => s.a.max_width() + s.b.max_width(),
                SplitDir::Horizontal => s.a.max_width().min(s.b.max_width()),
            },
        }
    }

    /// Compute the maximum height of this subtree in cells.
    pub fn max_height(&self) -> f32 {
        match self {
            TileNode::Leaf(leaf) => leaf.max_height.unwrap_or(f32::MAX),
            TileNode::Split(s) => match s.dir {
                SplitDir::Horizontal => s.a.max_height() + s.b.max_height(),
                SplitDir::Vertical => s.a.max_height().min(s.b.max_height()),
            },
        }
    }

    /// Find a leaf by the buffer index it is currently showing.
    pub fn find_leaf_by_buffer_idx_mut(&mut self, buffer_idx: usize) -> Option<&mut TileLeaf> {
        match self {
            TileNode::Leaf(leaf) if leaf.buffer_idx == buffer_idx => Some(leaf),
            TileNode::Leaf(_) => None,
            TileNode::Split(s) => {
                s.a.find_leaf_by_buffer_idx_mut(buffer_idx)
                    .or_else(|| s.b.find_leaf_by_buffer_idx_mut(buffer_idx))
            }
        }
    }

    /// Find a leaf by the buffer index it is currently showing.
    pub fn find_leaf_by_buffer_idx(&self, buffer_idx: usize) -> Option<&TileLeaf> {
        match self {
            TileNode::Leaf(leaf) if leaf.buffer_idx == buffer_idx => Some(leaf),
            TileNode::Leaf(_) => None,
            TileNode::Split(s) => {
                s.a.find_leaf_by_buffer_idx(buffer_idx)
                    .or_else(|| s.b.find_leaf_by_buffer_idx(buffer_idx))
            }
        }
    }

    /// Find a leaf by tile ID and return a mutable reference.
    pub fn find_leaf_mut(&mut self, id: TileId) -> Option<&mut TileLeaf> {
        match self {
            TileNode::Leaf(leaf) if leaf.id == id => Some(leaf),
            TileNode::Leaf(_) => None,
            TileNode::Split(s) => s.a.find_leaf_mut(id).or_else(|| s.b.find_leaf_mut(id)),
        }
    }

    /// Find a leaf by tile ID and return an immutable reference.
    pub fn find_leaf(&self, id: TileId) -> Option<&TileLeaf> {
        match self {
            TileNode::Leaf(leaf) if leaf.id == id => Some(leaf),
            TileNode::Leaf(_) => None,
            TileNode::Split(s) => s.a.find_leaf(id).or_else(|| s.b.find_leaf(id)),
        }
    }

    /// Collect all leaf tile IDs in order (left-to-right / top-to-bottom).
    pub fn leaf_ids(&self) -> Vec<TileId> {
        match self {
            TileNode::Leaf(leaf) => vec![leaf.id],
            TileNode::Split(s) => {
                let mut ids = s.a.leaf_ids();
                ids.extend(s.b.leaf_ids());
                ids
            }
        }
    }

    /// Clear `focused_widget_id` on every leaf except the one with `keep_id`.
    pub fn clear_focus_except(&mut self, keep_id: TileId) {
        match self {
            TileNode::Leaf(leaf) => {
                if leaf.id != keep_id {
                    leaf.focused_widget_id = None;
                    leaf.focused_widget_node = None;
                }
            }
            TileNode::Split(s) => {
                s.a.clear_focus_except(keep_id);
                s.b.clear_focus_except(keep_id);
            }
        }
    }

    /// Count total leaves.
    pub fn leaf_count(&self) -> usize {
        match self {
            TileNode::Leaf(_) => 1,
            TileNode::Split(s) => s.a.leaf_count() + s.b.leaf_count(),
        }
    }

    /// Find the parent split of a given tile ID and which side it's on.
    /// Returns None if id is the root.
    pub fn find_parent_split(&mut self, id: TileId) -> Option<&mut TileSplit> {
        match self {
            TileNode::Leaf(_) => None,
            TileNode::Split(s) => {
                let a_match = match s.a.as_ref() {
                    TileNode::Leaf(leaf) => leaf.id == id,
                    TileNode::Split(child) => child.id == id,
                };
                let b_match = match s.b.as_ref() {
                    TileNode::Leaf(leaf) => leaf.id == id,
                    TileNode::Split(child) => child.id == id,
                };
                if a_match || b_match {
                    Some(s)
                } else {
                    s.a.find_parent_split(id)
                        .or_else(|| s.b.find_parent_split(id))
                }
            }
        }
    }

    pub fn find_split_mut(&mut self, id: TileId) -> Option<&mut TileSplit> {
        match self {
            TileNode::Leaf(_) => None,
            TileNode::Split(s) => {
                if s.id == id {
                    return Some(s);
                }
                if let Some(split) = s.a.find_split_mut(id) {
                    return Some(split);
                }
                s.b.find_split_mut(id)
            }
        }
    }

    pub fn hit_test_split_divider(
        &self,
        area: Rect,
        col: f32,
        row: f32,
        tolerance: f32,
        gap_px_per_unit: f32,
        cell_w: f32,
        cell_h: f32,
    ) -> Option<SplitDividerHit> {
        match self {
            TileNode::Leaf(_) => None,
            TileNode::Split(s) => {
                let (a_rect, b_rect) = split_rect(
                    area,
                    s.dir,
                    s.ratio,
                    gap_to_cells(s.dir, s.gap, gap_px_per_unit, cell_w, cell_h),
                );
                if let Some(hit) = s.a.hit_test_split_divider(
                    a_rect,
                    col,
                    row,
                    tolerance,
                    gap_px_per_unit,
                    cell_w,
                    cell_h,
                ) {
                    return Some(hit);
                }
                if let Some(hit) = s.b.hit_test_split_divider(
                    b_rect,
                    col,
                    row,
                    tolerance,
                    gap_px_per_unit,
                    cell_w,
                    cell_h,
                ) {
                    return Some(hit);
                }

                let divider_hit = match s.dir {
                    SplitDir::Vertical => {
                        let divider_col = b_rect.col;
                        (col - divider_col).abs() <= tolerance
                            && row >= area.row
                            && row < area.row + area.height
                    }
                    SplitDir::Horizontal => {
                        let divider_row = b_rect.row;
                        (row - divider_row).abs() <= tolerance
                            && col >= area.col
                            && col < area.col + area.width
                    }
                };

                divider_hit.then_some(SplitDividerHit {
                    split_id: s.id,
                    dir: s.dir,
                    area,
                })
            }
        }
    }

    /// Split a leaf tile, creating a new split node. The existing leaf stays
    /// as child `a`, and a new leaf (with `new_tile_id` and `new_buffer_idx`)
    /// becomes child `b`.
    pub fn split_leaf(
        &mut self,
        target_id: TileId,
        split_id: TileId,
        new_tile_id: TileId,
        new_buffer_idx: usize,
        dir: SplitDir,
    ) -> bool {
        match self {
            TileNode::Leaf(leaf) if leaf.id == target_id => {
                let existing_leaf = std::mem::replace(
                    leaf,
                    TileLeaf::new(0, 0), // placeholder, will be overwritten
                );
                let new_leaf = TileLeaf::new(new_tile_id, new_buffer_idx);
                *self = TileNode::Split(TileSplit {
                    id: split_id,
                    dir,
                    ratio: 0.5,
                    gap: 0.0,
                    a: Box::new(TileNode::Leaf(existing_leaf)),
                    b: Box::new(TileNode::Leaf(new_leaf)),
                });
                true
            }
            TileNode::Leaf(_) => false,
            TileNode::Split(s) => {
                s.a.split_leaf(target_id, split_id, new_tile_id, new_buffer_idx, dir)
                    || s.b
                        .split_leaf(target_id, split_id, new_tile_id, new_buffer_idx, dir)
            }
        }
    }

    /// Remove a leaf by ID, collapsing its parent split. Returns the removed
    /// leaf's buffer_idx if successful.
    pub fn remove_leaf(&mut self, id: TileId) -> Option<usize> {
        match self {
            TileNode::Leaf(_) => None, // can't remove root leaf
            TileNode::Split(s) => {
                // Check if either direct child is the target leaf
                let a_is_target = matches!(s.a.as_ref(), TileNode::Leaf(l) if l.id == id);
                let b_is_target = matches!(s.b.as_ref(), TileNode::Leaf(l) if l.id == id);

                if a_is_target {
                    let buffer_idx = if let TileNode::Leaf(l) = s.a.as_ref() {
                        l.buffer_idx
                    } else {
                        0
                    };
                    // Replace self with child b
                    let b = std::mem::replace(s.b.as_mut(), TileNode::Leaf(TileLeaf::new(0, 0)));
                    *self = b;
                    Some(buffer_idx)
                } else if b_is_target {
                    let buffer_idx = if let TileNode::Leaf(l) = s.b.as_ref() {
                        l.buffer_idx
                    } else {
                        0
                    };
                    let a = std::mem::replace(s.a.as_mut(), TileNode::Leaf(TileLeaf::new(0, 0)));
                    *self = a;
                    Some(buffer_idx)
                } else {
                    // Recurse
                    s.a.remove_leaf(id).or_else(|| s.b.remove_leaf(id))
                }
            }
        }
    }

    /// Collapse to a single leaf showing the given buffer_idx.
    pub fn collapse_to_single(&mut self, tile_id: TileId, buffer_idx: usize) {
        *self = TileNode::Leaf(TileLeaf::new(tile_id, buffer_idx));
    }
}

/// Divide a rect by direction and ratio.
pub fn split_rect(area: Rect, dir: SplitDir, ratio: f32, gap: f32) -> (Rect, Rect) {
    let ratio = ratio.clamp(0.0, 1.0);
    let gap = gap.max(0.0);
    match dir {
        SplitDir::Horizontal => {
            // Stacked top/bottom — split height
            let effective_gap = gap.min(area.height.max(0.0));
            let content_height = (area.height - effective_gap).max(0.0);
            let a_height = (content_height * ratio).round();
            let b_height = (content_height - a_height).max(0.0);
            (
                Rect {
                    row: area.row,
                    col: area.col,
                    width: area.width,
                    height: a_height,
                },
                Rect {
                    row: area.row + a_height + effective_gap,
                    col: area.col,
                    width: area.width,
                    height: b_height,
                },
            )
        }
        SplitDir::Vertical => {
            // Stacked left/right — split width
            let effective_gap = gap.min(area.width.max(0.0));
            let content_width = (area.width - effective_gap).max(0.0);
            let a_width = (content_width * ratio).round();
            let b_width = (content_width - a_width).max(0.0);
            (
                Rect {
                    row: area.row,
                    col: area.col,
                    width: a_width,
                    height: area.height,
                },
                Rect {
                    row: area.row,
                    col: area.col + a_width + effective_gap,
                    width: b_width,
                    height: area.height,
                },
            )
        }
    }
}

pub fn gap_to_cells(
    dir: SplitDir,
    gap_units: f32,
    px_per_unit: f32,
    cell_w: f32,
    cell_h: f32,
) -> f32 {
    let px = (gap_units * px_per_unit).max(0.0);
    match dir {
        SplitDir::Horizontal => px / cell_h.max(1.0),
        SplitDir::Vertical => px / cell_w.max(1.0),
    }
}

pub fn split_ratio_for_point(area: Rect, dir: SplitDir, col: f32, row: f32) -> f32 {
    match dir {
        SplitDir::Horizontal => ((row - area.row) / area.height.max(1.0)).clamp(0.1, 0.9),
        SplitDir::Vertical => ((col - area.col) / area.width.max(1.0)).clamp(0.1, 0.9),
    }
}
