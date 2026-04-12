use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::layout::LayoutNode;
use crate::vm::Value;
use crate::widget_render::{WidgetKeyEvent, handle_event, map_key_event};

use super::{Editor, key_str};

impl Editor {
    /// Ensure only the active tile has widget focus (clear focus on all others).
    fn clear_focus_on_other_tiles(&mut self) {
        let active = self.active_tile;
        self.tile_root.clear_focus_except(active);
    }

    pub(super) fn try_click_focusable_widget(
        &mut self,
        precise_col: f32,
        precise_row: f32,
        content_col: u16,
        content_row: u16,
    ) -> bool {
        // If an overlay (dropdown menu) is active and click is inside it,
        // skip focus — the overlay intercept handles it. If outside, dismiss
        // the overlay and proceed with normal focus logic.
        if crate::widget_render::overlay_widget_id().is_some() {
            let local_row = precise_row - content_row as f32;
            let local_col = precise_col - content_col as f32;
            if crate::widget_render::overlay_contains(local_col, local_row) {
                return false;
            }
            // Dismiss overlay and fall through to normal focus handling
            if let Some(id) = crate::widget_render::overlay_widget_id() {
                crate::widget_render::dropdown::close_dropdown(id);
            }
            crate::widget_render::clear_overlay();
        }
        if !self.has_focusable_widgets() {
            return false;
        }
        let Some(layout) = self.runtime.current_layout.clone() else {
            return false;
        };
        if precise_col < content_col as f32 || precise_row < content_row as f32 {
            return false;
        }
        let scroll_top = self.total_scroll_top();
        let scroll_left = self.active_leaf().widget_scroll_left;
        let local_row = precise_row - content_row as f32 + scroll_top;
        let local_col = precise_col - content_col as f32 + scroll_left;

        // Find the focusable widget at this position
        let mut focusable_nodes: Vec<(u64, f32, f32, f32, f32)> = Vec::new();
        collect_focusable_nodes(&layout, &mut focusable_nodes);

        for (id, row, col, width, height) in &focusable_nodes {
            if local_row >= *row
                && local_row < row + height
                && local_col >= *col
                && local_col < col + width
            {
                self.active_leaf_mut().focused_widget_id = Some(*id);
                self.clear_focus_on_other_tiles();
                self.adjust_widget_scroll(*row);
                self.mark_needs_redraw();
                return self.activate_focused();
            }
        }
        // Click landed outside any focusable widget → blur
        if self.active_leaf().focused_widget_id.is_some() {
            self.active_leaf_mut().focused_widget_id = None;
            self.mark_needs_redraw();
        }
        false
    }

    fn widgets_active(&self) -> bool {
        self.active_buffer().read_only
            || self.active_buffer().view_mode == crate::editor::ViewMode::UiOnly
    }

    pub(super) fn has_focusable_widgets(&self) -> bool {
        self.runtime
            .current_layout
            .as_ref()
            .map(|layout| has_focusable_node(layout))
            .unwrap_or(false)
    }

    pub(super) fn handle_focus_key(&mut self, key: KeyEvent) -> bool {
        if !self.widgets_active() || !self.has_focusable_widgets() {
            return false;
        }

        // Only handle arrow/enter for focus navigation when something is already focused.
        // Otherwise let the key fall through to mode/global bindings.
        let has_focus = self.active_leaf().focused_widget_id.is_some();

        match key.code {
            KeyCode::Esc if has_focus => {
                self.active_leaf_mut().focused_widget_id = None;
                self.mark_needs_redraw();
                true
            }
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right if has_focus => {
                self.navigate_focus(key.code);
                true
            }
            KeyCode::Enter if has_focus => {
                self.activate_focused();
                true
            }
            _ => self.dispatch_focus_key(key),
        }
    }

    /// Dispatch a key event to the focused widget's :on-focus-key callback.
    /// Returns true if the widget handled the key.
    fn dispatch_focus_key(&mut self, key: KeyEvent) -> bool {
        let Some(focused_id) = self.active_leaf().focused_widget_id else {
            return false;
        };
        let Some(layout) = self.runtime.current_layout.clone() else {
            return false;
        };
        let Some(node) = find_node_by_id(&layout, focused_id) else {
            return false;
        };
        let Some(callback) = node.props.get("on-focus-key").cloned() else {
            return false;
        };
        let key_arg = Value::String(key_str(key));
        let text_arg = match key.code {
            KeyCode::Char(c)
                if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT =>
            {
                Value::String(c.to_string())
            }
            _ => Value::Bool(false),
        };
        let result = self.runtime.invoke(callback, vec![key_arg, text_arg]);
        if let Some(status) = self.runtime.take_status_message() {
            self.minibuffer = Some(status);
        }
        let handled = match result {
            Ok(Some(Value::Bool(h))) => h,
            Ok(Some(Value::Nil)) | Ok(None) => false,
            Ok(Some(_)) => true,
            Err(e) => {
                self.minibuffer = Some(format!("Error: {e:?}"));
                true
            }
        };
        if handled {
            self.refresh_runtime_side_effects();
            self.sync_runtime_context();
            self.mark_needs_redraw();
        }
        handled
    }

