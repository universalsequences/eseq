mod alignment;
mod display;
mod emit;
mod encapsulate;
mod generate;
mod geometry;
mod interaction;
mod layout;
mod lisp;
mod metrics;
mod model;
mod project;
mod render;
mod sidecar;
mod state;
#[cfg(test)]
mod tests;
mod text;
mod text_metrics;
mod writeback;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use crate::defmacro_library::{DefmacroLibrary, DefmacroPackage};

pub use lisp::{parse_patch_source, parse_patch_source_with_library};
pub use model::{
    ArgSource, ArgValue, AttributeSource, BindingId, BindingKind, BindingTarget, CableSegmentInfo,
    CallSourceShape, ConnectionKind, ConnectionSource, ExprPath, ExprPathSegment, MacroOrigin,
    MacroPatch, NodeKind, NodeSource, Patch, PatchConnection, PatchNode, PatcherIntent,
    SourceArgValue, SourceExprId, SourceFormId, SourceOwner, SourceScopeId,
};

pub fn emit_patch_writeback_source(source: &str, intent: PatcherIntent) -> Result<String, String> {
    let state = state::PatcherInteractionState::default();
    generate_source_for_state(source, intent, &state).map(|generated| generated.source)
}

/// Parse `source`, overlay the interaction state, and regenerate the full
/// canonical dsp.lisp from the resulting model (docs/patch-vs-code-editor-spec.md §4.2).
fn generate_source_for_state(
    source: &str,
    intent: PatcherIntent,
    state: &state::PatcherInteractionState,
) -> Result<generate::GeneratedPatchSource, String> {
    let root_patch = parse_source_with_default_library(source, intent)?;
    let visible = sidecar::root_patch_with_interaction(&root_patch, state);
    generate::generate_patch_source(&visible, intent)
}

/// §4.5 error surface: an agentic result that the projector cannot fully
/// represent is refused with a pointer at the eject path.
fn agentic_unprojectable_error(detail: impl AsRef<str>) -> String {
    format!(
        "agent result contains code the patch editor can't represent ({}); use \"Eject to code\" to accept it as text",
        detail.as_ref()
    )
}

pub fn emitted_source_buffer_name(path: &str) -> String {
    format!("*patcher-emitted:{path}*")
}

pub fn emitted_source_path_from_buffer_name(name: &str) -> Option<String> {
    name.strip_prefix("*patcher-emitted:")
        .and_then(|path| path.strip_suffix('*'))
        .map(str::to_string)
}

/// Editor-surface decision for an existing instrument/effect
/// (docs/patch-vs-code-editor-spec.md §3.2): the patch editor opens only for
/// patch-authored content — an `authored` layout sidecar AND source the
/// projector can represent without code islands. Everything else (agent- or
/// hand-written code, all pre-`authored` sidecars) gets the code editor.
pub fn source_opens_in_patch_editor(
    source_path: &Path,
    source: &str,
    intent: PatcherIntent,
) -> bool {
    if !sidecar::sidecar_is_authored(source_path) {
        return false;
    }
    let parsed = match crate::defmacro_library::default_library_root() {
        Some(root) => {
            let (_, library) = cached_defmacro_library(&root);
            parse_patch_source_with_library(source, intent, &library)
        }
        None => parse_patch_source(source, intent),
    };
    let Ok(patch) = parsed else {
        return false;
    };
    patch_is_fully_projectable(&patch)
}

/// Promotion ("Open as patch", spec §3.3): verify the source is fully
/// projectable and stamp an `authored: true` v2 layout sidecar next to it,
/// materializing default layout (or reusing any pre-existing sidecar layout).
/// Returns the projector diagnostics when the source contains code islands.
pub fn promote_source_to_patch(
    source_path: &Path,
    source: &str,
    intent: PatcherIntent,
) -> Result<(), String> {
    let mut patch = parse_source_with_default_library(source, intent)?;
    if !patch_is_fully_projectable(&patch) {
        let mut diagnostics = patch.diagnostics.clone();
        for macro_patch in &patch.macros {
            diagnostics.extend(macro_patch.patch.diagnostics.iter().cloned());
        }
        if diagnostics.is_empty() {
            diagnostics.push("source contains code the patch editor cannot represent".to_string());
        }
        return Err(format!("Cannot open as patch: {}", diagnostics.join("; ")));
    }
    sidecar::apply_or_materialize(source_path, &mut patch)?;
    sidecar::write_authored_layout(source_path, &patch)
}

/// Eject ("Eject to code", spec §3.4): flip the sidecar's `authored` flag to
/// false while keeping layout data for later re-promotion. The canonical
/// generated source is already on disk for patch-authored items, so no source
/// rewrite is needed.
pub fn eject_patch_authored_sidecar(source_path: &Path) -> Result<(), String> {
    sidecar::set_sidecar_authored(source_path, false)
}

fn parse_source_with_default_library(source: &str, intent: PatcherIntent) -> Result<Patch, String> {
    match crate::defmacro_library::default_library_root() {
        Some(root) => {
            let (_, library) = cached_defmacro_library(&root);
            parse_patch_source_with_library(source, intent, &library)
        }
        None => parse_patch_source(source, intent),
    }
}

fn patch_is_fully_projectable(patch: &Patch) -> bool {
    patch.diagnostics.is_empty()
        && !patch
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::CodeIsland)
        && patch.macros.iter().all(|macro_patch| {
            macro_patch.patch.diagnostics.is_empty()
                && !macro_patch
                    .patch
                    .nodes
                    .iter()
                    .any(|node| node.kind == NodeKind::CodeIsland)
        })
}

