use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::layout::LayoutNode;
use crate::vm::Value;
use crate::widget_render::{WidgetKeyEvent, handle_event, map_key_event};

use super::{Editor, key_str};

impl Editor {
    pub(super) fn try_click_focusable_widget(
        &mut self,
        precise_col: f32,
        precise_row: f32,
        content_col: u16,
        content_row: u16,
    ) -> bool {
        if !self.has_focusable_widgets() {
            return false;
        }
        let Some(layout) = self.runtime.current_layout.clone() else {
            return false;
        };
        if precise_col < content_col as f32 || precise_row < content_row as f32 {
            return false;
        }
        let scroll_top = self.active_leaf().widget_scroll_top as f32;
        let local_row = precise_row - content_row as f32 + scroll_top;
        let local_col = precise_col - content_col as f32;

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
                self.adjust_widget_scroll(*row);
                self.mark_needs_redraw();
                return self.activate_focused();
            }
        }
        false
    }

    pub(super) fn has_focusable_widgets(&self) -> bool {
        self.runtime
            .current_layout
            .as_ref()
            .map(|layout| has_focusable_node(layout))
            .unwrap_or(false)
    }

    pub(super) fn handle_focus_key(&mut self, key: KeyEvent) -> bool {
        if !self.active_buffer().read_only || !self.has_focusable_widgets() {
            return false;
        }

        match key.code {
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => {
                self.navigate_focus(key.code);
                true
            }
            KeyCode::Enter => {
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
                if key.modifiers == KeyModifiers::NONE
                    || key.modifiers == KeyModifiers::SHIFT =>
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
        let output = map_key_event(
            &node,
            WidgetKeyEvent {
                code: key.code,
                modifiers: key.modifiers,
            },
        )
        .and_then(|event| handle_event(&node, event));
        if output.is_none() {
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

        // Auto-focus first widget if nothing is focused
        if focused_id.is_none() {
            self.active_leaf_mut().focused_widget_id = Some(focusable_nodes[0].0);
            self.mark_needs_redraw();
            return;
        }

        let current_id = focused_id.unwrap();
        let current_idx = focusable_nodes
            .iter()
            .position(|(id, _, _, _, _)| *id == current_id)
            .unwrap_or(0);

        let next_idx = match direction {
            KeyCode::Down | KeyCode::Right => {
                (current_idx + 1).min(focusable_nodes.len() - 1)
            }
            KeyCode::Up | KeyCode::Left => {
                current_idx.saturating_sub(1)
            }
            _ => current_idx,
        };

        // When already at the first focusable and scrolling up, keep scrolling
        // the viewport toward the top so content above the first widget is visible.
        if next_idx == current_idx && current_idx == 0 && matches!(direction, KeyCode::Up | KeyCode::Left) {
            let leaf = self.active_leaf_mut();
            leaf.widget_scroll_top = leaf.widget_scroll_top.saturating_sub(3);
            self.mark_needs_redraw();
            return;
        }

        // Similarly, when at the last focusable and scrolling down, keep scrolling
        // the viewport so content below the last widget is visible.
        if next_idx == current_idx && current_idx == focusable_nodes.len() - 1 && matches!(direction, KeyCode::Down | KeyCode::Right) {
            let max_scroll = self
                .runtime
                .current_layout
                .as_ref()
                .map(|l| {
                    let viewport_height = self.runtime.layout_rows();
                    ((l.rect.row + l.rect.height).ceil() as u16)
                        .saturating_sub(viewport_height)
                })
                .unwrap_or(0);
            let leaf = self.active_leaf_mut();
            leaf.widget_scroll_top = leaf.widget_scroll_top.saturating_add(3).min(max_scroll);
            self.mark_needs_redraw();
            return;
        }

        self.active_leaf_mut().focused_widget_id = Some(focusable_nodes[next_idx].0);
        self.adjust_widget_scroll(focusable_nodes[next_idx].1);
        self.mark_needs_redraw();
    }

    pub(super) fn adjust_widget_scroll(&mut self, focused_row: f32) {
        let viewport_height = self.runtime.layout_rows();
        if viewport_height == 0 {
            return;
        }
        let focused_terminal_row = focused_row.round() as u16;
        let leaf = self.active_leaf_mut();
        if focused_terminal_row < leaf.widget_scroll_top {
            leaf.widget_scroll_top = focused_terminal_row;
        }
        if focused_terminal_row >= leaf.widget_scroll_top + viewport_height {
            leaf.widget_scroll_top = focused_terminal_row - viewport_height + 1;
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
            self.active_buffer_mut().widget_tree = Some(tree);
        }
    }

    /// Clear widget layout and focus state for a buffer with no widget tree.
    pub(super) fn clear_widget_focus(&mut self) {
        self.runtime.clear_current_widget_tree();
        let leaf = self.active_leaf_mut();
        leaf.focused_widget_id = None;
        leaf.widget_scroll_top = 0;
        leaf.widget_scroll_left = 0;
        leaf.active_widget_gesture = None;
        leaf.cached_layout = None;
    }

    pub(super) fn restore_buffer_widget_tree(&mut self) {
        let tree = self.active_buffer().widget_tree.clone();
        match tree {
            Some(tree) => {
                self.runtime.restore_widget_tree(tree);
                // Only auto-focus if this tile doesn't already have a focused widget
                if self.active_leaf().focused_widget_id.is_none() {
                    self.auto_focus_first_widget();
                }
            }
            None => {
                self.clear_widget_focus();
            }
        }
    }

    pub(super) fn auto_focus_first_widget(&mut self) {
        if !self.active_buffer().read_only {
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
