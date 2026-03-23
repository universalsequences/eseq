use std::time::Duration;

use crossterm::event::{MouseEvent, MouseEventKind};

use crate::layout::{LayoutNode, hit_test_layout};
use crate::tile::{WidgetClick, WidgetGesture};
use crate::ui::hit::{self, HitGrid};
use crate::vm::Value;
use crate::widget_render::{
    self, MouseEventOutcome, begin_widget_gesture as begin_widget_gesture_data,
    captures_scroll_gesture, handle_event, map_double_click_event, map_magnify_event,
    map_mouse_event, map_scroll_gesture_event,
};

use super::Editor;
use super::widget_focus::find_node_by_id;

impl Editor {
    pub(super) fn try_handle_widget_mouse_precise(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> bool {
        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            return false;
        };
        let output = {
            let Some(node) = self.widget_node_at_local(local_col, local_row) else {
                return false;
            };

            self.dispatch_widget_mouse_event(
                &node,
                mouse.kind,
                content_col,
                content_row,
                precise_col,
                precise_row,
                None,
                None,
            )
        };

        self.apply_widget_output(output)
    }

    /// Update SDF widget hover/pressed state and redraw if changed.
    pub(super) fn update_sdf_hover(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
        pressed: bool,
    ) {
        use crate::widget_render::sdf_widget::{self, SdfHitState};

        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            return;
        };
        let Some(node) = self.widget_node_at_local(local_col, local_row) else {
            return;
        };
        if sdf_widget::sdf_widget_def(&node.widget_type).is_none() {
            return;
        }

        let widget_col = local_col + self.active_leaf().widget_scroll_left as f32 - node.rect.col;
        let widget_row = local_row + self.total_scroll_top() - node.rect.row;
        let (cell_w, cell_h) = self.runtime.layout_cell_dims();
        let px_w = node.rect.width * cell_w;
        let px_h = node.rect.height * cell_h;
        let pixel_aspect = if px_h > 0.0 { px_w / px_h } else { 1.0 };

        let region = sdf_widget::sdf_widget_hit_test(&node, widget_col, widget_row, pixel_aspect);

