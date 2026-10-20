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

impl std::fmt::Display for PixelRegion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{} {}x{}", self.x, self.y, self.width, self.height)
    }
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

/// [`visit`] over a frame's layout, its scroll applied.
fn visit_frame<'a>(frame: &RenderFrame, root: &'a LayoutNode, viewport: Rect,
    visitor: &mut impl FnMut(&'a LayoutNode, Rect, Rect, bool)) {
    visit(root, -frame.widget_layout_scroll_left,
        -frame.widget_scroll_top - frame.text_scroll_top as f32, viewport, false, visitor);
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
    visit_frame(frame, root, viewport,
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
    /// What a package can hook to restyle what is on screen.
    pub hooks: HookOverview,
}

/// Every keyed node in the frame, in layout order, with its pixel region and
/// what is wrong with it, and the overlapping siblings in stacks and grids (a
/// flexed `wrap` overlapping its rows is the usual cause).
pub fn layout_report(frame: &RenderFrame, cell: (f32, f32), size: (u32, u32))
    -> Result<LayoutReport, String> {
    let (root, viewport) = frame_root(frame, cell, size)?;
    let mut nodes = Vec::new();
    visit_frame(frame, root, viewport,
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
                PixelRegion::from_rect(rect, cell)
            });
            nodes.push(KeyedNode { key: key.to_string(), widget_type: node.widget_type.clone(), region, problems });
        });
    let mut overlaps = Vec::new();
    sibling_overlaps(root, None, &mut overlaps);
    let hooks = hook_overview(frame, cell, size)?;
    Ok(LayoutReport { nodes, overlaps, hooks })
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

/// What a package can hook to restyle one widget, from least to most
/// fragile (customize-reference "Finding what draws it"): a theme slot it
/// paints with, its `:part`, the function that built it.
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetHooks {
    pub widget_type: String,
    pub key: Option<String>,
    pub region: PixelRegion,
    /// `(restyle "<part>" …)`.
    pub part: Option<String>,
    /// The function whose body built it: `(override <fn> :around …)`.
    pub built_by: Option<String>,
    /// Its `:background` widget.
    pub background: Option<String>,
    /// The built-in face (content/ui/widgets.lisp) it paints with.
    pub face: Option<String>,
    /// The theme slots `override-theme-slot` repaints it through, each with
    /// what reads it: a box's background prop, or its face.
    pub slots: Vec<(String, String)>,
}

impl WidgetHooks {
    fn of(node: &LayoutNode, region: PixelRegion) -> Self {
        let string = |prop: &str| match node.props.get(prop) {
            Some(Value::String(text) | Value::Keyword(text)) if !text.is_empty() => Some(text.clone()),
            _ => None,
        };
        // A slot's widget paints only a box's background and the shaders
        // that read the slot (a control's face), so only those are hooks.
        let mut reads: Vec<(String, String)> = Vec::new();
        if node.widget_type == "box" {
            for prop in BOX_BACKGROUND_PROPS {
                if let Some(Value::Keyword(name)) = node.props.get(prop)
                    && let Some(slot) = slot_name(name)
                {
                    reads.push((slot, format!(":{prop}")));
                }
            }
        }
        let face = crate::widget_render::builtin_face_of(node)
            .filter(|(face, _)| crate::widget_render::sdf_widget::sdf_widget_def(face).is_some());
        if let Some((face, slots)) = &face {
            reads.extend(slots.iter().map(|slot| (slot.to_string(), format!("its face {face}"))));
        }
        reads.sort();
        // One line per slot, naming everything that reads it.
        let mut slots: Vec<(String, String)> = Vec::new();
        for (slot, read_by) in reads {
            match slots.last_mut() {
                Some((last, readers)) if *last == slot => {
                    readers.push_str(", ");
                    readers.push_str(&read_by);
                }
                _ => slots.push((slot, read_by)),
            }
        }
        Self {
            widget_type: node.widget_type.clone(),
            key: node.stable_key.as_deref().or(authored_key(node)).map(str::to_string),
            region,
            part: string("part"),
            built_by: source_symbol(node).map(str::to_string),
            background: string("background"),
            face: face.map(|(face, _)| face.to_string()),
            slots,
        }
    }
}

/// The props a box paints its background from (box_widget `state_color_prop`).
const BOX_BACKGROUND_PROPS: [&str; 4] = [
    "background-color", "selected-background-color", "muted-background-color",
    "drop-hover-background-color",
];

/// The theme slot `name` names, as its canonical field (`primary` is
/// `accent`), or None for a colour that is no slot (`none`, `transparent`).
fn slot_name(name: &str) -> Option<String> {
    let (_, slots) = crate::theme::record_slot_reads(|| crate::theme::named_color(name));
    slots.into_iter().next()
}

/// The function whose body built `node`, module-qualified.
fn source_symbol(node: &LayoutNode) -> Option<&str> {
    match node.props.get(crate::vm::SOURCE_SYMBOL_PROP) {
        Some(Value::String(symbol)) if !symbol.is_empty() => Some(symbol),
        _ => None,
    }
}

impl PixelRegion {
    /// The pixels a layout rectangle (cells) covers at `cell` pixels a cell:
    /// its origin rounded down, its size up.
    pub fn from_rect(rect: Rect, cell: (f32, f32)) -> Self {
        PixelRegion {
            x: (rect.col * cell.0).max(0.0).floor() as u32,
            y: (rect.row * cell.1).max(0.0).floor() as u32,
            width: (rect.width * cell.0).max(0.0).ceil() as u32,
            height: (rect.height * cell.1).max(0.0).ceil() as u32,
        }
    }
}

