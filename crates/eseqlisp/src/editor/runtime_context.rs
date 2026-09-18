use super::*;

// Buffer and tile fields are publicly mutable. Compare the actual inputs,
// rather than requiring every editor/host mutation to remember a dirty flag.
#[derive(Default)]
pub(super) struct RuntimeContextCache {
    buffers: Vec<BufferMetadata>,
    recency: Vec<BufferId>,
    presentation: Option<PresentationMetadata>,
}

struct BufferMetadata {
    id: BufferId,
    name: String,
    mode: BufferMode,
    path: Option<PathBuf>,
    line_count: usize,
    dirty: bool,
    read_only: bool,
}

impl BufferMetadata {
    fn new(buffer: &Buffer) -> Self {
        Self {
            id: buffer.id,
            name: buffer.name.clone(),
            mode: buffer.mode.clone(),
            path: buffer.path.clone(),
            line_count: buffer.lines.len(),
            dirty: buffer.dirty,
            read_only: buffer.read_only,
        }
    }

    fn matches(&self, buffer: &Buffer) -> bool {
        self.id == buffer.id && self.name == buffer.name
            && self.mode == buffer.mode && self.path == buffer.path
            && self.line_count == buffer.lines.len()
            && self.dirty == buffer.dirty && self.read_only == buffer.read_only
    }
}

struct PresentationMetadata {
    buffers: Vec<(String, bool)>,
    tile_buffers: Vec<usize>,
}

fn is_presentation_buffer(buffer: &Buffer) -> bool {
    buffer.widget_tree.as_ref().is_some_and(|tree| !matches!(tree, Value::Nil))
}

impl PresentationMetadata {
    fn matches(&self, buffers: &[Buffer], tiles: &TileNode) -> bool {
        if self.buffers.len() != buffers.len()
            || !self.buffers.iter().zip(buffers).all(|((name, presentation), buffer)| {
                *name == buffer.name && *presentation == is_presentation_buffer(buffer)
            })
        {
            return false;
        }
        // Walk the small tile tree without allocating a leaf list or searching
        // from its root once per leaf. Geometry/focus changes do not matter here.
        fn matches_leaves(tiles: &TileNode, previous: &mut std::slice::Iter<'_, usize>) -> bool {
            match tiles {
                TileNode::Leaf(leaf) => previous.next() == Some(&leaf.buffer_idx),
                TileNode::Split(split) => {
                    matches_leaves(&split.a, previous) && matches_leaves(&split.b, previous)
                }
            }
        }
        let mut previous = self.tile_buffers.iter();
        matches_leaves(tiles, &mut previous) && previous.next().is_none()
    }
}

impl Editor {
    pub(super) fn sync_runtime_context(&mut self) {
        let cache = &self.runtime_context_cache;
        if cache.recency != self.buffer_recency
            || cache.buffers.len() != self.buffers.len()
            || !cache.buffers.iter().zip(&self.buffers).all(|(previous, buffer)| previous.matches(buffer))
        {
            // Also normalizes recency after direct additions/removals.
            let infos = self.buffer_infos_by_recency();
            let mut shared = self.runtime.shared.borrow_mut();
            shared.buffer_names = infos.iter().map(|info| info.name.clone()).collect();
            shared.buffer_infos = infos;
            self.runtime_context_cache.buffers = self.buffers.iter().map(BufferMetadata::new).collect();
            self.runtime_context_cache.recency.clone_from(&self.buffer_recency);
        }

        let active = self.active_buffer();
        {
            let mut shared = self.runtime.shared.borrow_mut();
            shared.current_buffer_id = Some(active.id);
            if shared.current_buffer_name != active.name {
                shared.current_buffer_name.clone_from(&active.name);
            }
            if shared.current_buffer_path != active.path {
                shared.current_buffer_path.clone_from(&active.path);
            }
            shared.current_buffer_read_only = active.read_only;
            if shared.current_buffer_mode != active.mode.name() {
                active.mode.name().clone_into(&mut shared.current_buffer_mode);
            }
            shared.current_line_number = active.cursor.0 + 1;
            let line = active.lines.get(active.cursor.0).map(String::as_str).unwrap_or_default();
            if shared.current_line_text != line {
                line.clone_into(&mut shared.current_line_text);
            }
            if shared.current_view_mode != active.view_mode.label() {
                active.view_mode.label().clone_into(&mut shared.current_view_mode);
            }
            shared.current_text_zoom = self.text_zoom as f64;
        }
        self.sync_visible_effect_buffers();
    }

