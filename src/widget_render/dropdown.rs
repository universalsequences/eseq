use std::cell::RefCell;
use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent,
    get_f32_prop, resolve_named_color, styled_cell,
};
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
};
use crate::vm::Value;

#[cfg(target_os = "macos")]
use super::{
    MetalPrimitive, MetalProportionalTextPrimitive, WidgetInstance,
    WidgetViewport, ndc_bounds,
};
#[cfg(target_os = "macos")]
use crate::backend::Color;

// ── Constants ────────────────────────────────────────────────────────────────

const PADDING_H: f32 = 0.6;
const MENU_ROW_HEIGHT: f32 = 1.4;
const MENU_PADDING_V: f32 = 0.3;
const CHEVRON_WIDTH: f32 = 1.5;

// ── Internal state ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
struct DropdownState {
    open: bool,
    hovered_idx: Option<usize>,
}

thread_local! {
    static STATES: RefCell<HashMap<u64, DropdownState>> = RefCell::new(HashMap::new());
}

fn get_state(widget_id: u64) -> DropdownState {
    STATES.with(|s| s.borrow().get(&widget_id).cloned().unwrap_or_default())
}

fn set_state(widget_id: u64, state: DropdownState) {
    STATES.with(|s| s.borrow_mut().insert(widget_id, state));
    super::bump_widget_state_generation();
}

/// Close the dropdown for a given widget_id (called when overlay is dismissed externally).
pub fn close_dropdown(widget_id: u64) {
    STATES.with(|s| {
        if let Some(state) = s.borrow_mut().get_mut(&widget_id) {
            state.open = false;
            state.hovered_idx = None;
        }
    });
}

