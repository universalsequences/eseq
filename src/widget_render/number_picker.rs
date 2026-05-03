use std::cell::RefCell;
use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent,
    get_bool_prop, get_f32_prop, resolve_named_color, should_trigger_integer_haptic, styled_cell,
    trigger_level_change_haptic,
};
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
};
use crate::vm::Value;

#[cfg(target_os = "macos")]
use super::{
    MetalPrimitive, MetalProportionalTextPrimitive, MetalRectPrimitive, WidgetInstance,
    WidgetViewport, ndc_bounds,
};
#[cfg(target_os = "macos")]
use crate::backend::Color;

// ── Constants ────────────────────────────────────────────────────────────────

const TEXT_PADDING_H: f32 = 0.5;
const RING_WIDTH: f32 = 0.15;
const TRIANGLE_WIDTH: f32 = 1.4;

// ── Internal state ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
struct NumberPickerState {
    /// When true, the user is typing a value. The display shows edit_text
    /// instead of the formatted prop value.
    editing: bool,
    edit_text: String,
    cursor_pos: usize,
}

thread_local! {
    static STATES: RefCell<HashMap<u64, NumberPickerState>> = RefCell::new(HashMap::new());
    /// Per-character cell widths for digits, '.', '-' at a given font size.
    /// Key: font_size_bits → HashMap<char, cell_width>.
    static CHAR_WIDTHS: RefCell<HashMap<u32, HashMap<char, f32>>> =
        RefCell::new(HashMap::new());
}

fn get_state(widget_id: u64) -> NumberPickerState {
    STATES.with(|s| s.borrow().get(&widget_id).cloned().unwrap_or_default())
}

fn set_state(widget_id: u64, state: NumberPickerState) {
    STATES.with(|s| s.borrow_mut().insert(widget_id, state));
    super::bump_widget_state_generation();
}

fn format_value(value: f64, decimals: u32) -> String {
    format!("{:.*}", decimals as usize, value)
}

/// Compute text X offset in cells by summing cached per-character widths.
#[cfg(target_os = "macos")]
fn cursor_x_from_cache(text: &str, cursor_pos: usize, font_size: f32, cell_w: f32) -> f32 {
    let key = font_size.to_bits();
    CHAR_WIDTHS.with(|cw| {
        let cache = cw.borrow();
        if let Some(widths) = cache.get(&key) {
            let fallback = font_size * 0.55 / cell_w;
            text.chars()
                .take(cursor_pos)
                .map(|ch| widths.get(&ch).copied().unwrap_or(fallback))
                .sum()
        } else {
            cursor_pos as f32 * font_size * 0.55 / cell_w
        }
    })
}

// ── Widget definition ───────────────────────────────────────────────────────

pub struct NumberPickerWidget;
pub static NUMBER_PICKER_WIDGET: NumberPickerWidget = NumberPickerWidget;

