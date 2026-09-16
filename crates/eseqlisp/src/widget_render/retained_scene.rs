//! Retained, backend-independent widget paint scene. Layout/topology, paint,
//! and presentation have distinct lifetimes: moving a viewport never changes
//! a run's geometry. Clip containers provide conservative subtree bounds, so
//! invisible panels are skipped before calling any widget's paint function.

use super::*;

static PAINT_REVISION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct PreparedRun {
    pub widget_id: u64,
    pub ordinal: u16,
    pub revision: u64,
    pub primitives: Rc<Vec<GpuPrimitive>>,
    /// Translation in layout cells, including nested scroll containers.
    pub translation: [f32; 2],
}

#[derive(Default)]
pub struct PreparedScene {
    pub runs: Vec<PreparedRun>,
    pub overlay: Vec<GpuPrimitive>,
    pub rebuilt_nodes: usize,
    pub reused_nodes: usize,
    pub culled_nodes: usize,
    pub reindexed_nodes: usize,
    pub bounds_refreshed_nodes: usize,
}

impl PreparedScene {
    pub fn flatten(&self, viewport: WidgetViewport) -> Vec<GpuPrimitive> {
        let mut result = Vec::new();
        for run in &self.runs {
            for primitive in run.primitives.iter() {
                let mut primitive = primitive.clone();
                offset_primitive_x_mut(&mut primitive, run.translation[0], viewport);
                offset_primitive_y_mut(&mut primitive, run.translation[1], viewport);
                let mut inner = &mut primitive;
                while let GpuPrimitive::ZLayer { primitive, .. } = inner { inner = primitive; }
                if let GpuPrimitive::WidgetInstance { instance, .. } = inner {
                    instance.itime = viewport.time_seconds;
                }
                result.push(primitive);
            }
        }
        result
    }
}

#[derive(Clone, Copy, PartialEq)]
struct PaintContext {
    epoch: u64,
    state_revision: u64,
    focused: bool,
    focused_branch: bool,
    inherited_hover: bool,
}

struct SceneNode {
    widget_id: u64,
    widget_type: String,
    layout_rect: Rect,
    box_background_rect: Option<Rect>,
    parent: Option<usize>,
    child_offset: usize,
    children: Vec<usize>,
    end: usize,
    bounds: Option<Rect>,
    has_overlay: bool,
    is_scroll: bool,
    dirty: bool,
    context: Option<PaintContext>,
    scroll_state: Option<scroll::ScrollState>,
    head: Rc<Vec<GpuPrimitive>>,
    tail: Rc<Vec<GpuPrimitive>>,
    revision: u64,
}

#[derive(Default)]
pub struct RetainedScene {
    nodes: Vec<SceneNode>,
    by_id: HashMap<u64, usize>,
    layout_revision: u64,
    content_revision: u64,
    layout_identity: usize,
    metrics: Option<[u32; 4]>,
    shared_generation: u64,
    theme_generation: u64,
    shader_generation: u64,
    epoch: u64,
}