fn get_options(props: &HashMap<String, Value>) -> Vec<String> {
    match props.get("options") {
        Some(Value::List(list)) => list
            .iter()
            .map(|v| match &*v.borrow() {
                Value::String(s) => s.clone(),
                Value::Keyword(k) => k.clone(),
                other => crate::vm::format_lisp_value(other),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn get_selected(props: &HashMap<String, Value>) -> String {
    match props.get("value") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Keyword(k)) => k.clone(),
        Some(other) => crate::vm::format_lisp_value(other),
        None => String::new(),
    }
}

fn selected_index(options: &[String], selected: &str) -> Option<usize> {
    options.iter().position(|o| o == selected)
}

// ── Widget definition ───────────────────────────────────────────────────────

pub struct DropdownWidget;
pub static DROPDOWN_WIDGET: DropdownWidget = DropdownWidget;

impl WidgetDefinition for DropdownWidget {
    fn names(&self) -> &'static [&'static str] {
        &["dropdown", "dropdown-chevron"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["value", "options", "width", "height", "font-size"]
    }

    fn renders_own_focus(&self) -> bool {
        true
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        Some(Size {
            width: get_prop_num(node, "width").map(f64_to_f32).unwrap_or(10.0),
            height: get_prop_num(node, "height").map(f64_to_f32).unwrap_or(1.5),
        })
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        _local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        _modifiers: KeyModifiers,
    ) -> MouseEventOutcome {
        if !matches!(mouse_kind, MouseEventKind::Down(MouseButton::Left)) {
            return MouseEventOutcome::Consume;
        }

        let mut state = get_state(node.widget_id);
        let options = get_options(&node.props);

        if state.open {
            // Check if click is in the menu area (overlay)
            let scroll_offset = super::scroll::any_active_scroll_offset();
            let screen_row = node.rect.row - scroll_offset;
            let menu_top = screen_row + node.rect.height;
            let menu_row = local_row - scroll_offset - menu_top;

            if menu_row >= 0.0 {
                let item_idx =
                    ((menu_row - MENU_PADDING_V) / MENU_ROW_HEIGHT).floor() as usize;
                if item_idx < options.len() {
                    state.open = false;
                    state.hovered_idx = None;
                    set_state(node.widget_id, state);
                    super::clear_overlay();
                    // Return selected option
                    return MouseEventOutcome::Dispatch(WidgetEvent::Custom(
                        Value::String(options[item_idx].clone()),
                    ));
                }
            }

            // Click on trigger area or outside menu → close
            state.open = false;
            state.hovered_idx = None;
            set_state(node.widget_id, state);
            super::clear_overlay();
            MouseEventOutcome::Consume
        } else {
            // Open the dropdown
            state.open = true;
            state.hovered_idx = selected_index(&options, &get_selected(&node.props));
            set_state(node.widget_id, state);
            MouseEventOutcome::Consume
        }
    }

    fn key_event(&self, node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
        let mut state = get_state(node.widget_id);
        let options = get_options(&node.props);
        if options.is_empty() {
            return None;
        }

        let selected = get_selected(&node.props);

        if !state.open {
            // When closed: Enter/Space opens the menu.
            // Up/Down are NOT consumed — they fall through to focus navigation.
            match key.code {
                KeyCode::Enter | KeyCode::Char(' ') => {
                    state.open = true;
                    state.hovered_idx = selected_index(&options, &selected);
                    set_state(node.widget_id, state);
                    return Some(WidgetEvent::Custom(Value::Nil));
                }
                _ => return None,
            }
        }

        // Menu is open
        match key.code {
            KeyCode::Down => {
                let next = state
                    .hovered_idx
                    .map(|i| (i + 1).min(options.len() - 1))
                    .unwrap_or(0);
                state.hovered_idx = Some(next);
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Up => {
                let prev = state
                    .hovered_idx
                    .map(|i| i.saturating_sub(1))
                    .unwrap_or(0);
                state.hovered_idx = Some(prev);
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Enter => {
                if let Some(idx) = state.hovered_idx {
                    let value = options.get(idx).cloned().unwrap_or_default();
                    state.open = false;
                    state.hovered_idx = None;
                    set_state(node.widget_id, state);
                    super::clear_overlay();
                    Some(WidgetEvent::Custom(Value::String(value)))
                } else {
                    state.open = false;
                    set_state(node.widget_id, state);
                    super::clear_overlay();
                    Some(WidgetEvent::Custom(Value::Nil))
                }
            }
            KeyCode::Esc => {
                state.open = false;
                state.hovered_idx = None;
                set_state(node.widget_id, state);
                super::clear_overlay();
                Some(WidgetEvent::Custom(Value::Nil))
            }
            _ => None,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(ref value) = event else {
            return None;
        };
        if matches!(value, Value::Nil) {
            return None;
        }
        let Value::String(new_value) = value else {
            return None;
        };
        let callback = node
            .props
            .get("on-change")
            .filter(|v| !matches!(v, Value::Nil | Value::Bool(false)))
            .cloned()?;
        Some(EventOutput {
            callback,
            args: vec![Value::String(new_value.clone())],
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let selected = get_selected(props);
        let text = format!("{} ▾", selected);
        let fg = crate::backend::Color {
            r: 0.9,
            g: 0.9,
            b: 0.92,
            a: 1.0,
        };
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16 + 1;
        let max_col = col_start + rect.width.round() as u16;
        for (i, ch) in text.chars().enumerate() {
            let c = col_start + i as u16;
            if c >= max_col {
                break;
            }
            buf.set(row, c, styled_cell(ch, fg, None));
        }
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, widget_type: &str) -> Option<&'static str> {
        match widget_type {
            "dropdown" => Some(super::ROUNDED_RECT_SHADER),
            "dropdown-chevron" => Some(DROPDOWN_CHEVRON_SHADER),
            _ => None,
        }
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let selected = get_selected(&node.props);
        let options = get_options(&node.props);
        let state = get_state(node.widget_id);
        let is_focused = viewport.focused_widget_id == Some(node.widget_id);

        let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);

        let bg_color = resolve_named_color(
            &node.props,
            "bg-color",
            Color { r: 0.22, g: 0.22, b: 0.24, a: 1.0 },
        );
        let text_color = resolve_named_color(
            &node.props,
            "text-color",
            Color { r: 0.90, g: 0.90, b: 0.92, a: 1.0 },
        );
        let ring_color = resolve_named_color(
            &node.props,
            "ring-color",
            Color { r: 0.25, g: 0.52, b: 0.96, a: 1.0 },
        );
        let chevron_color = resolve_named_color(
            &node.props,
            "chevron-color",
            Color { r: 0.95, g: 0.95, b: 0.97, a: 1.0 },
        );
        let menu_bg = resolve_named_color(
            &node.props,
            "menu-bg",
            Color { r: 0.12, g: 0.12, b: 0.14, a: 1.0 },
        );
        let hover_bg = resolve_named_color(
            &node.props,
            "hover-bg",
            Color { r: 0.25, g: 0.52, b: 0.96, a: 1.0 },
        );
        let check_color = resolve_named_color(
            &node.props,
            "check-color",
            Color { r: 0.90, g: 0.90, b: 0.92, a: 1.0 },
        );

        let transparent = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
        let mut prims = Vec::new();

        // ── Focus ring ──
        if is_focused {
            let ring_v = 0.15_f32;
            let ring_h = ring_v * viewport.cell_h / viewport.cell_w;
            let ring_rect = Rect {
                row: node.rect.row - ring_v,
                col: node.rect.col - ring_h,
                width: node.rect.width + ring_h * 2.0,
                height: node.rect.height + ring_v * 2.0,
            };
            emit_rounded_rect(&mut prims, ring_rect, ring_color, viewport, true);
        }

        // ── Background ──
        emit_rounded_rect(&mut prims, node.rect, bg_color, viewport, true);

        // ── Selected text ──
        let text_col = node.rect.col + PADDING_H;
        let text_row = node.rect.row + (node.rect.height - 1.0) * 0.5;
        if !selected.is_empty() {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: text_row,
                    col: text_col,
                    text: selected.clone(),
                    font_size,
                    fg: text_color,
                    bg: transparent,
                },
            ));
        }

        // ── Chevron badge + arrows ──
        {
            let ch_h = node.rect.height * 0.48;
            let ch_w = ch_h * 1.8;
            let ch_col = node.rect.col + node.rect.width - CHEVRON_WIDTH - ch_w * 0.55;
            let ch_rect = Rect {
                row: node.rect.row + (node.rect.height - ch_h) * 0.5,
                col: ch_col,
                width: ch_w,
                height: ch_h,
            };

            // Badge background behind chevrons
            let badge_color = resolve_named_color(
                &node.props,
                "badge-color",
                Color { r: 0.00, g: 0.35, b: 0.82, a: 1.0 },
            );
            let badge_pad = 0.1;
            let badge_rect = Rect {
                row: ch_rect.row - badge_pad,
                col: ch_rect.col - badge_pad * 0.5,
                width: ch_rect.width + badge_pad,
                height: ch_rect.height + badge_pad * 2.0,
            };
            // Use corner_radius 0.3 for a tighter badge shape
            {
                let (ndc_min, ndc_max) = ndc_bounds(badge_rect, viewport);
                let px_w = badge_rect.width * viewport.cell_w;
                let px_h = badge_rect.height * viewport.cell_h;
                prims.push(MetalPrimitive::WidgetInstance {
                    widget_type: "dropdown".to_string(),
                    instance: WidgetInstance {
                        ndc_min, ndc_max,
                        value_t: 0.0, orientation: 0.0,
                        itime: viewport.time_seconds,
                        uniform_a: [0.0; 4], uniform_b: [0.0; 4],
                        color_a: [badge_color.r, badge_color.g, badge_color.b, badge_color.a],
                        color_b: [0.0; 4], color_c: [0.0; 4], color_d: [0.0; 4],
                        corner_radius: 0.4,
                        pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                    },
                    is_background: false,
                });
            }
            let (ndc_min, ndc_max) = ndc_bounds(ch_rect, viewport);
            let px_w = ch_rect.width * viewport.cell_w;
            let px_h = ch_rect.height * viewport.cell_h;
            prims.push(MetalPrimitive::WidgetInstance {
                widget_type: "dropdown-chevron".to_string(),
                instance: WidgetInstance {
                    ndc_min,
                    ndc_max,
                    value_t: 0.0,
                    orientation: 0.0,
                    itime: viewport.time_seconds,
                    uniform_a: [0.0; 4],
                    uniform_b: [0.0; 4],
                    color_a: [chevron_color.r, chevron_color.g, chevron_color.b, chevron_color.a],
                    color_b: [0.0; 4],
                    color_c: [0.0; 4],
                    color_d: [0.0; 4],
                    corner_radius: 0.0,
                    pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                },
                is_background: false,
            });
        }

        // ── Menu overlay (when open) ──
        if state.open && !options.is_empty() {
            let scroll_offset = super::scroll::any_active_scroll_offset();
            let screen_row = node.rect.row - scroll_offset;
            let menu_top = screen_row + node.rect.height + 0.15;
            let menu_height =
                options.len() as f32 * MENU_ROW_HEIGHT + MENU_PADDING_V * 2.0;
            let menu_rect = Rect {
                row: menu_top,
                col: node.rect.col,
                width: node.rect.width,
                height: menu_height,
            };

            // Register overlay for hit-testing
            super::set_overlay(node.widget_id, menu_rect);

            // Menu background
            emit_rounded_rect_overlay(menu_rect, menu_bg, 0.15, viewport);

            // Menu items
            let sel_idx = selected_index(&options, &selected);
            for (i, option) in options.iter().enumerate() {
                let item_y = menu_top + MENU_PADDING_V + i as f32 * MENU_ROW_HEIGHT;

                // Hover/selected highlight
                let is_hovered = state.hovered_idx == Some(i);
                if is_hovered {
                    let hl_rect = Rect {
                        row: item_y,
                        col: node.rect.col + 0.15,
                        width: node.rect.width - 0.3,
                        height: MENU_ROW_HEIGHT,
                    };
                    emit_rounded_rect_overlay(hl_rect, hover_bg, 0.15, viewport);
                }

                // Check mark for selected item
                let label_col = node.rect.col + PADDING_H;
                if sel_idx == Some(i) {
                    super::push_overlay_primitive(MetalPrimitive::ProportionalText(
                        MetalProportionalTextPrimitive {
                            row: item_y + (MENU_ROW_HEIGHT - 1.0) * 0.5,
                            col: label_col,
                            text: "✓".to_string(),
                            font_size,
                            fg: check_color,
                            bg: transparent,
                        },
                    ));
                }

                // Option label
                super::push_overlay_primitive(MetalPrimitive::ProportionalText(
                    MetalProportionalTextPrimitive {
                        row: item_y + (MENU_ROW_HEIGHT - 1.0) * 0.5,
                        col: label_col + 1.5,
                        text: option.clone(),
                        font_size,
                        fg: text_color,
                        bg: transparent,
                    },
                ));
            }
        } else if !state.open {
            // Ensure overlay is cleared when closed
            if super::overlay_widget_id() == Some(node.widget_id) {
                super::clear_overlay();
            }
        }

        prims
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn emit_rounded_rect(
    prims: &mut Vec<MetalPrimitive>,
    rect: Rect,
    color: Color,
    viewport: WidgetViewport,
    is_background: bool,
) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    prims.push(MetalPrimitive::WidgetInstance {
        widget_type: "dropdown".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [0.0; 4],
            uniform_b: [0.0; 4],
            color_a: [color.r, color.g, color.b, color.a],
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: 0.0,
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
        is_background,
    });
}

#[cfg(target_os = "macos")]
fn emit_rounded_rect_overlay(
    rect: Rect,
    color: Color,
    corner_radius: f32,
    viewport: WidgetViewport,
) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    super::push_overlay_primitive(MetalPrimitive::WidgetInstance {
        widget_type: "dropdown".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [0.0; 4],
            uniform_b: [0.0; 4],
            color_a: [color.r, color.g, color.b, color.a],
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius,
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
        is_background: true,
    });
}

// ── Metal shaders ────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
const DROPDOWN_CHEVRON_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;
    float4 col = in.color_a;

    float2 p = float2((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);

    // Compact up chevron "^"
    float hw = 0.35 * aspect;
    float2 up_pt = float2(0.0, -0.70);
    float2 up_a  = float2(-hw, -0.22);
    float2 up_b  = float2( hw, -0.22);

    // Compact down chevron "v"
    float2 dn_pt = float2(0.0,  0.70);
    float2 dn_a  = float2(-hw,  0.22);
    float2 dn_b  = float2( hw,  0.22);

    // SDF for line segments
    float2 pa1 = p - up_a;  float2 ba1 = up_pt - up_a;
    float h1 = clamp(dot(pa1, ba1) / dot(ba1, ba1), 0.0, 1.0);
    float seg1 = length(pa1 - ba1 * h1);

    float2 pa2 = p - up_pt; float2 ba2 = up_b - up_pt;
    float h2 = clamp(dot(pa2, ba2) / dot(ba2, ba2), 0.0, 1.0);
    float seg2 = length(pa2 - ba2 * h2);

    float2 pa3 = p - dn_a;  float2 ba3 = dn_pt - dn_a;
    float h3 = clamp(dot(pa3, ba3) / dot(ba3, ba3), 0.0, 1.0);
    float seg3 = length(pa3 - ba3 * h3);

    float2 pa4 = p - dn_pt; float2 ba4 = dn_b - dn_pt;
    float h4 = clamp(dot(pa4, ba4) / dot(ba4, ba4), 0.0, 1.0);
    float seg4 = length(pa4 - ba4 * h4);

    float d = min(min(seg1, seg2), min(seg3, seg4));

    float stroke = 0.10;
    float edge = fwidth(d) * 1.2;
    float mask = smoothstep(stroke + edge, stroke - edge, d);

    if (mask < 0.002) { discard_fragment(); }
    return float4(col.rgb, col.a * mask);
}
"#;
