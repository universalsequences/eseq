mod display;
mod emit;
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
mod writeback;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

pub use lisp::parse_patch_source;
pub use model::{
    ArgSource, ArgValue, AttributeSource, BindingId, BindingKind, BindingTarget, CableSegmentInfo,
    CallSourceShape, ConnectionKind, ConnectionSource, ExprPath, ExprPathSegment, MacroPatch,
    NodeKind, NodeSource, Patch, PatchConnection, PatchNode, PatcherIntent, SourceArgValue,
    SourceExprId, SourceFormId, SourceOwner, SourceScopeId,
};

pub fn emit_patch_writeback_source(source: &str, intent: PatcherIntent) -> Result<String, String> {
    let state = state::PatcherInteractionState::default();
    writeback::emit_patch_writeback(source, intent, &state).map_err(|error| format!("{error:?}"))
}

pub fn emitted_source_buffer_name(path: &str) -> String {
    format!("*patcher-emitted:{path}*")
}

pub fn emitted_source_path_from_buffer_name(name: &str) -> Option<String> {
    name.strip_prefix("*patcher-emitted:")
        .and_then(|path| path.strip_suffix('*'))
        .map(str::to_string)
}

pub struct EmittedSourceBufferSnapshot {
    pub path: String,
    pub buffer_name: String,
    pub source: String,
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
        );
        if !wrote {
            let emitted =
                writeback::emit_patch_writeback(&source, intent, &materialized.writeback_state)
                    .map_err(|error| format!("{error:?}"))?;
            std::fs::write(path, emitted)
                .map_err(|error| format!("failed to write '{}': {error}", path.display()))?;
            wrote = true;
        }
        interaction
            .agentic_morph_nodes
            .insert(materialized.instance_node_id.clone(), Instant::now());
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
    let emitted = writeback::replace_macro_source(&source, macro_name, macro_source)
        .map_err(|error| format!("{error:?}"))?;
    std::fs::write(path, emitted)
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
            interaction
                .agentic_morph_nodes
                .insert(instance_node_id, Instant::now());
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
) -> MaterializedAgenticMacro {
    let node_id = state::allocate_created_node(interaction, "root", position);
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

    writeback::emit_patch_writeback(source, intent, &state).map_err(|error| format!("{error:?}"))
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

    writeback::emit_patch_writeback(source, intent, &state).map_err(|error| format!("{error:?}"))
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

    writeback::emit_patch_writeback(source, intent, &state).map_err(|error| format!("{error:?}"))
}

use display::{node_display_label, preview};
use emit::debug_log_patch_lisp;
use interaction::{
    PatcherChangeKind, handle_patcher_double_click, handle_patcher_pointer_down,
    handle_patcher_pointer_drag, handle_patcher_pointer_moved, handle_patcher_pointer_up,
    open_selected_macro_node, pan_patcher_by_delta, pan_patcher_by_wheel,
    promote_created_macro_definition, reset_patcher_pan, zoom_patcher_by_magnify,
};
use metrics::{DEFAULT_HEIGHT, DEFAULT_WIDTH, NODE_FONT_SIZE, TOUCHPAD_PAN_SPEED_CELLS_PER_PIXEL};
use state::{
    AgenticBubbleState, AgenticBubbleTarget, active_patcher_patch, active_patcher_view_key,
    allocate_agentic_bubble, allocate_agentic_bubble_with_target, debug_log_edit_event,
    debug_log_writeback_event, delete_connection_edit_or_mark_deleted, delete_selected_nodes,
    editing_agentic_bubble_id, get_patcher_interaction_state, patch_with_interaction_state,
    patcher_state_key, patcher_state_key_from_parts, set_connection_segment_edit,
    set_patcher_interaction_state,
};
use text::{
    apply_patcher_autocomplete, cancel_patcher_text_edit,
    clamp_patcher_autocomplete_selection_with_macros, commit_patcher_text_edit,
    move_patcher_autocomplete_selection, patcher_autocomplete_is_open,
};
use writeback::emit_patch_writeback_result;

