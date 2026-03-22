use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

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

pub struct TileLeaf {
    pub id: TileId,
    pub buffer_idx: usize,
    pub show_status: bool,
    // Per-tile interaction state (moved from Editor)
    pub focused_widget_id: Option<u64>,
    pub widget_scroll_top: u16,
    pub widget_scroll_left: u16,
    pub active_widget_gesture: Option<WidgetGesture>,
    pub last_widget_click: Option<WidgetClick>,
    pub hit_grid_cache: Option<CachedHitGrid>,
    pub highlight_cache: Option<HighlightCache>,
    // Per-tile layout cache
    pub cached_layout: Option<Arc<LayoutNode>>,
    pub layout_revision: u64,
    /// Cached RenderFrame for inactive tile optimization.
    /// Key is (buffer_revision, layout_revision, scroll_top, view_mode).
    pub cached_inactive_frame: Option<(
        (u64, u64, usize, crate::editor::ViewMode),
        crate::backend::RenderFrame,
    )>,
}

pub struct TileSplit {
    pub id: TileId,
    pub dir: SplitDir,
    pub ratio: f32, // 0.0..1.0, portion for child `a`
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
    pub start_precise_col: f32,
    pub start_precise_row: f32,
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
    pub scroll_top: u16,
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
            show_status: true,
            focused_widget_id: None,
            widget_scroll_top: 0,
            widget_scroll_left: 0,
            active_widget_gesture: None,
            last_widget_click: None,
            hit_grid_cache: None,
            highlight_cache: None,
            cached_layout: None,
            layout_revision: 0,
            cached_inactive_frame: None,
        }
    }
}

// ── Tree operations ──────────────────────────────────────────────────────

impl TileNode {
    /// Compute the screen rect for each leaf tile given the total available area.
    pub fn compute_rects(&self, area: Rect) -> Vec<(TileId, Rect)> {
        match self {
            TileNode::Leaf(leaf) => vec![(leaf.id, area)],
            TileNode::Split(s) => {
                let (a_rect, b_rect) = split_rect(area, s.dir, s.ratio);
                let mut r = s.a.compute_rects(a_rect);
                r.extend(s.b.compute_rects(b_rect));
                r
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
    ) -> Option<SplitDividerHit> {
        match self {
            TileNode::Leaf(_) => None,
            TileNode::Split(s) => {
                let (a_rect, b_rect) = split_rect(area, s.dir, s.ratio);
                if let Some(hit) = s.a.hit_test_split_divider(a_rect, col, row, tolerance) {
                    return Some(hit);
                }
                if let Some(hit) = s.b.hit_test_split_divider(b_rect, col, row, tolerance) {
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
pub fn split_rect(area: Rect, dir: SplitDir, ratio: f32) -> (Rect, Rect) {
    let ratio = ratio.clamp(0.1, 0.9);
    match dir {
        SplitDir::Horizontal => {
            // Stacked top/bottom — split height
            let a_height = (area.height * ratio).round();
            let b_height = (area.height - a_height).max(0.0);
            (
                Rect {
                    row: area.row,
                    col: area.col,
                    width: area.width,
                    height: a_height,
                },
                Rect {
                    row: area.row + a_height,
                    col: area.col,
                    width: area.width,
                    height: b_height,
                },
            )
        }
        SplitDir::Vertical => {
            // Stacked left/right — split width
            let a_width = (area.width * ratio).round();
            let b_width = (area.width - a_width).max(0.0);
            (
                Rect {
                    row: area.row,
                    col: area.col,
                    width: a_width,
                    height: area.height,
                },
                Rect {
                    row: area.row,
                    col: area.col + a_width,
                    width: b_width,
                    height: area.height,
                },
            )
        }
    }
}

pub fn split_ratio_for_point(area: Rect, dir: SplitDir, col: f32, row: f32) -> f32 {
    match dir {
        SplitDir::Horizontal => ((row - area.row) / area.height.max(1.0)).clamp(0.1, 0.9),
        SplitDir::Vertical => ((col - area.col) / area.width.max(1.0)).clamp(0.1, 0.9),
    }
}
