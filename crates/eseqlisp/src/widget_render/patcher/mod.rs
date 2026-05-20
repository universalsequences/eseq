mod display;
mod emit;
mod geometry;
mod interaction;
mod lisp;
mod metrics;
mod model;
mod project;
mod render;
mod state;
#[cfg(test)]
mod tests;
mod text;

use std::collections::HashMap;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

pub use lisp::parse_patch_source;
pub use model::{
    ArgSource, ArgValue, AttributeSource, BindingId, BindingKind, BindingTarget, CallSourceShape,
    ConnectionKind, ConnectionSource, ExprPath, ExprPathSegment, MacroPatch, NodeKind, NodeSource,
    Patch, PatchConnection, PatchNode, PatcherIntent, SourceArgValue, SourceExprId, SourceFormId,
    SourceOwner, SourceScopeId,
};

use display::node_display_label;
use emit::debug_log_patch_lisp;
use interaction::{
    handle_patcher_double_click, handle_patcher_pointer_down, handle_patcher_pointer_drag,
    handle_patcher_pointer_moved, handle_patcher_pointer_up, open_selected_macro_node,
    pan_patcher_by_delta, pan_patcher_by_wheel, reset_patcher_pan,
};
use metrics::{DEFAULT_HEIGHT, DEFAULT_WIDTH, NODE_FONT_SIZE, TOUCHPAD_PAN_SPEED_CELLS_PER_PIXEL};
use state::{
    active_patcher_patch, active_patcher_view_key, delete_connection_edit_or_mark_deleted,
    get_patcher_interaction_state, patch_with_interaction_state, patcher_state_key,
    patcher_state_key_from_parts, set_patcher_interaction_state,
};
use text::{cancel_patcher_text_edit, commit_patcher_text_edit};

use super::text_input::{TextEditOutcome, apply_text_entry_key, cache_char_widths};
use super::{CellBuffer, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent};
#[cfg(target_os = "macos")]
use super::{MetalPrimitive, WidgetViewport};
use crate::layout::{
    Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num, get_stable_widget_id,
};
use crate::vm::Value;

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
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                handle_patcher_pointer_down(node, local_col, local_row, modifiers);
                MouseEventOutcome::Consume
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                handle_patcher_pointer_drag(node, local_col, local_row);
                MouseEventOutcome::Consume
            }
            MouseEventKind::Up(MouseButton::Left) => {
                handle_patcher_pointer_up(node, local_col, local_row);
                MouseEventOutcome::Consume
            }
            MouseEventKind::Moved => {
                handle_patcher_pointer_moved(node, local_col, local_row);
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
        match key_event.code {
            KeyCode::Enter if state.text_edit.is_some() => {
                let changed = commit_patcher_text_edit(&mut state, &view_key);
                if changed && let Some(patch) = debug_patch_for_state(node, &state, &view_key) {
                    debug_log_patch_lisp(&view_key, &patch);
                }
                set_patcher_interaction_state(key, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Enter if open_selected_macro_node(node, &mut state) => {
                set_patcher_interaction_state(key, state);
                reset_patcher_pan(key);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Esc if state.text_edit.is_some() => {
                cancel_patcher_text_edit(&mut state, &view_key);
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
                    Some(WidgetEvent::Custom(Value::Nil))
                } else {
                    None
                }
            }
            _ => {
                let edit = state.text_edit.as_mut()?;
                match apply_text_entry_key(&edit.text, &mut edit.state, key_event, false, None)? {
                    TextEditOutcome::Changed(new_text) => {
                        edit.text = new_text;
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

    fn handle_event(&self, _node: &LayoutNode, event: WidgetEvent) -> Option<super::EventOutput> {
        match event {
            WidgetEvent::Custom(Value::Nil) => None,
            _ => None,
        }
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

pub(super) fn load_patch_from_props(
    props: &HashMap<String, Value>,
) -> Result<(PathBuf, Patch), String> {
    let path = prop_str(props, "path")
        .or_else(|| prop_str(props, "file"))
        .ok_or_else(|| "patcher requires :path".to_string())?;
    let path = PathBuf::from(path);
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    let intent = match props.get("intent") {
        Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "effect" => {
            PatcherIntent::Effect
        }
        _ => PatcherIntent::Instrument,
    };
    let patch = parse_patch_source(&source, intent)?;
    Ok((path, patch))
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
    if let Some(edit) = interaction_state.text_edit {
        cache_char_widths(edit.text, NODE_FONT_SIZE, ctx);
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