        let old = sdf_widget::get_sdf_hit_state(node.widget_id);
        if old.hit_region != region || old.hit_pressed != pressed {
            sdf_widget::set_sdf_hit_state(
                node.widget_id,
                SdfHitState {
                    hit_region: region,
                    hit_pressed: pressed,
                },
            );
            self.mark_needs_redraw();
        }
    }

    pub(super) fn try_handle_widget_double_click(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> bool {
        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            return false;
        };
        let Some(node) = self.widget_node_at_local(local_col, local_row) else {
            return false;
        };
        if !self.is_double_click_candidate(node.widget_id, precise_col, precise_row) {
            return false;
        }
        let scrolled_col = local_col + self.active_leaf().widget_scroll_left as f32;
        let scrolled_row = local_row + self.total_scroll_top();
        let Some(widget_event) = map_double_click_event(&node, scrolled_col, scrolled_row) else {
            return false;
        };
        let output = handle_event(&node, widget_event);
        self.apply_widget_output(output)
    }

    fn is_double_click_candidate(
        &self,
        widget_id: u64,
        precise_col: f32,
        precise_row: f32,
    ) -> bool {
        const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(350);
        const DOUBLE_CLICK_SLOP: f32 = 1.5;
        self.active_leaf()
            .last_widget_click
            .as_ref()
            .is_some_and(|click| {
                click.widget_id == widget_id
                    && click.at.elapsed() <= DOUBLE_CLICK_WINDOW
                    && (click.precise_col - precise_col).abs() <= DOUBLE_CLICK_SLOP
                    && (click.precise_row - precise_row).abs() <= DOUBLE_CLICK_SLOP
            })
    }

    pub(super) fn remember_widget_click(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) {
        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            self.active_leaf_mut().last_widget_click = None;
            return;
        };
        let click = self
            .widget_node_at_local(local_col, local_row)
            .map(|node| WidgetClick {
                widget_id: node.widget_id,
                precise_col,
                precise_row,
                at: std::time::Instant::now(),
            });
        self.active_leaf_mut().last_widget_click = click;
    }

    pub(super) fn begin_widget_gesture(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) {
        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            return;
        };
        let Some(node) = self.widget_node_at_local(local_col, local_row) else {
            return;
        };
        let scrolled_col = local_col + self.active_leaf().widget_scroll_left as f32;
        let scrolled_row = local_row + self.total_scroll_top();
        let gesture_data = begin_widget_gesture_data(&node, scrolled_col, scrolled_row);
        if widget_render::widget_captures_drag(&node.widget_type) || gesture_data.is_some() {
            self.active_leaf_mut().active_widget_gesture = Some(WidgetGesture {
                widget_id: node.widget_id,
                start_precise_col: precise_col,
                start_precise_row: precise_row,
                gesture_data,
            });
        }
    }

    pub(super) fn try_handle_widget_drag_segment(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        start: (f32, f32),
        end: (f32, f32),
    ) {
        let start_local = (start.0 - content_col as f32, start.1 - content_row as f32);
        let end_local = (end.0 - content_col as f32, end.1 - content_row as f32);
        let start_node = self.widget_node_at_local(start_local.0, start_local.1);
        let end_node = self.widget_node_at_local(end_local.0, end_local.1);

        if let Some(node) = start_node.as_ref()
            && widget_render::widget_captures_drag(&node.widget_type)
        {
            // Clamp drag to widget bounds in terminal-cell screen space
            let scroll = self.total_scroll_top();
            let screen_row = node.rect.row - scroll;
            let screen_height = node.rect.height;
            let clamped_col = end.0.clamp(
                content_col as f32 + node.rect.col,
                content_col as f32 + node.rect.col + (node.rect.width - 1.0).max(0.0),
            );
            let clamped_row = end.1.clamp(
                content_row as f32 + screen_row,
                content_row as f32 + screen_row + (screen_height - 1.0).max(0.0),
            );
            let output = self.dispatch_widget_mouse_event(
                node,
                mouse.kind,
                content_col,
                content_row,
                clamped_col,
                clamped_row,
                Some(start),
                None,
            );
            let _ = self.apply_widget_output(output);
            return;
        }

        if HitGrid::same_hit(start_node.as_ref(), end_node.as_ref()) {
            let _ =
                self.try_handle_widget_mouse_precise(mouse, content_col, content_row, end.0, end.1);
            return;
        }

        let steps = ((end.0 - start.0).abs().max((end.1 - start.1).abs()) * 2.0)
            .ceil()
            .max(1.0) as usize;
        let mut last_hit: Option<LayoutNode> = None;
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let col = start.0 + (end.0 - start.0) * t;
            let row = start.1 + (end.1 - start.1) * t;
            let local_col = col - content_col as f32;
            let local_row = row - content_row as f32;
            let node = self.widget_node_at_local(local_col, local_row);
            if node.is_some() && !HitGrid::same_hit(node.as_ref(), last_hit.as_ref()) {
                let _ =
                    self.try_handle_widget_mouse_precise(mouse, content_col, content_row, col, row);
            }
            last_hit = node;
        }
    }

    /// Hit-test the widget layout tree using f32 coordinates in layout row/col units.
    /// Takes local terminal-cell coords (relative to content area), adds scroll,
    /// and does a precise rect-contains walk.
    pub(super) fn widget_node_at_local(
        &mut self,
        local_col: f32,
        local_row: f32,
    ) -> Option<LayoutNode> {
        let layout = self.runtime.current_layout.as_ref()?;
        let widget_scroll = self.widget_scroll_top() as f32;
        let text_scroll = self.active_buffer().scroll_top as f32;
        let hscroll = self.active_leaf().widget_scroll_left as f32;

        let layout_col = local_col + hscroll;
        let layout_row = local_row + widget_scroll + text_scroll;

        hit_test_layout(layout, layout_row, layout_col).cloned()
    }

    pub(super) fn widget_node_at_screen(
        &mut self,
        precise_col: f32,
        precise_row: f32,
        content_col: u16,
        content_row: u16,
    ) -> Option<LayoutNode> {
        let (local_col, local_row) =
            hit::to_local(precise_col, precise_row, content_col, content_row)?;
        self.widget_node_at_local(local_col, local_row)
    }

    pub(super) fn handle_text_click(
        &mut self,
        mouse: MouseEvent,
        content_col: u16,
        content_row: u16,
        content_width: u16,
        content_height: u16,
    ) {
        if mouse.column < content_col || mouse.row < content_row {
            return;
        }
        let local_col = mouse.column - content_col;
        let local_row = mouse.row - content_row;
        if local_col >= content_width || local_row >= content_height {
            return;
        }

        let buffer = self.active_buffer_mut();
        if !buffer.read_only {
            let absolute_row = buffer
                .scroll_top
                .saturating_add(local_row as usize)
                .min(buffer.lines.len().saturating_sub(1));
            let absolute_col = (local_col as usize).min(buffer.lines[absolute_row].len());
            buffer.cursor = (absolute_row, absolute_col);
            let leaf = self.active_leaf_mut();
            leaf.focused_widget_id = None;
            leaf.active_widget_gesture = None;
        }
        self.completion = None;
        self.minibuffer = None;
        self.clear_mark();
        self.sync_runtime_context();
        self.mark_needs_redraw();
    }

    pub(super) fn dispatch_widget_mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
        drag_start: Option<(f32, f32)>,
        explicit_gesture: Option<&Value>,
    ) -> Option<crate::widget_render::EventOutput> {
        let total_scroll_top = self.total_scroll_top();
        let total_scroll_left = self.active_leaf().widget_scroll_left as f32;
        let local_col = precise_col - content_col as f32 + total_scroll_left;
        let local_row = precise_row - content_row as f32 + total_scroll_top;
        let drag_start = drag_start.map(|(start_col, start_row)| {
            (
                start_col - content_col as f32 + total_scroll_left,
                start_row - content_row as f32 + total_scroll_top,
            )
        });
        let leaf = self.active_leaf();
        let gesture = leaf
            .active_widget_gesture
            .as_ref()
            .and_then(|gesture| (gesture.widget_id == node.widget_id).then_some(gesture))
            .and_then(|gesture| gesture.gesture_data.as_ref())
            .or(explicit_gesture);
        match map_mouse_event(node, mouse_kind, local_col, local_row, drag_start, gesture) {
            MouseEventOutcome::Ignore | MouseEventOutcome::Consume => None,
            MouseEventOutcome::Dispatch(widget_event) => handle_event(node, widget_event),
        }
    }

    pub(super) fn dispatch_gesture_widget_mouse_event(
        &self,
        gesture: WidgetGesture,
        mouse_kind: MouseEventKind,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
    ) -> Option<crate::widget_render::EventOutput> {
        let layout = self.runtime.current_layout.as_ref()?;
        let node = find_node_by_id(layout, gesture.widget_id)?;
        self.dispatch_widget_mouse_event(
            &node,
            mouse_kind,
            content_col,
            content_row,
            precise_col,
            precise_row,
            Some((gesture.start_precise_col, gesture.start_precise_row)),
            gesture.gesture_data.as_ref(),
        )
    }

    pub(super) fn handle_touchpad_magnify_impl(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
        delta: f64,
    ) {
        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            return;
        };
        let Some(node) = self.widget_node_at_local(local_col, local_row) else {
            return;
        };
        let scrolled_col = local_col + self.active_leaf().widget_scroll_left as f32;
        let scrolled_row = local_row + self.total_scroll_top();
        let Some(widget_event) = map_magnify_event(&node, scrolled_col, scrolled_row, delta) else {
            return;
        };
        let output = handle_event(&node, widget_event);
        let _ = self.apply_widget_output(output);
    }

    pub(super) fn handle_touchpad_scroll_impl(
        &mut self,
        content_col: u16,
        content_row: u16,
        precise_col: f32,
        precise_row: f32,
        delta_x: f32,
        delta_y: f32,
    ) -> bool {
        let Some((local_col, local_row)) =
            hit::to_local(precise_col, precise_row, content_col, content_row)
        else {
            return false;
        };
        let Some(node) = self.widget_node_at_local(local_col, local_row) else {
            return false;
        };
        let scrolled_col = local_col + self.active_leaf().widget_scroll_left as f32;
        let scrolled_row = local_row + self.total_scroll_top();
        let Some(widget_event) =
            map_scroll_gesture_event(&node, scrolled_col, scrolled_row, delta_x, delta_y)
        else {
            return captures_scroll_gesture(&node);
        };
        let output = handle_event(&node, widget_event);
        let _ = self.apply_widget_output(output);
        true
    }
}