    pub(super) fn handle_focused_widget_key(&mut self, key: KeyEvent) -> bool {
        let Some(focused_id) = self.active_leaf().focused_widget_id else {
            return false;
        };
        let Some(layout) = self.runtime.current_layout.clone() else {
            return false;
        };
        let Some(node) = find_node_by_id(&layout, focused_id) else {
            return false;
        };
        // Space bar should only be consumed by text-input widgets (for typing).
        // All other widgets let space fall through to keybindings.
        let is_text_input = node.widget_type == "text-input";
        if key.code == KeyCode::Char(' ') && !is_text_input {
            return false;
        }
        let widget_event = map_key_event(
            &node,
            WidgetKeyEvent {
                code: key.code,
                modifiers: key.modifiers,
            },
        );
        // If key_event returned Some, the widget consumed the key — even if
        // handle_event produces no callback (e.g. cursor moves, menu navigation).
        let consumed = widget_event.is_some();
        let output = widget_event.and_then(|event| handle_event(&node, event));
        if !consumed {
            return false;
        }
        let _ = self.apply_widget_output(output);
        true
    }

    pub(super) fn navigate_focus(&mut self, direction: KeyCode) {
        let Some(layout) = self.runtime.current_layout.clone() else {
            return;
        };
        let mut focusable_nodes: Vec<(u64, f32, f32, f32, f32)> = Vec::new();
        collect_focusable_nodes(&layout, &mut focusable_nodes);
        if focusable_nodes.is_empty() {
            return;
        }

        let focused_id = self.active_leaf().focused_widget_id;

        // If nothing is focused, don't auto-focus — let the key fall through
        // to mode/global bindings (e.g. cursor navigation).
        let Some(current_id) = focused_id else {
            return;
        };
        let cur = focusable_nodes
            .iter()
            .find(|(id, _, _, _, _)| *id == current_id);
        let Some(&(_, cur_row, cur_col, cur_w, cur_h)) = cur else {
            return;
        };
        let cur_cy = cur_row + cur_h * 0.5;
        let cur_cx = cur_col + cur_w * 0.5;

        // 2D spatial navigation: find the nearest focusable widget in the
        // requested direction.  The primary-axis distance is weighted 1×,
        // the secondary axis 3× so we strongly prefer staying on-axis.
        let mut best: Option<(u64, f32, f32)> = None; // (id, row, score)

        for &(id, row, col, w, h) in &focusable_nodes {
            if id == current_id {
                continue;
            }
            let cy = row + h * 0.5;
            let cx = col + w * 0.5;

            let (primary, secondary, in_direction) = match direction {
                KeyCode::Down => (cy - cur_cy, (cx - cur_cx).abs(), cy > cur_cy),
                KeyCode::Up => (cur_cy - cy, (cx - cur_cx).abs(), cy < cur_cy),
                KeyCode::Right => (cx - cur_cx, (cy - cur_cy).abs(), cx > cur_cx),
                KeyCode::Left => (cur_cx - cx, (cy - cur_cy).abs(), cx < cur_cx),
                _ => continue,
            };

            if !in_direction {
                continue;
            }

            let score = primary + secondary * 3.0;

            if best.is_none() || score < best.unwrap().2 {
                best = Some((id, row, score));
            }
        }

        if let Some((next_id, next_row, _)) = best {
            self.active_leaf_mut().focused_widget_id = Some(next_id);
            self.clear_focus_on_other_tiles();
            self.adjust_widget_scroll(next_row);
            self.mark_needs_redraw();
            return;
        }

        // No candidate in that direction — scroll the viewport if at the edge
        match direction {
            KeyCode::Up | KeyCode::Left => {
                let leaf = self.active_leaf_mut();
                leaf.widget_scroll_top = (leaf.widget_scroll_top - 3.0).max(0.0);
                self.mark_needs_redraw();
            }
            KeyCode::Down | KeyCode::Right => {
                let max_scroll = self
                    .runtime
                    .current_layout
                    .as_ref()
                    .map(|l| {
                        let viewport_height = self.runtime.layout_rows();
                        (l.rect.row + l.rect.height).ceil() - viewport_height as f32
                    })
                    .unwrap_or(0.0);
                let leaf = self.active_leaf_mut();
                leaf.widget_scroll_top = (leaf.widget_scroll_top + 3.0).min(max_scroll).max(0.0);
                self.mark_needs_redraw();
            }
            _ => {}
        }
    }

