use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::layout::LayoutNode;
use crate::ui::platform::{
    has_primary_shortcut_modifier, ShortcutPlatform, CURRENT_SHORTCUT_PLATFORM,
};
use crate::vm::Value;
use crate::widget_render::{WidgetEvent, WidgetKeyEvent, handle_event, map_key_event};

use super::{Editor, ViewMode, key_str};

impl Editor {
    pub(super) fn set_focused_widget(&mut self, node: LayoutNode) {
        // A relayout can hand the same logical widget a fresh widget_id; the
        // post-layout remap then re-focuses it here. That is not a new focus:
        // re-applying select-all-on-focus would select what was just typed and
        // make the next keystroke replace it.
        let previous_id = self.active_leaf().focused_widget_id;
        let remapped_same_widget = previous_id != Some(node.widget_id)
            && self
                .active_leaf()
                .focused_widget_node
                .as_ref()
                .is_some_and(|previous| same_focus_identity(previous, &node));
        let newly_focused = previous_id != Some(node.widget_id) && !remapped_same_widget;
        if node.widget_type == "text-input" {
            if newly_focused
                && matches!(
                    node.props.get("select-all-on-focus"),
                    Some(Value::Bool(true))
                )
            {
                let text = crate::widget_render::text_input::get_text(&node.props);
                crate::widget_render::text_input::select_all(node.widget_id, &text);
            } else if remapped_same_widget && let Some(previous_id) = previous_id {
                // Keep the caret/selection the user already has under the new id.
                crate::widget_render::text_input::transfer_state(previous_id, node.widget_id);
            }
        }
        let leaf = self.active_leaf_mut();
        leaf.focused_widget_id = Some(node.widget_id);
        leaf.focused_widget_node = Some(node);
    }

    pub(super) fn clear_focused_widget(&mut self) {
        let leaf = self.active_leaf_mut();
        leaf.focused_widget_id = None;
        leaf.focused_widget_node = None;
    }

    fn blur_focused_widget(&mut self) {
        let callback = self
            .focused_widget_node()
            .and_then(|node| node.props.get("on-blur").cloned())
            .filter(|value| !matches!(value, Value::Nil | Value::Bool(false)));
        self.clear_focused_widget();
        if let Some(callback) = callback {
            self.sync_runtime_source_context();
            if let Err(error) = self.runtime.invoke(callback, Vec::new()) {
                self.minibuffer = Some(format!("Error: {error:?}"));
            }
            self.refresh_runtime_side_effects();
            self.sync_runtime_context();
        }
    }

    /// Drop widget focus on every tile, firing the active tile's `:on-blur`.
    ///
    /// Hosts call this when the transport starts or stops: a click leaves a
    /// widget focused, and the play/record shortcut pressed seconds later
    /// must not keep typing into it. While a modal is open focus is trapped
    /// inside it deliberately, so it is left alone.
    pub fn blur_all_widget_focus(&mut self) {
        if self.modal_is_open() {
            return;
        }
        if self.active_leaf().focused_widget_id.is_some() {
            self.blur_focused_widget();
            self.mark_needs_redraw();
        }
        // Non-active tiles can hold stale focus that becomes live again when
        // their tile is re-activated; clear those too (no on-blur: their
        // widgets are not receiving input).
        self.clear_focus_on_other_tiles();
    }

    pub fn focused_widget_node(&self) -> Option<LayoutNode> {
        let focused_id = self.active_leaf().focused_widget_id?;
        if let Some(layout) = self.active_leaf().cached_layout.as_ref()
            && let Some(node) = find_node_by_id_ref(layout, focused_id)
        {
            return Some(node.clone());
        }
        if let Some(layout) = self.runtime.current_layout.as_ref()
            && let Some(node) = find_node_by_id_ref(layout, focused_id)
        {
            return Some(node.clone());
        }
        self.active_leaf()
            .focused_widget_node
            .clone()
            .filter(|node| node.widget_id == focused_id)
    }

    pub fn focus_widget_by_stable_key(
        &mut self,
        stable_key: &str,
        widget_type: Option<&str>,
    ) -> bool {
        let node = self
            .runtime
            .current_layout
            .as_ref()
            .and_then(|layout| find_focusable_node_by_stable_key(layout, stable_key, widget_type))
            .or_else(|| {
                self.active_leaf()
                    .cached_layout
                    .as_ref()
                    .and_then(|layout| {
                        find_focusable_node_by_stable_key(layout, stable_key, widget_type)
                    })
            });
        let Some(node) = node else {
            return false;
        };
        self.set_focused_widget(node);
        self.clear_focus_on_other_tiles();
        self.mark_needs_redraw();
        true
    }

