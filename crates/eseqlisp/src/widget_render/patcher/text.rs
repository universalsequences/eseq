use crate::layout::Rect;
use crate::parser::{ASTParser, Expression, Parser, Token};
use std::collections::HashMap;

use super::super::text_input::{TextInputState, selection_range as text_selection_range};
use super::lisp::{attribute_span_len, is_attribute_key};
use super::metrics::{MIN_ZOOM, NODE_FONT_SIZE, NODE_TEXT_COL_OFFSET};
use super::model::MacroPatch;
use super::project::{
    OperatorDocumentation, OperatorPortDocumentation, dgenlisp_operator_attributes,
    dgenlisp_operator_documentation, dgenlisp_operator_names,
};
use super::state::{
    PatcherInteractionState, PatcherNodeOrigin, PatcherTextEdit, debug_log_edit_event,
    node_edit_key, note_touched_node,
};
#[cfg(target_os = "macos")]
use super::text_metrics::measured_closest_char_index;

pub(super) const PATCHER_AUTOCOMPLETE_MAX_ITEMS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PatcherAutocompleteSuggestion {
    pub(super) name: String,
    pub(super) documentation: Option<OperatorDocumentation>,
}

pub(super) fn patcher_text_cursor_at_col(rect: Rect, text: &str, local_col: f32) -> Option<usize> {
    patcher_text_cursor_at_col_with_font_size(rect, text, local_col, NODE_FONT_SIZE, 1.0)
}

pub(super) fn patcher_text_cursor_at_col_with_zoom(
    rect: Rect,
    text: &str,
    local_col: f32,
    font_size: f32,
    zoom: f32,
) -> Option<usize> {
    patcher_text_cursor_at_col_with_font_size(rect, text, local_col, font_size, zoom)
}