    pub(super) fn adjust_widget_scroll(&mut self, focused_row: f32) {
        let viewport_height = self.runtime.layout_rows();
        if viewport_height == 0 {
            return;
        }
        let focused_terminal_row = focused_row.round();
        let leaf = self.active_leaf_mut();
        if focused_terminal_row < leaf.widget_scroll_top {
            leaf.widget_scroll_top = focused_terminal_row;
        }
        if focused_terminal_row >= leaf.widget_scroll_top + viewport_height as f32 {
            leaf.widget_scroll_top = focused_terminal_row - viewport_height as f32 + 1.0;
        }
    }

    pub(super) fn activate_focused(&mut self) -> bool {
        let Some(focused_id) = self.active_leaf().focused_widget_id else {
            return false;
        };
        let Some(layout) = self.runtime.current_layout.clone() else {
            return false;
        };
        let Some(node) = find_node_by_id(&layout, focused_id) else {
            return false;
        };
        // Look for :on-enter callback in props
        if let Some(callback) = node.props.get("on-enter").cloned() {
            let result = self.runtime.invoke(callback, vec![]);
            if let Some(status) = self.runtime.take_status_message() {
                self.minibuffer = Some(status);
            } else if let Err(error) = result {
                self.minibuffer = Some(format!("Error: {error:?}"));
            }
            self.refresh_runtime_side_effects();
            self.sync_runtime_context();
            self.mark_needs_redraw();
            return true;
        }
        false
    }

    pub(super) fn save_current_widget_tree(&mut self) {
        if let Some(tree) = self.runtime.current_widget_tree() {
            let source = self.active_buffer().widget_tree_source;
            self.active_buffer_mut().set_widget_tree(Some(tree), source);
        }
    }

    /// Clear widget layout and focus state for a buffer with no widget tree.
    pub(super) fn clear_widget_focus(&mut self) {
        crate::widget_render::clear_overlay();
        self.runtime.clear_current_widget_tree();
        let leaf = self.active_leaf_mut();
        leaf.focused_widget_id = None;
        leaf.widget_scroll_top = 0.0;
        leaf.widget_scroll_left = 0.0;
        leaf.active_widget_gesture = None;
        leaf.cached_layout = None;
    }

    pub(super) fn restore_buffer_widget_tree(&mut self) {
        crate::widget_render::clear_overlay();
        let buf = self.active_buffer();
        let tree = buf.widget_tree.clone();
        let buffer_id = buf.id as u64;
        self.runtime.set_widget_id_offset(buffer_id * 100_000);
        match tree {
            Some(tree) => {
                self.runtime.restore_widget_tree(tree);
            }
            None => {
                self.clear_widget_focus();
            }
        }
    }

    pub(super) fn auto_focus_first_widget(&mut self) {
        if !self.widgets_active() {
            self.active_leaf_mut().focused_widget_id = None;
            return;
        }
        let Some(layout) = self.runtime.current_layout.clone() else {
            return;
        };
        let mut focusable_nodes: Vec<(u64, f32, f32, f32, f32)> = Vec::new();
        collect_focusable_nodes(&layout, &mut focusable_nodes);
        if let Some((id, _, _, _, _)) = focusable_nodes.first() {
            self.active_leaf_mut().focused_widget_id = Some(*id);
        }
    }
}

pub(super) fn has_focusable_node(node: &LayoutNode) -> bool {
    if node.focusable {
        return true;
    }
    node.children.iter().any(has_focusable_node)
}

pub(super) fn collect_focusable_nodes(node: &LayoutNode, out: &mut Vec<(u64, f32, f32, f32, f32)>) {
    if node.focusable {
        out.push((
            node.widget_id,
            node.rect.row,
            node.rect.col,
            node.rect.width,
            node.rect.height,
        ));
    }
    for child in &node.children {
        collect_focusable_nodes(child, out);
    }
}

pub(super) fn find_node_by_id(node: &LayoutNode, id: u64) -> Option<LayoutNode> {
    if node.widget_id == id {
        return Some(node.clone());
    }
    for child in &node.children {
        if let Some(found) = find_node_by_id(child, id) {
            return Some(found);
        }
    }
    None
}