    pub(super) fn remap_focused_widget_after_layout_change(&mut self) {
        self.sync_modal_focus_state();
        let Some(layout) = self.runtime.current_layout.clone() else {
            self.clear_focused_widget();
            return;
        };
        // `:auto-focus` is a one-shot per appearance of the widget: honouring
        // it marks it consumed, so a deliberate focus-clear (dismissing an
        // inline rename, Esc) stays cleared across the relayouts that follow.
        // The mark is dropped as soon as the auto-focus widget disappears or a
        // different one takes its place, so the next genuine appearance fires
        // again. Kept in sync on every remap, not just the unfocused path.
        let mut consumed_auto_focus = self.active_leaf().consumed_auto_focus.clone();
        let pending_auto_focus = pending_auto_focus_target(&layout, &mut consumed_auto_focus);
        let Some(previous) = self.active_leaf().focused_widget_node.clone() else {
            if let Some(node) = pending_auto_focus {
                consumed_auto_focus = Some(AutoFocusMark::of(&node));
                self.active_leaf_mut().consumed_auto_focus = consumed_auto_focus;
                self.set_focused_widget(node);
                self.clear_focus_on_other_tiles();
                self.mark_needs_redraw();
            } else {
                self.active_leaf_mut().consumed_auto_focus = consumed_auto_focus;
            }
            return;
        };
        let remapped = previous
            .stable_widget_id
            .and_then(|stable_id| find_focusable_node_by_stable_widget_id(&layout, stable_id))
            .or_else(|| {
                previous.stable_key.as_deref().and_then(|stable_key| {
                    find_focusable_node_by_stable_key_and_type(
                        &layout,
                        stable_key,
                        &previous.widget_type,
                    )
                })
            })
            .or_else(|| {
                previous.subtree_root_id.and_then(|subtree_root_id| {
                    find_focusable_node_by_subtree_root_and_type(
                        &layout,
                        subtree_root_id,
                        &previous.widget_type,
                    )
                })
            })
            .or_else(|| {
                self.active_leaf()
                    .focused_widget_id
                    .and_then(|id| find_node_by_id(&layout, id))
                    .filter(|node| same_focus_identity(&previous, node))
            });
        // A failed remap means the widget the user was on is gone: leave focus
        // cleared rather than redirecting it into an unrelated `:auto-focus`
        // widget that happens to be on screen.
        if let Some(node) = remapped {
            self.set_focused_widget(node);
        } else {
            self.clear_focused_widget();
        }
        // Persist the re-armed/cleared one-shot even on the focused path, so a
        // vanished or replaced auto-focus target fires again next appearance.
        self.active_leaf_mut().consumed_auto_focus = consumed_auto_focus;
    }

    /// Ensure only the active tile has widget focus (clear focus on all others).
    fn clear_focus_on_other_tiles(&mut self) {
        let active = self.active_tile;
        self.tile_root.clear_focus_except(active);
    }

    /// Focus trap bookkeeping for the modal overlay. Called after every
    /// layout change: when an open modal appears, remember the previously
    /// focused widget and focus the first focusable child inside the modal;
    /// when it closes, restore the remembered focus (if the widget still
    /// exists). While open, focus that escaped the modal subtree (e.g. via a
    /// stale remap) is pulled back inside.
    pub(super) fn sync_modal_focus_state(&mut self) {
        let modal = self
            .runtime
            .current_layout
            .as_deref()
            .and_then(find_open_modal_node)
            .cloned();
        match (modal, self.modal_focus_return.is_some()) {
            (Some(modal), false) => {
                self.modal_focus_return = Some(self.focused_widget_node());
                self.focus_first_focusable_in(&modal);
                self.mark_needs_redraw();
            }
            (None, true) => {
                let previous = self.modal_focus_return.take().flatten();
                let restored = previous.as_ref().and_then(|previous| {
                    let layout = self.runtime.current_layout.as_deref()?;
                    previous
                        .stable_widget_id
                        .and_then(|id| find_focusable_node_by_stable_widget_id(layout, id))
                        .or_else(|| {
                            previous.stable_key.as_deref().and_then(|key| {
                                find_focusable_node_by_stable_key_and_type(
                                    layout,
                                    key,
                                    &previous.widget_type,
                                )
                            })
                        })
                        .or_else(|| {
                            find_node_by_id(layout, previous.widget_id)
                                .filter(|node| same_focus_identity(previous, node))
                        })
                });
                match restored {
                    Some(node) => self.set_focused_widget(node),
                    None => self.clear_focused_widget(),
                }
                self.mark_needs_redraw();
            }
            (Some(modal), true) => {
                if let Some(focused_id) = self.active_leaf().focused_widget_id
                    && !crate::layout::layout_contains_widget_id(&modal, focused_id)
                {
                    self.focus_first_focusable_in(&modal);
                    self.mark_needs_redraw();
                }
            }
            (None, false) => {}
        }
    }

    fn focus_first_focusable_in(&mut self, root: &LayoutNode) {
        let mut focusable = Vec::new();
        collect_focusable_nodes(root, &mut focusable);
        let first = focusable
            .first()
            .and_then(|(id, ..)| find_node_by_id(root, *id));
        match first {
            Some(node) => {
                self.set_focused_widget(node);
                self.clear_focus_on_other_tiles();
            }
            None => self.clear_focused_widget(),
        }
    }

