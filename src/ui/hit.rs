use super::layout::LayoutNode;

/// A spatial index over a laid-out widget tree.
///
/// Provides O(1) point queries: given a (row, col) in layout space,
/// return the leaf `LayoutNode` at that position. Rebuilt whenever
/// the layout revision changes.
pub struct HitGrid {
    cols: u16,
    rows: u16,
    cells: Vec<Option<LayoutNode>>,
}

impl HitGrid {
    /// Build a new hit grid from the root layout node.
    /// Uses the max extent of all descendants, not just the root rect,
    /// so overflowing children (wider than viewport) are hittable.
    pub fn build(layout: &LayoutNode) -> Self {
        let (cols, rows) = max_extent(layout);
        let mut cells = vec![None; cols as usize * rows as usize];
        fill_cells(layout, cols, rows, &mut cells);
        HitGrid { cols, rows, cells }
    }

    /// Look up the leaf widget node at a position in layout space (not screen space).
    /// Callers must add scroll offset to row before calling.
    pub fn node_at(&self, row: u16, col: u16) -> Option<&LayoutNode> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        self.cells[row as usize * self.cols as usize + col as usize].as_ref()
    }

    /// Returns true if two optional nodes occupy the same widget hit region.
    pub fn same_hit(a: Option<&LayoutNode>, b: Option<&LayoutNode>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => hit_key(a) == hit_key(b),
            _ => false,
        }
    }
}

/// Convert screen-space coordinates to local widget-area coordinates.
/// Returns `None` if the point is outside the content area (negative offset).
pub fn to_local(
    precise_col: f32,
    precise_row: f32,
    content_col: u16,
    content_row: u16,
) -> Option<(f32, f32)> {
    let local_col = precise_col - content_col as f32;
    let local_row = precise_row - content_row as f32;
    if local_col < 0.0 || local_row < 0.0 {
        return None;
    }
    Some((local_col, local_row))
}

/// Convert local float coordinates to integer grid coordinates
/// suitable for `HitGrid::node_at`.
pub fn to_query(local_col: f32, local_row: f32) -> (u16, u16) {
    (
        local_col.floor() as u16,
        local_row.floor() as u16,
    )
}

// ── Private helpers ──────────────────────────────────────────────────────

fn fill_cells(node: &LayoutNode, cols: u16, rows: u16, cells: &mut [Option<LayoutNode>]) {
    if node.children.is_empty() {
        let r = &node.rect;
        for row in r.row..r.row.saturating_add(r.height).min(rows) {
            for col in r.col..r.col.saturating_add(r.width).min(cols) {
                cells[row as usize * cols as usize + col as usize] = Some(node.clone());
            }
        }
    } else {
        for child in &node.children {
            fill_cells(child, cols, rows, cells);
        }
    }
}

/// Find the max (cols, rows) extent across all descendants.
fn max_extent(node: &LayoutNode) -> (u16, u16) {
    let own_cols = node.rect.col.saturating_add(node.rect.width);
    let own_rows = node.rect.row.saturating_add(node.rect.height);
    node.children.iter().fold((own_cols, own_rows), |(c, r), child| {
        let (cc, cr) = max_extent(child);
        (c.max(cc), r.max(cr))
    })
}

fn hit_key(node: &LayoutNode) -> (String, u16, u16, u16, u16) {
    (
        node.widget_type.clone(),
        node.rect.row,
        node.rect.col,
        node.rect.width,
        node.rect.height,
    )
}
