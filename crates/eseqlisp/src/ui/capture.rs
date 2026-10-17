//! Keyed crops of an already laid-out, single-buffer render frame.
//! The renderer still draws the entire frame; these bounds select its pixels.

use crate::backend::RenderFrame;
use crate::layout::{LayoutNode, Rect};
use crate::vm::Value;
use crate::widget_render::{is_overlay_panel_widget, scroll};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

fn intersection(a: Rect, b: Rect) -> Rect {
    let col = a.col.max(b.col);
    let row = a.row.max(b.row);
    Rect { col, row,
        width: ((a.col + a.width).min(b.col + b.width) - col).max(0.0),
        height: ((a.row + a.height).min(b.row + b.height) - row).max(0.0),
    }
}

fn visit<'a>(node: &'a LayoutNode, dx: f32, dy: f32, clip: Rect, overlay: bool,
    visitor: &mut impl FnMut(&'a LayoutNode, Rect, Rect, bool)) {
    let rect = Rect { col: node.rect.col + dx, row: node.rect.row + dy, ..node.rect };
    let overlay = overlay || is_overlay_panel_widget(&node.widget_type);
    visitor(node, rect, clip, overlay);
    let clips = node.widget_type == "scroll"
        || (node.widget_type == "box" && node.props.contains_key("background"));
    let clip = if clips { intersection(clip, rect) } else { clip };
    let dy = if node.widget_type == "scroll" {
        dy - scroll::sync_node_state(node).offset_y
    } else { dy };
    for child in &node.children {
        visit(child, dx, dy, clip, overlay, visitor);
    }
}

fn authored_key(node: &LayoutNode) -> Option<&str> {
    match node.props.get("key") {
        Some(Value::String(key)) => Some(key),
        _ => None,
    }
}

/// Both widget keys and explicit subtree keys are accepted. Namespaced stable
/// keys are listed verbatim; a short authored key must be unique in the buffer.
pub fn layout_keys(root: &LayoutNode) -> Vec<String> {
    let mut keys = Vec::new();
    let mut nodes = vec![root];
    while let Some(node) = nodes.pop() {
        for key in [node.stable_key.as_deref(), authored_key(node)].into_iter().flatten() {
            keys.push(key.to_string());
        }
        nodes.extend(&node.children);
    }
    keys.sort();
    keys.dedup();
    keys
}

/// The layout rectangle (cells, scroll applied) of the one widget keyed
/// `key`, refusing what a crop or a click would get wrong: no match, several,
/// an overlay, no area, or a widget clipped or outside `viewport`.
fn keyed_rect(frame: &RenderFrame, key: &str, viewport: Rect) -> Result<Rect, String> {
    let root = frame.widget_layout.as_ref().ok_or("capture buffer has no widget layout")?;
    let mut matches = Vec::new();
    visit(root, -frame.widget_layout_scroll_left,
        -frame.widget_scroll_top - frame.text_scroll_top as f32, viewport, false,
        &mut |node, rect, clip, overlay| {
            if node.stable_key.as_deref() == Some(key) || authored_key(node) == Some(key) {
                matches.push((rect, clip, overlay));
            }
        });
    let (rect, clip, overlay) = match matches.as_slice() {
        [] => return Err(format!("capture key {key:?} was not found; list the keys with --list-keys or the layout report")),
        [found] => *found,
        _ => return Err(format!("capture key {key:?} matches {} nodes; use a unique stable key from the key list", matches.len())),
    };
    if overlay {
        return Err(format!("capture key {key:?} belongs to an overlay; overlay geometry is not a buffer-layout rectangle"));
    }
    if ![rect.col, rect.row, rect.width, rect.height].iter().all(|value| value.is_finite())
        || rect.width <= 0.0 || rect.height <= 0.0 {
        return Err(format!("capture key {key:?} has no finite, nonzero layout rectangle"));
    }
    // Refuse a misleading partial illustration (or a click that would land
    // on whatever covers the hidden part). Enlarge the viewport or scroll the
    // target into view instead of changing layout.
    let visible = intersection(rect, clip);
    if visible.width + 0.001 < rect.width || visible.height + 0.001 < rect.height {
        return Err(format!("capture key {key:?} is clipped or offscreen; enlarge the buffer or scroll it into view"));
    }
    Ok(rect)
}

