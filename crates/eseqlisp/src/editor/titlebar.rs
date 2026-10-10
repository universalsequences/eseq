//! The titlebar slot: a UI buffer mounted in the window's titlebar band by
//! `set-titlebar`, outside the user's tile tree (epic eseq-9ocu).
//!
//! The slot is an ordinary `TileLeaf` held beside the split tree
//! (`TileTree::titlebar`), so layout caching, reactive sync and rendering
//! treat it like any inactive tile. What differs is focus. Input is routed
//! to it the way the overlay and modal paths route to an inactive owner: the
//! titlebar is made active for the one event and activity is handed back, so
//! clicking the transport never moves the active tile. A widget that takes
//! input (a number picker, a dropdown) keeps the titlebar's own focus, which
//! then sees each key first; Enter, Escape, Tab, or a click anywhere else
//! releases it. Buttons act without holding focus.

use super::*;
use crate::runtime::TitlebarSpec;

/// Widgets that keep the titlebar's keyboard focus after a click.
fn titlebar_widget_holds_focus(node: &LayoutNode) -> bool {
    widget_focus::node_captures_text_input(node)
        || matches!(
            node.widget_type.as_str(),
            "number-picker" | "number-picker-tri" | "knob-number" | "dropdown"
        )
}

impl Editor {
    /// Name of the buffer mounted in the titlebar, if any.
    pub fn titlebar_buffer_name(&self) -> Option<&str> {
        let leaf = self.tile_root.titlebar.as_ref()?;
        self.buffers.get(leaf.buffer_idx).map(|buffer| buffer.name.as_str())
    }

    pub(super) fn mount_titlebar(&mut self, spec: TitlebarSpec) -> Result<(), String> {
        let buffer_idx = self
            .buffers
            .iter()
            .position(|buffer| buffer.name == spec.buffer)
            .ok_or_else(|| format!("no buffer named '{}'", spec.buffer))?;
        if self.tile_root.content.find_leaf_by_buffer_idx(buffer_idx).is_some() {
            return Err(format!("'{}' is already showing in a tile", spec.buffer));
        }
        // Layout code remounts on every layout change; an unchanged mount is free.
        if self.tile_root.titlebar.as_ref().is_some_and(|leaf| {
            leaf.buffer_idx == buffer_idx
                && leaf.background_color_name == spec.background_color_name
        }) && self.titlebar_height == spec.height.max(0.0)
        {
            return Ok(());
        }
        // Remounting the same buffer keeps its leaf, so its cached layout and
        // widget state survive a height or background change.
        let leaf = match self.tile_root.titlebar.take() {
            Some(leaf) if leaf.buffer_idx == buffer_idx => leaf,
            _ => TileLeaf::new(self.alloc_tile_id(), buffer_idx),
        };
        let mut leaf = leaf;
        leaf.show_status = false;
        leaf.show_border = false;
        leaf.border_width_px = 0.0;
        leaf.border_radius_px = 0.0;
        leaf.background_color_name = spec.background_color_name;
        leaf.cached_inactive_frame = None;
        self.tile_root.titlebar = Some(leaf);
        self.titlebar_height = spec.height.max(0.0);
        self.after_titlebar_change();
        self.refresh_inactive_tile_layouts_for_buffer(buffer_idx);
        Ok(())
    }

    pub(super) fn unmount_titlebar(&mut self) {
        let Some(titlebar_id) = self.tile_root.titlebar_id() else {
            return;
        };
        if self.active_tile == titlebar_id
            && let Some(first) = self.tile_root.content_leaf_ids().first().copied()
        {
            self.switch_active_tile(first);
        }
        self.tile_root.titlebar = None;
        self.titlebar_height = 0.0;
        self.after_titlebar_change();
    }

    fn after_titlebar_change(&mut self) {
        if let Some((width, height)) = self.cached_tiled_frame_size {
            self.update_tile_rects(width as u16, height as u16);
        }
        self.sync_visible_effect_buffers();
        self.mark_needs_redraw();
    }

    /// The titlebar while it holds keyboard focus over the active tile.
    pub(super) fn titlebar_focus_tile(&self) -> Option<TileId> {
        let leaf = self.tile_root.titlebar.as_ref()?;
        (leaf.id != self.active_tile && leaf.focused_widget_id.is_some()).then_some(leaf.id)
    }

    pub(super) fn titlebar_focused_widget_node(&self) -> Option<LayoutNode> {
        self.titlebar_focus_tile()?;
        let leaf = self.tile_root.titlebar.as_ref()?;
        let focused_id = leaf.focused_widget_id?;
        leaf.cached_layout
            .as_deref()
            .and_then(|layout| widget_focus::find_node_by_id(layout, focused_id))
            .or_else(|| leaf.focused_widget_node.clone().filter(|node| node.widget_id == focused_id))
    }

    fn release_titlebar_focus(&mut self) {
        if let Some(leaf) = self.tile_root.titlebar.as_mut()
            && leaf.focused_widget_id.is_some()
        {
            leaf.focused_widget_id = None;
            leaf.focused_widget_node = None;
            leaf.cached_inactive_frame = None;
            self.mark_needs_redraw();
        }
    }