    pub(super) fn sync_visible_effect_buffers(&mut self) {
        if self.runtime_context_cache.presentation.as_ref()
            .is_some_and(|previous| previous.matches(&self.buffers, &self.tile_root))
        {
            return;
        }
        let tile_buffers: Vec<_> = self.tile_root.leaf_ids().into_iter()
            .filter_map(|id| self.tile_root.find_leaf(id).map(|leaf| leaf.buffer_idx))
            .collect();
        let visible_names: HashSet<_> = tile_buffers.iter()
            .filter_map(|idx| self.buffers.get(*idx))
            .map(|buffer| buffer.name.as_str())
            .collect();
        let mut ordered: Vec<_> = visible_names.iter().map(|name| name.to_string()).collect();
        ordered.sort();
        self.runtime.shared.borrow_mut().visible_buffer_names = ordered;
        // Nil-returning projections are side-effect producers, not presentation
        // buffers: they must keep running even when no tile displays them.
        let hidden_names = self.buffers.iter()
            .filter(|buffer| !visible_names.contains(buffer.name.as_str()))
            .filter(|buffer| is_presentation_buffer(buffer))
            .map(|buffer| buffer.name.clone())
            .collect();
        self.runtime.set_hidden_effect_buffer_names(hidden_names);
        self.runtime_context_cache.presentation = Some(PresentationMetadata {
            buffers: self.buffers.iter()
                .map(|buffer| (buffer.name.clone(), is_presentation_buffer(buffer)))
                .collect(),
            tile_buffers,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_metadata_reuses_unchanged_lists_and_tracks_direct_buffer_edits() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.open_scratch_buffer("*context*", "one\ntwo");
        editor.runtime_mut();
        let pointers = {
            let shared = editor.runtime.shared.borrow();
            (shared.buffer_infos.as_ptr(), shared.buffer_names.as_ptr(), shared.visible_buffer_names.as_ptr())
        };
        for _ in 0..10 { editor.runtime_mut(); }
        {
            let shared = editor.runtime.shared.borrow();
            assert_eq!(pointers, (shared.buffer_infos.as_ptr(), shared.buffer_names.as_ptr(), shared.visible_buffer_names.as_ptr()));
        }

        // These public fields can change without going through editor commands
        // or incrementing the text revision.
        let buffer = editor.active_buffer_mut();
        buffer.name = "*renamed-context*".into();
        buffer.path = Some(PathBuf::from("/tmp/context.txt"));
        buffer.mode = BufferMode::Named("context-test-mode".into());
        buffer.read_only = true;
        buffer.dirty = true;
        buffer.view_mode = ViewMode::Both;
        buffer.lines.push("three".into());
        buffer.lines[1] = "revised two".into();
        buffer.cursor = (1, 2);
        editor.runtime_mut();
        {
            let shared = editor.runtime.shared.borrow();
            assert_eq!(shared.current_buffer_name, "*renamed-context*");
            assert_eq!(shared.current_buffer_path.as_deref(), Some(std::path::Path::new("/tmp/context.txt")));
            assert!(shared.current_buffer_read_only);
            assert_eq!(shared.current_buffer_mode, "context-test-mode");
            assert_eq!(shared.current_view_mode, ViewMode::Both.label());
            assert_eq!(shared.current_line_number, 2);
            assert_eq!(shared.current_line_text, "revised two");
            let info = &shared.buffer_infos[0];
            assert_eq!(info.name, shared.current_buffer_name);
            assert_eq!(info.mode, shared.current_buffer_mode);
            assert_eq!(info.path.as_deref(), Some("/tmp/context.txt"));
            assert_eq!(info.line_count, 3);
            assert!(info.dirty && info.read_only);
            assert_eq!(shared.visible_buffer_names, vec!["*renamed-context*"]);
        }
        editor.active_buffer_mut().lines[1] = "edited again".into();
        assert_eq!(editor.runtime_mut().shared.borrow().current_line_text, "edited again");
        editor.active_buffer_mut().cursor.0 = 0;
        assert_eq!(editor.runtime_mut().shared.borrow().current_line_text, "one");
        editor.set_text_zoom(1.5).unwrap();
        editor.runtime_mut();
        assert_eq!(editor.runtime.shared.borrow().current_text_zoom, 1.5);
    }

    #[test]
    fn runtime_metadata_tracks_buffer_membership_recency_and_direct_tile_changes() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        let first = editor.open_scratch_buffer("*first*", "first");
        let second = editor.open_scratch_buffer("*second*", "second");
        editor.set_active_buffer(first);
        assert_eq!(editor.runtime_mut().shared.borrow().buffer_names[..2], ["*first*", "*second*"]);
        let second_idx = editor.buffers.iter().position(|buffer| buffer.id == second).unwrap();
        let tile = editor.split_active_tile(SplitDir::Vertical, second_idx).unwrap();
        assert_eq!(editor.runtime_mut().shared.borrow().visible_buffer_names, ["*first*", "*second*"]);
        let first_idx = editor.active_buffer_idx();
        editor.tile_root.find_leaf_mut(tile).unwrap().buffer_idx = first_idx;
        assert_eq!(editor.runtime_mut().shared.borrow().visible_buffer_names, ["*first*"]);
        assert!(editor.remove_buffer_by_name("*second*"));
        assert!(!editor.runtime_mut().shared.borrow().buffer_names.contains(&"*second*".to_string()));
        // Removing a buffer can shift the indices stored in existing tiles.
        assert_eq!(editor.runtime_mut().shared.borrow().current_buffer_id, Some(first));
        assert_eq!(editor.runtime_mut().shared.borrow().visible_buffer_names, ["*first*"]);
    }
}