impl WidgetDefinition for NumberPickerWidget {
    fn names(&self) -> &'static [&'static str] {
        &["number-picker", "number-picker-tri"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "font-size", "decimals", "noui", "unit"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        // Cache per-character widths for digit rendering
        if let Some(measurer) = ctx.text_measurer {
            let font_size = get_prop_num(node, "font-size")
                .map(f64_to_f32)
                .unwrap_or(ctx.inherited_font_size);
            let key = font_size.to_bits();
            CHAR_WIDTHS.with(|cw| {
                let mut cache = cw.borrow_mut();
                if !cache.contains_key(&key) {
                    let mut widths = HashMap::new();
                    for ch in
                        "0123456789.- %abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ".chars()
                    {
                        let px = measurer.measure_text_px(&ch.to_string(), font_size);
                        widths.insert(ch, px / ctx.cell_w);
                    }
                    cache.insert(key, widths);
                }
            });
        }
        Some(Size {
            width: get_prop_num(node, "width").map(f64_to_f32).unwrap_or(8.0),
            height: get_prop_num(node, "height").map(f64_to_f32).unwrap_or(1.4),
        })
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn unclamped_drag(&self) -> bool {
        true
    }

    fn begin_gesture(&self, node: &LayoutNode, _local_col: f32, local_row: f32) -> Option<Value> {
        // Store start value and start row (layout-space).
        // local_row ≈ screen_row_from_content_top + scroll, so for non-scrolled
        // views it equals the screen distance from the content area top.
        let value = get_f32_prop(&node.props, "value", 0.0);
        Some(Value::List(vec![
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(value as f64))),
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(local_row as f64))),
        ]))
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        _local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        gesture: Option<&Value>,
        _modifiers: KeyModifiers,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Exit any existing edit mode on fresh click
                let mut state = get_state(node.widget_id);
                state.editing = false;
                set_state(node.widget_id, state);
                MouseEventOutcome::Consume
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(Value::List(gesture_list)) = gesture else {
                    return MouseEventOutcome::Consume;
                };
                let (start_value, start_row) = match (gesture_list.first(), gesture_list.get(1)) {
                    (Some(v), Some(r)) => {
                        let sv = match &*v.borrow() {
                            Value::Number(n) => *n as f32,
                            _ => return MouseEventOutcome::Consume,
                        };
                        let sr = match &*r.borrow() {
                            Value::Number(n) => *n as f32,
                            _ => return MouseEventOutcome::Consume,
                        };
                        (sv, sr)
                    }
                    _ => return MouseEventOutcome::Consume,
                };

                let min = get_f32_prop(&node.props, "min", 0.0);
                let max = get_f32_prop(&node.props, "max", 1.0);
                let delta_rows = start_row - local_row; // positive = dragging up

                // Dynamic sensitivity: dragging up to the content-area top
                // (start_row rows above) reaches max; equal distance below → min.
                // Minimum of 5 rows to avoid division-by-near-zero.
                let room = start_row.max(5.0);

                let new_value = if delta_rows >= 0.0 {
                    // Dragging up → towards max
                    let frac = (delta_rows / room).min(1.0);
                    start_value + frac * (max - start_value)
                } else {
                    // Dragging down → towards min
                    let frac = ((-delta_rows) / room).min(1.0);
                    start_value - frac * (start_value - min)
                };

                MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::Number(
                    new_value.clamp(min, max) as f64,
                )))
            }
            _ => MouseEventOutcome::Consume,
        }
    }

    fn key_event(&self, node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
        let mut state = get_state(node.widget_id);
        let value = get_f32_prop(&node.props, "value", 0.0);
        let decimals = get_f32_prop(&node.props, "decimals", 2.0) as u32;

        match key.code {
            KeyCode::Char(c)
                if (c.is_ascii_digit() || c == '.' || c == '-')
                    && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
            {
                if !state.editing {
                    // First keypress: clear and start fresh
                    state.editing = true;
                    state.edit_text = String::new();
                    state.cursor_pos = 0;
                }
                state.edit_text.insert(state.cursor_pos, c);
                state.cursor_pos += 1;
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Backspace if state.editing => {
                if state.cursor_pos > 0 {
                    state.cursor_pos -= 1;
                    state.edit_text.remove(state.cursor_pos);
                }
                if state.edit_text.is_empty() {
                    state.editing = false;
                }
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Left if state.editing => {
                state.cursor_pos = state.cursor_pos.saturating_sub(1);
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Right if state.editing => {
                state.cursor_pos = (state.cursor_pos + 1).min(state.edit_text.len());
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Enter => {
                if state.editing {
                    // Parse and commit
                    let min = get_f32_prop(&node.props, "min", 0.0);
                    let max = get_f32_prop(&node.props, "max", 1.0);
                    let parsed = state
                        .edit_text
                        .parse::<f64>()
                        .unwrap_or(value as f64)
                        .clamp(min as f64, max as f64);
                    state.editing = false;
                    state.edit_text.clear();
                    state.cursor_pos = 0;
                    set_state(node.widget_id, state);
                    Some(WidgetEvent::Custom(Value::Number(parsed)))
                } else {
                    // Enter edit mode with current value
                    state.editing = true;
                    state.edit_text = format_value(value as f64, decimals);
                    state.cursor_pos = state.edit_text.len();
                    set_state(node.widget_id, state);
                    Some(WidgetEvent::Custom(Value::Nil))
                }
            }
            KeyCode::Esc => {
                state.editing = false;
                state.edit_text.clear();
                state.cursor_pos = 0;
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            _ => None,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(ref value) = event else {
            return None;
        };
        // Nil = cursor/edit state change only, no callback
        if matches!(value, Value::Nil) {
            return None;
        }
        let Value::Number(new_value) = value else {
            return None;
        };
        let previous = get_f32_prop(&node.props, "value", 0.0);
        let min = get_f32_prop(&node.props, "min", 0.0);
        let max = get_f32_prop(&node.props, "max", 1.0);
        if should_trigger_integer_haptic(previous, *new_value as f32, min, max) {
            trigger_level_change_haptic();
        }
        let callback = node
            .props
            .get("on-change")
            .filter(|v| !matches!(v, Value::Nil | Value::Bool(false)))
            .cloned()?;
        Some(EventOutput {
            callback,
            args: vec![Value::Number(*new_value)],
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let value = get_f32_prop(props, "value", 0.0);
        let decimals = get_f32_prop(props, "decimals", 2.0) as u32;
        let unit = match props.get("unit") {
            Some(Value::String(unit)) => unit.as_str(),
            _ => "",
        };
        let value_text = format_value(value as f64, decimals);
        let display_value = if unit.is_empty() {
            value_text
        } else {
            format!("{value_text} {unit}")
        };
        let text = format!("▶ {display_value}");
        let fg = crate::backend::Color {
            r: 0.9,
            g: 0.9,
            b: 0.9,
            a: 1.0,
        };
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16;
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
    fn renders_own_focus(&self) -> bool {
        true
    }

    fn metal_fragment_shader(&self, widget_type: &str) -> Option<&'static str> {
        match widget_type {
            "number-picker" => Some(super::ROUNDED_RECT_SHADER),
            "number-picker-tri" => Some(NUMBER_PICKER_TRI_SHADER),
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
        let value = get_f32_prop(&node.props, "value", 0.0);
        let decimals = get_f32_prop(&node.props, "decimals", 2.0) as u32;
        let state = get_state(node.widget_id);
        let is_focused = viewport.focused_widget_id == Some(node.widget_id);
        let noui = get_bool_prop(&node.props, "noui", false);
        let unit = match node.props.get("unit") {
            Some(Value::String(unit)) => unit.as_str(),
            _ => "",
        };
        let text_align = match node.props.get("text-align") {
            Some(Value::Keyword(align)) => align.as_str(),
            _ => "start",
        };

        let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);

        let text_color = resolve_named_color(
            &node.props,
            "text-color",
            Color {
                r: 0.90,
                g: 0.90,
                b: 0.92,
                a: 1.0,
            },
        );
        let edit_color = resolve_named_color(
            &node.props,
            "edit-color",
            Color {
                r: 1.0,
                g: 0.95,
                b: 0.25,
                a: 1.0,
            },
        );
        let cursor_color = resolve_named_color(
            &node.props,
            "cursor-color",
            Color {
                r: 1.0,
                g: 0.95,
                b: 0.25,
                a: 1.0,
            },
        );

        let mut prims = Vec::new();

        if !noui {
            let bg_color = resolve_named_color(
                &node.props,
                "bg-color",
                Color {
                    r: 0.14,
                    g: 0.14,
                    b: 0.16,
                    a: 1.0,
                },
            );
            let ring_color = resolve_named_color(
                &node.props,
                "ring-color",
                Color {
                    r: 0.25,
                    g: 0.52,
                    b: 0.96,
                    a: 1.0,
                },
            );
            let tri_color = resolve_named_color(
                &node.props,
                "tri-color",
                Color {
                    r: 0.60,
                    g: 0.60,
                    b: 0.65,
                    a: 1.0,
                },
            );

            // ── Focus ring ──
            if is_focused {
                let ring_v = RING_WIDTH;
                let ring_h = RING_WIDTH * viewport.cell_h / viewport.cell_w;
                let ring_rect = Rect {
                    row: node.rect.row - ring_v,
                    col: node.rect.col - ring_h,
                    width: node.rect.width + ring_h * 2.0,
                    height: node.rect.height + ring_v * 2.0,
                };
                let (ndc_min, ndc_max) = ndc_bounds(ring_rect, viewport);
                let px_w = ring_rect.width * viewport.cell_w;
                let px_h = ring_rect.height * viewport.cell_h;
                prims.push(MetalPrimitive::WidgetInstance {
                    widget_type: "number-picker".to_string(),
                    instance: WidgetInstance {
                        ndc_min,
                        ndc_max,
                        value_t: 0.0,
                        orientation: 0.0,
                        itime: viewport.time_seconds,
                        uniform_a: [0.0; 4],
                        uniform_b: [0.0; 4],
                        color_a: [ring_color.r, ring_color.g, ring_color.b, ring_color.a],
                        color_b: [0.0; 4],
                        color_c: [0.0; 4],
                        color_d: [0.0; 4],
                        corner_radius: 0.0,
                        pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                    },
                    is_background: true,
                });
            }

            // ── Background ──
            {
                let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
                let px_w = node.rect.width * viewport.cell_w;
                let px_h = node.rect.height * viewport.cell_h;
                prims.push(MetalPrimitive::WidgetInstance {
                    widget_type: "number-picker".to_string(),
                    instance: WidgetInstance {
                        ndc_min,
                        ndc_max,
                        value_t: 0.0,
                        orientation: 0.0,
                        itime: viewport.time_seconds,
                        uniform_a: [0.0; 4],
                        uniform_b: [0.0; 4],
                        color_a: [bg_color.r, bg_color.g, bg_color.b, bg_color.a],
                        color_b: [0.0; 4],
                        color_c: [0.0; 4],
                        color_d: [0.0; 4],
                        corner_radius: 0.0,
                        pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                    },
                    is_background: true,
                });
            }

            // ── Triangle indicator ──
            {
                let tri_h = node.rect.height * 0.5;
                let tri_w = tri_h * 1.5;
                let tri_rect = Rect {
                    row: node.rect.row + (node.rect.height - tri_h) * 0.5,
                    col: node.rect.col + TEXT_PADDING_H * 0.6,
                    width: tri_w,
                    height: tri_h,
                };
                let (ndc_min, ndc_max) = ndc_bounds(tri_rect, viewport);
                let px_w = tri_rect.width * viewport.cell_w;
                let px_h = tri_rect.height * viewport.cell_h;
                prims.push(MetalPrimitive::WidgetInstance {
                    widget_type: "number-picker-tri".to_string(),
                    instance: WidgetInstance {
                        ndc_min,
                        ndc_max,
                        value_t: 0.0,
                        orientation: 0.0,
                        itime: viewport.time_seconds,
                        uniform_a: [0.0; 4],
                        uniform_b: [0.0; 4],
                        color_a: [tri_color.r, tri_color.g, tri_color.b, tri_color.a],
                        color_b: [0.0; 4],
                        color_c: [0.0; 4],
                        color_d: [0.0; 4],
                        corner_radius: 0.0,
                        pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
                    },
                    is_background: false,
                });
            }
        }

        // ── Value text ──
        let base_text_col = if noui {
            node.rect.col + TEXT_PADDING_H
        } else {
            node.rect.col + TEXT_PADDING_H + TRIANGLE_WIDTH
        };
        let text_row = node.rect.row + (node.rect.height - 1.0) * 0.5;
        let transparent = Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };

        let value_text = format_value(value as f64, decimals);
        let display_value = if unit.is_empty() {
            value_text
        } else {
            format!("{value_text} {unit}")
        };
        let (display_text, fg) = if state.editing {
            (state.edit_text.clone(), edit_color)
        } else if is_focused {
            (display_value, edit_color)
        } else {
            (display_value, text_color)
        };
        let text_width = cursor_x_from_cache(
            &display_text,
            display_text.chars().count(),
            font_size,
            viewport.cell_w,
        );
        let text_col = match text_align {
            "center" => node.rect.col + ((node.rect.width - text_width).max(0.0) * 0.5),
            "right" => node.rect.col + (node.rect.width - text_width - TEXT_PADDING_H).max(0.0),
            _ => base_text_col,
        };

        if !display_text.is_empty() {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: text_row,
                    col: text_col,
                    align_width: 0.0,
                    h_align: 0.0,
                    text: display_text.clone(),
                    font_size,
                    fg,
                    bg: transparent,
                },
            ));
        }

        // ── Cursor (when editing) ──
        if is_focused && state.editing {
            let cursor_x =
                cursor_x_from_cache(&display_text, state.cursor_pos, font_size, viewport.cell_w);
            let cursor_col = text_col + cursor_x;
            let cursor_rect = Rect {
                row: node.rect.row + 0.15,
                col: cursor_col,
                width: 0.08,
                height: node.rect.height - 0.3,
            };
            prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: cursor_rect,
                color: cursor_color,
            }));
        }

        prims
    }
}