    /// Route a pointer event to the titlebar without making it the active
    /// tile. Returns whether the event was the titlebar's.
    pub(super) fn handle_titlebar_mouse(
        &mut self,
        mouse: MouseEvent,
        precise_col: f32,
        precise_row: f32,
        border_inset: u16,
    ) -> bool {
        let Some(titlebar_id) = self.tile_root.titlebar_id() else {
            return false;
        };
        let captured = self
            .tile_root
            .titlebar
            .as_ref()
            .is_some_and(|leaf| leaf.active_widget_gesture.is_some());
        let over = self.tile_at_screen(precise_col, precise_row) == Some(titlebar_id);
        let ours = match mouse.kind {
            // A gesture that began in the titlebar keeps its drag and release
            // there, wherever the pointer goes.
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left) => {
                captured || (over && self.active_leaf().active_widget_gesture.is_none())
            }
            MouseEventKind::Down(MouseButton::Left) if !over => {
                // A click anywhere else gives the keyboard back.
                self.release_titlebar_focus();
                false
            }
            MouseEventKind::Down(_)
            | MouseEventKind::Up(_)
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => over,
            // Hover over an inactive tile already has its own path.
            _ => false,
        };
        if !ours {
            return false;
        }
        self.route_event_to_tile(titlebar_id, border_inset, false, |editor| {
            let Some((content_col, content_row, content_width, content_height)) =
                editor.tile_content_area(titlebar_id, border_inset)
            else {
                return;
            };
            let (event_col, event_row) = editor
                .tile_content_precise_event_position(
                    titlebar_id,
                    border_inset,
                    content_col,
                    content_row,
                    precise_col,
                    precise_row,
                )
                .unwrap_or((precise_col, precise_row));
            editor.handle_mouse_precise(
                mouse,
                content_col,
                content_row,
                content_width,
                content_height,
                event_col,
                event_row,
            );
            // Buttons act without holding the keyboard.
            if !editor
                .active_focused_widget_node()
                .is_some_and(|node| titlebar_widget_holds_focus(&node))
            {
                editor.clear_focused_widget();
            }
        });
        self.mark_needs_redraw();
        true
    }

    /// Give a key to the focused titlebar widget first. Returns whether the
    /// key was consumed; an unconsumed key continues to the active tile.
    pub(super) fn handle_titlebar_focus_key(&mut self, key: KeyEvent) -> bool {
        let Some(titlebar_id) = self.titlebar_focus_tile() else {
            return false;
        };
        let releases = matches!(
            key.code,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab
        ) && key.modifiers.difference(KeyModifiers::SHIFT).is_empty();
        let consumed = self.route_event_to_tile(titlebar_id, 0, false, |editor| {
            editor.handle_focused_widget_key(key)
        });
        if releases {
            self.release_titlebar_focus();
            return true;
        }
        consumed
    }

    /// Give the keyboard to a widget of the titlebar when `buffer` is the one
    /// mounted there; the active tile stays active.
    pub fn focus_titlebar_widget_by_stable_key(
        &mut self,
        buffer: &str,
        stable_key: &str,
        widget_type: Option<&str>,
    ) -> bool {
        if self.titlebar_buffer_name() != Some(buffer) {
            return false;
        }
        let Some(titlebar_id) = self.tile_root.titlebar_id() else {
            return false;
        };
        self.route_event_to_tile(titlebar_id, 0, false, |editor| {
            editor.focus_widget_by_stable_key(stable_key, widget_type)
        })
    }

    /// Run an edit-menu action against the focused titlebar widget.
    pub(super) fn titlebar_edit_action(&mut self, action: &str) -> Option<bool> {
        let titlebar_id = self.titlebar_focus_tile()?;
        Some(self.route_event_to_tile(titlebar_id, 0, false, |editor| {
            editor.perform_edit_action(action)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::window_chrome::{self, WindowChrome};

    fn mouse(kind: MouseEventKind, col: f32, row: f32) -> MouseEvent {
        MouseEvent {
            kind,
            column: col as u16,
            row: row as u16,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn click(editor: &mut Editor, col: f32, row: f32) {
        for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
            editor.handle_tiled_mouse_precise(mouse(kind, col, row), col, row, 0);
        }
    }

    fn eval(editor: &mut Editor, source: &str) -> Value {
        editor.runtime_mut().eval_str(source).expect("eval").unwrap_or(Value::Nil)
    }

    /// A 60x20 window with a 2-row native band whose buttons take 5 columns,
    /// `*main*` in the only tile and `*bar*` mounted in the titlebar: a button
    /// at titlebar columns 0..4 and a number picker at 5..11.
    fn editor_with_titlebar() -> Editor {
        window_chrome::set(WindowChrome { titlebar_rows: 2.0, leading_cols: 5.0, content_rows: 0.0 });
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor.set_layout_viewport(60, 20);
        editor.update_tile_rects(60, 20);
        eval(
            &mut editor,
            r#"
            (def clicks 0)
            (def bpm 120)
            (effect-buffer "*main*" (box :key "main-box" :width 20 :height 4))
            (effect-buffer "*bar*"
              (h-stack :width :fill :height :fill :align :center
                (box :key "bar-button" :width 4 :height 1
                  :on-click (lambda (event) (set! clicks (+ clicks 1))))
                (box :width 1)
                (number-picker :key "bar-bpm" :value bpm :min 20 :max 300 :decimals 0
                  :on-change (lambda (v) (set! bpm v))
                  :width 6 :height 1)))
            (set-window-buffer "*main*")
            (set-titlebar "*bar*" :height 1.5)
            "#,
        );
        editor.refresh_runtime_side_effects();
        let _ = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);
        editor
    }

    fn reset_chrome() {
        window_chrome::set(WindowChrome::default());
    }

    #[test]
    fn titlebar_sits_in_the_band_beside_the_window_buttons() {
        let mut editor = editor_with_titlebar();
        let titlebar_id = editor.tile_root.titlebar_id().expect("titlebar mounted");
        assert_eq!(editor.titlebar_buffer_name(), Some("*bar*"));

        // The native band (2 rows) outranks the requested 1.5.
        let rect = editor.tile_rect(titlebar_id).expect("titlebar rect");
        assert_eq!((rect.row, rect.col, rect.height), (0.0, 5.0, 2.0));
        assert_eq!(rect.width, 55.0);
        let content = editor.tile_rect(editor.active_tile).expect("content rect");
        assert_eq!(content.row, 2.0, "user tiles start below the band");

        // Not a user tile: cycling and tile counts never see it.
        assert_eq!(editor.tile_root.leaf_count(), 1);
        assert!(!editor.tile_root.content_leaf_ids().contains(&titlebar_id));

        let frame = crate::ui::frame::build_tiled_render_frame_borderless(&mut editor, 60, 20);
        assert_eq!(frame.titlebar_tile, Some(titlebar_id));
        let tile = frame.tiles.iter().find(|tile| tile.tile_id == titlebar_id).unwrap();
        assert!(!tile.is_active && !tile.show_border && !tile.show_status);
        assert!(tile.frame.widget_layout.is_some(), "the titlebar buffer is laid out");

        eval(&mut editor, "(set-titlebar nil)");
        editor.refresh_runtime_side_effects();
        assert!(editor.tile_root.titlebar.is_none());
        assert_eq!(editor.tile_rect(editor.active_tile).unwrap().row, 2.0, "the native band stays reserved");
        reset_chrome();
    }

    #[test]
    fn titlebar_buttons_act_without_taking_the_active_tile_or_focus() {
        let mut editor = editor_with_titlebar();
        let active = editor.active_tile;
        click(&mut editor, 6.5, 1.0);
        assert_eq!(eval(&mut editor, "clicks"), Value::Number(1.0));
        assert_eq!(editor.active_tile, active);
        assert_eq!(editor.active_buffer().name, "*main*");
        assert_eq!(editor.focused_widget_id(), None, "a button does not hold the keyboard");
        reset_chrome();
    }

    #[test]
    fn focused_titlebar_picker_sees_keys_first_and_enter_releases_it() {
        let mut editor = editor_with_titlebar();
        let active = editor.active_tile;
        click(&mut editor, 13.0, 1.0);
        assert_eq!(editor.active_tile, active, "focusing the picker keeps the active tile");
        let node = editor.focused_widget_node().expect("the picker owns the keyboard");
        assert_eq!(node.widget_type, "number-picker");
        assert!(editor.focused_widget_captures_numeric_input());

        for key in [KeyCode::Char('9'), KeyCode::Char('0'), KeyCode::Enter] {
            editor.handle_key(KeyEvent::new(key, KeyModifiers::NONE));
        }
        assert_eq!(eval(&mut editor, "bpm"), Value::Number(90.0));
        assert_eq!(editor.focused_widget_id(), None, "Enter hands the keyboard back");
        assert_eq!(editor.active_tile, active);
        reset_chrome();
    }

    #[test]
    fn clicking_a_tile_releases_titlebar_focus() {
        let mut editor = editor_with_titlebar();
        click(&mut editor, 13.0, 1.0);
        assert!(editor.titlebar_focus_tile().is_some());
        click(&mut editor, 30.0, 10.0);
        assert!(editor.titlebar_focus_tile().is_none());
        reset_chrome();
    }

    #[test]
    fn a_buffer_shows_in_a_tile_or_the_titlebar_not_both() {
        let mut editor = editor_with_titlebar();
        eval(&mut editor, r#"(set-titlebar "*main*")"#);
        editor.refresh_runtime_side_effects();
        assert_eq!(editor.titlebar_buffer_name(), Some("*bar*"));
        assert!(editor.minibuffer.as_deref().is_some_and(|m| m.contains("already showing")));
        reset_chrome();
    }
}