pub fn keyed_region(frame: &RenderFrame, key: &str, cell: (f32, f32),
    size: (u32, u32), padding: u32) -> Result<PixelRegion, String> {
    let (cell_w, cell_h) = cell;
    let (width, height) = size;
    if !cell_w.is_finite() || !cell_h.is_finite() || cell_w <= 0.0 || cell_h <= 0.0
        || width == 0 || height == 0 {
        return Err("capture dimensions must be finite and positive".to_string());
    }
    let viewport = Rect { col: 0.0, row: 0.0,
        width: width as f32 / cell_w, height: height as f32 / cell_h };
    let rect = keyed_rect(frame, key, viewport)?;
    let x = (rect.col * cell_w).floor().max(0.0) as u32;
    let y = (rect.row * cell_h).floor().max(0.0) as u32;
    let right = ((rect.col + rect.width) * cell_w).ceil().max(0.0) as u32;
    let bottom = ((rect.row + rect.height) * cell_h).ceil().max(0.0) as u32;
    let x = x.saturating_sub(padding);
    let y = y.saturating_sub(padding);
    Ok(PixelRegion { x, y,
        width: right.saturating_add(padding).min(width) - x,
        height: bottom.saturating_add(padding).min(height) - y,
    })
}

/// The cell under the center of the widget keyed `key` in a frame of
/// `viewport` cells (columns, rows), for a host that clicks it; refused as
/// [`keyed_region`] refuses a crop, so a click never lands on another widget.
pub fn keyed_cell_center(frame: &RenderFrame, key: &str, viewport: (f32, f32)) -> Result<(f32, f32), String> {
    let rect = keyed_rect(frame, key, Rect { col: 0.0, row: 0.0, width: viewport.0, height: viewport.1 })?;
    Ok((rect.col + rect.width * 0.5, rect.row + rect.height * 0.5))
}

/// One keyed node of a laid-out frame, for checking a layout as data rather
/// than by eye (the agent `capture` tool reports these beside the image).
#[derive(Debug, Clone, PartialEq)]
pub struct KeyedNode {
    pub key: String,
    pub widget_type: String,
    /// The node's rectangle in pixels, before clipping; None for overlays,
    /// whose geometry is not a buffer-layout rectangle.
    pub region: Option<PixelRegion>,
    pub problems: Vec<LayoutProblem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutProblem {
    ZeroSize,
    /// Entirely outside the frame or its clipping ancestor.
    Offscreen,
    /// Partly hidden by a scroll, a background box or the frame edge.
    Clipped,
}

impl std::fmt::Display for LayoutProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LayoutProblem::ZeroSize => "zero-size",
            LayoutProblem::Offscreen => "offscreen",
            LayoutProblem::Clipped => "clipped",
        })
    }
}

/// A frame's layout as data: its keyed nodes and its overlapping siblings.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LayoutReport {
    pub nodes: Vec<KeyedNode>,
    /// One line per pair of overlapping siblings in a stack or grid.
    pub overlaps: Vec<String>,
}

/// Every keyed node in the frame, in layout order, with its pixel region and
/// what is wrong with it, and the overlapping siblings in stacks and grids (a
/// flexed `wrap` overlapping its rows is the usual cause).
pub fn layout_report(frame: &RenderFrame, cell: (f32, f32), size: (u32, u32))
    -> Result<LayoutReport, String> {
    let (cell_w, cell_h) = cell;
    let root = frame.widget_layout.as_ref().ok_or("the buffer has no widget layout")?;
    let viewport = Rect { col: 0.0, row: 0.0,
        width: size.0 as f32 / cell_w, height: size.1 as f32 / cell_h };
    let mut nodes = Vec::new();
    visit(root, -frame.widget_layout_scroll_left,
        -frame.widget_scroll_top - frame.text_scroll_top as f32, viewport, false,
        &mut |node, rect, clip, overlay| {
            let Some(key) = node.stable_key.as_deref().or(authored_key(node)) else { return };
            let mut problems = Vec::new();
            let region = (!overlay).then(|| {
                let finite = [rect.col, rect.row, rect.width, rect.height].iter().all(|v| v.is_finite());
                if !finite || rect.width <= 0.0 || rect.height <= 0.0 {
                    problems.push(LayoutProblem::ZeroSize);
                } else {
                    let visible = intersection(rect, clip);
                    if visible.width <= 0.0 || visible.height <= 0.0 {
                        problems.push(LayoutProblem::Offscreen);
                    } else if visible.width + 0.001 < rect.width || visible.height + 0.001 < rect.height {
                        problems.push(LayoutProblem::Clipped);
                    }
                }
                PixelRegion {
                    x: (rect.col * cell_w).max(0.0).floor() as u32,
                    y: (rect.row * cell_h).max(0.0).floor() as u32,
                    width: (rect.width * cell_w).max(0.0).ceil() as u32,
                    height: (rect.height * cell_h).max(0.0).ceil() as u32,
                }
            });
            nodes.push(KeyedNode { key: key.to_string(), widget_type: node.widget_type.clone(), region, problems });
        });
    let mut overlaps = Vec::new();
    sibling_overlaps(root, None, &mut overlaps);
    Ok(LayoutReport { nodes, overlaps })
}