    /// Invoke the modal's `:on-close` handler (a request to close — the app
    /// closes by flipping the `:is-open` binding). Consumes the trigger even
    /// when no handler is present.
    pub(super) fn fire_modal_on_close(&mut self, modal_widget_id: u64) {
        let Some(layout) = self.runtime.current_layout.clone() else {
            return;
        };
        // Stale-id fallback: a relayout can reassign widget ids before the
        // next render refreshes the overlay entry. There is at most one open
        // modal, so fall back to it by type; only a layout with no open modal
        // at all means the entry is truly dead and gets dropped.
        let Some(node) = find_node_by_id(&layout, modal_widget_id)
            .or_else(|| find_open_modal_node(&layout).cloned())
        else {
            crate::widget_render::remove_overlay(modal_widget_id);
            self.mark_needs_redraw();
            return;
        };
        let Some(callback) = node.props.get("on-close").cloned() else {
            return;
        };
        self.sync_runtime_source_context();
        let result = self.runtime.invoke(callback, vec![]);
        if let Some(status) = self.runtime.take_status_message() {
            self.minibuffer = Some(status);
        } else if let Err(error) = result {
            self.minibuffer = Some(format!("Error: {error:?}"));
        }
        self.refresh_runtime_side_effects();
        self.sync_runtime_context();
        self.mark_needs_redraw();
    }

    /// Focus the focusable widget at a point within the modal subtree (used
    /// by the pointer intercept, which bypasses the normal focus-click path).
    pub(super) fn focus_modal_child_at(&mut self, modal_node: &LayoutNode, row: f32, col: f32) {
        if let Some(node) = crate::ui::layout::hit_test_focusable(modal_node, row, col).cloned() {
            self.set_focused_widget(node);
            self.clear_focus_on_other_tiles();
            self.mark_needs_redraw();
        }
    }

    /// Dismiss one overlay entry: dropdowns close directly; modal-family
    /// overlays get an `:on-close` request (the app flips the binding).
    pub(super) fn dismiss_overlay_entry(&mut self, entry: crate::widget_render::OverlayEntry) {
        match entry.kind {
            crate::widget_render::OverlayKind::Dropdown => {
                crate::widget_render::dropdown::close_dropdown(entry.widget_id);
                crate::widget_render::remove_overlay(entry.widget_id);
            }
            crate::widget_render::OverlayKind::Modal => {
                // Request to close; the app flips the :is-open binding.
                self.fire_modal_on_close(entry.widget_id);
            }
        }
    }

