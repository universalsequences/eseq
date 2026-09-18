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
        let mut frozen = layout.clone();
        frozen.props = self.props.iter().map(|(key, value)| (key.clone(), value.to_value())).collect();
        Some(frozen)
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