fn frame_root<'a>(frame: &'a RenderFrame, cell: (f32, f32), size: (u32, u32))
    -> Result<(&'a LayoutNode, Rect), String> {
    let root = frame.widget_layout.as_ref().ok_or("the buffer has no widget layout")?;
    let viewport = Rect { col: 0.0, row: 0.0,
        width: size.0 as f32 / cell.0, height: size.1 as f32 / cell.1 };
    Ok((root, viewport))
}

/// The widgets under pixel `(x, y)`, the topmost drawn first (the innermost
/// of a nested chain), with their hooks. Overlays (menus, modals) are not
/// looked into.
pub fn hooks_at(frame: &RenderFrame, cell: (f32, f32), size: (u32, u32), (x, y): (f32, f32))
    -> Result<Vec<WidgetHooks>, String> {
    let (root, viewport) = frame_root(frame, cell, size)?;
    let (col, row) = (x / cell.0, y / cell.1);
    let mut stack = Vec::new();
    visit_frame(frame, root, viewport,
        &mut |node, rect, clip, overlay| {
            let visible = intersection(rect, clip);
            if !overlay && col >= visible.col && col < visible.col + visible.width
                && row >= visible.row && row < visible.row + visible.height
            {
                stack.push(WidgetHooks::of(node, PixelRegion::from_rect(rect, cell)));
            }
        });
    stack.reverse();
    Ok(stack)
}

/// One hook and where it shows: how many widgets, and the first one's place.
#[derive(Debug, Clone, PartialEq)]
pub struct HookUse {
    pub name: String,
    pub count: usize,
    pub first: PixelRegion,
}

/// A buffer's hooks: its parts, the functions that build its regions (each
/// where its output starts, not every widget inside), and the theme slots
/// it paints with, in layout order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HookOverview {
    pub parts: Vec<HookUse>,
    pub functions: Vec<HookUse>,
    pub slots: Vec<HookUse>,
}

pub fn hook_overview(frame: &RenderFrame, cell: (f32, f32), size: (u32, u32))
    -> Result<HookOverview, String> {
    let (root, viewport) = frame_root(frame, cell, size)?;
    let mut roots = std::collections::HashSet::new();
    function_roots(root, None, &mut roots);
    let mut overview = HookOverview::default();
    let note = |uses: &mut Vec<HookUse>, name: &str, region: PixelRegion| {
        match uses.iter_mut().find(|used| used.name == name) {
            Some(used) => used.count += 1,
            None => uses.push(HookUse { name: name.to_string(), count: 1, first: region }),
        }
    };
    visit_frame(frame, root, viewport,
        &mut |node, rect, clip, overlay| {
            let visible = intersection(rect, clip);
            if overlay || visible.width <= 0.0 || visible.height <= 0.0 {
                return;
            }
            let hooks = WidgetHooks::of(node, PixelRegion::from_rect(rect, cell));
            if let Some(part) = &hooks.part {
                note(&mut overview.parts, part, hooks.region);
            }
            if let Some(built_by) = &hooks.built_by
                && roots.contains(&(node as *const LayoutNode))
            {
                note(&mut overview.functions, built_by, hooks.region);
            }
            for (slot, _) in &hooks.slots {
                note(&mut overview.slots, slot, hooks.region);
            }
        });
    Ok(overview)
}

/// The nodes where a function's output starts: built by a different
/// function than their parent.
fn function_roots(node: &LayoutNode, parent: Option<&str>, out: &mut std::collections::HashSet<*const LayoutNode>) {
    let built_by = source_symbol(node).or(parent);
    if built_by.is_some() && built_by != parent {
        out.insert(node as *const LayoutNode);
    }
    for child in &node.children {
        function_roots(child, built_by, out);
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
        let LayoutReport { nodes, overlaps, .. } = layout_report(&frame, (10.0, 10.0), (1000, 600)).unwrap();
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

    #[test]
    fn hooks_name_the_part_slots_and_building_function_of_what_is_drawn() {
        let frame = frame(r#"(module test.hooks)
            (def plate (text)
              (box :key "plate" :part "test/plate" :width 10 :height 4
                :background-color :accent (label text)))
            ;; The cell is built in a lambda written in `row`: `row` built it.
            (def row ()
              (h-stack (plate "a") (each (list 1) |x| (box :key "cell" :width 3 :height 3))))
            (effect (v-stack (row)))"#);
        let stack = hooks_at(&frame, (10.0, 10.0), (1000, 600), (50.0, 20.0)).unwrap();
        let plate = stack.iter().find(|hooks| hooks.key.as_deref() == Some("test.hooks/plate")).unwrap();
        assert_eq!(plate.part.as_deref(), Some("test/plate"));
        assert_eq!(plate.built_by.as_deref(), Some("test.hooks/plate"));
        assert_eq!(plate.slots, vec![("accent".to_string(), ":background-color".to_string())]);
        assert!(stack.iter().position(|h| h.widget_type == "label")
            < stack.iter().position(|h| h.key.as_deref() == Some("test.hooks/plate")),
            "innermost first: {stack:?}");

        let cell = hooks_at(&frame, (10.0, 10.0), (1000, 600), (115.0, 15.0)).unwrap();
        assert_eq!(cell[0].built_by.as_deref(), Some("test.hooks/row"), "{cell:?}");

        let overview = hook_overview(&frame, (10.0, 10.0), (1000, 600)).unwrap();
        let names = |uses: &[HookUse]| uses.iter().map(|used| used.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&overview.parts), vec!["test/plate"]);
        assert!(names(&overview.slots).contains(&"accent".to_string()));
        let functions = names(&overview.functions);
        assert!(functions.contains(&"test.hooks/row".to_string()), "{functions:?}");
        assert!(functions.contains(&"test.hooks/plate".to_string()), "{functions:?}");
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