impl RetainedScene {
    pub fn prepare(
        &mut self,
        layout: &LayoutNode,
        layout_revision: u64,
        content_revision: u64,
        dirty_ids: &[u64],
        viewport: WidgetViewport,
        visible: Rect,
    ) -> PreparedScene {
        let mut scene = PreparedScene::default();
        let identity = layout as *const LayoutNode as usize;
        let metrics = [viewport.cell_w.to_bits(), viewport.cell_h.to_bits(),
            viewport.vp_w.to_bits(), viewport.vp_h.to_bits()];
        let metrics_changed = self.metrics != Some(metrics);
        let content_changed = self.content_revision != content_revision;
        let shader_generation = sdf_widget::sdf_widget_registry_generation();
        let topology_changed = self.nodes.is_empty() || self.layout_revision != layout_revision
            || self.shader_generation != shader_generation
            || (self.content_revision != content_revision && dirty_ids.is_empty())
            || ((self.layout_identity != identity || content_changed) && !self.same_topology(layout, 0));
        if topology_changed || metrics_changed {
            self.nodes.clear();
            self.by_id.clear();
            self.index(layout, viewport, None, 0);
            scene.reindexed_nodes = self.nodes.len();
        } else if !dirty_ids.is_empty() && (self.layout_identity != identity || content_changed) {
            scene.bounds_refreshed_nodes = self.refresh_dirty_bounds(layout, dirty_ids, viewport);
        }
        self.layout_identity = identity;
        self.layout_revision = layout_revision;
        self.content_revision = content_revision;
        self.metrics = Some(metrics);
        self.shader_generation = shader_generation;
        let shared_generation = widget_state_shared_generation();
        let theme_generation = theme::generation();
        if topology_changed || metrics_changed || self.shared_generation != shared_generation
            || self.theme_generation != theme_generation
        {
            self.epoch = self.epoch.wrapping_add(1);
        }
        self.shared_generation = shared_generation;
        self.theme_generation = theme_generation;
        for id in dirty_ids.iter().copied().chain(active_animation_widget_ids(layout)) {
            if let Some(&index) = self.by_id.get(&id) {
                // The editor includes scroll gestures in dirty IDs even when
                // authored content is unchanged. They move child runs without
                // invalidating their paint. Content updates still dirty the
                // complete subtree, including newly exposed children.
                let end = if self.nodes[index].is_scroll && !content_changed {
                    index + 1
                } else { self.nodes[index].end };
                for node in &mut self.nodes[index..end] { node.dirty = true; }
            }
        }
        self.visit(layout, 0, viewport, visible, [0.0, 0.0],
            any_overlay_active(), &mut scene);
        scene.overlay = drain_overlay_primitives();
        scene
    }

    fn same_topology(&self, layout: &LayoutNode, index: usize) -> bool {
        let Some(node) = self.nodes.get(index) else { return false; };
        node.widget_id == layout.widget_id && node.children.len() == layout.children.len()
            && node.widget_type == layout.widget_type && node.layout_rect == layout.rect
            && node.children.iter().zip(&layout.children)
                .all(|(&index, child)| self.same_topology(child, index))
    }