// ── Metal shaders ────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
const NUMBER_PICKER_TRI_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;
    float4 col = in.color_a;

    // Aspect-corrected coordinates
    float2 p = float2((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);

    // Right-pointing filled triangle (play button style)
    // Vertices: left-top (-0.5a, -0.7), left-bottom (-0.5a, 0.7), right (0.6a, 0)
    float2 a = float2(-0.5 * aspect, -0.7);
    float2 b = float2(-0.5 * aspect,  0.7);
    float2 c = float2( 0.6 * aspect,  0.0);

    // Point-in-triangle using cross products
    float d1 = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    float d2 = (c.x - b.x) * (p.y - b.y) - (c.y - b.y) * (p.x - b.x);
    float d3 = (a.x - c.x) * (p.y - c.y) - (a.y - c.y) * (p.x - c.x);

    bool has_neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
    bool has_pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);
    float inside = (has_neg && has_pos) ? 0.0 : 1.0;

    // Soften edges with SDF approximation
    // Distance to each edge
    float2 ab = b - a;
    float2 bc = c - b;
    float2 ca = a - c;
    float e1 = abs(d1) / length(ab);
    float e2 = abs(d2) / length(bc);
    float e3 = abs(d3) / length(ca);
    float dist_to_edge = min(e1, min(e2, e3));
    float aa = fwidth(dist_to_edge) * 1.5;
    float mask = inside * smoothstep(0.0, aa, dist_to_edge);

    if (mask < 0.01) { discard_fragment(); }
    return float4(col.rgb, col.a * mask);
}
"#;