pub struct EmittedSourceBufferSnapshot {
    pub path: String,
    pub buffer_name: String,
    pub source: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacroLibraryActionKind {
    SaveToLibrary,
    Fork,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveMacroLibraryAction {
    pub macro_name: String,
    pub kind: MacroLibraryActionKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacroLibraryActionResult {
    pub macro_name: String,
    pub kind: MacroLibraryActionKind,
    pub source: String,
    pub layout: String,
}

pub fn emitted_source_buffer_snapshot(
    node: &crate::layout::LayoutNode,
) -> Result<EmittedSourceBufferSnapshot, String> {
    if node.widget_type != "patcher" {
        return Err("focused widget is not a patcher".to_string());
    }
    let payload = patcher_writeback_payload(node);
    let Value::Map(map) = payload else {
        return Err("patcher did not produce a writeback payload".to_string());
    };
    let status = map.get("status").and_then(|value| match &*value.borrow() {
        Value::Keyword(status) | Value::String(status) => Some(status.clone()),
        _ => None,
    });
    if status.as_deref() != Some("valid") {
        let diagnostic = map
            .get("diagnostic")
            .and_then(|value| match &*value.borrow() {
                Value::String(diagnostic) => Some(diagnostic.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "patcher emitted source is invalid".to_string());
        return Err(diagnostic);
    }
    let path = map
        .get("path")
        .and_then(|value| match &*value.borrow() {
            Value::String(path) if !path.is_empty() => Some(path.clone()),
            _ => None,
        })
        .ok_or_else(|| "patcher emitted source payload did not include a path".to_string())?;
    let source = map
        .get("source")
        .and_then(|value| match &*value.borrow() {
            Value::String(source) => Some(source.clone()),
            _ => None,
        })
        .ok_or_else(|| "patcher emitted source payload did not include source".to_string())?;
    Ok(EmittedSourceBufferSnapshot {
        buffer_name: emitted_source_buffer_name(&path),
        path,
        source,
    })
}

pub(crate) fn patcher_has_selected_cable(node: &crate::layout::LayoutNode) -> bool {
    if node.widget_type != "patcher" {
        return false;
    }
    let interaction = state::get_patcher_interaction_state(state::patcher_state_key(node));
    interaction.text_edit.is_none() && interaction.selected_cable.is_some()
}

pub(crate) fn patcher_agentic_bubble_count(node: &crate::layout::LayoutNode) -> usize {
    if node.widget_type != "patcher" {
        return 0;
    }
    state::get_patcher_interaction_state(state::patcher_state_key(node))
        .agentic_bubbles
        .len()
}

#[cfg(test)]
pub(crate) fn select_first_patcher_cable_for_test(
    node: &crate::layout::LayoutNode,
) -> Option<String> {
    let Ok((_, root_patch)) = load_patch_from_props(&node.props) else {
        return None;
    };
    let key = state::patcher_state_key(node);
    let mut interaction = state::get_patcher_interaction_state(key);
    let patch = active_patcher_patch(&root_patch, &interaction);
    let selected = patch.connections.first().map(state::source_connection_id)?;
    interaction.selected_cable = Some(selected.clone());
    state::set_patcher_interaction_state(key, interaction);
    Some(selected)
}

#[cfg(test)]
pub(crate) fn selected_patcher_cable_is_segmented_for_test(
    node: &crate::layout::LayoutNode,
) -> Option<bool> {
    let Ok((_, root_patch)) = load_patch_from_props(&node.props) else {
        return None;
    };
    let key = state::patcher_state_key(node);
    let interaction = state::get_patcher_interaction_state(key);
    let selected = interaction.selected_cable.as_deref()?;
    let patch = active_patcher_patch(&root_patch, &interaction);
    let patch =
        patch_with_interaction_state(patch, &interaction, &active_patcher_view_key(&interaction));
    patch
        .connections
        .iter()
        .find(|connection| state::source_connection_id(connection) == selected)
        .map(|connection| {
            connection
                .segment
                .as_ref()
                .is_some_and(|segment| segment.is_segmented)
        })
}

pub fn reset_patcher_state_for_path(path: impl AsRef<std::path::Path>, intent: PatcherIntent) {
    let path = path.as_ref();
    let path_string = path.to_string_lossy().to_string();
    let intent = match intent {
        PatcherIntent::Instrument => "instrument",
        PatcherIntent::Effect => "effect",
    };
    let props = HashMap::from([
        ("path".to_string(), Value::String(path_string)),
        ("intent".to_string(), Value::Keyword(intent.to_string())),
    ]);
    let key = state::patcher_state_key_from_parts(None, &props);
    state::reset_patcher_widget_states_for_path(path, key);
}

/// The macro view currently open in the patcher for `path`, if any
/// (None = root view). Drives the macro sidebar's selected row.
pub fn active_macro_view_for_path(path: impl AsRef<std::path::Path>) -> Option<String> {
    state::patcher_keys_for_path(path.as_ref())
        .into_iter()
        .find_map(|key| state::get_patcher_interaction_state(key).active_macro)
}

/// Navigate the patcher for `path` to a macro view (or the root with None),
/// preserving staged edits — unlike `reload_patcher_macro_view_for_path`,
/// which resets the whole interaction state after a save.
pub fn navigate_patcher_view_for_path(path: impl AsRef<std::path::Path>, macro_name: Option<&str>) {
    for key in state::patcher_keys_for_path(path.as_ref()) {
        let mut state = state::get_patcher_interaction_state(key);
        state.active_macro = macro_name.map(str::to_string);
        state.selected_nodes.clear();
        state.selected_cable = None;
        state.hovered_node = None;
        state.hover_back_button = false;
        state.drag = None;
        state.text_edit = None;
        state::set_patcher_interaction_state(key, state);
    }
}

pub fn reload_patcher_macro_view_for_path(
    path: impl AsRef<std::path::Path>,
    macro_name: impl Into<String>,
) {
    let macro_name = macro_name.into();
    for key in state::patcher_keys_for_path(path.as_ref()) {
        state::set_patcher_interaction_state(
            key,
            state::PatcherInteractionState {
                active_macro: Some(macro_name.clone()),
                ..state::PatcherInteractionState::default()
            },
        );
    }
}

pub fn resolve_agentic_bubble(
    path: impl AsRef<std::path::Path>,
    intent: PatcherIntent,
    bubble_id: &str,
    generation: u64,
    macro_name: &str,
    macro_source: &str,
) -> Result<(), String> {
    let path = path.as_ref();
    let keys = state::patcher_keys_for_path(path);
    if keys.is_empty() {
        return Ok(());
    }
    let has_matching_bubble = keys.iter().any(|key| {
        state::get_patcher_interaction_state(*key)
            .agentic_bubbles
            .get(bubble_id)
            .is_some_and(|bubble| bubble.generation == generation)
    });
    if !has_matching_bubble {
        return Ok(());
    }
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    // Ids already present in the source model must not be reused for the
    // created macro-instance node (older generated sources can contain
    // `created-N` bindings).
    let taken_node_ids = parse_source_with_default_library(&source, intent)
        .map(|patch| {
            patch
                .nodes
                .iter()
                .map(|node| node.id.clone())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let mut wrote = false;
    for key in keys {
        let mut interaction = state::get_patcher_interaction_state(key);
        let Some(bubble) = interaction.agentic_bubbles.get(bubble_id).cloned() else {
            continue;
        };
        if bubble.generation != generation {
            continue;
        }
        let materialized = materialize_agentic_macro_edit(
            &mut interaction,
            bubble.position,
            macro_name,
            macro_source,
            &taken_node_ids,
        );
        if !wrote {
            // §4.5: the agent's code becomes the model and is regenerated
            // canonically; it is only accepted if it projects cleanly.
            let generated =
                generate_source_for_state(&source, intent, &materialized.writeback_state)
                    .map_err(agentic_unprojectable_error)?;
            std::fs::write(path, generated.source)
                .map_err(|error| format!("failed to write '{}': {error}", path.display()))?;
            wrote = true;
        }
        interaction.agentic_morph_nodes.insert(
            materialized.instance_node_id.clone(),
            state::AgenticMorph {
                started_at: Instant::now(),
                from: state::agentic_bubble_pose(bubble_id),
            },
        );
        interaction.agentic_bubbles.remove(bubble_id);
        state::set_patcher_interaction_state(key, interaction);
    }
    Ok(())
}

pub fn resolve_agentic_bubble_macro_edit(
    path: impl AsRef<std::path::Path>,
    intent: PatcherIntent,
    bubble_id: &str,
    generation: u64,
    macro_name: &str,
    macro_source: &str,
) -> Result<(), String> {
    let path = path.as_ref();
    let keys = state::patcher_keys_for_path(path);
    if keys.is_empty() {
        return Ok(());
    }
    let has_matching_bubble = keys.iter().any(|key| {
        state::get_patcher_interaction_state(*key)
            .agentic_bubbles
            .get(bubble_id)
            .is_some_and(|bubble| bubble.generation == generation)
    });
    if !has_matching_bubble {
        return Ok(());
    }
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    // §4.5: splice the agent's macro into a candidate source, accept it only
    // if the whole file still projects with zero code islands, and write the
    // canonical regeneration of the resulting model (never the raw splice).
    let candidate = writeback::replace_macro_source(&source, macro_name, macro_source)
        .map_err(|error| format!("{error:?}"))?;
    let candidate_patch = parse_source_with_default_library(&candidate, intent)?;
    if !patch_is_fully_projectable(&candidate_patch) {
        let mut diagnostics = candidate_patch.diagnostics.clone();
        for macro_patch in &candidate_patch.macros {
            diagnostics.extend(macro_patch.patch.diagnostics.iter().cloned());
        }
        return Err(agentic_unprojectable_error(diagnostics.join("; ")));
    }
    let generated = generate::generate_patch_source(&candidate_patch, intent)
        .map_err(agentic_unprojectable_error)?;
    std::fs::write(path, generated.source)
        .map_err(|error| format!("failed to write '{}': {error}", path.display()))?;
    rematerialize_edited_macro_layout(path, intent, macro_name)?;
    for key in keys {
        let mut interaction = state::get_patcher_interaction_state(key);
        let Some(bubble) = interaction.agentic_bubbles.get(bubble_id).cloned() else {
            continue;
        };
        if bubble.generation != generation {
            continue;
        }
        if let AgenticBubbleTarget::EditMacro {
            instance_node_id, ..
        } = bubble.target
        {
            interaction.agentic_morph_nodes.insert(
                instance_node_id,
                state::AgenticMorph {
                    started_at: Instant::now(),
                    from: state::agentic_bubble_pose(bubble_id),
                },
            );
        }
        interaction.agentic_bubbles.remove(bubble_id);
        state::set_patcher_interaction_state(key, interaction);
    }
    Ok(())
}

fn rematerialize_edited_macro_layout(
    path: &std::path::Path,
    intent: PatcherIntent,
    macro_name: &str,
) -> Result<(), String> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    let mut patch = parse_patch_source(&source, intent)?;
    let excluded = std::iter::once(macro_name.to_string()).collect();
    sidecar::apply_or_materialize_excluding_macro_scopes(path, &mut patch, &excluded)
}

pub fn resolve_agentic_bubble_answer(
    path: impl AsRef<std::path::Path>,
    bubble_id: &str,
    generation: u64,
    answer: impl Into<String>,
) {
    let answer = answer.into();
    for key in state::patcher_keys_for_path(path.as_ref()) {
        let mut interaction = state::get_patcher_interaction_state(key);
        if let Some(bubble) = interaction.agentic_bubbles.get_mut(bubble_id)
            && bubble.generation == generation
        {
            bubble.state = state::AgenticBubbleState::Answer {
                text: answer.clone(),
                answered_at: Instant::now(),
            };
        }
        state::set_patcher_interaction_state(key, interaction);
    }
}

struct MaterializedAgenticMacro {
    instance_node_id: String,
    writeback_state: state::PatcherInteractionState,
}

fn materialize_agentic_macro_edit(
    interaction: &mut state::PatcherInteractionState,
    position: (f32, f32),
    macro_name: &str,
    macro_source: &str,
    taken_node_ids: &HashSet<String>,
) -> MaterializedAgenticMacro {
    let node_id =
        state::allocate_created_node_avoiding(interaction, "root", position, taken_node_ids);
    let node_key = state::node_edit_key("root", &node_id);
    if let Some(edit) = interaction.edit_state.nodes.get_mut(&node_key) {
        edit.text = macro_name.to_string();
    }
    let macro_edit = state::PatcherMacroEdit {
        name: macro_name.to_string(),
        instance_node_id: node_id.clone(),
        source: Some(macro_source.to_string()),
    };
    interaction
        .edit_state
        .created_macros
        .insert(macro_name.to_string(), macro_edit.clone());

    let mut writeback_state = state::PatcherInteractionState::default();
    writeback_state.edit_state.next_created_node = interaction.edit_state.next_created_node;
    if let Some(edit) = interaction.edit_state.nodes.get(&node_key).cloned() {
        writeback_state.edit_state.nodes.insert(node_key, edit);
    }
    writeback_state
        .edit_state
        .created_macros
        .insert(macro_name.to_string(), macro_edit);

    MaterializedAgenticMacro {
        instance_node_id: node_id,
        writeback_state,
    }
}

pub fn fail_agentic_bubble(
    path: impl AsRef<std::path::Path>,
    bubble_id: &str,
    generation: u64,
    summary: impl Into<String>,
    raw_output: impl Into<String>,
) {
    let summary = summary.into();
    let raw_output = raw_output.into();
    for key in state::patcher_keys_for_path(path.as_ref()) {
        let mut interaction = state::get_patcher_interaction_state(key);
        if let Some(bubble) = interaction.agentic_bubbles.get_mut(bubble_id)
            && bubble.generation == generation
        {
            bubble.state = state::AgenticBubbleState::Error {
                summary: summary.clone(),
                raw_output: raw_output.clone(),
                failed_at: Instant::now(),
            };
        }
        state::set_patcher_interaction_state(key, interaction);
    }
}

pub fn patcher_has_text_edit(node: &crate::layout::LayoutNode) -> bool {
    if node.widget_type != "patcher" {
        return false;
    }
    let state = state::get_patcher_interaction_state(state::patcher_state_key(node));
    state.text_edit.is_some() || state::editing_agentic_bubble_id(&state).is_some()
}

/// Drag type accepted by the patcher canvas: macro items dragged from the
/// macro sidebar. Dropping one creates a node calling that macro.
pub const PATCHER_MACRO_DRAG_TYPE: &str = "dgen-macro";

pub fn patcher_accepts_drop(node: &LayoutNode, drag_type: &str) -> bool {
    node.widget_type == "patcher" && drag_type == PATCHER_MACRO_DRAG_TYPE
}

fn macro_drop_payload_name(payload: &Value) -> Option<String> {
    let Value::Map(map) = payload else {
        return None;
    };
    let name = map.get("name").or_else(|| map.get("label"))?;
    match &*name.borrow() {
        Value::String(name) if !name.trim().is_empty() => Some(name.trim().to_string()),
        _ => None,
    }
}

/// Drop a macro item onto the patcher canvas: allocate a created node whose
/// text is the macro call at the drop point, then emit the standard writeback
/// payload so the host persists and recompiles exactly as for a typed node.
pub fn handle_patcher_drop(
    node: &LayoutNode,
    drag_type: &str,
    payload: &Value,
    local_col: f32,
    local_row: f32,
) -> Option<super::EventOutput> {
    if !patcher_accepts_drop(node, drag_type) {
        return None;
    }
    let macro_name = macro_drop_payload_name(payload)?;
    let key = state::patcher_state_key(node);
    let (_, root_patch) = load_patch_from_props(&node.props).ok()?;
    let mut state = state::get_patcher_interaction_state(key);
    // A macro view must not gain a node calling the macro it defines.
    if state.active_macro.as_deref() == Some(macro_name.as_str()) {
        return None;
    }
    let view_key = state::active_patcher_view_key(&state);
    let patch = state::active_patcher_patch(&root_patch, &state);
    let patch = state::patch_with_interaction_state(patch, &state, &view_key);
    let mut pan_state = state::get_patcher_pan_state(key);
    interaction::sync_patcher_pan_bounds_from_patch(node, &mut pan_state, &patch);
    let position = geometry::screen_to_model(node.rect, &pan_state, (local_col, local_row));
    let taken_node_ids = patch
        .nodes
        .iter()
        .map(|patch_node| patch_node.id.clone())
        .collect::<HashSet<_>>();
    let created_id =
        state::allocate_created_node_avoiding(&mut state, &view_key, position, &taken_node_ids);
    state
        .edit_state
        .nodes
        .get_mut(&state::node_edit_key(&view_key, &created_id))?
        .text = macro_name;
    state::set_patcher_interaction_state(key, state);
    patcher_change_output(node, patcher_writeback_payload(node))
}

#[cfg(any(test, feature = "patcher-test-support"))]
pub fn emit_patch_writeback_with_inserted_node_before_first_output(
    source: &str,
    intent: PatcherIntent,
    node_text: &str,
) -> Result<String, String> {
    let patch = parse_patch_source(source, intent)?;
    let output_node = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .ok_or_else(|| "patch has no output node".to_string())?;
    let incoming = patch
        .connections
        .iter()
        .find(|connection| connection.to_node == output_node.id && connection.to_input == 0)
        .ok_or_else(|| "output node has no incoming signal connection".to_string())?;
    let mut state = state::PatcherInteractionState::default();
    let view_key = "root";
    let inserted = state::allocate_created_node(&mut state, view_key, (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&state::node_edit_key(view_key, &inserted))
        .expect("created node edit should be present")
        .text = node_text.to_string();
    state
        .edit_state
        .deleted_connections
        .insert(state::connection_edit_key(
            view_key,
            &state::source_connection_id(incoming),
        ));
    state::allocate_created_connection(
        &mut state,
        view_key,
        model::OutputPortRef {
            node_id: incoming.from_node.clone(),
            output_index: incoming.from_output,
        },
        model::InputPortRef {
            node_id: inserted.clone(),
            input_index: 0,
        },
    );
    state::allocate_created_connection(
        &mut state,
        view_key,
        model::OutputPortRef {
            node_id: inserted,
            output_index: 0,
        },
        model::InputPortRef {
            node_id: output_node.id.clone(),
            input_index: 0,
        },
    );

    generate_source_for_state(source, intent, &state).map(|generated| generated.source)
}

#[cfg(any(test, feature = "patcher-test-support"))]
pub fn emit_patch_writeback_with_first_node_text_edit(
    source: &str,
    intent: PatcherIntent,
    op: &str,
    edited_text: &str,
) -> Result<String, String> {
    let patch = parse_patch_source(source, intent)?;
    let node = patch
        .nodes
        .iter()
        .find(|node| node.op == op)
        .ok_or_else(|| format!("patch has no `{op}` node"))?;
    let mut state = state::PatcherInteractionState::default();
    state::set_node_edit_position(
        &mut state,
        "root",
        node,
        node.position,
        display::node_display_label(node),
    );
    state
        .edit_state
        .nodes
        .get_mut(&state::node_edit_key("root", &node.id))
        .expect("source node edit should be present")
        .text = edited_text.to_string();

    generate_source_for_state(source, intent, &state).map(|generated| generated.source)
}

#[cfg(any(test, feature = "patcher-test-support"))]
pub fn emit_patch_writeback_with_created_phasor_multiply_before_first_output(
    source: &str,
    intent: PatcherIntent,
    phasor_frequency_text: &str,
) -> Result<String, String> {
    let patch = parse_patch_source(source, intent)?;
    let output_node = patch
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Out)
        .ok_or_else(|| "patch has no output node".to_string())?;
    let incoming = patch
        .connections
        .iter()
        .find(|connection| connection.to_node == output_node.id && connection.to_input == 0)
        .ok_or_else(|| "output node has no incoming signal connection".to_string())?;
    let mut state = state::PatcherInteractionState::default();
    let view_key = "root";
    let multiply = state::allocate_created_node(&mut state, view_key, (1.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&state::node_edit_key(view_key, &multiply))
        .expect("created multiply edit should be present")
        .text = "*".to_string();
    let phasor = state::allocate_created_node(&mut state, view_key, (2.0, 1.0));
    state
        .edit_state
        .nodes
        .get_mut(&state::node_edit_key(view_key, &phasor))
        .expect("created phasor edit should be present")
        .text = "phasor".to_string();
    let frequency = state::allocate_created_node(&mut state, view_key, (2.0, 0.0));
    state
        .edit_state
        .nodes
        .get_mut(&state::node_edit_key(view_key, &frequency))
        .expect("created frequency edit should be present")
        .text = phasor_frequency_text.to_string();
    state
        .edit_state
        .deleted_connections
        .insert(state::connection_edit_key(
            view_key,
            &state::source_connection_id(incoming),
        ));
    state::allocate_created_connection(
        &mut state,
        view_key,
        model::OutputPortRef {
            node_id: incoming.from_node.clone(),
            output_index: incoming.from_output,
        },
        model::InputPortRef {
            node_id: multiply.clone(),
            input_index: 0,
        },
    );
    state::allocate_created_connection(
        &mut state,
        view_key,
        model::OutputPortRef {
            node_id: frequency,
            output_index: 0,
        },
        model::InputPortRef {
            node_id: phasor.clone(),
            input_index: 0,
        },
    );
    state::allocate_created_connection(
        &mut state,
        view_key,
        model::OutputPortRef {
            node_id: phasor,
            output_index: 0,
        },
        model::InputPortRef {
            node_id: multiply.clone(),
            input_index: 1,
        },
    );
    state::allocate_created_connection(
        &mut state,
        view_key,
        model::OutputPortRef {
            node_id: multiply,
            output_index: 0,
        },
        model::InputPortRef {
            node_id: output_node.id.clone(),
            input_index: 0,
        },
    );

    generate_source_for_state(source, intent, &state).map(|generated| generated.source)
}

use display::{node_display_label, preview};
use emit::debug_log_patch_lisp;
use encapsulate::encapsulate_patcher_selection;
use interaction::{
    PatcherChangeKind, connect_last_touched_nodes, copy_selected_patcher_nodes,
    create_patcher_node_below_anchor, handle_patcher_double_click, handle_patcher_pointer_down,
    handle_patcher_pointer_drag, handle_patcher_pointer_moved, handle_patcher_pointer_up,
    open_selected_macro_node, pan_patcher_by_delta, pan_patcher_by_wheel, paste_patcher_clipboard,
    promote_created_macro_definition, reset_patcher_pan, zoom_patcher_by_magnify,
};
use metrics::{DEFAULT_HEIGHT, DEFAULT_WIDTH, TOUCHPAD_PAN_SPEED_CELLS_PER_PIXEL};
use state::{
    AgenticBubbleState, AgenticBubbleTarget, active_patcher_patch, active_patcher_view_key,
    allocate_agentic_bubble, allocate_agentic_bubble_with_target, apply_patcher_history_step,
    debug_log_edit_event, debug_log_writeback_event, delete_connection_edit_or_mark_deleted,
    delete_selected_nodes, editing_agentic_bubble_id, get_patcher_interaction_state,
    patch_with_interaction_state, patcher_state_key, patcher_state_key_from_parts,
    prune_unreferenced_created_macros, set_connection_segment_edit, set_patcher_interaction_state,
    set_patcher_interaction_state_without_history,
};
use text::{
    apply_patcher_autocomplete, cancel_patcher_text_edit,
    clamp_patcher_autocomplete_selection_with_macros, commit_patcher_text_edit,
    move_patcher_autocomplete_selection, patcher_autocomplete_is_open,
};

use super::text_input::{TextEditOutcome, apply_text_entry_key};
use super::{CellBuffer, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent};
#[cfg(target_os = "macos")]
use super::{MetalPrimitive, WidgetViewport};
use crate::layout::{
    Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num, get_stable_widget_id,
};
use crate::parser::{ASTParser, Expression, Parser, format_expression};
use crate::vm::Value;
use std::time::Instant;
use text_metrics::cache_text_widths;

pub struct PatcherWidget;

pub static PATCHER_WIDGET: PatcherWidget = PatcherWidget;

impl WidgetDefinition for PatcherWidget {
    fn names(&self) -> &'static [&'static str] {
        &["patcher"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let width = if matches!(prop_keyword(node, "width").as_deref(), Some("fill")) {
            constraints.max_width
        } else {
            get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or(DEFAULT_WIDTH)
                .min(constraints.max_width.max(1.0))
        };
        let height = if matches!(prop_keyword(node, "height").as_deref(), Some("fill")) {
            constraints.max_height
        } else {
            get_prop_num(node, "height")
                .map(f64_to_f32)
                .unwrap_or(DEFAULT_HEIGHT)
        };
        cache_patcher_text_widths(node, ctx);
        Some(Size {
            width: width.max(1.0),
            height: height.max(1.0),
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        render::render_tui(props, rect, buf);
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        modifiers: KeyModifiers,
        cell_w: f32,
        cell_h: f32,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                handle_patcher_pointer_down(node, local_col, local_row, modifiers, cell_w, cell_h);
                MouseEventOutcome::Consume
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                handle_patcher_pointer_drag(node, local_col, local_row, modifiers, cell_w, cell_h);
                MouseEventOutcome::Consume
            }
            MouseEventKind::Up(MouseButton::Left) => MouseEventOutcome::Dispatch(
                patcher_widget_event(handle_patcher_pointer_up(node, local_col, local_row)),
            ),
            MouseEventKind::Moved => {
                handle_patcher_pointer_moved(node, local_col, local_row, cell_w, cell_h);
                MouseEventOutcome::Consume
            }
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {
                pan_patcher_by_wheel(node, mouse_kind);
                MouseEventOutcome::Consume
            }
            _ => MouseEventOutcome::Consume,
        }
    }

    fn scroll_gesture_event(
        &self,
        node: &LayoutNode,
        _local_col: f32,
        _local_row: f32,
        delta_x: f32,
        delta_y: f32,
    ) -> Option<WidgetEvent> {
        pan_patcher_by_delta(
            node,
            -delta_x * TOUCHPAD_PAN_SPEED_CELLS_PER_PIXEL,
            -delta_y * TOUCHPAD_PAN_SPEED_CELLS_PER_PIXEL,
        );
        Some(WidgetEvent::Custom(Value::Nil))
    }

    fn magnify_event(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
        delta: f64,
    ) -> Option<WidgetEvent> {
        if zoom_patcher_by_magnify(node, local_col, local_row, delta) {
            Some(WidgetEvent::Custom(Value::Nil))
        } else {
            None
        }
    }

    fn captures_scroll_gesture(&self, _node: &LayoutNode) -> bool {
        true
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn unclamped_drag(&self) -> bool {
        true
    }

    fn double_click_event(
        &self,
        node: &LayoutNode,
        local_col: f32,
        local_row: f32,
    ) -> Option<WidgetEvent> {
        if handle_patcher_double_click(node, local_col, local_row) {
            Some(WidgetEvent::Custom(Value::Nil))
        } else {
            None
        }
    }

    fn key_event(&self, node: &LayoutNode, key_event: WidgetKeyEvent) -> Option<WidgetEvent> {
        let key = patcher_state_key(node);
        let mut state = get_patcher_interaction_state(key);
        let view_key = active_patcher_view_key(&state);
        let autocomplete_macros = autocomplete_macros_for_state(node, &state, &view_key);
        if let Ok((path, _)) = load_patch_from_props(&node.props) {
            state::register_patcher_path_key(path, key);
        }
        if let Some(bubble_id) = editing_agentic_bubble_id(&state) {
            return handle_agentic_bubble_edit_key(node, key, state, bubble_id, key_event);
        }
        if key_event.modifiers.contains(KeyModifiers::SUPER) {
            match key_event.code {
                KeyCode::Enter => {
                    let committed = commit_active_patcher_text_edit(node, &mut state, &view_key);
                    if committed {
                        // Flush the committed text edit as its own undo step
                        // before the next created node opens a fresh gesture.
                        set_patcher_interaction_state(key, state.clone());
                    }
                    create_patcher_node_below_anchor(node, &mut state, &view_key);
                    set_patcher_interaction_state(key, state);
                    return Some(patcher_semantic_event(committed));
                }
                KeyCode::Up => {
                    let committed = commit_active_patcher_text_edit(node, &mut state, &view_key);
                    if committed {
                        set_patcher_interaction_state(key, state.clone());
                    }
                    let connected = connect_last_touched_nodes(node, &mut state, &view_key);
                    if (committed || connected)
                        && let Some(patch) = debug_patch_for_state(node, &state, &view_key)
                    {
                        debug_log_patch_lisp(&view_key, &patch);
                    }
                    set_patcher_interaction_state(key, state);
                    return Some(patcher_semantic_event(committed || connected));
                }
                _ => {}
            }
        }
        if state.text_edit.is_none() {
            return match key_event.code {
                // Cmd+Z / Cmd+Shift+Z: graph-level undo/redo. The app-level
                // sequencer history shortcut yields to a focused patcher
                // (input.rs sequencer_history_shortcut), so the key arrives
                // here. Cmd+C/V arrive with SUPER rewritten to CONTROL by
                // normalize_command_shortcuts, hence the intersects checks.
                KeyCode::Char('z') | KeyCode::Char('Z')
                    if key_event
                        .modifiers
                        .intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL)
                        && !key_event.modifiers.contains(KeyModifiers::ALT)
                        && state.drag.is_none() =>
                {
                    let redo = key_event.modifiers.contains(KeyModifiers::SHIFT);
                    if apply_patcher_history_step(key, &mut state, redo) {
                        if let Some(patch) = debug_patch_for_state(node, &state, &view_key) {
                            debug_log_patch_lisp(&view_key, &patch);
                        }
                        set_patcher_interaction_state_without_history(key, state);
                        Some(patcher_semantic_event(true))
                    } else {
                        None
                    }
                }
                KeyCode::Char('c') | KeyCode::Char('C')
                    if key_event
                        .modifiers
                        .intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL)
                        && !state.selected_nodes.is_empty() =>
                {
                    if copy_selected_patcher_nodes(node, &state, &view_key) {
                        Some(WidgetEvent::Custom(Value::Nil))
                    } else {
                        None
                    }
                }
                // Cmd+E: encapsulate the selection into a new local defmacro.
                KeyCode::Char('e') | KeyCode::Char('E')
                    if key_event
                        .modifiers
                        .intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL)
                        && !key_event.modifiers.contains(KeyModifiers::ALT)
                        && !state.selected_nodes.is_empty()
                        && state.drag.is_none() =>
                {
                    let changed = encapsulate_patcher_selection(node, &mut state, &view_key);
                    if changed && let Some(patch) = debug_patch_for_state(node, &state, &view_key) {
                        debug_log_patch_lisp(&view_key, &patch);
                    }
                    if changed {
                        set_patcher_interaction_state(key, state);
                    }
                    Some(patcher_semantic_event(changed))
                }
                KeyCode::Char('v') | KeyCode::Char('V')
                    if key_event
                        .modifiers
                        .intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL) =>
                {
                    let changed = paste_patcher_clipboard(node, &mut state, &view_key);
                    if changed && let Some(patch) = debug_patch_for_state(node, &state, &view_key) {
                        debug_log_patch_lisp(&view_key, &patch);
                    }
                    set_patcher_interaction_state(key, state);
                    Some(patcher_semantic_event(changed))
                }
                KeyCode::Char('y') | KeyCode::Char('Y')
                    if state.selected_cable.is_some()
                        && key_event.modifiers.contains(KeyModifiers::SUPER) =>
                {
                    eprintln!(
                        "[patcher cmd-y] widget received selected_cable={:?} view_key={view_key}",
                        state.selected_cable
                    );
                    if toggle_selected_cable_segmented(node, &mut state, &view_key) {
                        eprintln!("[patcher cmd-y] widget toggled selected cable segmentation");
                        set_patcher_interaction_state(key, state);
                        Some(patcher_widget_event(PatcherChangeKind::Layout))
                    } else {
                        eprintln!("[patcher cmd-y] widget could not toggle selected cable");
                        None
                    }
                }
                KeyCode::Char('r') | KeyCode::Char('R')
                    if state.agentic_bubbles.values().any(|bubble| {
                        !bubble.is_dismissed()
                            && matches!(bubble.state, AgenticBubbleState::Error { .. })
                    }) =>
                {
                    let bubble_id = state
                        .agentic_bubbles
                        .values()
                        .find(|bubble| {
                            !bubble.is_dismissed()
                                && matches!(bubble.state, AgenticBubbleState::Error { .. })
                        })
                        .map(|bubble| bubble.id.clone())?;
                    let payload = {
                        let bubble = state.agentic_bubbles.get_mut(&bubble_id)?;
                        bubble.generation = bubble.generation.wrapping_add(1);
                        bubble.state = AgenticBubbleState::Pending {
                            started_at: Instant::now(),
                        };
                        agentic_submit_payload(node, bubble)
                    };
                    set_patcher_interaction_state(key, state);
                    Some(WidgetEvent::Custom(payload))
                }
                KeyCode::Esc if dismissable_agentic_bubble_id(&state).is_some() => {
                    // Kept in the map so it can shrink out; `is_dismissed`
                    // makes it invisible to everything else from here.
                    if let Some(bubble_id) = dismissable_agentic_bubble_id(&state)
                        && let Some(bubble) = state.agentic_bubbles.get_mut(&bubble_id)
                    {
                        bubble.closing_at = Some(Instant::now());
                    }
                    set_patcher_interaction_state(key, state);
                    Some(WidgetEvent::Custom(Value::Nil))
                }
                KeyCode::Char('k') | KeyCode::Char('K')
                    if key_event.modifiers.contains(KeyModifiers::SUPER) =>
                {
                    open_agentic_bubble_for_context(node, key, &mut state)?;
                    set_patcher_interaction_state(key, state);
                    Some(WidgetEvent::Custom(Value::Nil))
                }
                KeyCode::Enter if open_selected_macro_node(node, &mut state) => {
                    set_patcher_interaction_state(key, state);
                    reset_patcher_pan(key);
                    Some(WidgetEvent::Custom(Value::Nil))
                }
                KeyCode::Backspace | KeyCode::Delete if state.selected_cable.is_some() => {
                    if let Some(cable_id) = state.selected_cable.clone() {
                        let changed = delete_connection_edit_or_mark_deleted(
                            &mut state, &view_key, &cable_id,
                        );
                        state.drag = None;
                        if changed
                            && let Some(patch) = debug_patch_for_state(node, &state, &view_key)
                        {
                            debug_log_patch_lisp(&view_key, &patch);
                        }
                        set_patcher_interaction_state(key, state);
                        Some(patcher_semantic_event(changed))
                    } else {
                        None
                    }
                }
                KeyCode::Backspace | KeyCode::Delete if !state.selected_nodes.is_empty() => {
                    let changed = delete_selected_nodes(&mut state, &view_key);
                    if changed {
                        prune_unreferenced_created_macros(&mut state);
                    }
                    if changed && let Some(patch) = debug_patch_for_state(node, &state, &view_key) {
                        debug_log_patch_lisp(&view_key, &patch);
                    }
                    set_patcher_interaction_state(key, state);
                    Some(patcher_semantic_event(changed))
                }
                _ => None,
            };
        }
        match key_event.code {
            KeyCode::Enter if state.text_edit.is_some() => {
                let changed = commit_active_patcher_text_edit(node, &mut state, &view_key);
                if changed && let Some(patch) = debug_patch_for_state(node, &state, &view_key) {
                    debug_log_patch_lisp(&view_key, &patch);
                }
                set_patcher_interaction_state(key, state);
                Some(patcher_semantic_event(changed))
            }
            KeyCode::Esc if state.text_edit.is_some() => {
                cancel_patcher_text_edit(&mut state, &view_key);
                set_patcher_interaction_state(key, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Tab if state.text_edit.is_some() => {
                if let Some(edit) = state.text_edit.as_mut() {
                    apply_patcher_autocomplete(edit, &autocomplete_macros);
                }
                set_patcher_interaction_state(key, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Down
                if state.text_edit.as_ref().is_some_and(|edit| {
                    patcher_autocomplete_is_open(edit, &autocomplete_macros)
                }) =>
            {
                if let Some(edit) = state.text_edit.as_mut() {
                    move_patcher_autocomplete_selection(edit, &autocomplete_macros, 1);
                }
                set_patcher_interaction_state(key, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Up
                if state.text_edit.as_ref().is_some_and(|edit| {
                    patcher_autocomplete_is_open(edit, &autocomplete_macros)
                }) =>
            {
                if let Some(edit) = state.text_edit.as_mut() {
                    move_patcher_autocomplete_selection(edit, &autocomplete_macros, -1);
                }
                set_patcher_interaction_state(key, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Backspace | KeyCode::Delete if state.selected_cable.is_some() => {
                if let Some(cable_id) = state.selected_cable.clone() {
                    let changed =
                        delete_connection_edit_or_mark_deleted(&mut state, &view_key, &cable_id);
                    state.drag = None;
                    if changed && let Some(patch) = debug_patch_for_state(node, &state, &view_key) {
                        debug_log_patch_lisp(&view_key, &patch);
                    }
                    set_patcher_interaction_state(key, state);
                    Some(patcher_semantic_event(changed))
                } else {
                    None
                }
            }
            KeyCode::Backspace | KeyCode::Delete
                if state.text_edit.is_none() && !state.selected_nodes.is_empty() =>
            {
                let changed = delete_selected_nodes(&mut state, &view_key);
                if changed {
                    prune_unreferenced_created_macros(&mut state);
                }
                if changed && let Some(patch) = debug_patch_for_state(node, &state, &view_key) {
                    debug_log_patch_lisp(&view_key, &patch);
                }
                set_patcher_interaction_state(key, state);
                Some(patcher_semantic_event(changed))
            }
            _ => {
                let edit = state.text_edit.as_mut()?;
                match apply_text_entry_key(&edit.text, &mut edit.state, key_event, false, None)? {
                    TextEditOutcome::Changed(new_text) => {
                        edit.text = new_text;
                        clamp_patcher_autocomplete_selection_with_macros(
                            edit,
                            &autocomplete_macros,
                        );
                        set_patcher_interaction_state(key, state);
                        Some(WidgetEvent::Custom(Value::Nil))
                    }
                    TextEditOutcome::StateOnly => {
                        set_patcher_interaction_state(key, state);
                        Some(WidgetEvent::Custom(Value::Nil))
                    }
                }
            }
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<super::EventOutput> {
        match event {
            WidgetEvent::Custom(Value::Keyword(kind)) if kind == "semantic-change" => {
                patcher_change_output(node, patcher_writeback_payload(node))
            }
            WidgetEvent::Custom(Value::Map(map))
                if map.get("status").is_some_and(|value| {
                    matches!(&*value.borrow(), Value::Keyword(kind) if kind.starts_with("agentic-"))
                }) =>
            {
                patcher_change_output(node, Value::Map(map))
            }
            WidgetEvent::Custom(Value::Keyword(kind)) if kind == "layout-change" => {
                patcher_change_output(node, patcher_layout_payload(node))
            }
            WidgetEvent::Custom(Value::Bool(true)) => {
                patcher_change_output(node, patcher_writeback_payload(node))
            }
            WidgetEvent::Custom(Value::Nil) | WidgetEvent::Custom(Value::Bool(false)) => None,
            _ => None,
        }
    }

    fn wants_animation_frames(&self, node: &LayoutNode) -> bool {
        let state = get_patcher_interaction_state(patcher_state_key(node));
        state.agentic_bubbles.values().any(|bubble| {
            // Shrinking out wins: a dismissed bubble animates regardless of the
            // state it was dismissed from.
            if bubble.is_dismissed() {
                return !bubble.close_finished();
            }
            let state_animating = match bubble.state {
                AgenticBubbleState::Pending { .. } => true,
                AgenticBubbleState::Answer { answered_at, .. } => {
                    answered_at.elapsed().as_secs_f32() < metrics::AGENTIC_ANSWER_RESIZE_SECS
                }
                _ => false,
            };
            state_animating
                || bubble.created_at.elapsed().as_secs_f32() < metrics::AGENTIC_APPEAR_SECS
        }) || state.agentic_morph_nodes.values().any(|morph| {
            morph.started_at.elapsed().as_secs_f32() < metrics::AGENTIC_MORPH_COLOR_SECS
        })
    }

    fn animation_frame_policy(&self) -> super::AnimationFramePolicy {
        super::AnimationFramePolicy::RuntimeState
    }

    fn renders_own_focus(&self) -> bool {
        true
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        render::build_metal_primitives_for_patcher(node, viewport)
    }
}

/// Commit the active node text edit (if any) including created-macro
/// promotion; returns whether the patch semantically changed.
fn commit_active_patcher_text_edit(
    node: &LayoutNode,
    state: &mut state::PatcherInteractionState,
    view_key: &str,
) -> bool {
    let Some((committed_node_id, previous_text)) = state
        .text_edit
        .as_ref()
        .map(|edit| (edit.node_id.clone(), edit.original_text.clone()))
    else {
        return false;
    };
    let changed = commit_patcher_text_edit(state, view_key);
    let promoted_macro = load_patch_from_props(&node.props)
        .ok()
        .is_some_and(|(_, root_patch)| {
            promote_created_macro_definition(&root_patch, state, view_key, &committed_node_id)
        });
    // Retyping the header of a created macro's instance renames the macro
    // (that is how an encapsulated `sub1` gets a real name) rather than
    // leaving a call to an operator that does not exist.
    let renamed_macro = encapsulate::rename_created_macro_from_instance_text(
        node,
        state,
        view_key,
        &committed_node_id,
        &previous_text,
    );
    changed || promoted_macro || renamed_macro
}

fn handle_agentic_bubble_edit_key(
    node: &LayoutNode,
    key: u64,
    mut state: state::PatcherInteractionState,
    bubble_id: String,
    key_event: WidgetKeyEvent,
) -> Option<WidgetEvent> {
    match key_event.code {
        KeyCode::Enter => {
            let payload = {
                let bubble = state.agentic_bubbles.get_mut(&bubble_id)?;
                let prompt = bubble.prompt.trim().to_string();
                if prompt.is_empty() {
                    return Some(WidgetEvent::Custom(Value::Nil));
                }
                bubble.generation = bubble.generation.wrapping_add(1);
                bubble.macro_name = slug_agentic_macro_name(&prompt);
                bubble.state = AgenticBubbleState::Pending {
                    started_at: Instant::now(),
                };
                agentic_submit_payload(node, bubble)
            };
            set_patcher_interaction_state(key, state);
            Some(WidgetEvent::Custom(payload))
        }
        KeyCode::Esc => {
            // Kept in the map so it can shrink out; `is_dismissed` makes it
            // invisible to everything else from here.
            if let Some(bubble) = state.agentic_bubbles.get_mut(&bubble_id) {
                bubble.closing_at = Some(Instant::now());
            }
            set_patcher_interaction_state(key, state);
            Some(WidgetEvent::Custom(Value::Nil))
        }
        _ => {
            let bubble = state.agentic_bubbles.get_mut(&bubble_id)?;
            match apply_text_entry_key(
                &bubble.prompt,
                &mut bubble.text_state,
                key_event,
                false,
                None,
            )? {
                TextEditOutcome::Changed(new_text) => {
                    bubble.prompt = new_text;
                    set_patcher_interaction_state(key, state);
                    Some(WidgetEvent::Custom(Value::Nil))
                }
                TextEditOutcome::StateOnly => {
                    set_patcher_interaction_state(key, state);
                    Some(WidgetEvent::Custom(Value::Nil))
                }
            }
        }
    }
}

/// The settled bubble Escape would dismiss: one showing an answer or an error.
/// A bubble already shrinking out is not a candidate, so a second Escape falls
/// through to the patcher's other Escape handling instead of restarting it.
fn dismissable_agentic_bubble_id(state: &state::PatcherInteractionState) -> Option<String> {
    state
        .agentic_bubbles
        .values()
        .find(|bubble| {
            !bubble.is_dismissed()
                && matches!(
                    bubble.state,
                    AgenticBubbleState::Error { .. } | AgenticBubbleState::Answer { .. }
                )
        })
        .map(|bubble| bubble.id.clone())
}

fn agentic_submit_payload(node: &LayoutNode, bubble: &state::AgenticBubble) -> Value {
    let path = prop_str(&node.props, "path").or_else(|| prop_str(&node.props, "file"));
    let intent = match patcher_intent_from_props(&node.props) {
        PatcherIntent::Effect => "effect",
        PatcherIntent::Instrument => "instrument",
    };
    let mut entries = vec![
        ("status", Value::Keyword("agentic-submit".to_string())),
        ("path", Value::String(path.unwrap_or_default())),
        ("intent", Value::Keyword(intent.to_string())),
        ("bubble-id", Value::String(bubble.id.clone())),
        ("generation", Value::Number(bubble.generation as f64)),
        ("prompt", Value::String(bubble.prompt.clone())),
        ("macro-name", Value::String(bubble.macro_name.clone())),
        ("x", Value::Number(bubble.position.0 as f64)),
        ("y", Value::Number(bubble.position.1 as f64)),
    ];
    match &bubble.target {
        AgenticBubbleTarget::CreateMacro => {
            entries.push(("target", Value::Keyword("create-macro".to_string())));
        }
        AgenticBubbleTarget::EditMacro {
            instance_node_id,
            macro_name,
            params,
            source,
        } => {
            entries.push(("target", Value::Keyword("edit-macro".to_string())));
            entries.push(("target-node-id", Value::String(instance_node_id.clone())));
            entries.push(("existing-macro-name", Value::String(macro_name.clone())));
            entries.push(("existing-macro-params", Value::String(params.join(" "))));
            entries.push(("existing-macro-source", Value::String(source.clone())));
        }
    }
    map_value(entries)
}

fn open_agentic_bubble_for_context(
    node: &LayoutNode,
    key: u64,
    state: &mut state::PatcherInteractionState,
) -> Option<String> {
    let macro_target = selected_macro_target(node, state);
    if matches!(macro_target, SelectedMacroTarget::Ambiguous) {
        return None;
    }
    match macro_target {
        SelectedMacroTarget::Edit {
            instance_node_id,
            macro_name,
            params,
            source,
            position,
        } => Some(allocate_agentic_bubble_with_target(
            state,
            position,
            AgenticBubbleTarget::EditMacro {
                instance_node_id,
                macro_name,
                params,
                source,
            },
        )),
        SelectedMacroTarget::None => {
            let pan_state = state::get_patcher_pan_state(key);
            let position = state.last_pointer_model_position.unwrap_or_else(|| {
                geometry::screen_to_model(
                    node.rect,
                    &pan_state,
                    (node.rect.width * 0.5, node.rect.height * 0.5),
                )
            });
            Some(allocate_agentic_bubble(state, position))
        }
        SelectedMacroTarget::Ambiguous => None,
    }
}

enum SelectedMacroTarget {
    None,
    Ambiguous,
    Edit {
        instance_node_id: String,
        macro_name: String,
        params: Vec<String>,
        source: String,
        position: (f32, f32),
    },
}

fn selected_macro_target(
    node: &LayoutNode,
    state: &state::PatcherInteractionState,
) -> SelectedMacroTarget {
    let Ok((path, root_patch)) = load_patch_from_props(&node.props) else {
        return SelectedMacroTarget::None;
    };
    let source = std::fs::read_to_string(&path).ok();
    let view_key = active_patcher_view_key(state);
    let patch = active_patcher_patch(&root_patch, state);
    let patch = patch_with_interaction_state(patch, state, &view_key);
    let selected = patch
        .nodes
        .iter()
        .filter(|patch_node| {
            state.selected_nodes.contains(&patch_node.id)
                && patch_node.kind == NodeKind::MacroInstance
        })
        .collect::<Vec<_>>();
    let [patch_node] = selected.as_slice() else {
        return if selected.is_empty() {
            SelectedMacroTarget::None
        } else {
            SelectedMacroTarget::Ambiguous
        };
    };
    let macro_name = patch_node.op.clone();
    let root_patch = state::patch_with_created_macros(root_patch, state);
    let params = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == macro_name)
        .map(|macro_patch| macro_patch.params.clone())
        .unwrap_or_default();
    let source = state
        .edit_state
        .created_macros
        .get(&macro_name)
        .and_then(|edit| edit.source.clone())
        .or_else(|| {
            source
                .as_deref()
                .and_then(|source| macro_source(source, &macro_name))
        });
    let Some(source) = source else {
        return SelectedMacroTarget::None;
    };
    SelectedMacroTarget::Edit {
        instance_node_id: patch_node.id.clone(),
        macro_name,
        params,
        source,
        position: patch_node.position,
    }
}

fn macro_source(source: &str, macro_name: &str) -> Option<String> {
    let tokens = Parser::new(source.to_string()).parse().ok()?;
    let exprs = ASTParser::new(tokens).parse().ok()?;
    exprs.into_iter().find_map(|expr| {
        let Expression::List(items) = &expr else {
            return None;
        };
        match items.as_slice() {
            [Expression::Symbol(head), Expression::Symbol(name), ..]
                if head == "defmacro" && name == macro_name =>
            {
                Some(format_expression(&expr))
            }
            _ => None,
        }
    })
}

fn slug_agentic_macro_name(prompt: &str) -> String {
    let mut out = String::from("agentic");
    let mut last_dash = false;
    for ch in prompt.chars().flat_map(char::to_lowercase).take(64) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out == "agentic" {
        out.push_str("-macro");
    }
    out
}

fn patcher_widget_event(change: PatcherChangeKind) -> WidgetEvent {
    match change {
        PatcherChangeKind::None => WidgetEvent::Custom(Value::Nil),
        PatcherChangeKind::Layout => WidgetEvent::Custom(Value::Keyword("layout-change".into())),
        PatcherChangeKind::Semantic => {
            WidgetEvent::Custom(Value::Keyword("semantic-change".into()))
        }
    }
}

fn patcher_semantic_event(changed: bool) -> WidgetEvent {
    patcher_widget_event(if changed {
        PatcherChangeKind::Semantic
    } else {
        PatcherChangeKind::None
    })
}

fn patcher_change_output(node: &LayoutNode, payload: Value) -> Option<super::EventOutput> {
    let callback = node.props.get("on-change")?.clone();
    Some(super::EventOutput {
        callback,
        args: vec![payload],
    })
}

fn patcher_layout_payload(node: &LayoutNode) -> Value {
    let path = prop_str(&node.props, "path").or_else(|| prop_str(&node.props, "file"));
    let key = patcher_state_key(node);
    let state = get_patcher_interaction_state(key);
    let Some(path_str) = path else {
        return map_value(vec![
            ("status", Value::Keyword("invalid".to_string())),
            (
                "diagnostic",
                Value::String("patcher requires :path".to_string()),
            ),
        ]);
    };
    let path_buf = PathBuf::from(&path_str);
    let root_patch = match load_patch_from_props(&node.props) {
        Ok((_, patch)) => patch,
        Err(error) => {
            return map_value(vec![
                ("status", Value::Keyword("invalid".to_string())),
                ("path", Value::String(path_str)),
                (
                    "diagnostic",
                    Value::String(format!("failed to load patch for layout: {error}")),
                ),
            ]);
        }
    };
    match sidecar::current_layout_json(&root_patch, &state) {
        Ok(layout) => map_value(vec![
            ("status", Value::Keyword("layout".to_string())),
            ("path", Value::String(path_str)),
            ("layout", Value::String(layout)),
        ]),
        Err(error) => map_value(vec![
            ("status", Value::Keyword("invalid".to_string())),
            ("path", Value::String(path_str)),
            (
                "diagnostic",
                Value::String(format!(
                    "failed to build layout sidecar for '{}': {error}",
                    path_buf.display()
                )),
            ),
        ]),
    }
}

fn defmacro_library_root_for_props(props: &HashMap<String, Value>) -> Option<PathBuf> {
    prop_str(props, "defmacro-library-root")
        .map(PathBuf::from)
        .or_else(crate::defmacro_library::default_library_root)
}

struct LibraryCacheEntry {
    fingerprint: u64,
    library: Rc<DefmacroLibrary>,
}

thread_local! {
    static DEFMACRO_LIBRARY_CACHE: RefCell<HashMap<PathBuf, LibraryCacheEntry>> =
        RefCell::new(HashMap::new());
    static PATCH_LOAD_CACHE: RefCell<HashMap<PatchCacheKey, PatchCacheEntry>> =
        RefCell::new(HashMap::new());
}

/// Cheap content stamp for a library root: hashes file names and mtimes one
/// level deep so edits to package sources invalidate the cache without
/// re-reading any file contents.
fn defmacro_library_fingerprint(root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let stamp_entry = |path: &Path, hasher: &mut std::collections::hash_map::DefaultHasher| {
        path.hash(hasher);
        if let Ok(meta) = std::fs::metadata(path)
            && let Ok(modified) = meta.modified()
        {
            modified.hash(hasher);
        }
    };
    if let Ok(entries) = std::fs::read_dir(root) {
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            stamp_entry(&path, &mut hasher);
            if path.is_dir()
                && let Ok(children) = std::fs::read_dir(&path)
            {
                let mut child_paths: Vec<PathBuf> =
                    children.flatten().map(|entry| entry.path()).collect();
                child_paths.sort();
                for child in child_paths {
                    stamp_entry(&child, &mut hasher);
                }
            }
        }
    }
    hasher.finish()
}

/// Library macro entries for the macro sidebar:
/// (name, params, outputs, summary, imports). `imports` are the package's own
/// `use-defmacro` dependencies, so the sidebar can nest library call chains.
pub type MacroLibrarySidebarEntry = (
    String,
    Vec<String>,
    Vec<String>,
    Option<String>,
    Vec<String>,
);

pub fn macro_library_sidebar_entries() -> Vec<MacroLibrarySidebarEntry> {
    let Some(root) = crate::defmacro_library::default_library_root() else {
        return Vec::new();
    };
    let (_, library) = cached_defmacro_library(&root);
    library
        .packages()
        .values()
        .map(|package| {
            (
                package.name.clone(),
                package.params.clone(),
                package.outputs.clone(),
                package.manifest.summary.clone(),
                package.imports.clone(),
            )
        })
        .collect()
}

fn cached_defmacro_library(root: &Path) -> (u64, Rc<DefmacroLibrary>) {
    DEFMACRO_LIBRARY_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let fingerprint = defmacro_library_fingerprint(root);
        if let Some(entry) = cache.get(root)
            && entry.fingerprint == fingerprint
        {
            return (fingerprint, Rc::clone(&entry.library));
        }
        let library = match crate::defmacro_library::DefmacroLibrary::load_available(root) {
            Ok((library, errors)) => {
                for error in errors {
                    eprintln!(
                        "failed to load defmacro library package from '{}': {error}",
                        root.display()
                    );
                }
                library
            }
            Err(error) => {
                eprintln!(
                    "failed to load defmacro library '{}': {error}",
                    root.display()
                );
                DefmacroLibrary::empty(root)
            }
        };
        let library = Rc::new(library);
        cache.insert(
            root.to_path_buf(),
            LibraryCacheEntry {
                fingerprint,
                library: Rc::clone(&library),
            },
        );
        (fingerprint, library)
    })
}

fn defmacro_library_for_props(props: &HashMap<String, Value>) -> Option<Rc<DefmacroLibrary>> {
    let root = defmacro_library_root_for_props(props)?;
    Some(cached_defmacro_library(&root).1)
}

fn parse_patch_source_for_props(
    source: &str,
    intent: PatcherIntent,
    props: &HashMap<String, Value>,
) -> Result<Patch, String> {
    if let Some(library) = defmacro_library_for_props(props) {
        parse_patch_source_with_library(source, intent, &library)
    } else {
        parse_patch_source(source, intent)
    }
}

fn interaction_state_for_only_view(
    state: &state::PatcherInteractionState,
    view_key: &str,
) -> state::PatcherInteractionState {
    let mut filtered = state.clone();
    filtered
        .edit_state
        .nodes
        .retain(|_, edit| edit.view_key == view_key);
    filtered
        .edit_state
        .deleted_nodes
        .retain(|key| key.starts_with(&format!("{view_key}::")));
    filtered
        .edit_state
        .connections
        .retain(|_, edit| edit.view_key == view_key);
    filtered
        .edit_state
        .deleted_connections
        .retain(|key| key.starts_with(&format!("{view_key}::")));
    filtered
        .edit_state
        .input_presentations
        .retain(|_, edit| edit.view_key == view_key);
    filtered.edit_state.created_macros.clear();
    filtered.text_edit = filtered
        .text_edit
        .take()
        .filter(|_| active_patcher_view_key(state) == view_key);
    filtered.active_macro = view_key.strip_prefix("macro:").map(str::to_string);
    filtered
}

fn interaction_state_without_library_macro_views(
    state: &state::PatcherInteractionState,
    root_patch: &Patch,
) -> state::PatcherInteractionState {
    let library_views = root_patch
        .macros
        .iter()
        .filter(|macro_patch| matches!(macro_patch.origin, MacroOrigin::Library { .. }))
        .map(|macro_patch| format!("macro:{}", macro_patch.name))
        .collect::<std::collections::HashSet<_>>();
    if library_views.is_empty() {
        return state.clone();
    }
    let mut filtered = state.clone();
    filtered
        .edit_state
        .nodes
        .retain(|_, edit| !library_views.contains(&edit.view_key));
    filtered.edit_state.deleted_nodes.retain(|key| {
        !key.split_once("::")
            .is_some_and(|(view, _)| library_views.contains(view))
    });
    filtered
        .edit_state
        .connections
        .retain(|_, edit| !library_views.contains(&edit.view_key));
    filtered.edit_state.deleted_connections.retain(|key| {
        !key.split_once("::")
            .is_some_and(|(view, _)| library_views.contains(view))
    });
    filtered
        .edit_state
        .input_presentations
        .retain(|_, edit| !library_views.contains(&edit.view_key));
    if library_views.contains(&active_patcher_view_key(state)) {
        filtered.active_macro = None;
        filtered.text_edit = None;
    }
    filtered
}

fn clear_persisted_macro_view_edits(
    state: &mut state::PatcherInteractionState,
    macro_names: &[String],
) {
    if macro_names.is_empty() {
        return;
    }
    let views = macro_names
        .iter()
        .map(|name| format!("macro:{name}"))
        .collect::<std::collections::HashSet<_>>();
    state
        .edit_state
        .nodes
        .retain(|_, edit| !views.contains(&edit.view_key));
    state.edit_state.deleted_nodes.retain(|key| {
        !key.split_once("::")
            .is_some_and(|(view, _)| views.contains(view))
    });
    state
        .edit_state
        .connections
        .retain(|_, edit| !views.contains(&edit.view_key));
    state.edit_state.deleted_connections.retain(|key| {
        !key.split_once("::")
            .is_some_and(|(view, _)| views.contains(view))
    });
    state
        .edit_state
        .input_presentations
        .retain(|_, edit| !views.contains(&edit.view_key));
    for view in &views {
        state.z_order.remove(view);
    }
    if views.contains(&active_patcher_view_key(state)) {
        state.selected_nodes.clear();
        state.selected_cable = None;
        state.hovered_node = None;
        state.hovered_input_port = None;
        state.hovered_output_port = None;
        state.text_edit = None;
        state.drag = None;
    }
}

fn public_macro_library_action_kind(
    kind: state::PatcherMacroLibraryActionKind,
) -> MacroLibraryActionKind {
    match kind {
        state::PatcherMacroLibraryActionKind::SaveToLibrary => {
            MacroLibraryActionKind::SaveToLibrary
        }
        state::PatcherMacroLibraryActionKind::Fork => MacroLibraryActionKind::Fork,
    }
}

fn state_macro_library_action_kind(
    kind: MacroLibraryActionKind,
) -> state::PatcherMacroLibraryActionKind {
    match kind {
        MacroLibraryActionKind::SaveToLibrary => {
            state::PatcherMacroLibraryActionKind::SaveToLibrary
        }
        MacroLibraryActionKind::Fork => state::PatcherMacroLibraryActionKind::Fork,
    }
}

pub fn active_macro_library_action_for_path(
    path: impl AsRef<Path>,
    source: &str,
    intent: PatcherIntent,
) -> Result<Option<ActiveMacroLibraryAction>, String> {
    let Some(active_macro) = active_macro_name_for_path(path.as_ref()) else {
        return Ok(None);
    };
    let library = default_defmacro_library_for_write()?;
    let root_patch = parse_patch_source_with_library(source, intent, &library)?;
    let action = macro_library_action_for_macro(&root_patch, &active_macro);
    Ok(action.map(|kind| ActiveMacroLibraryAction {
        macro_name: active_macro,
        kind: public_macro_library_action_kind(kind),
    }))
}

pub fn apply_active_macro_library_action_for_path(
    path: impl AsRef<Path>,
    source: &str,
    layout: Option<&str>,
    intent: PatcherIntent,
) -> Result<MacroLibraryActionResult, String> {
    let path = path.as_ref();
    let action = active_macro_library_action_for_path(path, source, intent)?
        .ok_or_else(|| "No active macro is selected in the patch editor".to_string())?;
    let library = default_defmacro_library_for_write()?;
    let mut root_patch = parse_patch_source_with_library(source, intent, &library)?;
    if let Some(layout) = layout {
        sidecar::apply_layout_json(layout, "active patcher layout", &mut root_patch)?;
    }
    let state = active_interaction_state_for_path(path, &action.macro_name).unwrap_or_default();
    let props = HashMap::from([
        (
            "path".to_string(),
            Value::String(path.to_string_lossy().into_owned()),
        ),
        (
            "intent".to_string(),
            Value::Keyword(match intent {
                PatcherIntent::Effect => "effect".to_string(),
                PatcherIntent::Instrument => "instrument".to_string(),
            }),
        ),
        (
            "defmacro-library-root".to_string(),
            Value::String(library.root().to_string_lossy().into_owned()),
        ),
    ]);
    let writeback_state = match action.kind {
        MacroLibraryActionKind::SaveToLibrary => state.clone(),
        MacroLibraryActionKind::Fork => {
            interaction_state_without_library_macro_views(&state, &root_patch)
        }
    };
    let writeback_result = writeback::emit_patch_writeback_result_with_library(
        source,
        intent,
        &writeback_state,
        &library,
    )
    .map_err(|error| format!("{error:?}"))?;
    let emitted_layout = writeback_layout_for_source(
        &writeback_result.source,
        intent,
        &props,
        &root_patch,
        &writeback_state,
        &writeback_result.generated_node_ids,
    )?;
    let mut emitted_root_patch =
        parse_patch_source_with_library(&writeback_result.source, intent, &library)?;
    sidecar::apply_layout_json(
        &emitted_layout,
        "active patcher emitted layout",
        &mut emitted_root_patch,
    )?;
    let state_action = state::PatcherMacroLibraryAction {
        kind: state_macro_library_action_kind(action.kind),
        macro_name: action.macro_name.clone(),
    };
    let (source, layout) = apply_macro_library_action(
        writeback_result.source,
        emitted_layout,
        &state_action,
        &emitted_root_patch,
        intent,
        &props,
        &library,
        &writeback_state,
        &writeback_result.generated_node_ids,
    )?;
    Ok(MacroLibraryActionResult {
        macro_name: action.macro_name,
        kind: action.kind,
        source,
        layout,
    })
}

pub fn flush_staged_library_macro_edits_for_path(
    path: impl AsRef<Path>,
    source: &str,
    layout: Option<&str>,
    intent: PatcherIntent,
) -> Result<Vec<String>, String> {
    let path = path.as_ref();
    let Some((state_key, state)) = interaction_state_for_path(path) else {
        return Ok(Vec::new());
    };
    let library = default_defmacro_library_for_write()?;
    let mut root_patch = parse_patch_source_with_library(source, intent, &library)?;
    if let Some(layout) = layout {
        sidecar::apply_layout_json(layout, "active patcher layout", &mut root_patch)?;
    }
    let persisted = persist_library_macro_edits(&root_patch, intent, &state, &library)?;
    if persisted.is_empty() {
        return Ok(persisted);
    }
    let keys = state::patcher_keys_for_path(path);
    for key in &keys {
        let mut state = state::get_patcher_interaction_state(*key);
        clear_persisted_macro_view_edits(&mut state, &persisted);
        state::set_patcher_interaction_state(*key, state);
    }
    if !keys.contains(&state_key) {
        let mut state = state;
        clear_persisted_macro_view_edits(&mut state, &persisted);
        state::set_patcher_interaction_state(state_key, state);
    }
    Ok(persisted)
}

fn active_macro_name_for_path(path: &Path) -> Option<String> {
    state::patcher_keys_for_path(path)
        .into_iter()
        .rev()
        .find_map(|key| {
            state::get_patcher_interaction_state(key)
                .active_macro
                .filter(|name| !name.trim().is_empty())
        })
}

fn interaction_state_for_path(path: &Path) -> Option<(u64, state::PatcherInteractionState)> {
    state::patcher_keys_for_path(path)
        .into_iter()
        .rev()
        .map(|key| (key, state::get_patcher_interaction_state(key)))
        .enumerate()
        .max_by_key(|(idx, (_, state))| (interaction_edit_score(state), *idx))
        .map(|(_, entry)| entry)
}

fn interaction_edit_score(state: &state::PatcherInteractionState) -> usize {
    state.edit_state.nodes.len()
        + state.edit_state.connections.len()
        + state.edit_state.input_presentations.len()
        + state.edit_state.deleted_nodes.len()
        + state.edit_state.deleted_connections.len()
        + state.edit_state.created_macros.len()
        + usize::from(state.text_edit.is_some())
}

fn active_interaction_state_for_path(
    path: &Path,
    macro_name: &str,
) -> Option<state::PatcherInteractionState> {
    let view_key = format!("macro:{macro_name}");
    state::patcher_keys_for_path(path)
        .into_iter()
        .map(state::get_patcher_interaction_state)
        .filter(|state| state.active_macro.as_deref() == Some(macro_name))
        .enumerate()
        .max_by_key(|(idx, state)| (macro_view_edit_score(state, &view_key), *idx))
        .map(|(_, state)| state)
}

fn macro_view_edit_score(state: &state::PatcherInteractionState, view_key: &str) -> usize {
    let key_prefix = format!("{view_key}::");
    state
        .edit_state
        .nodes
        .values()
        .filter(|edit| edit.view_key == view_key)
        .count()
        + state
            .edit_state
            .connections
            .values()
            .filter(|edit| edit.view_key == view_key)
            .count()
        + state
            .edit_state
            .input_presentations
            .values()
            .filter(|edit| edit.view_key == view_key)
            .count()
        + state
            .edit_state
            .deleted_nodes
            .iter()
            .filter(|key| key.starts_with(&key_prefix))
            .count()
        + state
            .edit_state
            .deleted_connections
            .iter()
            .filter(|key| key.starts_with(&key_prefix))
            .count()
        + usize::from(
            state
                .text_edit
                .as_ref()
                .is_some_and(|_| active_patcher_view_key(state) == view_key),
        )
}

fn library_macro_view_has_edits(state: &state::PatcherInteractionState, view_key: &str) -> bool {
    let scoped_prefix = format!("{view_key}::");
    state
        .edit_state
        .nodes
        .values()
        .any(|edit| edit.view_key == view_key)
        || state
            .edit_state
            .connections
            .values()
            .any(|edit| edit.view_key == view_key)
        || state
            .edit_state
            .deleted_nodes
            .iter()
            .any(|key| key.starts_with(&scoped_prefix))
        || state
            .edit_state
            .deleted_connections
            .iter()
            .any(|key| key.starts_with(&scoped_prefix))
        || state
            .edit_state
            .input_presentations
            .values()
            .any(|edit| edit.view_key == view_key)
}

fn macro_library_action_for_macro(
    root_patch: &Patch,
    macro_name: &str,
) -> Option<state::PatcherMacroLibraryActionKind> {
    let macro_patch = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == macro_name)?;
    match macro_patch.origin {
        MacroOrigin::Local => Some(state::PatcherMacroLibraryActionKind::SaveToLibrary),
        MacroOrigin::Library { .. } => Some(state::PatcherMacroLibraryActionKind::Fork),
    }
}

fn default_defmacro_library_for_write() -> Result<DefmacroLibrary, String> {
    let root = crate::defmacro_library::default_library_root()
        .ok_or_else(|| "defmacro library root was not found".to_string())?;
    let (library, errors) =
        DefmacroLibrary::load_available(&root).map_err(|error| error.to_string())?;
    for error in errors {
        eprintln!(
            "failed to load defmacro library package from '{}': {error}",
            root.display()
        );
    }
    Ok(library)
}

fn staged_library_macro_writeback(
    macro_patch: &MacroPatch,
    intent: PatcherIntent,
    state: &state::PatcherInteractionState,
    library: &crate::defmacro_library::DefmacroLibrary,
) -> Result<writeback::PatchWritebackResult, String> {
    let MacroOrigin::Library { source_path, .. } = &macro_patch.origin else {
        return Err(format!(
            "cannot stage `{}`: macro is not from the library",
            macro_patch.name
        ));
    };
    let source_path = PathBuf::from(source_path);
    let source = std::fs::read_to_string(&source_path)
        .map_err(|error| format!("failed to read '{}': {error}", source_path.display()))?;
    let view_key = format!("macro:{}", macro_patch.name);
    let library_state = interaction_state_for_only_view(state, &view_key);
    writeback::emit_patch_writeback_result_with_library(&source, intent, &library_state, library)
        .map_err(|error| format!("{error:?}"))
}

fn library_with_staged_macro_edits(
    root_patch: &Patch,
    intent: PatcherIntent,
    state: &state::PatcherInteractionState,
    library: &crate::defmacro_library::DefmacroLibrary,
) -> Result<crate::defmacro_library::DefmacroLibrary, String> {
    let mut staged_library = library.clone();
    for macro_patch in &root_patch.macros {
        if !matches!(macro_patch.origin, MacroOrigin::Library { .. }) {
            continue;
        }
        let view_key = format!("macro:{}", macro_patch.name);
        if !library_macro_view_has_edits(state, &view_key) {
            continue;
        }
        let emitted = staged_library_macro_writeback(macro_patch, intent, state, &staged_library)?;
        staged_library = staged_library
            .with_package_source(&macro_patch.name, &emitted.source)
            .map_err(|error| error.to_string())?;
    }
    Ok(staged_library)
}

fn persist_library_macro_edits(
    root_patch: &Patch,
    intent: PatcherIntent,
    state: &state::PatcherInteractionState,
    library: &crate::defmacro_library::DefmacroLibrary,
) -> Result<Vec<String>, String> {
    let mut persisted = Vec::new();
    for macro_patch in &root_patch.macros {
        let MacroOrigin::Library {
            source_path,
            layout_path,
        } = &macro_patch.origin
        else {
            continue;
        };
        let view_key = format!("macro:{}", macro_patch.name);
        if !library_macro_view_has_edits(state, &view_key) {
            continue;
        }
        let source_path = PathBuf::from(source_path);
        let source = std::fs::read_to_string(&source_path)
            .map_err(|error| format!("failed to read '{}': {error}", source_path.display()))?;
        let library_state = interaction_state_for_only_view(state, &view_key);
        let mut previous_patch = parse_patch_source_for_props_like_library_source(
            &source,
            intent,
            library,
            &macro_patch.name,
        )?;
        sidecar::apply_layout_file(&PathBuf::from(layout_path), &mut previous_patch)?;
        let emitted = writeback::emit_patch_writeback_result_with_library(
            &source,
            intent,
            &library_state,
            library,
        )
        .map_err(|error| format!("{error:?}"))?;
        let package_dir = source_path.parent().ok_or_else(|| {
            format!(
                "failed to resolve package directory for '{}'",
                source_path.display()
            )
        })?;
        let package = DefmacroPackage::from_source(package_dir, &macro_patch.name, &emitted.source)
            .map_err(|error| error.to_string())?;
        let mut emitted_patch = parse_patch_source_for_props_like_library_source(
            &emitted.source,
            intent,
            library,
            &macro_patch.name,
        )?;
        let layout = sidecar::emitted_layout_json_with_node_map(
            &mut emitted_patch,
            &previous_patch,
            &library_state,
            &emitted.generated_node_ids,
        )?;
        write_atomic_text(&source_path, &emitted.source)?;
        write_atomic_text(&PathBuf::from(layout_path), &layout)?;
        let manifest_json = serde_json::to_string_pretty(&package.rebuilt_manifest())
            .map(|json| format!("{json}\n"))
            .map_err(|error| {
                format!(
                    "failed to serialize `{}` manifest: {error}",
                    macro_patch.name
                )
            })?;
        write_atomic_text(&package.manifest_path, &manifest_json)?;
        persisted.push(macro_patch.name.clone());
    }
    Ok(persisted)
}

fn parse_patch_source_for_props_like_library_source(
    source: &str,
    intent: PatcherIntent,
    library: &crate::defmacro_library::DefmacroLibrary,
    macro_name: &str,
) -> Result<Patch, String> {
    let mut patch = parse_patch_source_with_library(source, intent, library)?;
    patch
        .macros
        .retain(|macro_patch| macro_patch.name == macro_name);
    Ok(patch)
}

fn write_atomic_text(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
    }
    let tmp_path = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("patcher-write"),
        std::process::id()
    ));
    std::fs::write(&tmp_path, contents)
        .map_err(|error| format!("failed to write '{}': {error}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("failed to replace '{}': {error}", path.display())
    })
}

fn writeback_layout_for_source(
    source: &str,
    intent: PatcherIntent,
    props: &HashMap<String, Value>,
    previous_root_patch: &Patch,
    interaction_state: &state::PatcherInteractionState,
    generated_node_ids: &std::collections::HashMap<(String, String), String>,
) -> Result<String, String> {
    let mut emitted_patch = parse_patch_source_for_props(source, intent, props)?;
    sidecar::emitted_layout_json_with_node_map(
        &mut emitted_patch,
        previous_root_patch,
        interaction_state,
        generated_node_ids,
    )
}

fn apply_macro_library_action(
    source: String,
    layout: String,
    action: &state::PatcherMacroLibraryAction,
    root_patch: &Patch,
    intent: PatcherIntent,
    props: &HashMap<String, Value>,
    library: &DefmacroLibrary,
    interaction_state: &state::PatcherInteractionState,
    generated_node_ids: &std::collections::HashMap<(String, String), String>,
) -> Result<(String, String), String> {
    match action.kind {
        state::PatcherMacroLibraryActionKind::SaveToLibrary => save_local_macro_to_library(
            source,
            layout,
            &action.macro_name,
            root_patch,
            intent,
            props,
            library,
            interaction_state,
            generated_node_ids,
        ),
        state::PatcherMacroLibraryActionKind::Fork => fork_library_macro_to_local(
            source,
            &action.macro_name,
            root_patch,
            intent,
            props,
            library,
            interaction_state,
            generated_node_ids,
        ),
    }
}

/// Prefix `macro_source` with a `(use-defmacro …)` line for every library
/// package it calls, so a saved package carries its own dependencies. Already
/// present imports are kept as-is and the macro's own name is never imported.
fn with_library_imports(macro_source: &str, macro_name: &str, library: &DefmacroLibrary) -> String {
    let Ok(tokens) = Parser::new(macro_source.to_string()).parse() else {
        return macro_source.to_string();
    };
    let Ok(exprs) = ASTParser::new(tokens).parse() else {
        return macro_source.to_string();
    };
    let mut declared = exprs
        .iter()
        .filter_map(|expr| match expr {
            Expression::List(items) if lisp::symbol_at(items, 0) == Some("use-defmacro") => {
                lisp::symbol_at(items, 1).map(str::to_string)
            }
            _ => None,
        })
        .collect::<HashSet<_>>();
    declared.insert(macro_name.to_string());

    let mut missing = std::collections::BTreeSet::new();
    let mut stack = exprs.iter().collect::<Vec<_>>();
    while let Some(expr) = stack.pop() {
        let (Expression::List(items) | Expression::QuoteList(items)) = expr else {
            continue;
        };
        if let Some(op) = lisp::symbol_at(items, 0)
            && !declared.contains(op)
            && library.packages().contains_key(op)
        {
            missing.insert(op.to_string());
        }
        stack.extend(items.iter());
    }
    if missing.is_empty() {
        return macro_source.to_string();
    }
    let imports = missing
        .iter()
        .map(|name| format!("(use-defmacro {name})"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{imports}\n{macro_source}")
}

fn save_local_macro_to_library(
    source: String,
    layout: String,
    macro_name: &str,
    root_patch: &Patch,
    intent: PatcherIntent,
    props: &HashMap<String, Value>,
    library: &DefmacroLibrary,
    interaction_state: &state::PatcherInteractionState,
    generated_node_ids: &std::collections::HashMap<(String, String), String>,
) -> Result<(String, String), String> {
    eprintln!(
        "[patcher macro-library-action save start] macro={macro_name} library_root={} source_len={} layout_len={}",
        library.root().display(),
        source.len(),
        layout.len()
    );
    let Some(macro_patch) = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == macro_name)
    else {
        return Err(format!(
            "cannot save `{macro_name}`: local macro was not found"
        ));
    };
    if !matches!(macro_patch.origin, MacroOrigin::Local) {
        return Err(format!(
            "cannot save `{macro_name}`: macro is already from the library"
        ));
    }
    let package_dir = library.root().join(macro_name);
    if package_dir.exists() {
        eprintln!(
            "[patcher macro-library-action save blocked] macro={macro_name} package_dir={} reason=exists",
            package_dir.display()
        );
        return Err(format!(
            "cannot save `{macro_name}`: library package already exists"
        ));
    }
    let macro_source = writeback::extract_macro_source(&source, macro_name)
        .map_err(|error| format!("{error:?}"))?;
    // The extracted form is the `defmacro` alone. Any library macro it calls
    // must be re-declared as the package's own `use-defmacro` dependency, or
    // materializing the package into another patch resolves the call against
    // nothing and the compiler reports "Unknown operator".
    let macro_source = with_library_imports(&macro_source, macro_name, library);
    let package = DefmacroPackage::from_source(&package_dir, macro_name, &macro_source)
        .map_err(|error| error.to_string())?;
    let package_layout = layout_json_for_single_macro_scope(&layout, macro_name)?;
    let manifest_json = serde_json::to_string_pretty(&package.rebuilt_manifest())
        .map(|json| format!("{json}\n"))
        .map_err(|error| format!("failed to serialize `{macro_name}` manifest: {error}"))?;

    write_atomic_text(&package.source_path, &format!("{macro_source}\n"))?;
    eprintln!(
        "[patcher macro-library-action save wrote-source] macro={macro_name} path={}",
        package.source_path.display()
    );
    write_atomic_text(&package.layout_path, &package_layout)?;
    eprintln!(
        "[patcher macro-library-action save wrote-layout] macro={macro_name} path={}",
        package.layout_path.display()
    );
    write_atomic_text(&package.manifest_path, &manifest_json)?;
    eprintln!(
        "[patcher macro-library-action save wrote-manifest] macro={macro_name} path={}",
        package.manifest_path.display()
    );

    let imported_source = writeback::replace_macro_with_import(&source, macro_name)
        .map_err(|error| format!("{error:?}"))?;
    let root_state = interaction_state_without_library_macro_views(interaction_state, root_patch);
    let final_layout = writeback_layout_for_source(
        &imported_source,
        intent,
        props,
        root_patch,
        &root_state,
        generated_node_ids,
    )
    .and_then(|layout| remove_macro_scope_from_layout_json(&layout, macro_name))?;
    eprintln!(
        "[patcher macro-library-action save success] macro={macro_name} package_dir={}",
        package_dir.display()
    );
    Ok((imported_source, final_layout))
}

fn fork_library_macro_to_local(
    source: String,
    macro_name: &str,
    root_patch: &Patch,
    intent: PatcherIntent,
    props: &HashMap<String, Value>,
    library: &DefmacroLibrary,
    interaction_state: &state::PatcherInteractionState,
    generated_node_ids: &std::collections::HashMap<(String, String), String>,
) -> Result<(String, String), String> {
    eprintln!(
        "[patcher macro-library-action fork start] macro={macro_name} library_root={} source_len={}",
        library.root().display(),
        source.len()
    );
    let Some(macro_patch) = root_patch
        .macros
        .iter()
        .find(|macro_patch| macro_patch.name == macro_name)
    else {
        return Err(format!(
            "cannot fork `{macro_name}`: library macro was not found"
        ));
    };
    if !matches!(macro_patch.origin, MacroOrigin::Library { .. }) {
        return Err(format!(
            "cannot fork `{macro_name}`: macro is already local"
        ));
    }
    let Some(package) = library.package(macro_name) else {
        return Err(format!(
            "cannot fork `{macro_name}`: library package was not found"
        ));
    };
    let forked_source = writeback::replace_import_with_macro(&source, macro_name, &package.source)
        .map_err(|error| format!("{error:?}"))?;
    let final_layout = writeback_layout_for_source(
        &forked_source,
        intent,
        props,
        root_patch,
        interaction_state,
        generated_node_ids,
    )
    .and_then(|layout| merge_package_macro_layout(&layout, &package.layout_path, macro_name))?;
    eprintln!(
        "[patcher macro-library-action fork success] macro={macro_name} source_path={} layout_path={}",
        package.source_path.display(),
        package.layout_path.display()
    );
    Ok((forked_source, final_layout))
}

fn layout_json_for_single_macro_scope(layout: &str, macro_name: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(layout)
        .map_err(|error| format!("failed to parse emitted layout json: {error}"))?;
    let scope = value
        .get("macros")
        .and_then(|macros| macros.get(macro_name))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    serde_json::to_string_pretty(&serde_json::json!({
        "version": 1,
        "root": {},
        "macros": {
            macro_name: scope
        }
    }))
    .map(|json| format!("{json}\n"))
    .map_err(|error| format!("failed to serialize `{macro_name}` layout: {error}"))
}

fn remove_macro_scope_from_layout_json(layout: &str, macro_name: &str) -> Result<String, String> {
    let mut value: serde_json::Value = serde_json::from_str(layout)
        .map_err(|error| format!("failed to parse emitted layout json: {error}"))?;
    if let Some(macros) = value
        .get_mut("macros")
        .and_then(|macros| macros.as_object_mut())
    {
        macros.remove(macro_name);
    }
    serde_json::to_string_pretty(&value)
        .map(|json| format!("{json}\n"))
        .map_err(|error| format!("failed to serialize root layout: {error}"))
}

fn merge_package_macro_layout(
    layout: &str,
    package_layout_path: &Path,
    macro_name: &str,
) -> Result<String, String> {
    if !package_layout_path.exists() {
        return Ok(layout.to_string());
    }
    let package_layout = std::fs::read_to_string(package_layout_path).map_err(|error| {
        format!(
            "failed to read library macro layout '{}': {error}",
            package_layout_path.display()
        )
    })?;
    let package_value: serde_json::Value =
        serde_json::from_str(&package_layout).map_err(|error| {
            format!(
                "failed to parse library macro layout '{}': {error}",
                package_layout_path.display()
            )
        })?;
    let Some(scope) = package_value
        .get("macros")
        .and_then(|macros| macros.get(macro_name))
        .cloned()
    else {
        return Ok(layout.to_string());
    };
    let mut value: serde_json::Value = serde_json::from_str(layout)
        .map_err(|error| format!("failed to parse emitted layout json: {error}"))?;
    if value
        .get("macros")
        .and_then(|macros| macros.as_object())
        .is_none()
    {
        value["macros"] = serde_json::json!({});
    }
    let Some(macros) = value
        .get_mut("macros")
        .and_then(|macros| macros.as_object_mut())
    else {
        return Err("emitted layout macros field is not an object".to_string());
    };
    macros.insert(macro_name.to_string(), scope);
    serde_json::to_string_pretty(&value)
        .map(|json| format!("{json}\n"))
        .map_err(|error| format!("failed to serialize forked layout: {error}"))
}

fn patcher_writeback_payload(node: &LayoutNode) -> Value {
    let path = prop_str(&node.props, "path").or_else(|| prop_str(&node.props, "file"));
    let intent = patcher_intent_from_props(&node.props);
    let key = patcher_state_key(node);
    let state = get_patcher_interaction_state(key);

    let Some(path_str) = path else {
        return map_value(vec![
            ("status", Value::Keyword("invalid".to_string())),
            (
                "diagnostic",
                Value::String("patcher requires :path".to_string()),
            ),
        ]);
    };
    let root_patch = match load_patch_from_props(&node.props) {
        Ok((_, patch)) => patch,
        Err(error) => {
            debug_log_writeback_event(
                "layout-source-load-failed",
                format!("path={path_str}\nintent={intent:?}\nerror={error}"),
            );
            return map_value(vec![
                ("status", Value::Keyword("invalid".to_string())),
                ("path", Value::String(path_str)),
                (
                    "diagnostic",
                    Value::String(format!(
                        "failed to load current patch for layout persistence: {error}"
                    )),
                ),
            ]);
        }
    };

    let library = defmacro_library_for_props(&node.props);
    let root_state = if library.is_some() {
        interaction_state_without_library_macro_views(&state, &root_patch)
    } else {
        state.clone()
    };
    // Full deterministic regeneration from the in-memory model
    // (docs/patch-vs-code-editor-spec.md §4): no surgical source rewriting,
    // no source-position reasoning.
    let visible = sidecar::root_patch_with_interaction(&root_patch, &root_state);
    let generated = match generate::generate_patch_source(&visible, intent) {
        Ok(generated) => generated,
        Err(error) => {
            debug_log_edit_event("generate-payload-invalid-state", &state);
            debug_log_writeback_event(
                "payload-invalid",
                format!("path={path_str}\nintent={intent:?}\nerror={error}"),
            );
            return map_value(vec![
                ("status", Value::Keyword("invalid".to_string())),
                ("path", Value::String(path_str)),
                ("diagnostic", Value::String(error)),
            ]);
        }
    };
    let source = generated.source;
    let mut emitted_patch = match parse_patch_source_for_props(&source, intent, &node.props) {
        Ok(patch) => patch,
        Err(error) => {
            eprintln!(
                "[patcher generate invalid]\npath={path_str}\nintent={intent:?}\nstage=parse-generated-source\nerror={error}\ngenerated-source:\n{source}\n[/patcher generate invalid]"
            );
            return map_value(vec![
                ("status", Value::Keyword("invalid".to_string())),
                ("path", Value::String(path_str)),
                (
                    "diagnostic",
                    Value::String(format!("generated source failed to parse: {error}")),
                ),
            ]);
        }
    };
    if !patch_is_fully_projectable(&emitted_patch) {
        eprintln!(
            "[patcher generate invalid]\npath={path_str}\nintent={intent:?}\nstage=projectability\ndiagnostics={:?}\ngenerated-source:\n{source}\n[/patcher generate invalid]",
            emitted_patch.diagnostics
        );
        return map_value(vec![
            ("status", Value::Keyword("invalid".to_string())),
            ("path", Value::String(path_str)),
            (
                "diagnostic",
                Value::String(format!(
                    "generated source is not fully projectable: {}",
                    emitted_patch.diagnostics.join("; ")
                )),
            ),
        ]);
    }
    let layout = match sidecar::emitted_layout_json_with_node_map(
        &mut emitted_patch,
        &root_patch,
        &root_state,
        &generated.renamed_node_ids,
    ) {
        Ok(layout) => layout,
        Err(error) => {
            return map_value(vec![
                ("status", Value::Keyword("invalid".to_string())),
                ("path", Value::String(path_str)),
                (
                    "diagnostic",
                    Value::String(format!("failed to build emitted patcher layout: {error}")),
                ),
            ]);
        }
    };
    debug_log_writeback_event(
        "payload-valid",
        format!("path={path_str}\nintent={intent:?}\nsource:\n{source}"),
    );
    let compile_source = if let Some(library) = library.as_ref() {
        let staged_library =
            match library_with_staged_macro_edits(&root_patch, intent, &state, library) {
                Ok(library) => library,
                Err(error) => {
                    return map_value(vec![
                        ("status", Value::Keyword("invalid".to_string())),
                        ("path", Value::String(path_str)),
                        (
                            "diagnostic",
                            Value::String(format!("failed to stage library macro edits: {error}")),
                        ),
                    ]);
                }
            };
        match staged_library.materialize_source(&source) {
            Ok(materialized) => materialized.source,
            Err(error) => {
                return map_value(vec![
                    ("status", Value::Keyword("invalid".to_string())),
                    ("path", Value::String(path_str)),
                    (
                        "diagnostic",
                        Value::String(format!(
                            "failed to materialize staged defmacro imports: {error}"
                        )),
                    ),
                ]);
            }
        }
    } else {
        source.clone()
    };
    map_value(vec![
        ("status", Value::Keyword("valid".to_string())),
        ("path", Value::String(path_str)),
        ("source", Value::String(source)),
        ("compile-source", Value::String(compile_source)),
        ("layout", Value::String(layout)),
    ])
}

fn patcher_intent_from_props(props: &HashMap<String, Value>) -> PatcherIntent {
    match props.get("intent") {
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "effect" => {
            PatcherIntent::Effect
        }
        _ => PatcherIntent::Instrument,
    }
}

fn map_value(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), Rc::new(RefCell::new(value))))
            .collect(),
    )
}

fn prop_keyword(node: &Value, key: &str) -> Option<String> {
    let Value::Map(map) = node else {
        return None;
    };
    map.get(key).and_then(|value| match &*value.borrow() {
        Value::Keyword(value) | Value::String(value) => Some(value.clone()),
        _ => None,
    })
}

fn debug_patch_for_state(
    node: &LayoutNode,
    state: &state::PatcherInteractionState,
    view_key: &str,
) -> Option<Patch> {
    let (_, root_patch) = load_patch_from_props(&node.props).ok()?;
    let patch = active_patcher_patch(&root_patch, state);
    Some(patch_with_interaction_state(patch, state, view_key))
}

fn autocomplete_macros_for_state(
    node: &LayoutNode,
    state: &state::PatcherInteractionState,
    view_key: &str,
) -> Vec<MacroPatch> {
    let patch = debug_patch_for_state(node, state, view_key);
    autocomplete_macros_for_patch(&node.props, patch.as_ref())
}

pub(super) fn autocomplete_macros_for_patch(
    props: &HashMap<String, Value>,
    patch: Option<&Patch>,
) -> Vec<MacroPatch> {
    let mut macros = patch.map(|patch| patch.macros.clone()).unwrap_or_default();
    if let Some(library) = defmacro_library_for_props(props) {
        let existing = macros
            .iter()
            .map(|macro_patch| macro_patch.name.clone())
            .collect::<std::collections::HashSet<_>>();
        for package in library.packages().values() {
            if existing.contains(&package.name) {
                continue;
            }
            macros.push(MacroPatch {
                name: package.name.clone(),
                params: package.params.clone(),
                outputs: package.outputs.clone(),
                patch: Patch::default(),
                origin: MacroOrigin::Library {
                    source_path: package.source_path.to_string_lossy().to_string(),
                    layout_path: package.layout_path.to_string_lossy().to_string(),
                },
            });
        }
    }
    macros
}

fn toggle_selected_cable_segmented(
    node: &LayoutNode,
    state: &mut state::PatcherInteractionState,
    view_key: &str,
) -> bool {
    let selected_cable = match state.selected_cable.clone() {
        Some(selected_cable) => selected_cable,
        None => return false,
    };
    let Ok((_, root_patch)) = load_patch_from_props(&node.props) else {
        return false;
    };
    let patch = active_patcher_patch(&root_patch, state);
    let patch = patch_with_interaction_state(patch, state, view_key);
    let Some(connection) = patch
        .connections
        .iter()
        .find(|connection| state::source_connection_id(connection) == selected_cable)
        .cloned()
    else {
        return false;
    };
    let mut segment = connection.segment.unwrap_or(CableSegmentInfo {
        is_segmented: false,
        segment_row: 0.0,
    });
    segment.is_segmented = !segment.is_segmented;
    if segment.is_segmented && segment.segment_row == 0.0 {
        let pan_state = state::get_patcher_pan_state(patcher_state_key(node));
        let node_rects = geometry::patch_node_rects(&patch, node.rect, &pan_state);
        let input_indices = geometry::patch_input_indices(&patch);
        let input_slot_counts = geometry::patch_input_slot_counts(&patch, &input_indices);
        let output_counts = geometry::patch_output_counts(&patch);
        if let Some((start, end)) = geometry::connection_endpoints(
            &connection,
            &node_rects,
            &input_indices,
            &input_slot_counts,
            &output_counts,
        ) {
            let midpoint = ((start.0 + end.0) * 0.5, (start.1 + end.1) * 0.5);
            segment.segment_row = geometry::screen_to_model(node.rect, &pan_state, midpoint).1;
        }
    }
    set_connection_segment_edit(state, view_key, &connection, Some(segment));
    state.drag = None;
    true
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct PatchCacheKey {
    path: PathBuf,
    effect_intent: bool,
    library_root: Option<PathBuf>,
}

struct PatchCacheEntry {
    source_mtime: Option<std::time::SystemTime>,
    sidecar_mtime: Option<std::time::SystemTime>,
    library_fingerprint: u64,
    patch: Patch,
}

fn file_mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

pub(super) fn load_patch_from_props(
    props: &HashMap<String, Value>,
) -> Result<(PathBuf, Patch), String> {
    let path = prop_str(props, "path")
        .or_else(|| prop_str(props, "file"))
        .ok_or_else(|| "patcher requires :path".to_string())?;
    let path = PathBuf::from(path);
    let intent = patcher_intent_from_props(props);
    let library_root = defmacro_library_root_for_props(props);
    let library_fingerprint = library_root
        .as_deref()
        .map(|root| cached_defmacro_library(root).0)
        .unwrap_or(0);
    let key = PatchCacheKey {
        path: path.clone(),
        effect_intent: intent == PatcherIntent::Effect,
        library_root,
    };
    let source_mtime = file_mtime(&path);
    let sidecar_mtime = file_mtime(&sidecar::sidecar_path_for_source(&path));
    let cached = PATCH_LOAD_CACHE.with(|cache| {
        cache.borrow().get(&key).and_then(|entry| {
            (source_mtime.is_some()
                && entry.source_mtime == source_mtime
                && entry.sidecar_mtime == sidecar_mtime
                && entry.library_fingerprint == library_fingerprint)
                .then(|| entry.patch.clone())
        })
    });
    if let Some(patch) = cached {
        return Ok((path, patch));
    }
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    let mut patch = parse_patch_source_for_props(&source, intent, props)?;
    sidecar::apply_or_materialize(&path, &mut patch)?;
    // apply_or_materialize may have just written the sidecar; stamp after it ran.
    let sidecar_mtime = file_mtime(&sidecar::sidecar_path_for_source(&path));
    PATCH_LOAD_CACHE.with(|cache| {
        cache.borrow_mut().insert(
            key,
            PatchCacheEntry {
                source_mtime,
                sidecar_mtime,
                library_fingerprint,
                patch: patch.clone(),
            },
        );
    });
    Ok((path, patch))
}

pub(in crate::widget_render::patcher) fn persist_patcher_layout(
    node: &LayoutNode,
    state: &state::PatcherInteractionState,
) -> Result<(), String> {
    let (path, root_patch) = load_patch_from_props(&node.props)?;
    sidecar::save_current_layout(&path, &root_patch, state)
}

/// Measure every string a bubble might render, so the render pass can wrap them
/// (it can only wrap text whose glyph widths are already cached on this thread).
fn cache_agentic_bubble_text_widths(bubble: &state::AgenticBubble, ctx: &MeasureCtx<'_>) {
    cache_text_widths(bubble.body_text(), 13.0, ctx);
    // The prompt is measured even once an answer has replaced it: the answer's
    // arrival eases the box out of the prompt's layout, so that layout has to
    // stay wrappable for the length of the transition.
    cache_text_widths(bubble.prompt_text(), 13.0, ctx);
    if matches!(bubble.state, state::AgenticBubbleState::Editing) {
        cache_text_widths(bubble.prompt.clone(), 13.0, ctx);
    }
}

fn cache_patcher_text_widths(node: &Value, ctx: &MeasureCtx<'_>) {
    if ctx.text_measurer.is_none() {
        return;
    }
    let Some(props) = owned_props_from_value(node) else {
        return;
    };
    let key = patcher_state_key_from_parts(get_stable_widget_id(node), &props);
    let interaction_state = get_patcher_interaction_state(key);
    let Ok((_, root_patch)) = load_patch_from_props(&props) else {
        return;
    };
    let view_key = active_patcher_view_key(&interaction_state);
    let patch = active_patcher_patch(&root_patch, &interaction_state);
    let patch = patch_with_interaction_state(patch, &interaction_state, &view_key);
    for patch_node in &patch.nodes {
        let font_size = display::node_font_size(patch_node);
        cache_text_widths(node_display_label(patch_node), font_size, ctx);
    }
    if let Some(tooltip) = interaction_state
        .hovered_input_port
        .as_ref()
        .and_then(|port| render::input_port_tooltip(&patch, port))
        .or_else(|| {
            interaction_state
                .hovered_output_port
                .as_ref()
                .and_then(|port| render::output_port_tooltip(&patch, port))
        })
    {
        cache_text_widths(preview(&tooltip, 48), 10.5, ctx);
    }
    if let Some(edit) = interaction_state.text_edit {
        if let Some(edit_node) = patch.nodes.iter().find(|node| node.id == edit.node_id) {
            cache_text_widths(edit.text, display::node_font_size(edit_node), ctx);
        }
    }
    for bubble in interaction_state.agentic_bubbles.values() {
        cache_agentic_bubble_text_widths(bubble, ctx);
    }
}

fn owned_props_from_value(node: &Value) -> Option<HashMap<String, Value>> {
    let Value::Map(map) = node else {
        return None;
    };
    Some(
        map.iter()
            .map(|(key, value)| (key.clone(), value.borrow().clone()))
            .collect(),
    )
}

pub(super) fn prop_str(props: &HashMap<String, Value>, key: &str) -> Option<String> {
    props.get(key).and_then(|value| match value {
        Value::String(value) | Value::Keyword(value) | Value::Symbol(value) => Some(value.clone()),
        _ => None,
    })
}