    fn layout_node_at<'a>(&self, root: &'a LayoutNode, mut index: usize) -> &'a LayoutNode {
        let mut path = Vec::new();
        while let Some(parent) = self.nodes[index].parent {
            path.push(self.nodes[index].child_offset);
            index = parent;
        }
        let mut layout = root;
        for offset in path.into_iter().rev() { layout = &layout.children[offset]; }
        layout
    }

    fn refresh_dirty_bounds(&mut self, root: &LayoutNode, dirty_ids: &[u64], viewport: WidgetViewport) -> usize {
        let mut affected = std::collections::BTreeSet::new();
        for id in dirty_ids {
            if let Some(&index) = self.by_id.get(id) {
                affected.extend(index..self.nodes[index].end);
                let mut parent = self.nodes[index].parent;
                while let Some(index) = parent {
                    affected.insert(index);
                    parent = self.nodes[index].parent;
                }
            }
        }
        // Preorder indices put children after parents. Recompute from leaves
        // upward so each ancestor sees its children's final paint bounds.
        for &index in affected.iter().rev() {
            let layout = self.layout_node_at(root, index);
            let node = &self.nodes[index];
            let bounds = conservative_bounds(layout, &node.children, &self.nodes, viewport, node.box_background_rect);
            let has_overlay = is_overlay_panel_widget(&layout.widget_type)
                || node.children.iter().any(|&child| self.nodes[child].has_overlay);
            self.nodes[index].bounds = bounds;
            self.nodes[index].has_overlay = has_overlay;
            self.nodes[index].is_scroll = layout.widget_type == "scroll";
        }
        affected.len()
    }

    fn index(&mut self, layout: &LayoutNode, viewport: WidgetViewport, parent: Option<usize>, child_offset: usize) -> usize {
        let index = self.nodes.len();
        let box_background_rect = (layout.widget_type == "box").then(|| box_widget::background_rect(layout));
        self.by_id.insert(layout.widget_id, index);
        self.nodes.push(SceneNode {
            widget_id: layout.widget_id, parent, child_offset, children: Vec::new(), end: 0,
            widget_type: layout.widget_type.clone(), layout_rect: layout.rect, box_background_rect,
            bounds: None, has_overlay: is_overlay_panel_widget(&layout.widget_type),
            is_scroll: layout.widget_type == "scroll",
            dirty: true, context: None, scroll_state: None, head: Rc::new(Vec::new()),
            tail: Rc::new(Vec::new()), revision: 0,
        });
        let children: Vec<_> = layout.children.iter().enumerate().map(|(offset, child)|
            self.index(child, viewport, Some(index), offset)).collect();
        let has_overlay = self.nodes[index].has_overlay
            || children.iter().any(|&child| self.nodes[child].has_overlay);
        let bounds = conservative_bounds(layout, &children, &self.nodes, viewport, box_background_rect);
        let end = self.nodes.len();
        self.nodes[index].children = children;
        self.nodes[index].end = end;
        self.nodes[index].bounds = bounds;
        self.nodes[index].has_overlay = has_overlay;
        index
    }

    fn visit(
        &mut self, layout: &LayoutNode, index: usize, viewport: WidgetViewport,
        visible: Rect, translation: [f32; 2], overlays_active: bool,
        scene: &mut PreparedScene,
    ) {
        let node = &self.nodes[index];
        if !overlays_active && !node.has_overlay
            && ((finite_bounds(visible) && (visible.width == 0.0 || visible.height == 0.0))
                || node.bounds.is_some_and(|bounds| !intersects(bounds, visible)))
        {
            scene.culled_nodes += node.end - index;
            return;
        }
        let focused = layout.focusable && viewport.focused_widget_id == Some(layout.widget_id);
        let viewport = WidgetViewport {
            focused_branch: viewport.focused_branch || focused, ..viewport
        };
        if is_overlay_panel_widget(&layout.widget_type) {
            collect_modal_overlay(layout, viewport, viewport.scroll_top,
                (viewport.vp_h / viewport.cell_h).ceil() as u16);
            return;
        }

        // Synchronize scroll state even on a paint-cache hit. Follow/centering
        // may change at render time; it is presentation state, not child paint.
        let scroll_state = (layout.widget_type == "scroll").then(|| scroll::sync_node_state(layout));
        let context = PaintContext {
            epoch: self.epoch, state_revision: widget_state_revision(layout.widget_id),
            focused, focused_branch: viewport.focused_branch,
            inherited_hover: viewport.inherited_hover,
        };
        if node.dirty || node.context != Some(context) || overlays_active || node.scroll_state != scroll_state {
            let mut head = Vec::new();
            let mut tail = Vec::new();
            if focused && is_layout_widget_type(&layout.widget_type)
                && !suppresses_default_focus(layout)
            {
                head.push(GpuPrimitive::Rect(GpuRectPrimitive {
                    rect: layout.rect, color: theme::WIDGET_FOCUS_BG(),
                }));
            }
            if scroll_state.is_some() {
                head.push(GpuPrimitive::PushClipRect(layout.rect));
                tail.push(GpuPrimitive::PopClipRect);
                tail.extend(widget_primitives_for_node(layout, viewport));
            } else {
                head.extend(widget_primitives_for_node(layout, viewport));
                if layout.widget_type == "box" && layout.props.contains_key("background") {
                    head.push(GpuPrimitive::PushClipRect(layout.rect));
                    tail.push(GpuPrimitive::PopClipRect);
                }
            }
            let node = &mut self.nodes[index];
            node.head = Rc::new(head);
            node.tail = Rc::new(tail);
            node.revision = PAINT_REVISION.fetch_add(1, Ordering::Relaxed);
            node.context = Some(context);
            node.scroll_state = scroll_state.clone();
            node.dirty = false;
            scene.rebuilt_nodes += 1;
        } else {
            scene.reused_nodes += 1;
        }
        self.push_run(index, 0, translation, scene);
        let mut child_translation = translation;
        let mut child_visible = visible;
        let mut child_viewport = viewport;
        if let Some(state) = scroll_state {
            child_visible = intersection(visible, layout.rect);
            child_visible.row += state.offset_y;
            child_translation[1] -= state.offset_y;
            // Overlay anchors are expressed in screen coordinates by painters.
            child_viewport.scroll_top += state.offset_y;
        } else if layout.widget_type == "box" && layout.props.contains_key("background") {
            child_visible = intersection(visible, layout.rect);
            child_viewport.inherited_hover |=
                sdf_widget::get_sdf_hit_state(layout.widget_id).hit_region >= 0;
        }
        for (child_offset, child) in layout.children.iter().enumerate() {
            let child_index = self.nodes[index].children[child_offset];
            self.visit(child, child_index, child_viewport, child_visible,
                child_translation, overlays_active, scene);
        }
        self.push_run(index, 1, translation, scene);
    }

    fn push_run(&self, index: usize, ordinal: u16, translation: [f32; 2], scene: &mut PreparedScene) {
        let node = &self.nodes[index];
        let primitives = if ordinal == 0 { &node.head } else { &node.tail };
        if !primitives.is_empty() {
            scene.runs.push(PreparedRun {
                widget_id: node.widget_id, ordinal, revision: node.revision,
                primitives: Rc::clone(primitives), translation,
            });
        }
    }
}

