use crate::layout::Rect;

#[cfg(target_os = "macos")]
use super::super::text_input::closest_char_index_for_x;
use super::super::text_input::{TextInputState, selection_range as text_selection_range};
use super::metrics::{MIN_ZOOM, NODE_FONT_SIZE, NODE_TEXT_COL_OFFSET};
use super::project::{
    OperatorDocumentation, dgenlisp_operator_documentation, dgenlisp_operator_names,
};
use super::state::{
    PatcherInteractionState, PatcherNodeOrigin, PatcherTextEdit, debug_log_edit_event,
    node_edit_key,
};

pub(super) const PATCHER_AUTOCOMPLETE_MAX_ITEMS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PatcherAutocompleteSuggestion {
    pub(super) name: String,
    pub(super) documentation: Option<OperatorDocumentation>,
}

fn patcher_text_char_width_cells() -> f32 {
    (NODE_FONT_SIZE * 0.55 / 10.0).max(0.25)
}

pub(super) fn patcher_text_cursor_at_col(rect: Rect, text: &str, local_col: f32) -> usize {
    patcher_text_cursor_at_col_with_cell_width(rect, text, local_col, 10.0, 1.0)
}

pub(super) fn patcher_text_cursor_at_col_with_zoom(
    rect: Rect,
    text: &str,
    local_col: f32,
    zoom: f32,
) -> usize {
    patcher_text_cursor_at_col_with_cell_width(rect, text, local_col, 10.0, zoom)
}

pub(super) fn patcher_text_cursor_at_col_with_cell_width(
    rect: Rect,
    text: &str,
    local_col: f32,
    cell_w: f32,
    zoom: f32,
) -> usize {
    let zoom = zoom.max(MIN_ZOOM);
    let target = ((local_col - rect.col - NODE_TEXT_COL_OFFSET * zoom) / zoom).max(0.0);
    #[cfg(target_os = "macos")]
    {
        closest_char_index_for_x(text, NODE_FONT_SIZE, target, cell_w).min(text.chars().count())
    }
    #[cfg(not(target_os = "macos"))]
    {
        ((target / (patcher_text_char_width_cells() * zoom))
            .round()
            .max(0.0) as usize)
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
        autocomplete_selected: 0,
    });
    state.hover_back_button = false;
    debug_log_edit_event("begin-text-edit", state);
}

pub(super) fn patcher_autocomplete_matches(edit: &PatcherTextEdit) -> Vec<String> {
    patcher_autocomplete_suggestions(edit)
        .into_iter()
        .map(|suggestion| suggestion.name)
        .collect()
}

pub(super) fn patcher_autocomplete_suggestions(
    edit: &PatcherTextEdit,
) -> Vec<PatcherAutocompleteSuggestion> {
    let Some(prefix) = autocomplete_prefix(&edit.text) else {
        return Vec::new();
    };
    let prefix = prefix.to_lowercase();
    let mut matches: Vec<String> = dgenlisp_operator_names()
        .iter()
        .filter(|name| name.to_lowercase().starts_with(&prefix))
        .cloned()
        .collect();
    matches.sort_by(|left, right| {
        left.to_lowercase()
            .cmp(&right.to_lowercase())
            .then_with(|| left.cmp(right))
    });
    matches.truncate(PATCHER_AUTOCOMPLETE_MAX_ITEMS);
    let docs = dgenlisp_operator_documentation();
    matches
        .into_iter()
        .map(|name| PatcherAutocompleteSuggestion {
            documentation: docs.get(&name).cloned(),
            name,
        })
        .collect()
}

pub(super) fn patcher_autocomplete_is_open(edit: &PatcherTextEdit) -> bool {
    !patcher_autocomplete_matches(edit).is_empty()
}

