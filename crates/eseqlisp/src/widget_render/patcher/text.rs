use crate::layout::Rect;

#[cfg(target_os = "macos")]
use super::super::text_input::closest_char_index_for_x;
use super::super::text_input::{TextInputState, selection_range as text_selection_range};
use super::metrics::{NODE_FONT_SIZE, NODE_TEXT_COL_OFFSET};
use super::state::{PatcherInteractionState, PatcherNodeOrigin, PatcherTextEdit, node_edit_key};

fn patcher_text_char_width_cells() -> f32 {
    (NODE_FONT_SIZE * 0.55 / 10.0).max(0.25)
}

pub(super) fn patcher_text_cursor_at_col(rect: Rect, text: &str, local_col: f32) -> usize {
    patcher_text_cursor_at_col_with_cell_width(rect, text, local_col, 10.0)
}

pub(super) fn patcher_text_cursor_at_col_with_cell_width(
    rect: Rect,
    text: &str,
    local_col: f32,
    cell_w: f32,
) -> usize {
    let target = (local_col - rect.col - NODE_TEXT_COL_OFFSET).max(0.0);
    #[cfg(target_os = "macos")]
    {
        closest_char_index_for_x(text, NODE_FONT_SIZE, target, cell_w).min(text.chars().count())
    }
    #[cfg(not(target_os = "macos"))]
    {
        ((target / patcher_text_char_width_cells()).round().max(0.0) as usize)
            .min(text.chars().count())
    }
}

pub(super) fn begin_patcher_text_edit(
    state: &mut PatcherInteractionState,
    node_id: String,
    text: String,
    cursor_pos: usize,
) {
    state.selected_nodes.clear();
    state.selected_nodes.insert(node_id.clone());
    state.selected_cable = None;
    state.drag = None;
    state.text_edit = Some(PatcherTextEdit {
        node_id,
        text: text.clone(),
        original_text: text,
        state: TextInputState {
            cursor_pos,
            selection_anchor: None,
            selecting: false,
        },
    });
    state.hover_back_button = false;
}

pub(super) fn commit_patcher_text_edit(state: &mut PatcherInteractionState, view_key: &str) {
    let Some(edit) = state.text_edit.take() else {
        return;
    };
    let committed_text = edit.text.trim().to_string();
    let key = node_edit_key(view_key, &edit.node_id);
    let Some(node_edit) = state.edit_state.nodes.get_mut(&key) else {
        return;
    };
    node_edit.text = committed_text;
    if node_edit.text.is_empty() && matches!(node_edit.origin, PatcherNodeOrigin::Created { .. }) {
        state.edit_state.nodes.remove(&key);
        state.selected_nodes.remove(&edit.node_id);
    }
}

pub(super) fn cancel_patcher_text_edit(state: &mut PatcherInteractionState, view_key: &str) {
    let Some(edit) = state.text_edit.take() else {
        return;
    };
    let key = node_edit_key(view_key, &edit.node_id);
    if let Some(node_edit) = state.edit_state.nodes.get(&key)
        && matches!(node_edit.origin, PatcherNodeOrigin::Created { .. })
        && node_edit.text.is_empty()
    {
        state.edit_state.nodes.remove(&key);
        state.selected_nodes.remove(&edit.node_id);
    }
}

pub(super) fn update_patcher_text_edit_pointer(
    edit: &mut PatcherTextEdit,
    rect: Rect,
    local_col: f32,
    selecting: bool,
    release: bool,
) {
    let cursor_pos = patcher_text_cursor_at_col(rect, &edit.text, local_col);
    if selecting {
        if edit.state.selection_anchor.is_none() {
            edit.state.selection_anchor = Some(edit.state.cursor_pos);
        }
        edit.state.selecting = true;
    }
    edit.state.cursor_pos = cursor_pos;
    if release {
        edit.state.selecting = false;
        if text_selection_range(&edit.state).is_none() {
            edit.state.selection_anchor = None;
        }
    }
}