/// Containers that place their children side by side: two children that
/// intersect there is a layout bug, not a design.
const TILING_CONTAINERS: &[&str] = &["v-stack", "h-stack", "wrap", "grid", "responsive-grid", "virtual-v-stack"];

fn sibling_overlaps(node: &LayoutNode, keyed_ancestor: Option<&str>, out: &mut Vec<String>) {
    let here = node.stable_key.as_deref().or(authored_key(node)).or(keyed_ancestor);
    if TILING_CONTAINERS.contains(&node.widget_type.as_str()) {
        let mut children: Vec<(usize, &LayoutNode)> = node.children.iter().enumerate()
            .filter(|(_, child)| !is_overlay_panel_widget(&child.widget_type))
            .collect();
        // Sweep by top edge: once a later sibling starts below `a`'s bottom,
        // none after it can overlap `a`, so a stack of n rows costs O(n log n).
        children.sort_by(|(_, a), (_, b)| a.rect.row.total_cmp(&b.rect.row));
        let name = |n: &LayoutNode, index: usize| match n.stable_key.as_deref().or(authored_key(n)) {
            Some(key) => format!("{} {key:?}", n.widget_type),
            None => format!("{} #{index}", n.widget_type),
        };
        for (at, (i, a)) in children.iter().enumerate() {
            for (j, b) in &children[at + 1..] {
                if b.rect.row >= a.rect.row + a.rect.height {
                    break;
                }
                let both = intersection(a.rect, b.rect);
                if both.width > 0.05 && both.height > 0.05 {
                    out.push(format!("in {} {}: {} overlaps {} by {:.1}x{:.1} cells",
                        node.widget_type,
                        here.map(|k| format!("{k:?}")).unwrap_or_else(|| "(unkeyed)".to_string()),
                        name(a, *i), name(b, *j), both.width, both.height));
                }
            }
        }
    }
    for child in &node.children {
        sibling_overlaps(child, here, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_target_is_the_center_of_its_keyed_widget() {
        let frame = frame(r#"(effect (v-stack (box :height 3)
            (box :key "target" :width 10 :height 4)
            (box :key "same" :width 1 :height 1) (box :key "same" :width 1 :height 1)))"#);
        let (col, row) = keyed_cell_center(&frame, "target", (100.0, 60.0)).unwrap();
        assert_eq!((col, row), (5.0, 5.0));
        assert!(keyed_cell_center(&frame, "missing", (100.0, 60.0)).unwrap_err().contains("not found"));
        assert!(keyed_cell_center(&frame, "same", (100.0, 60.0)).unwrap_err().contains("matches 2"));
        assert!(keyed_cell_center(&frame, "target", (100.0, 6.0)).unwrap_err().contains("clipped or offscreen"));
    }

    #[test]
    fn layout_report_lists_keyed_nodes_with_problems_and_sibling_overlaps() {
        let frame = frame(r#"(effect (v-stack
            (box :key "ok" :width 4 :height 2)
            (box :key "empty" :width 0 :height 0)
            (scroll :width 10 :height 3 (box :key "clipped" :width 10 :height 20))))"#);
        let LayoutReport { nodes, overlaps } = layout_report(&frame, (10.0, 10.0), (1000, 600)).unwrap();
        let problems = |key: &str| nodes.iter().find(|n| n.key == key).map(|n| n.problems.clone()).unwrap();
        assert!(problems("ok").is_empty());
        assert_eq!(problems("empty"), vec![LayoutProblem::ZeroSize]);
        assert_eq!(problems("clipped"), vec![LayoutProblem::Clipped]);
        assert_eq!(nodes.iter().find(|n| n.key == "ok").unwrap().region.unwrap().width, 40);
        assert!(overlaps.is_empty(), "{overlaps:?}");

        let mut node = frame.widget_layout.as_deref().unwrap().clone();
        let first = node.children[0].rect;
        node.children[1].rect = Rect { width: 4.0, height: 2.0, ..first };
        let mut found = Vec::new();
        sibling_overlaps(&node, None, &mut found);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("box \"ok\" overlaps box \"empty\""), "{found:?}");
    }

    fn frame(source: &str) -> RenderFrame {
        let mut editor = crate::Editor::new(crate::Runtime::new(), crate::EditorConfig::default());
        editor.runtime_mut().eval_str(source).unwrap();
        editor.refresh_runtime_side_effects();
        editor.active_buffer_mut().view_mode = crate::editor::ViewMode::UiOnly;
        crate::frame::build_render_frame(&mut editor, 100, 60)
    }

    #[test]
    fn keyed_crop_uses_layout_position_scroll_and_outward_pixel_rounding() {
        let mut frame = frame(r#"(effect (v-stack (box :height 3)
            (box :key "target" :width 10 :height 4)))"#);
        let root = frame.widget_layout.as_ref().unwrap();
        assert!(layout_keys(root).contains(&"target".to_string()));
        let mut found = None;
        visit(root, 0.0, 0.0, root.rect, false, &mut |node, rect, _, _| {
            if authored_key(node) == Some("target") { found = Some(rect); }
        });
        let rect = found.unwrap();
        frame.widget_scroll_top = 0.25;
        let crop = keyed_region(&frame, "target", (7.5, 11.5), (750, 690), 2).unwrap();
        let y = ((rect.row - 0.25) * 11.5).floor() as u32 - 2;
        let bottom = ((rect.row + rect.height - 0.25) * 11.5).ceil() as u32 + 2;
        assert_eq!(crop.y, y);
        assert_eq!(crop.height, bottom - y);
        assert_eq!(crop.width, 77);
        assert!(keyed_region(&frame, "missing", (7.5, 11.5), (750, 690), 0).unwrap_err().contains("not found"));
    }

    #[test]
    fn keyed_crop_rejects_ambiguous_empty_and_clipped_targets() {
        let frame = frame(r#"(effect (v-stack
            (box :key "same" :width 4 :height 2)
            (box :key "same" :width 4 :height 2)
            (box :key "empty" :width 0 :height 0)
            (scroll :width 10 :height 3 (box :key "clipped" :width 10 :height 20))))"#);
        for (key, error) in [("same", "matches 2"), ("empty", "nonzero"), ("clipped", "clipped")] {
            assert!(keyed_region(&frame, key, (10.0, 10.0), (1000, 600), 0)
                .unwrap_err().contains(error), "{key}");
        }
    }

    #[test]
    fn keyed_crop_tracks_nested_scroll_offsets() {
        let frame = frame(r#"(effect (scroll :key "scroll" :width 20 :height 12
            (v-stack (box :height 5) (box :key "target" :width 10 :height 3)
                (box :height 20))))"#);
        let root = frame.widget_layout.as_ref().unwrap();
        let mut scroll_node = None;
        visit(root, 0.0, 0.0, root.rect, false, &mut |node, _, _, _| {
            if node.widget_type == "scroll" { scroll_node = Some(node); }
        });
        let node = scroll_node.unwrap_or_else(|| panic!("scroll missing: {root:#?}"));
        let mut state = scroll::sync_node_state(node);
        let before = keyed_region(&frame, "target", (10.0, 10.0), (1000, 600), 0).unwrap();
        state.offset_y = 4.0;
        scroll::set_scroll_state(scroll::scroll_state_key(node), state);
        let after = keyed_region(&frame, "target", (10.0, 10.0), (1000, 600), 0).unwrap();
        assert_eq!(before.y - after.y, 40);
        assert_eq!(before.height, after.height);
    }
}