fn patcher_text_cursor_at_col_with_font_size(
    rect: Rect,
    text: &str,
    local_col: f32,
    font_size: f32,
    zoom: f32,
) -> Option<usize> {
    let zoom = zoom.max(MIN_ZOOM);
    let target = ((local_col - rect.col - NODE_TEXT_COL_OFFSET * zoom) / zoom).max(0.0);
    #[cfg(target_os = "macos")]
    {
        measured_closest_char_index(text, font_size, target)
            .map(|index| index.min(text.chars().count()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Some((target.round().max(0.0) as usize).min(text.chars().count()))
    }
}

pub(super) fn begin_patcher_text_edit(
    state: &mut PatcherInteractionState,
    node_id: String,
    text: String,
    cursor_pos: usize,
    asset_paths: Vec<String>,
) {
    state.selected_nodes.clear();
    state.selected_nodes.insert(node_id.clone());
    note_touched_node(state, &node_id);
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
    state.autocomplete_asset_paths = asset_paths;
    state.hover_back_button = false;
    debug_log_edit_event("begin-text-edit", state);
}

pub(super) fn patcher_autocomplete_matches(
    edit: &PatcherTextEdit,
    local_macros: &[MacroPatch],
    asset_paths: &[String],
) -> Vec<String> {
    patcher_autocomplete_suggestions(edit, local_macros, asset_paths)
        .into_iter()
        .map(|suggestion| suggestion.name)
        .collect()
}

pub(super) fn patcher_autocomplete_suggestions(
    edit: &PatcherTextEdit,
    local_macros: &[MacroPatch],
    asset_paths: &[String],
) -> Vec<PatcherAutocompleteSuggestion> {
    let Some(context) = autocomplete_context(edit) else {
        return Vec::new();
    };
    let prefix = context.prefix.to_lowercase();
    let mut candidates = HashMap::new();
    match context.kind {
        AutocompleteKind::Operator => {
            let docs = dgenlisp_operator_documentation();
            candidates.extend(
                dgenlisp_operator_names()
                    .iter()
                    .filter(|name| name.to_lowercase().starts_with(&prefix))
                    .map(|name| (name.clone(), docs.get(name).cloned())),
            );
            if "history".starts_with(&prefix) {
                candidates.insert("history".to_string(), Some(patcher_history_documentation()));
            }
            for macro_patch in local_macros {
                if macro_patch.name.to_lowercase().starts_with(&prefix) {
                    candidates.insert(
                        macro_patch.name.clone(),
                        Some(local_macro_documentation(macro_patch)),
                    );
                }
            }
        }
        AutocompleteKind::Attribute { operator } => {
            if let Some(attributes) = dgenlisp_operator_attributes().get(operator) {
                candidates.extend(
                    attributes
                        .iter()
                        .filter(|name| name.to_lowercase().starts_with(&prefix))
                        .map(|name| (name.clone(), None)),
                );
            }
        }
        AutocompleteKind::AssetPath => {
            let path_prefix = prefix.strip_prefix('"').unwrap_or(&prefix);
            let path_prefix = path_prefix.strip_suffix('"').unwrap_or(path_prefix);
            candidates.extend(
                asset_paths
                    .iter()
                    .filter(|path| path.to_lowercase().starts_with(path_prefix))
                    .map(|path| (format!("\"{path}\""), None)),
            );
        }
    }
    let mut matches: Vec<PatcherAutocompleteSuggestion> = candidates
        .into_iter()
        .map(|(name, documentation)| PatcherAutocompleteSuggestion {
            name,
            documentation,
        })
        .collect();
    matches.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    matches.truncate(PATCHER_AUTOCOMPLETE_MAX_ITEMS);
    matches
}

fn patcher_history_documentation() -> OperatorDocumentation {
    OperatorDocumentation {
        category: Some("patcher".to_string()),
        summary: Some(
            "One-sample feedback cell. The outlet reads the previous frame and the inlet writes the current frame. With @shape it holds a tensor and reads/writes whole tensors per frame."
                .to_string(),
        ),
        signatures: vec![
            "(history)".to_string(),
            "(history @shape [d1 d2 ...])".to_string(),
        ],
        inputs: vec![OperatorPortDocumentation {
            name: Some("write".to_string()),
            kind: Some("signal|tensor".to_string()),
            required: Some(false),
            index: Some(0),
            summary: Some("Value stored for the next frame.".to_string()),
        }],
        outputs: vec![OperatorPortDocumentation {
            name: Some("previous".to_string()),
            kind: Some("signal|tensor".to_string()),
            required: Some(true),
            index: Some(0),
            summary: Some("Value stored by the preceding frame.".to_string()),
        }],
    }
}

fn local_macro_documentation(macro_patch: &MacroPatch) -> OperatorDocumentation {
    OperatorDocumentation {
        category: Some("macro".to_string()),
        summary: None,
        signatures: vec![if macro_patch.params.is_empty() {
            format!("({})", macro_patch.name)
        } else {
            format!("({} {})", macro_patch.name, macro_patch.params.join(" "))
        }],
        inputs: macro_patch
            .params
            .iter()
            .map(|param| OperatorPortDocumentation {
                name: Some(param.clone()),
                kind: None,
                required: Some(true),
                index: None,
                summary: None,
            })
            .collect(),
        outputs: macro_patch
            .outputs
            .iter()
            .enumerate()
            .map(|(index, output)| OperatorPortDocumentation {
                name: Some(output.clone()),
                kind: None,
                required: Some(true),
                index: Some(index),
                summary: None,
            })
            .collect(),
    }
}

pub(super) fn patcher_autocomplete_is_open(
    edit: &PatcherTextEdit,
    local_macros: &[MacroPatch],
    asset_paths: &[String],
) -> bool {
    !patcher_autocomplete_matches(edit, local_macros, asset_paths).is_empty()
}

pub(super) fn move_patcher_autocomplete_selection(
    edit: &mut PatcherTextEdit,
    local_macros: &[MacroPatch],
    asset_paths: &[String],
    delta: isize,
) -> bool {
    let matches = patcher_autocomplete_matches(edit, local_macros, asset_paths);
    if matches.is_empty() {
        edit.autocomplete_selected = 0;
        return false;
    }
    let len = matches.len() as isize;
    let current = edit.autocomplete_selected.min(matches.len() - 1) as isize;
    edit.autocomplete_selected = (current + delta).rem_euclid(len) as usize;
    true
}

pub(super) fn apply_patcher_autocomplete(
    edit: &mut PatcherTextEdit,
    local_macros: &[MacroPatch],
    asset_paths: &[String],
) -> bool {
    let Some(context) = autocomplete_context(edit) else {
        return false;
    };
    let (start, end) = (context.start, context.end);
    let matches = patcher_autocomplete_matches(edit, local_macros, asset_paths);
    if matches.is_empty() {
        edit.autocomplete_selected = 0;
        return false;
    }
    let selected = edit.autocomplete_selected.min(matches.len() - 1);
    let replacement = &matches[selected];
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
    edit.state.cursor_pos = edit.text[..start].chars().count()
        + replacement.chars().count()
        + 1;
    edit.state.selection_anchor = None;
    edit.state.selecting = false;
    edit.autocomplete_selected = 0;
    true
}

/// The untyped tail of the selected completion, for rendering immediately
/// after the caret. Ghost text is intentionally limited to a trailing token:
/// drawing it inside authored suffix text would obscure that text rather than
/// preview the result that Tab will produce.
pub(super) fn patcher_autocomplete_ghost_text(
    edit: &PatcherTextEdit,
    local_macros: &[MacroPatch],
) -> Option<String> {
    if edit.state.selection_anchor.is_some() {
        return None;
    }
    let context = autocomplete_context(edit)?;
    if !matches!(context.kind, AutocompleteKind::Attribute { .. })
        || context.cursor != context.end
        || context.end != edit.text.len()
    {
        return None;
    }
    let matches = patcher_autocomplete_matches(edit, local_macros, &[]);
    let selected = matches.get(edit.autocomplete_selected.min(matches.len().saturating_sub(1)))?;
    let prefix_chars = context.prefix.chars().count();
    let remainder = selected.chars().skip(prefix_chars).collect::<String>();
    (!remainder.is_empty()).then_some(remainder)
}

pub(super) fn clamp_patcher_autocomplete_selection(edit: &mut PatcherTextEdit) {
    clamp_patcher_autocomplete_selection_with_macros(edit, &[], &[]);
}

pub(super) fn clamp_patcher_autocomplete_selection_with_macros(
    edit: &mut PatcherTextEdit,
    local_macros: &[MacroPatch],
    asset_paths: &[String],
) {
    let len = patcher_autocomplete_matches(edit, local_macros, asset_paths).len();
    if len == 0 {
        edit.autocomplete_selected = 0;
    } else {
        edit.autocomplete_selected = edit.autocomplete_selected.min(len - 1);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutocompleteKind<'a> {
    Operator,
    Attribute { operator: &'a str },
    AssetPath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AutocompleteContext<'a> {
    kind: AutocompleteKind<'a>,
    prefix: &'a str,
    start: usize,
    end: usize,
    cursor: usize,
}

fn autocomplete_context(edit: &PatcherTextEdit) -> Option<AutocompleteContext<'_>> {
    let cursor = char_to_byte_index(&edit.text, edit.state.cursor_pos);
    if let Some((start, end)) = asset_path_value_span_at_cursor(&edit.text, cursor) {
        return Some(AutocompleteContext {
            kind: AutocompleteKind::AssetPath,
            prefix: &edit.text[start..cursor],
            start,
            end,
            cursor,
        });
    }

    let (start, end) = token_byte_span_at_cursor(&edit.text, cursor)?;
    let prefix = &edit.text[start..cursor];
    if prefix.is_empty() {
        return None;
    }
    let first_token = first_token_byte_span(&edit.text)?;
    let kind = if (start, end) == first_token {
        AutocompleteKind::Operator
    } else {
        if !prefix.starts_with('@') {
            return None;
        }
        AutocompleteKind::Attribute {
            operator: &edit.text[first_token.0..first_token.1],
        }
    };
    Some(AutocompleteContext {
        kind,
        prefix,
        start,
        end,
        cursor,
    })
}

/// Returns the source span of the `@file`/`@default-file` value under the
/// cursor. Attribute traversal deliberately uses `attribute_span_len`: a
/// preceding bracketed value may occupy several parser items, and treating
/// every attribute as a key/value pair would misidentify its tail as a path.
fn asset_path_value_span_at_cursor(text: &str, cursor: usize) -> Option<(usize, usize)> {
    let source = format!("({text})");
    let tokens = Parser::new(source).parse().ok()?;
    let expressions = ASTParser::new(tokens).parse().ok()?;
    let Expression::List(items) = expressions.first()? else {
        return None;
    };
    let item_spans = top_level_item_byte_spans(text)?;
    if items.len() != item_spans.len() {
        return None;
    }

    let mut idx = 1;
    while idx < items.len() {
        if !is_attribute_key(&items[idx]) {
            idx += 1;
            continue;
        }
        let span = attribute_span_len(items, idx);
        let is_file_attribute = matches!(
            &items[idx],
            Expression::Symbol(key) if key == "@file" || key == "@default-file"
        );
        if is_file_attribute {
            if span > 1 {
                let value_span = item_spans[idx + 1];
                if value_span.0 <= cursor && cursor <= value_span.1 {
                    return Some(value_span);
                }
            } else {
                let key_end = item_spans[idx].1;
                if key_end <= cursor && text[key_end..cursor].chars().all(char::is_whitespace) {
                    return Some((cursor, cursor));
                }
            }
        }
        idx += span;
    }
    None
}

fn top_level_item_byte_spans(text: &str) -> Option<Vec<(usize, usize)>> {
    let tokens = Parser::new(text.to_string()).parse_spanned().ok()?;
    let mut spans = Vec::new();
    let mut idx = 0;
    while idx < tokens.len() {
        let start = tokens[idx].span.start_byte;
        let end_idx = token_expression_end(&tokens, idx)?;
        spans.push((start, tokens[end_idx].span.end_byte));
        idx = end_idx + 1;
    }
    Some(spans)
}

fn token_expression_end(tokens: &[crate::parser::SpannedToken], start: usize) -> Option<usize> {
    match tokens.get(start)?.token {
        Token::Quote | Token::Backtick | Token::Comma | Token::CommaAt => {
            token_expression_end(tokens, start + 1)
        }
        Token::LeftParen => {
            let mut idx = start + 1;
            while idx < tokens.len() && !matches!(tokens[idx].token, Token::RightParen) {
                idx = token_expression_end(tokens, idx)? + 1;
            }
            Some(idx.min(tokens.len().saturating_sub(1)))
        }
        _ => Some(start),
    }
}

fn char_to_byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(byte_index, _)| byte_index)
        .unwrap_or(text.len())
}

fn token_byte_span_at_cursor(text: &str, cursor: usize) -> Option<(usize, usize)> {
    let cursor_char = text[cursor..].chars().next();
    let previous_char = text[..cursor].chars().next_back();
    if cursor_char.is_none_or(char::is_whitespace)
        && previous_char.is_none_or(char::is_whitespace)
    {
        return None;
    }
    let anchor = if cursor_char.is_some_and(|ch| !ch.is_whitespace()) {
        cursor
    } else {
        text[..cursor]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)?
    };
    let start = text[..anchor]
        .char_indices()
        .rev()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index + ch.len_utf8()))
        .unwrap_or(0);
    let end = text[anchor..]
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(anchor + index))
        .unwrap_or(text.len());
    Some((start, end))
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
    font_size: f32,
    zoom: f32,
    selecting: bool,
    release: bool,
) {
    let Some(cursor_pos) =
        patcher_text_cursor_at_col_with_font_size(rect, &edit.text, local_col, font_size, zoom)
    else {
        return;
    };
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
