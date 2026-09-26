//! Immutable observations of paint props. Cloning `Value` is insufficient:
//! lists/maps share mutable cells. Reactive inputs are frozen once and the
//! painter consumes that same snapshot, including the original Value types.

use super::*;
use super::super::value_snapshot::{ValueSnapshot, SnapshotContext};

#[derive(PartialEq)]
pub(super) struct PaintInputs {
    focusable: bool,
    props: HashMap<String, ValueSnapshot>,
    reactive: bool,
}
impl PaintInputs {
    pub(super) fn capture(layout: &LayoutNode) -> Option<Self> {
        // This is the same explicit painter contract as the primitive cache:
        // own props, rect, viewport, theme and tracked widget state. The box's
        // descendant geometry is checked by RetainedScene::same_topology.
        // Unknown painters still paint when dirty; output equality can retain
        // their geometry without making assumptions about their inputs.
        if !cacheable_widget_primitives(&layout.widget_type)
            && sdf_widget::sdf_widget_def(&layout.widget_type).is_none() { return None; }
        let mut capture = SnapshotContext::default();
        let props = layout.props.iter().filter(|(key, _)| paint_prop(key))
            .map(|(key, value)| Some((key.clone(), ValueSnapshot::capture(value, &mut capture)?)))
            .collect::<Option<_>>()?;
        Some(Self { focusable: layout.focusable, props, reactive: !capture.slots.is_empty() })
    }

    pub(super) fn matches_literal(&self, layout: &LayoutNode) -> bool {
        !self.reactive && self.focusable == layout.focusable
            && self.props.len() == layout.props.keys().filter(|key| paint_prop(key)).count()
            && self.props.iter().all(|(key, old)|
                layout.props.get(key).is_some_and(|value| old.matches(value)))
    }

    /// Only reactive snapshots require a substitute node. Literal inputs cannot
    /// change concurrently on the UI thread. Unknown/opaque inputs never enter
    /// this contract. Children are preserved because box paint reads geometry.
    pub(super) fn frozen_layout(&self, layout: &LayoutNode) -> Option<LayoutNode> {
        if !self.reactive { return None; }
        Some(LayoutNode {
            stable_key: layout.stable_key.clone(),
            props: self.props.iter().map(|(key, value)| (key.clone(), value.to_value())).collect(),
            children: layout.children.iter().map(geometry_skeleton).collect(),
            focusable: layout.focusable,
            ..geometry_skeleton_node(layout, Vec::new())
        })
    }

}

/// A copy of `layout` carrying only what painters read from descendants:
/// type, rect and nesting (box background extent). Descendant props are
/// never painted by an ancestor, and a deep `LayoutNode::clone` of a large
/// reactive box — rebuilt every frame while an overlay is open — dominated
/// hover frames. Extent never recurses past scroll containers or overlay
/// panels, so their subtrees are left out.
fn geometry_skeleton(layout: &LayoutNode) -> LayoutNode {
    let children = if layout.widget_type == "scroll" || is_overlay_panel_widget(&layout.widget_type) {
        Vec::new()
    } else {
        layout.children.iter().map(geometry_skeleton).collect()
    };
    geometry_skeleton_node(layout, children)
}

fn geometry_skeleton_node(layout: &LayoutNode, children: Vec<LayoutNode>) -> LayoutNode {
    LayoutNode {
        widget_id: layout.widget_id,
        stable_widget_id: layout.stable_widget_id,
        subtree_root_id: layout.subtree_root_id,
        parent_subtree_root_id: layout.parent_subtree_root_id,
        stable_key: None,
        widget_type: layout.widget_type.clone(),
        rect: layout.rect,
        props: HashMap::new(),
        children,
        focusable: false,
        animation: layout.animation,
    }
}

fn paint_prop(key: &str) -> bool {
    // These callbacks are consumed by event dispatch, never by the painters
    // opted in above. Do not ignore arbitrary `on-*` props or opaque values:
    // a custom paint prop must remain observable even if it looks like one.
    !is_internal_source_prop(key) && !matches!(key,
        "on-click" | "on-right-click" | "on-double-click" | "on-change"
        | "on-press" | "on-release" | "on-drag" | "on-drop"
        | "on-mouse-down" | "on-mouse-up")
}