pub(super) fn move_patcher_autocomplete_selection(
    edit: &mut PatcherTextEdit,
    delta: isize,
) -> bool {
    let matches = patcher_autocomplete_matches(edit);
    if matches.is_empty() {
        edit.autocomplete_selected = 0;
        return false;
    }
    let len = matches.len() as isize;
    let current = edit.autocomplete_selected.min(matches.len() - 1) as isize;
    edit.autocomplete_selected = (current + delta).rem_euclid(len) as usize;
    true
}

pub(super) fn apply_patcher_autocomplete(edit: &mut PatcherTextEdit) -> bool {
    let matches = patcher_autocomplete_matches(edit);
    if matches.is_empty() {
        edit.autocomplete_selected = 0;
        return false;
    }
    let selected = edit.autocomplete_selected.min(matches.len() - 1);
    let replacement = &matches[selected];
    let Some((start, end)) = first_token_byte_span(&edit.text) else {
        return false;
    };
    let mut completed = String::with_capacity(edit.text.len() + replacement.len() + 1);
    completed.push_str(&edit.text[..start]);
    completed.push_str(replacement);
    let has_suffix = edit.text[end..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace);
    if !has_suffix {
        completed.push(' ');
    }
    completed.push_str(&edit.text[end..]);
    edit.text = completed;
    edit.state.cursor_pos = edit.text[..start].chars().count() + replacement.chars().count() + 1;
    edit.state.selection_anchor = None;
    edit.state.selecting = false;
    edit.autocomplete_selected = 0;
    true
}

pub(super) fn clamp_patcher_autocomplete_selection(edit: &mut PatcherTextEdit) {
    let len = patcher_autocomplete_matches(edit).len();
    if len == 0 {
        edit.autocomplete_selected = 0;
    } else {
        edit.autocomplete_selected = edit.autocomplete_selected.min(len - 1);
    }
}

fn autocomplete_prefix(text: &str) -> Option<&str> {
    let (start, end) = first_token_byte_span(text)?;
    if text[end..].chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let prefix = &text[start..end];
    if prefix.is_empty() {
        None
    } else {
        Some(prefix)
    }
}

fn first_token_byte_span(text: &str) -> Option<(usize, usize)> {
    let start = text
        .char_indices()
        .find_map(|(idx, ch)| (!ch.is_whitespace()).then_some(idx))?;
    let end = text[start..]
        .char_indices()
        .find_map(|(idx, ch)| ch.is_whitespace().then_some(start + idx))
        .unwrap_or(text.len());
    Some((start, end))
}

pub(super) fn commit_patcher_text_edit(
    state: &mut PatcherInteractionState,
    view_key: &str,
) -> bool {
    let Some(edit) = state.text_edit.take() else {
        return false;
    };
    let committed_text = edit.text.trim().to_string();
    let key = node_edit_key(view_key, &edit.node_id);
    let Some(node_edit) = state.edit_state.nodes.get_mut(&key) else {
        return false;
    };
    let changed = node_edit.text != committed_text;
    node_edit.text = committed_text;
    if node_edit.text.is_empty() && matches!(node_edit.origin, PatcherNodeOrigin::Created { .. }) {
        state.edit_state.nodes.remove(&key);
        state.selected_nodes.remove(&edit.node_id);
        debug_log_edit_event(
            &format!(
                "commit-text-edit view={view_key} node={} removed-empty-created",
                edit.node_id
            ),
            state,
        );
        return true;
    }
    debug_log_edit_event(
        &format!(
            "commit-text-edit view={view_key} node={} changed={changed}",
            edit.node_id
        ),
        state,
    );
    changed
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
    debug_log_edit_event(
        &format!("cancel-text-edit view={view_key} node={}", edit.node_id),
        state,
    );
}

pub(super) fn update_patcher_text_edit_pointer(
    edit: &mut PatcherTextEdit,
    rect: Rect,
    local_col: f32,
    zoom: f32,
    selecting: bool,
    release: bool,
) {
    let cursor_pos =
        patcher_text_cursor_at_col_with_cell_width(rect, &edit.text, local_col, 10.0, zoom);
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
