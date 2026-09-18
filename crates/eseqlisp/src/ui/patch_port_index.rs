//! Port discovery follows layout ownership, not frame cadence. Values and
//! reactive levels are still read at presentation time from the current nodes.
use std::sync::Arc;
use super::layout::LayoutNode;

#[derive(Default)]
pub(crate) struct PatchPortIndex {
    root: Option<Arc<LayoutNode>>,
    paths: Vec<Vec<usize>>,
}

impl PatchPortIndex {
    pub fn nodes<'a>(&'a mut self, root: &'a Arc<LayoutNode>) -> impl Iterator<Item = &'a LayoutNode> {
        if !self.root.as_ref().is_some_and(|old| Arc::ptr_eq(old, root)) {
            self.paths.clear();
            let mut pending = vec![(root.as_ref(), Vec::new())];
            while let Some((node, path)) = pending.pop() {
                if node.props.contains_key("patch-port") { self.paths.push(path.clone()); }
                for (index, child) in node.children.iter().enumerate().rev() {
                    let mut child_path = path.clone();
                    child_path.push(index);
                    pending.push((child, child_path));
                }
            }
            // Own the identity: allocator address reuse cannot resurrect an index.
            self.root = Some(Arc::clone(root));
        }
        self.paths.iter().map(move |path| {
            path.iter().fold(root.as_ref(), |node, &index| &node.children[index])
        })
    }
}