/// Unknown painters are deliberately unbounded. A parent's measured rect is
/// not a paint bound: children and text can overflow it. Clip containers are
/// different: they bound their children irrespective of those children's size.
fn conservative_bounds(layout: &LayoutNode, children: &[usize], nodes: &[SceneNode], viewport: WidgetViewport, box_background_rect: Option<Rect>) -> Option<Rect> {
    if !finite_bounds(layout.rect) { return None; }
    if layout.widget_type == "scroll" { return Some(layout.rect); }
    if let Some(definition) = sdf_widget::sdf_widget_def(&layout.widget_type) {
        let mut bounds = sdf_widget::visual_style_paint_bounds(layout.widget_id, &layout.props,
            sdf_widget::sdf_widget_paint_rect(layout.rect, definition.paint_margin))?;
        for &child in children { bounds = union(bounds, nodes[child].bounds?); }
        return Some(bounds);
    }
    if layout.widget_type == "box" && layout.props.contains_key("background") {
        return box_widget::paint_bounds(layout, viewport, box_background_rect?);
    }
    if matches!(layout.widget_type.as_str(), "h-stack" | "hstack" | "v-stack" | "vstack"
        | "grid" | "wrap" | "virtual-v-stack")
    {
        let mut bounds = layout.rect;
        for &child in children { bounds = union(bounds, nodes[child].bounds?); }
        return Some(bounds);
    }
    None
}

fn intersects(a: Rect, b: Rect) -> bool {
    // Non-finite bounds must never suppress drawing.
    if [a.col, a.row, a.width, a.height, b.col, b.row, b.width, b.height]
        .iter().any(|v| !v.is_finite()) { return true; }
    a.col + a.width > b.col && a.col < b.col + b.width
        && a.row + a.height > b.row && a.row < b.row + b.height
}

fn finite_bounds(rect: Rect) -> bool {
    [rect.col, rect.row, rect.width, rect.height, rect.col + rect.width, rect.row + rect.height]
        .iter().all(|v| v.is_finite()) && rect.width >= 0.0 && rect.height >= 0.0
}

fn union(a: Rect, b: Rect) -> Rect {
    if !finite_bounds(a) { return a; }
    if !finite_bounds(b) { return b; }
    let col = a.col.min(b.col);
    let row = a.row.min(b.row);
    Rect { col, row, width: (a.col + a.width).max(b.col + b.width) - col,
        height: (a.row + a.height).max(b.row + b.height) - row }
}