    pub(super) fn try_click_focusable_widget(
        &mut self,
        precise_col: f32,
        precise_row: f32,
        content_col: u16,
        content_row: u16,
    ) -> bool {
        // If an overlay is active and the click is inside the topmost entry,
        // skip focus so the overlay intercept handles it (the modal intercept
        // does its own subtree focus click). If outside, dismiss the topmost
        // entry only — a dropdown above a modal closes first, the modal
        // survives — and consume the click so it cannot activate the widget
        // underneath in the same mouse-down.
        if let Some(entry) = crate::widget_render::topmost_overlay() {
            let local_row = precise_row - content_row as f32;
            let local_col = precise_col - content_col as f32;
            if crate::widget_render::overlay_contains(local_col, local_row) {
                return false;
            }
            self.dismiss_overlay_entry(entry);
            self.mark_needs_redraw();
            return true;
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

        // Find the focusable widget at this position. The hit test is
        // scroll-aware (crate::ui::layout::hit_test_focusable): a widget
        // inside a scrolled container is matched at its RENDERED position,
        // not its unscrolled layout rect.
        if let Some(node) =
            crate::ui::layout::hit_test_focusable(&layout, local_row, local_col).cloned()
        {
            if self.active_leaf().focused_widget_id != Some(node.widget_id) {
                self.blur_focused_widget();
            }
            self.set_focused_widget(node);
            self.clear_focus_on_other_tiles();
            self.mark_needs_redraw();
            return self.activate_focused();
        }
        // Click landed outside any focusable widget → blur
        if self.active_leaf().focused_widget_id.is_some() {
            self.blur_focused_widget();
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
                self.clear_focused_widget();
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
    pub(super) fn dispatch_focus_key(&mut self, key: KeyEvent) -> bool {
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
        self.sync_runtime_source_context();
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
        let Some(node) = self.focused_widget_node() else {
            return false;
        };
        // Space bar should only be consumed by text-entry widgets (for typing).
        // All other widgets let space fall through to keybindings.
        let is_text_input = node.widget_type == "text-input"
            || node.widget_type == "textbox"
            || crate::widget_render::patcher::patcher_has_text_edit(&node);
        if key.code == KeyCode::Char(' ') && !is_text_input {
            return false;
        }
        let gen_before = crate::widget_render::widget_state_generation();
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
        if crate::widget_render::widget_state_generation() != gen_before {
            self.runtime.invalidate_layout_deferred();
            self.mark_needs_redraw();
        }
        true
    }

    pub(super) fn handle_visible_patcher_selected_cable_shortcut(&mut self, key: KeyEvent) -> bool {
        if !matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y'))
            || !has_primary_shortcut_modifier(key.modifiers)
        {
            return false;
        }
        eprintln!(
            "[patcher cmd-y] editor received shortcut active-buffer={} focused-widget={:?}",
            self.active_buffer().name,
            self.active_leaf().focused_widget_id
        );
        if self.focused_widget_captures_text_input()
            || (CURRENT_SHORTCUT_PLATFORM != ShortcutPlatform::MacOS
                && self.active_buffer().view_mode == ViewMode::TextOnly)
        {
            eprintln!("[patcher cmd-y] ignored: active context is capturing text input");
            return false;
        }
        let Some(layout) = self.runtime.current_layout.clone() else {
            eprintln!("[patcher cmd-y] ignored: no current widget layout");
            return false;
        };
        let mut patchers = Vec::new();
        collect_patcher_nodes_with_selected_cable(&layout, &mut patchers);
        eprintln!(
            "[patcher cmd-y] visible patchers with selected cable={}",
            patchers.len()
        );
        let Some(node) = patchers.into_iter().next() else {
            return false;
        };
        eprintln!(
            "[patcher cmd-y] forwarding to patcher widget_id={} stable_id={:?} path={:?}",
            node.widget_id,
            node.stable_widget_id,
            node.props
                .get("path")
                .or_else(|| node.props.get("file"))
                .cloned()
        );

        let gen_before = crate::widget_render::widget_state_generation();
        let widget_event = map_key_event(
            &node,
            WidgetKeyEvent {
                code: key.code,
                modifiers: key.modifiers,
            },
        );
        let consumed = widget_event.is_some();
        eprintln!("[patcher cmd-y] patcher map_key_event consumed={consumed}");
        let output = widget_event.and_then(|event| handle_event(&node, event));
        if !consumed {
            return false;
        }
        let _ = self.apply_widget_output(output);
        if crate::widget_render::widget_state_generation() != gen_before {
            self.runtime.invalidate_layout_deferred();
            self.mark_needs_redraw();
        }
        true
    }

    /// Cmd+K opens a patcher's agentic bubble. Like the Cmd+Y cable shortcut
    /// above, it addresses the patcher you can see rather than the one you last
    /// clicked, so opening a patch and hitting Cmd+K works without first
    /// clicking the canvas to focus it.
    pub(super) fn handle_visible_patcher_agentic_shortcut(&mut self, key: KeyEvent) -> bool {
        if !matches!(key.code, KeyCode::Char('k') | KeyCode::Char('K'))
            || !has_primary_shortcut_modifier(key.modifiers)
        {
            return false;
        }
        // Text editing keeps Ctrl+K (kill line) on Linux. A focused widget
        // keeps the platform-primary chord for itself on every platform.
        if self.focused_widget_captures_text_input()
            || (CURRENT_SHORTCUT_PLATFORM != ShortcutPlatform::MacOS
                && self.active_buffer().view_mode == ViewMode::TextOnly)
        {
            return false;
        }
        let Some(layout) = self.runtime.current_layout.clone() else {
            return false;
        };
        let mut patchers = Vec::new();
        collect_patcher_nodes(&layout, &mut patchers);
        let Some(node) = patchers.into_iter().next() else {
            return false;
        };
        // Focus follows, so the bubble's own key handling (typing, Enter,
        // Escape) lands on the patcher from here on.
        self.set_focused_widget(node.clone());
        self.clear_focus_on_other_tiles();
        self.forward_key_to_widget(&node, key)
    }

    /// Fire the semantic-change notification for the patcher showing `path`,
    /// exactly as a mouse-drawn cable does.
    ///
    /// The patcher's `:on-change` callback is the only thing that recompiles a
    /// patch, and it only runs off a widget event. An edit the host applied to
    /// the interaction state directly — an agentic connect plan — never passes
    /// through the widget's own event handling, so without this the cables
    /// appear on the canvas and the patch stays silent until the next edit.
    ///
    /// Notification reads interaction state without writing it, so it does not
    /// open a second undo step.
    pub fn notify_patcher_semantic_change(&mut self, path: &std::path::Path) -> bool {
        let Some(layout) = self.runtime.current_layout.clone() else {
            return false;
        };
        let mut patchers = Vec::new();
        collect_patcher_nodes(&layout, &mut patchers);
        let Some(node) = patchers
            .into_iter()
            .find(|node| patcher_node_path(node).is_some_and(|node_path| node_path == path))
        else {
            return false;
        };
        let output = handle_event(
            &node,
            WidgetEvent::Custom(Value::Keyword("semantic-change".to_string())),
        );
        self.apply_widget_output(output);
        self.mark_needs_redraw();
        true
    }

    /// Dispatch `key` to `node` as if it were focused, applying whatever the
    /// widget produced. Returns whether the widget consumed the key.
    fn forward_key_to_widget(&mut self, node: &LayoutNode, key: KeyEvent) -> bool {
        let gen_before = crate::widget_render::widget_state_generation();
        let widget_event = map_key_event(
            node,
            WidgetKeyEvent {
                code: key.code,
                modifiers: key.modifiers,
            },
        );
        let consumed = widget_event.is_some();
        let output = widget_event.and_then(|event| handle_event(node, event));
        if !consumed {
            return false;
        }
        let _ = self.apply_widget_output(output);
        if crate::widget_render::widget_state_generation() != gen_before {
            self.runtime.invalidate_layout_deferred();
            self.mark_needs_redraw();
        }
        true
    }

    /// Whether the focused widget takes numeric text input — digits, `.`, `-`.
    ///
    /// Number pickers and knob-numbers start an edit on the *first* digit, so a
    /// digit-keyed global shortcut (roll rates at the live-keyboard seam) must
    /// not consume the key out from under a focused one. Checking "is it
    /// already editing" is too late: that first digit is exactly the key that
    /// would begin the edit.
    ///
    /// Deliberately declarative rather than asking the widget through
    /// `map_key_event`: widget key handlers mutate edit state, so they must
    /// never be run from a predicate.
    pub fn focused_widget_captures_numeric_input(&self) -> bool {
        self.focused_widget_node().is_some_and(|node| {
            node_captures_text_input(&node)
                || matches!(
                    node.widget_type.as_str(),
                    "number-picker"
                        | "number-picker-tri"
                        | "knob-number"
                        | "knob-number-mod-range"
                )
        })
    }

    /// Whether the focused widget would consume this key itself.
    ///
    /// Text entry is identified declaratively: probing its key handler would
    /// apply the edit to persistent cursor state before the real dispatch.
    /// Other widget key handlers may also mutate interaction state, so callers
    /// must still restrict this probe to keys whose non-text handlers are
    /// known to be no-ops when idle. Backspace/Delete meet that requirement
    /// for the number picker and knob-number. Do not call this with digits or
    /// Enter; use `focused_widget_captures_numeric_input` for numeric input.
    ///
    /// Destructive global shortcuts (Backspace/Delete over a step selection)
    /// must defer to a focused widget only when that widget genuinely handles
    /// the key — a text input, a number-picker or knob mid-edit, a lane with
    /// its own selection. Gating on "something is focused" instead meant that
    /// clicking any button silently disarmed Cmd+A followed by Backspace,
    /// because a button consumes only Enter and Space.
    pub fn focused_widget_consumes_key(
        &self,
        code: crossterm::event::KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        self.focused_widget_node().is_some_and(|node| {
            node_captures_text_input(&node)
                || crate::widget_render::map_key_event(
                    &node,
                    WidgetKeyEvent {
                        code,
                        modifiers,
                    },
                )
                .is_some()
        })
    }

    fn focused_widget_captures_text_input(&self) -> bool {
        self.focused_widget_node()
            .is_some_and(|node| node_captures_text_input(&node))
    }

    pub(super) fn navigate_focus(&mut self, direction: KeyCode) {
        let Some(layout) = self.runtime.current_layout.clone() else {
            return;
        };
        // While a modal is open, focus navigation is trapped inside it.
        let scan_root = find_open_modal_node(&layout).unwrap_or(&layout);
        let mut focusable_nodes: Vec<(u64, f32, f32, f32, f32)> = Vec::new();
        collect_focusable_nodes(scan_root, &mut focusable_nodes);
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
        let mut best: Option<(u64, f32, f32, f32)> = None; // (id, row, height, score)

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

            if best.is_none() || score < best.unwrap().3 {
                best = Some((id, row, h, score));
            }
        }

        if let Some((next_id, next_row, next_height, _)) = best {
            if let Some(node) = find_node_by_id(&layout, next_id) {
                self.set_focused_widget(node);
            } else {
                self.active_leaf_mut().focused_widget_id = Some(next_id);
            }
            self.clear_focus_on_other_tiles();
            self.adjust_widget_scroll(next_row, next_height);
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
                let max_scroll = self.max_widget_vertical_scroll();
                let leaf = self.active_leaf_mut();
                leaf.widget_scroll_top = (leaf.widget_scroll_top + 3.0).min(max_scroll).max(0.0);
                self.mark_needs_redraw();
            }
            _ => {}
        }
    }

    pub(super) fn adjust_widget_scroll(&mut self, focused_row: f32, focused_height: f32) {
        let viewport_height = self.active_leaf().widget_viewport_height.max(0.0);
        let viewport_height = if viewport_height > 0.0 {
            viewport_height
        } else {
            self.runtime.layout_rows() as f32
        };
        if viewport_height <= 0.0 {
            return;
        }
        let focused_top = focused_row.floor();
        let focused_bottom = (focused_row + focused_height).ceil();
        let leaf = self.active_leaf_mut();
        if focused_top < leaf.widget_scroll_top {
            leaf.widget_scroll_top = focused_top.max(0.0);
        }
        if focused_bottom > leaf.widget_scroll_top + viewport_height {
            leaf.widget_scroll_top = (focused_bottom - viewport_height).max(0.0);
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
            self.sync_runtime_source_context();
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
        if let Some(runtime_generation) = self.runtime.current_committed_ui_snapshot_generation() {
            let Some(snapshot) = self.runtime.current_committed_ui_snapshot() else {
                return;
            };
            self.active_buffer_mut()
                .adopt_runtime_committed_ui_snapshot(snapshot, runtime_generation);
        } else if let Some(tree) = self.runtime.current_widget_tree() {
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
        leaf.focused_widget_node = None;
        leaf.widget_scroll_top = 0.0;
        leaf.widget_viewport_width = 0.0;
        leaf.widget_viewport_height = 0.0;
        leaf.layout_frame_viewport = None;
        leaf.widget_scroll_left = 0.0;
        leaf.active_widget_gesture = None;
        leaf.cached_layout = None;
        leaf.cached_layout_widget_tree_revision = 0;
    }

    pub(super) fn restore_buffer_widget_tree(&mut self) {
        crate::widget_render::clear_overlay();
        if super::widget_only_scratch_buffer_should_show_ui(self.active_buffer()) {
            self.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
        }
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

    pub(super) fn restore_buffer_widget_tree_with_cached_layout(
        &mut self,
        cached_layout: Option<std::sync::Arc<LayoutNode>>,
        viewport: Option<(f32, f32)>,
        layout_revision: u64,
    ) {
        crate::widget_render::clear_overlay();
        if super::widget_only_scratch_buffer_should_show_ui(self.active_buffer()) {
            self.active_buffer_mut().view_mode = super::ViewMode::UiOnly;
        }
        let buf = self.active_buffer();
        let tree = buf.widget_tree.clone();
        let snapshot = buf.committed_ui_snapshot.clone();
        let buffer_id = buf.id as u64;
        match tree {
            Some(tree) => {
                self.runtime.restore_widget_tree_with_cached_layout(
                    tree,
                    snapshot,
                    cached_layout,
                    viewport,
                    buffer_id * 100_000,
                    layout_revision,
                );
                self.sync_layout_to_active_leaf();
            }
            None => {
                self.clear_widget_focus();
            }
        }
    }
}

fn same_focus_identity(a: &LayoutNode, b: &LayoutNode) -> bool {
    a.focusable
        && b.focusable
        && a.widget_type == b.widget_type
        && ((a.stable_widget_id.is_some() && a.stable_widget_id == b.stable_widget_id)
            || (a.stable_key.is_some() && a.stable_key == b.stable_key)
            || (a.subtree_root_id.is_some() && a.subtree_root_id == b.subtree_root_id))
}

fn find_focusable_node_by_stable_widget_id(
    node: &LayoutNode,
    stable_id: u64,
) -> Option<LayoutNode> {
    if node.focusable && node.stable_widget_id == Some(stable_id) {
        return Some(node.clone());
    }
    for child in &node.children {
        if let Some(found) = find_focusable_node_by_stable_widget_id(child, stable_id) {
            return Some(found);
        }
    }
    None
}

fn find_focusable_node_by_stable_key_and_type(
    node: &LayoutNode,
    stable_key: &str,
    widget_type: &str,
) -> Option<LayoutNode> {
    if node.focusable
        && node.stable_key.as_deref() == Some(stable_key)
        && node.widget_type == widget_type
    {
        return Some(node.clone());
    }
    for child in &node.children {
        if let Some(found) =
            find_focusable_node_by_stable_key_and_type(child, stable_key, widget_type)
        {
            return Some(found);
        }
    }
    None
}

fn find_focusable_node_by_stable_key(
    node: &LayoutNode,
    stable_key: &str,
    widget_type: Option<&str>,
) -> Option<LayoutNode> {
    if node.focusable
        && node.stable_key.as_deref() == Some(stable_key)
        && widget_type.is_none_or(|expected| node.widget_type == expected)
    {
        return Some(node.clone());
    }
    for child in &node.children {
        if let Some(found) = find_focusable_node_by_stable_key(child, stable_key, widget_type) {
            return Some(found);
        }
    }
    None
}

fn find_focusable_node_by_subtree_root_and_type(
    node: &LayoutNode,
    subtree_root_id: u64,
    widget_type: &str,
) -> Option<LayoutNode> {
    if node.focusable
        && node.subtree_root_id == Some(subtree_root_id)
        && node.widget_type == widget_type
    {
        return Some(node.clone());
    }
    for child in &node.children {
        if let Some(found) =
            find_focusable_node_by_subtree_root_and_type(child, subtree_root_id, widget_type)
        {
            return Some(found);
        }
    }
    None
}

/// The open modal-family overlay node (modal or context menu) in a layout,
/// if any (open = it laid out children).
pub(super) fn find_open_modal_node(node: &LayoutNode) -> Option<&LayoutNode> {
    if crate::widget_render::is_overlay_panel_widget(&node.widget_type) && !node.children.is_empty()
    {
        return Some(node);
    }
    node.children.iter().find_map(find_open_modal_node)
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

/// Identity of the `:auto-focus` widget that has already been honoured, so it
/// is not re-focused on every subsequent relayout. Stable across the id churn
/// of a relayout: matched by stable id / stable key / subtree root when the
/// node carries one, by raw widget id otherwise.
#[derive(Clone, PartialEq, Eq)]
pub struct AutoFocusMark {
    stable_widget_id: Option<u64>,
    stable_key: Option<String>,
    subtree_root_id: Option<u64>,
    widget_type: String,
    widget_id: u64,
}

impl AutoFocusMark {
    fn of(node: &LayoutNode) -> Self {
        Self {
            stable_widget_id: node.stable_widget_id,
            stable_key: node.stable_key.clone(),
            subtree_root_id: node.subtree_root_id,
            widget_type: node.widget_type.clone(),
            widget_id: node.widget_id,
        }
    }

    fn matches(&self, node: &LayoutNode) -> bool {
        if self.widget_type != node.widget_type {
            return false;
        }
        if self.stable_widget_id.is_some() {
            return self.stable_widget_id == node.stable_widget_id;
        }
        if self.stable_key.is_some() {
            return self.stable_key == node.stable_key;
        }
        if self.subtree_root_id.is_some() {
            return self.subtree_root_id == node.subtree_root_id;
        }
        self.widget_id == node.widget_id
    }
}

/// The layout's `:auto-focus` widget if it has not been honoured yet.
///
/// Also re-syncs the consumed mark: it is dropped when the auto-focus widget
/// leaves the layout, or when a different widget becomes the auto-focus
/// target, so the one-shot re-arms for the next genuine appearance.
fn pending_auto_focus_target(
    layout: &LayoutNode,
    consumed: &mut Option<AutoFocusMark>,
) -> Option<LayoutNode> {
    match find_auto_focus_node(layout) {
        None => {
            *consumed = None;
            None
        }
        Some(node) => {
            if consumed.as_ref().is_some_and(|mark| mark.matches(&node)) {
                None
            } else {
                *consumed = None;
                Some(node)
            }
        }
    }
}

fn find_auto_focus_node(node: &LayoutNode) -> Option<LayoutNode> {
    if node.focusable && matches!(node.props.get("auto-focus"), Some(Value::Bool(true))) {
        return Some(node.clone());
    }
    node.children.iter().find_map(find_auto_focus_node)
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

fn find_node_by_id_ref(node: &LayoutNode, id: u64) -> Option<&LayoutNode> {
    if node.widget_id == id {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_node_by_id_ref(child, id) {
            return Some(found);
        }
    }
    None
}

fn node_captures_text_input(node: &LayoutNode) -> bool {
    matches!(node.widget_type.as_str(), "text-input" | "textbox")
        || crate::widget_render::patcher::patcher_has_text_edit(node)
}

/// The patch file a patcher node is showing, from the props the widget itself
/// reads.
fn patcher_node_path(node: &LayoutNode) -> Option<&std::path::Path> {
    node.props
        .get("path")
        .or_else(|| node.props.get("file"))
        .and_then(|value| match value {
            Value::String(path) => Some(std::path::Path::new(path.as_str())),
            _ => None,
        })
}

fn collect_patcher_nodes(node: &LayoutNode, out: &mut Vec<LayoutNode>) {
    if node.widget_type == "patcher" {
        out.push(node.clone());
    }
    for child in &node.children {
        collect_patcher_nodes(child, out);
    }
}

fn collect_patcher_nodes_with_selected_cable(node: &LayoutNode, out: &mut Vec<LayoutNode>) {
    if node.widget_type == "patcher"
        && crate::widget_render::patcher::patcher_has_selected_cable(node)
    {
        out.push(node.clone());
    }
    for child in &node.children {
        collect_patcher_nodes_with_selected_cable(child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{Editor, EditorConfig, ViewMode};
    use crate::runtime::Runtime;

    /// An inline-rename style fixture: a plain focusable button plus a
    /// text-input that only exists while `renaming` is true and carries
    /// `:auto-focus`.
    fn rename_fixture_editor() -> Editor {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def renaming (state false))
                (def other-visible (state true))
                (def tick (state 0))
                (effect
                  (v-stack :width 30 :height 8
                    (if other-visible
                      (text-input :key "other" :width 10 :value "Other")
                      (label "other gone"))
                    (if renaming
                      (text-input :key "rename-input" :width 20 :value "Track"
                        :auto-focus true :select-all-on-focus true)
                      (label (str "idle " tick)))))
                "#,
            )
            .expect("build rename fixture");
        editor.refresh_runtime_side_effects();
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor.set_layout_viewport(40, 10);
        editor.refresh_runtime_side_effects();
        editor
    }

    fn relayout(editor: &mut Editor) {
        editor
            .runtime_mut()
            .eval_str("(set! tick (+ tick 1))")
            .expect("bump tick");
        editor.refresh_runtime_side_effects();
        // Belt and braces: exercise the post-layout remap directly, so the
        // test does not depend on which internal path happens to call it.
        editor.remap_focused_widget_after_layout_change();
    }

    fn stable_key_of_focused(editor: &Editor) -> Option<String> {
        editor
            .focused_widget_node()
            .and_then(|node| node.stable_key.clone())
    }

    #[test]
    fn blur_all_widget_focus_clears_focus_and_fires_on_blur() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def blurred (state false))
                (effect
                  (v-stack :width 30 :height 8
                    (text-input :key "field" :width 10 :value "Track"
                      :on-blur (lambda () (set! blurred true)))))
                "#,
            )
            .expect("build blur fixture");
        editor.refresh_runtime_side_effects();
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor.set_layout_viewport(40, 10);
        editor.refresh_runtime_side_effects();

        assert!(
            editor.focus_widget_by_stable_key("field", Some("text-input")),
            "focus the input"
        );

        editor.blur_all_widget_focus();

        assert_eq!(editor.focused_widget_id(), None, "focus must be cleared");
        assert_eq!(
            editor.runtime_mut().eval_str("blurred"),
            Ok(Some(Value::Bool(true))),
            ":on-blur must fire on the host-driven blur"
        );
    }

    #[test]
    fn blur_all_widget_focus_leaves_modal_trapped_focus_alone() {
        let mut editor = Editor::new(Runtime::new(), EditorConfig::default());
        editor
            .runtime_mut()
            .eval_str(
                r#"
                (def modal-open (state true))
                (effect
                  (v-stack :width 30 :height 10
                    (text-input :key "behind" :width 10 :value "x")
                    (modal :is-open modal-open
                      (v-stack
                        (text-input :key "inside" :width 10 :value "y")))))
                "#,
            )
            .expect("build modal fixture");
        editor.refresh_runtime_side_effects();
        editor.active_buffer_mut().view_mode = ViewMode::UiOnly;
        editor.set_layout_viewport(40, 12);
        editor.refresh_runtime_side_effects();

        assert!(editor.modal_is_open(), "precondition: modal is open");
        assert_eq!(
            stable_key_of_focused(&editor).as_deref(),
            Some("inside"),
            "precondition: focus trapped in the modal"
        );

        editor.blur_all_widget_focus();

        assert_eq!(
            stable_key_of_focused(&editor).as_deref(),
            Some("inside"),
            "modal focus trap must survive a transport-driven blur"
        );
    }

    #[test]
    fn auto_focus_fires_on_first_appearance() {
        let mut editor = rename_fixture_editor();
        assert_eq!(
            editor.focused_widget_id(),
            None,
            "nothing has auto-focus before the rename input appears"
        );

        editor
            .runtime_mut()
            .eval_str("(set! renaming true)")
            .expect("open rename");
        editor.refresh_runtime_side_effects();

        assert_eq!(
            stable_key_of_focused(&editor).as_deref(),
            Some("rename-input"),
            "auto-focus must focus the rename input the first time it appears"
        );
    }

    #[test]
    fn cleared_focus_stays_cleared_across_relayout() {
        let mut editor = rename_fixture_editor();
        editor
            .runtime_mut()
            .eval_str("(set! renaming true)")
            .expect("open rename");
        editor.refresh_runtime_side_effects();
        assert_eq!(
            stable_key_of_focused(&editor).as_deref(),
            Some("rename-input"),
            "precondition: auto-focus fired"
        );

        // Deliberate focus-clear (Esc / dismiss), auto-focus widget still on
        // screen.
        editor.clear_focused_widget();
        relayout(&mut editor);

        assert_eq!(
            editor.focused_widget_id(),
            None,
            "auto-focus is a one-shot: a cleared focus must survive relayout"
        );
    }

    #[test]
    fn failed_remap_does_not_steal_focus_into_auto_focus_widget() {
        let mut editor = rename_fixture_editor();
        assert!(
            editor.focus_widget_by_stable_key("other", Some("text-input")),
            "focus the unrelated widget"
        );
        assert_eq!(stable_key_of_focused(&editor).as_deref(), Some("other"));

        // The focused widget disappears in the same relayout that introduces
        // the auto-focus widget: the remap fails and must NOT redirect.
        editor
            .runtime_mut()
            .eval_str("(do (set! other-visible false) (set! renaming true))")
            .expect("swap widgets");
        editor.refresh_runtime_side_effects();

        assert_eq!(
            editor.focused_widget_id(),
            None,
            "a failed remap must leave focus cleared, not jump into :auto-focus"
        );
    }
}