use super::text_input::{TextEditOutcome, apply_text_entry_key, cache_char_widths};
use super::{CellBuffer, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent};
#[cfg(target_os = "macos")]
use super::{MetalPrimitive, WidgetViewport};
use crate::layout::{
    Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num, get_stable_widget_id,
};
use crate::parser::{ASTParser, Expression, Parser, format_expression};
use crate::vm::Value;
use std::time::Instant;

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
                handle_patcher_pointer_drag(node, local_col, local_row);
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

    fn captures_scroll_gesture(&self) -> bool {
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
        if state.text_edit.is_none() {
            return match key_event.code {
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
                    if state
                        .agentic_bubbles
                        .values()
                        .any(|bubble| matches!(bubble.state, AgenticBubbleState::Error { .. })) =>
                {
                    let bubble_id = state
                        .agentic_bubbles
                        .values()
                        .find(|bubble| matches!(bubble.state, AgenticBubbleState::Error { .. }))
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
                KeyCode::Esc
                    if state.agentic_bubbles.values().any(|bubble| {
                        matches!(
                            bubble.state,
                            AgenticBubbleState::Error { .. } | AgenticBubbleState::Answer { .. }
                        )
                    }) =>
                {
                    if let Some(bubble_id) = state
                        .agentic_bubbles
                        .values()
                        .find(|bubble| {
                            matches!(
                                bubble.state,
                                AgenticBubbleState::Error { .. }
                                    | AgenticBubbleState::Answer { .. }
                            )
                        })
                        .map(|bubble| bubble.id.clone())
                    {
                        state.agentic_bubbles.remove(&bubble_id);
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
                let committed_node_id = state.text_edit.as_ref().map(|edit| edit.node_id.clone());
                let changed = commit_patcher_text_edit(&mut state, &view_key);
                let promoted_macro = committed_node_id.as_deref().is_some_and(|node_id| {
                    load_patch_from_props(&node.props)
                        .ok()
                        .is_some_and(|(_, root_patch)| {
                            promote_created_macro_definition(
                                &root_patch,
                                &mut state,
                                &view_key,
                                node_id,
                            )
                        })
                });
                let changed = changed || promoted_macro;
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
        state
            .agentic_bubbles
            .values()
            .any(|bubble| matches!(bubble.state, AgenticBubbleState::Pending { .. }))
            || state
                .agentic_morph_nodes
                .values()
                .any(|started| started.elapsed().as_secs_f32() < 1.2)
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
            state.agentic_bubbles.remove(&bubble_id);
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
    let path_buf = PathBuf::from(&path_str);
    let source = match std::fs::read_to_string(&path_buf) {
        Ok(source) => source,
        Err(error) => {
            return map_value(vec![
                ("status", Value::Keyword("invalid".to_string())),
                ("path", Value::String(path_str)),
                (
                    "diagnostic",
                    Value::String(format!("failed to read '{}': {error}", path_buf.display())),
                ),
            ]);
        }
    };

    let root_patch = match load_patch_from_props(&node.props) {
        Ok((_, patch)) => Some(patch),
        Err(error) => {
            debug_log_writeback_event(
                "layout-source-load-failed",
                format!("path={path_str}\nintent={intent:?}\nerror={error}"),
            );
            None
        }
    };

    match emit_patch_writeback_result(&source, intent, &state) {
        Ok(result) => {
            let layout = root_patch.and_then(|root_patch| {
                match parse_patch_source(&result.source, intent) {
                    Ok(mut emitted_patch) => match sidecar::emitted_layout_json_with_node_map(
                        &mut emitted_patch,
                        &root_patch,
                        &state,
                        &result.generated_node_ids,
                    ) {
                        Ok(layout) => Some(layout),
                        Err(error) => {
                            eprintln!("failed to build emitted patcher layout: {error}");
                            None
                        }
                    },
                    Err(error) => {
                        eprintln!("failed to parse emitted patch for layout persistence: {error}");
                        None
                    }
                }
            });
            debug_log_writeback_event(
                "payload-valid",
                format!(
                    "path={path_str}\nintent={intent:?}\nsource:\n{}",
                    result.source
                ),
            );
            let mut entries = vec![
                ("status", Value::Keyword("valid".to_string())),
                ("path", Value::String(path_str)),
                ("source", Value::String(result.source)),
            ];
            if let Some(layout) = layout {
                entries.push(("layout", Value::String(layout)));
            }
            map_value(entries)
        }
        Err(error) => {
            debug_log_edit_event("writeback-payload-invalid-state", &state);
            debug_log_writeback_event(
                "payload-invalid",
                format!("path={path_str}\nintent={intent:?}\nerror={error:?}"),
            );
            map_value(vec![
                ("status", Value::Keyword("invalid".to_string())),
                ("path", Value::String(path_str)),
                ("diagnostic", Value::String(format!("{error:?}"))),
            ])
        }
    }
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
    debug_patch_for_state(node, state, view_key)
        .map(|patch| patch.macros)
        .unwrap_or_default()
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
            let origin = geometry::patcher_origin(node.rect, &pan_state);
            segment.segment_row = ((start.1 + end.1) * 0.5) - origin.1;
        }
    }
    set_connection_segment_edit(state, view_key, &connection, Some(segment));
    state.drag = None;
    true
}

pub(super) fn load_patch_from_props(
    props: &HashMap<String, Value>,
) -> Result<(PathBuf, Patch), String> {
    let path = prop_str(props, "path")
        .or_else(|| prop_str(props, "file"))
        .ok_or_else(|| "patcher requires :path".to_string())?;
    let path = PathBuf::from(path);
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    let intent = patcher_intent_from_props(props);
    let mut patch = parse_patch_source(&source, intent)?;
    sidecar::apply_or_materialize(&path, &mut patch)?;
    Ok((path, patch))
}

pub(in crate::widget_render::patcher) fn persist_patcher_layout(
    node: &LayoutNode,
    state: &state::PatcherInteractionState,
) -> Result<(), String> {
    let (path, root_patch) = load_patch_from_props(&node.props)?;
    sidecar::save_current_layout(&path, &root_patch, state)
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
        cache_char_widths(node_display_label(patch_node), NODE_FONT_SIZE, ctx);
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
        cache_char_widths(preview(&tooltip, 48), 10.5, ctx);
    }
    if let Some(edit) = interaction_state.text_edit {
        cache_char_widths(edit.text, NODE_FONT_SIZE, ctx);
    }
    for bubble in interaction_state.agentic_bubbles.values() {
        cache_char_widths(bubble.prompt.clone(), 13.0, ctx);
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