fn intersection(a: Rect, b: Rect) -> Rect {
    if !finite_bounds(a) { return b; }
    if !finite_bounds(b) { return a; }
    let col = a.col.max(b.col);
    let row = a.row.max(b.row);
    Rect { col, row, width: ((a.col + a.width).min(b.col + b.width) - col).max(0.0),
        height: ((a.row + a.height).min(b.row + b.height) - row).max(0.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u64, kind: &str, rect: Rect, children: Vec<LayoutNode>) -> LayoutNode {
        LayoutNode { widget_id: id, stable_widget_id: None, subtree_root_id: None,
            parent_subtree_root_id: None, stable_key: None, widget_type: kind.into(), rect,
            props: HashMap::new(), children, focusable: false, animation: Default::default() }
    }

    fn rect(col: f32, row: f32, width: f32, height: f32) -> Rect {
        Rect { col, row, width, height }
    }

    fn viewport() -> WidgetViewport {
        WidgetViewport { cell_w: 10.0, cell_h: 20.0, vp_w: 400.0, vp_h: 400.0,
            time_seconds: 0.0, focused_widget_id: None, focused_branch: false,
            overlay_viewport_bottom: 20.0, scroll_top: 0.0, scroll_left: 0.0,
            inherited_hover: false }
    }

    fn panel(id: u64, col: f32) -> LayoutNode {
        let mut label = node(id + 1, "label", rect(col + 1.0, 1.0, 8.0, 1.0), vec![]);
        label.props.insert("text".into(), Value::String("control".into()));
        let mut panel = node(id, "box", rect(col, 0.0, 20.0, 10.0), vec![label]);
        panel.props.insert("background".into(), Value::String("unregistered-background".into()));
        panel
    }

    #[test]
    fn scroll_culls_panels_before_paint_and_reuses_resident_geometry() {
        let root = node(80001, "hstack", rect(0.0, 0.0, 400.0, 10.0),
            vec![panel(80010, 0.0), panel(80020, 100.0)]);
        let mut cache = RetainedScene::default();
        let first = cache.prepare(&root, 1, 1, &[], viewport(), rect(0.0, 0.0, 40.0, 20.0));
        assert_eq!(first.culled_nodes, 2);
        assert!(!first.runs.iter().any(|run| run.widget_id == 80021));
        let first_label = first.runs.iter().find(|run| run.widget_id == 80011).unwrap();
        let mut vp = viewport();
        vp.scroll_left = 2.0;
        let moved = cache.prepare(&root, 1, 1, &[], vp, rect(2.0, 0.0, 40.0, 20.0));
        assert_eq!(moved.rebuilt_nodes, 0);
        let moved_label = moved.runs.iter().find(|run| run.widget_id == 80011).unwrap();
        assert!(Rc::ptr_eq(&first_label.primitives, &moved_label.primitives));
        let entered = cache.prepare(&root, 1, 1, &[], vp, rect(100.0, 0.0, 40.0, 20.0));
        assert!(entered.runs.iter().any(|run| run.widget_id == 80021));
    }

    #[test]
    fn offscreen_dirty_content_is_refreshed_when_it_enters() {
        let mut root = node(80101, "hstack", rect(0.0, 0.0, 400.0, 10.0),
            vec![panel(80110, 0.0), panel(80120, 100.0)]);
        let mut cache = RetainedScene::default();
        cache.prepare(&root, 1, 1, &[], viewport(), rect(100.0, 0.0, 40.0, 20.0));
        root.children[1].children[0].props.insert("text".into(), Value::String("changed".into()));
        cache.prepare(&root, 1, 2, &[80121], viewport(), rect(0.0, 0.0, 40.0, 20.0));
        let entered = cache.prepare(&root, 1, 2, &[], viewport(), rect(100.0, 0.0, 40.0, 20.0));
        assert!(entered.flatten(viewport()).iter().any(|p|
            matches!(p, GpuPrimitive::ProportionalText(text) if text.text == "changed")
            || matches!(p, GpuPrimitive::GlyphRun(text) if text.text == "changed")));
    }

    #[test]
    fn ordinary_parent_does_not_cull_overflowing_children() {
        let root = node(80201, "hstack", rect(-100.0, 0.0, 10.0, 10.0),
            vec![panel(80210, 0.0)]);
        let frame = RetainedScene::default().prepare(&root, 1, 1, &[], viewport(),
            rect(0.0, 0.0, 40.0, 20.0));
        assert!(frame.runs.iter().any(|run| run.widget_id == 80211));
    }

    #[test]
    fn widget_revision_does_not_repaint_an_unrelated_scene() {
        let root = panel(80310, 0.0);
        let mut cache = RetainedScene::default();
        cache.prepare(&root, 1, 1, &[], viewport(), rect(0.0, 0.0, 40.0, 20.0));
        bump_widget_state_revision(80399);
        let next = cache.prepare(&root, 1, 1, &[], viewport(), rect(0.0, 0.0, 40.0, 20.0));
        assert_eq!(next.rebuilt_nodes, 0);
        bump_widget_state_revision(80311);
        let next = cache.prepare(&root, 1, 1, &[], viewport(), rect(0.0, 0.0, 40.0, 20.0));
        assert_eq!(next.rebuilt_nodes, 1);
    }

    #[test]
    fn nested_scroll_moves_runs_without_changing_child_geometry() {
        let mut root = node(80401, "scroll", rect(0.0, 0.0, 40.0, 10.0), vec![panel(80410, 0.0)]);
        root.props.insert("_content_height".into(), Value::Number(100.0));
        let mut cache = RetainedScene::default();
        let first = cache.prepare(&root, 1, 1, &[], viewport(), rect(0.0, 0.0, 40.0, 20.0));
        let first_label = first.runs.iter().find(|run| run.widget_id == 80411).unwrap();
        let mut state = scroll::get_scroll_state(80401);
        state.offset_y = 2.0;
        scroll::set_scroll_state(80401, state);
        let next = cache.prepare(&root, 1, 1, &[80401], viewport(), rect(0.0, 0.0, 40.0, 20.0));
        let next_label = next.runs.iter().find(|run| run.widget_id == 80411).unwrap();
        assert!(Rc::ptr_eq(&first_label.primitives, &next_label.primitives));
        assert_eq!(next_label.translation, [0.0, -2.0]);
    }
    #[test]
    fn nested_clips_cull_in_their_own_content_coordinates() {
        fn move_down(node: &mut LayoutNode, rows: f32) {
            node.rect.row += rows;
            for child in &mut node.children { move_down(child, rows); }
        }
        let mut lower = panel(80520, 0.0);
        move_down(&mut lower, 20.0);
        let mut inner = node(80502, "scroll", rect(0.0, 0.0, 40.0, 40.0),
            vec![panel(80510, 0.0), lower]);
        inner.props.insert("_content_height".into(), Value::Number(100.0));
        let mut root = node(80501, "scroll", rect(0.0, 0.0, 40.0, 10.0), vec![inner]);
        root.props.insert("_content_height".into(), Value::Number(100.0));
        let mut cache = RetainedScene::default();
        let visible = rect(0.0, 0.0, 40.0, 20.0);
        let first = cache.prepare(&root, 1, 1, &[], viewport(), visible);
        assert!(!first.runs.iter().any(|run| run.widget_id == 80521));
        for (id, offset) in [(80501, 15.0), (80502, 3.0)] {
            let mut state = scroll::get_scroll_state(id);
            state.offset_y = offset;
            scroll::set_scroll_state(id, state);
        }
        let moved = cache.prepare(&root, 1, 1, &[80501, 80502], viewport(), visible);
        let lower = moved.runs.iter().find(|run| run.widget_id == 80521).unwrap();
        assert_eq!(lower.translation, [0.0, -18.0]);
        assert!(!moved.runs.iter().any(|run| run.widget_id == 80511));
        let primitives = moved.flatten(viewport());
        assert_eq!(primitives.iter().filter(|p| matches!(p, GpuPrimitive::PushClipRect(_))).count(),
            primitives.iter().filter(|p| matches!(p, GpuPrimitive::PopClipRect)).count());
    }

    #[test]
    fn offscreen_reactive_control_reads_latest_value_when_revealed() {
        use std::sync::Arc;
        let slot = Arc::new(AtomicU64::new(0.25_f64.to_bits()));
        let mut root = panel(80610, 100.0);
        root.children[0].widget_type = "knob".into();
        root.children[0].props.insert("value".into(), Value::ReactiveRef {
            namespace: "TEST".into(), field: "value".into(), index: None,
            kind: crate::vm::BindingKind::Float, slot: Arc::clone(&slot),
        });
        let mut cache = RetainedScene::default();
        cache.prepare(&root, 1, 1, &[], viewport(), rect(100.0, 0.0, 40.0, 20.0));
        slot.store(0.75_f64.to_bits(), Ordering::Release);
        cache.prepare(&root, 1, 2, &[80611], viewport(), rect(0.0, 0.0, 40.0, 20.0));
        let shown = cache.prepare(&root, 1, 2, &[], viewport(), rect(100.0, 0.0, 40.0, 20.0));
        assert!(shown.flatten(viewport()).iter().any(|primitive| matches!(primitive,
            GpuPrimitive::WidgetInstance { instance, .. } if (instance.value_t - 0.75).abs() < 0.001)));
    }

    #[test]
    fn hover_focus_and_shared_state_have_explicit_invalidation() {
        let mut root = panel(80710, 0.0);
        root.children[0].focusable = true;
        let visible = rect(0.0, 0.0, 40.0, 20.0);
        let mut cache = RetainedScene::default();
        cache.prepare(&root, 1, 1, &[], viewport(), visible);
        sdf_widget::set_sdf_hit_state(80710, sdf_widget::SdfHitState { hit_region: 0, hit_pressed: false });
        let hover = cache.prepare(&root, 1, 1, &[], viewport(), visible);
        assert_eq!(hover.rebuilt_nodes, 2, "inherited hover reaches child painters");
        let mut vp = viewport();
        vp.focused_widget_id = Some(80711);
        assert_eq!(cache.prepare(&root, 1, 1, &[], vp, visible).rebuilt_nodes, 1);
        bump_widget_state_generation();
        assert_eq!(cache.prepare(&root, 1, 1, &[], vp, visible).rebuilt_nodes, 2);
        theme::set_current(theme::default_theme());
        assert_eq!(cache.prepare(&root, 1, 1, &[], vp, visible).rebuilt_nodes, 2);
        vp.vp_w = 800.0;
        assert_eq!(cache.prepare(&root, 1, 1, &[], vp, visible).rebuilt_nodes, 2);
    }

    #[test]
    fn overlay_owners_are_visited_outside_the_normal_viewport() {
        let root = panel(80810, 100.0);
        let mut cache = RetainedScene::default();
        set_overlay(80811, rect(1.0, 1.0, 10.0, 10.0));
        let scene = cache.prepare(&root, 1, 1, &[], viewport(), rect(0.0, 0.0, 40.0, 20.0));
        assert!(scene.runs.iter().any(|run| run.widget_id == 80811));
        clear_overlay();
    }

    #[test]
    fn paint_bounds_include_shadows_and_hover_growth() {
        sdf_widget::register_sdf_widget(sdf_widget::SdfWidgetDef {
            name: "retained-shadow".into(), shader_source: String::new(),
            sdf_expr: crate::parser::Expression::Number(0.0), state_uniforms: vec![],
            bindable_props: vec![], region_count: 0, width: 1.0, height: 1.0,
            paint_margin: 2.0, animates: false,
        });
        let shadow = node(80901, "retained-shadow", rect(41.0, 0.0, 2.0, 2.0), vec![]);
        let scene = RetainedScene::default().prepare(&shadow, 1, 1, &[], viewport(), rect(0.0, 0.0, 40.0, 20.0));
        assert!(!scene.runs.is_empty(), "shadow overlaps the viewport");
        let mut growing = panel(80910, 41.0);
        growing.props.insert("background".into(), Value::String("retained-shadow".into()));
        let value = |v| Rc::new(std::cell::RefCell::new(v));
        growing.props.insert("style".into(), Value::Map(HashMap::from([
            ("hover".into(), value(Value::Map(HashMap::from([("scale".into(), value(Value::Number(2.0)))])))),
        ])));
        let scene = RetainedScene::default().prepare(&growing, 1, 1, &[], viewport(), rect(0.0, 0.0, 40.0, 20.0));
        assert!(scene.runs.iter().any(|run| run.widget_id == 80910),
            "the growing background remains drawable even when its child clip is offscreen");
    }

    #[test]
    fn clipped_box_keeps_its_background_over_overflowing_content() {
        sdf_widget::register_sdf_widget(sdf_widget::SdfWidgetDef {
            name: "retained-expanded-background".into(), shader_source: String::new(),
            sdf_expr: crate::parser::Expression::Number(0.0), state_uniforms: vec![],
            bindable_props: vec![], region_count: 0, width: 1.0, height: 1.0,
            paint_margin: 0.0, animates: false,
        });
        let mut root = node(81001, "box", rect(0.0, 0.0, 20.0, 2.0), vec![
            node(81002, "label", rect(1.0, 8.0, 18.0, 2.0), vec![]),
        ]);
        root.props.insert("background".into(), Value::String("retained-expanded-background".into()));
        let frame = RetainedScene::default().prepare(&root, 1, 1, &[], viewport(), rect(0.0, 8.0, 40.0, 10.0));
        assert!(frame.runs.iter().any(|run| run.widget_id == 81001),
            "the background extends below the measured box even though children remain clipped");
        assert!(!frame.runs.iter().any(|run| run.widget_id == 81002),
            "an empty ancestor clip excludes even painters without a known bound");
        assert!(frame.flatten(viewport()).iter().any(|primitive| matches!(primitive,
            GpuPrimitive::WidgetInstance { widget_type, .. } if widget_type == "retained-expanded-background")));
    }

    #[test]
    fn dirty_control_refreshes_only_its_ancestor_bounds() {
        let mut root = node(82000, "hstack", rect(0.0, 0.0, 3000.0, 10.0),
            (0..100).map(|i| panel(82010 + i * 10, i as f32 * 30.0)).collect());
        let visible = rect(0.0, 0.0, 40.0, 20.0);
        let mut cache = RetainedScene::default();
        let first = cache.prepare(&root, 1, 1, &[], viewport(), visible);
        let bystander = first.runs.iter().find(|run| run.widget_id == 82021).unwrap();
        root.children[0].children[0].props.insert("text".into(), Value::String("updated".into()));
        let changed = cache.prepare(&root, 1, 2, &[82011], viewport(), visible);
        assert_eq!(changed.reindexed_nodes, 0);
        assert_eq!(changed.bounds_refreshed_nodes, 3, "one control, its box, and the root");
        assert_eq!(changed.rebuilt_nodes, 1);
        let unchanged = changed.runs.iter().find(|run| run.widget_id == 82021).unwrap();
        assert!(Rc::ptr_eq(&bystander.primitives, &unchanged.primitives));
        assert!(changed.flatten(viewport()).iter().any(|primitive| matches!(primitive,
            GpuPrimitive::ProportionalText(text) if text.text == "updated")));
        // A content update may also replace/reorder nodes at the same measured
        // positions. Validate topology before using retained index paths.
        root.children.swap(0, 1);
        let reordered = cache.prepare(&root, 1, 3, &[82000], viewport(), visible);
        assert_eq!(reordered.reindexed_nodes, 201);
        assert!(reordered.runs.iter().any(|run| run.widget_id == 82011));
    }

}
