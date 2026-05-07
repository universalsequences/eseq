use std::cell::RefCell;
use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent,
    get_f32_prop, resolve_named_color, should_trigger_integer_haptic, styled_cell,
    trigger_level_change_haptic,
};
use crate::backend::Color;
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
};
use crate::theme;
use crate::vm::Value;

#[cfg(target_os = "macos")]
use super::{
    MetalPrimitive, MetalProportionalTextPrimitive, MetalRectPrimitive, WidgetInstance,
    WidgetViewport, ndc_bounds,
};

#[derive(Clone, Debug, Default)]
struct KnobNumberState {
    editing: bool,
    edit_text: String,
    cursor_pos: usize,
}

thread_local! {
    static STATES: RefCell<HashMap<u64, KnobNumberState>> = RefCell::new(HashMap::new());
    static CHAR_WIDTHS: RefCell<HashMap<u32, HashMap<char, f32>>> = RefCell::new(HashMap::new());
}

fn get_state(widget_id: u64) -> KnobNumberState {
    STATES.with(|s| s.borrow().get(&widget_id).cloned().unwrap_or_default())
}

fn set_state(widget_id: u64, state: KnobNumberState) {
    STATES.with(|s| s.borrow_mut().insert(widget_id, state));
    super::bump_widget_state_generation();
}

fn format_value(value: f64, decimals: u32) -> String {
    format!("{:.*}", decimals as usize, value)
}

fn quantized_value(props: &HashMap<String, Value>, value: f32) -> f32 {
    let min = get_f32_prop(props, "min", 0.0);
    let max = get_f32_prop(props, "max", 1.0);
    let step = get_f32_prop(props, "step", 0.0);
    let clamped = value.clamp(min, max);
    if step.is_finite() && step > 0.0 {
        (min + ((clamped - min) / step).round() * step).clamp(min, max)
    } else {
        clamped
    }
}

fn normalized_value(props: &HashMap<String, Value>) -> f32 {
    let value = quantized_value(props, get_f32_prop(props, "value", 0.0));
    let min = get_f32_prop(props, "min", 0.0);
    let max = get_f32_prop(props, "max", 1.0);
    let range = max - min;
    if range > 0.0 {
        ((value - min) / range).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

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

pub struct KnobNumberWidget;
pub static KNOB_NUMBER_WIDGET: KnobNumberWidget = KnobNumberWidget;

impl WidgetDefinition for KnobNumberWidget {
    fn names(&self) -> &'static [&'static str] {
        &["knob-number"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height", "font-size", "decimals", "step"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        #[cfg(target_os = "macos")]
        if let Some(measurer) = ctx.text_measurer {
            let font_size = get_prop_num(node, "font-size")
                .map(f64_to_f32)
                .unwrap_or(ctx.inherited_font_size);
            let key = font_size.to_bits();
            CHAR_WIDTHS.with(|cw| {
                let mut cache = cw.borrow_mut();
                cache.entry(key).or_insert_with(|| {
                    let mut widths = HashMap::new();
                    for ch in "0123456789.-".chars() {
                        let px = measurer.measure_text_px(&ch.to_string(), font_size);
                        widths.insert(ch, px / ctx.cell_w);
                    }
                    widths
                });
            });
        }
        Some(Size {
            width: get_prop_num(node, "width").map(f64_to_f32).unwrap_or(5.2),
            height: get_prop_num(node, "height").map(f64_to_f32).unwrap_or(2.8),
        })
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn unclamped_drag(&self) -> bool {
        true
    }

    fn begin_gesture(&self, node: &LayoutNode, local_col: f32, local_row: f32) -> Option<Value> {
        let value = get_f32_prop(&node.props, "value", 0.0);
        Some(Value::List(vec![
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(value as f64))),
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(local_col as f64))),
            std::rc::Rc::new(std::cell::RefCell::new(Value::Number(local_row as f64))),
        ]))
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        gesture: Option<&Value>,
        _modifiers: KeyModifiers,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let mut state = get_state(node.widget_id);
                state.editing = false;
                set_state(node.widget_id, state);
                MouseEventOutcome::Consume
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(Value::List(gesture_list)) = gesture else {
                    return MouseEventOutcome::Consume;
                };
                let Some(start_value) = gesture_list.first().and_then(|v| match &*v.borrow() {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                }) else {
                    return MouseEventOutcome::Consume;
                };
                let Some(start_col) = gesture_list.get(1).and_then(|v| match &*v.borrow() {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                }) else {
                    return MouseEventOutcome::Consume;
                };
                let Some(start_row) = gesture_list.get(2).and_then(|v| match &*v.borrow() {
                    Value::Number(n) => Some(*n as f32),
                    _ => None,
                }) else {
                    return MouseEventOutcome::Consume;
                };

                let min = get_f32_prop(&node.props, "min", 0.0);
                let max = get_f32_prop(&node.props, "max", 1.0);
                let range = (max - min).max(0.0001);
                let dx = local_col - start_col;
                let dy = start_row - local_row;
                let drag_cells = get_f32_prop(&node.props, "drag-cells", 8.0).max(1.0);
                let delta = ((dy * 1.0) + (dx * 0.15)) / drag_cells * range;
                let new_value = quantized_value(&node.props, start_value + delta);
                MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::Number(new_value as f64)))
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
                    state.editing = true;
                    state.edit_text.clear();
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
                    let min = get_f32_prop(&node.props, "min", 0.0);
                    let max = get_f32_prop(&node.props, "max", 1.0);
                    let parsed = state
                        .edit_text
                        .parse::<f64>()
                        .unwrap_or(value as f64)
                        .clamp(min as f64, max as f64);
                    let parsed = quantized_value(&node.props, parsed as f32) as f64;
                    state.editing = false;
                    state.edit_text.clear();
                    state.cursor_pos = 0;
                    set_state(node.widget_id, state);
                    Some(WidgetEvent::Custom(Value::Number(parsed)))
                } else {
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
        let new_value = match event {
            WidgetEvent::SetNormalized(t) => {
                let min = get_f32_prop(&node.props, "min", 0.0);
                let max = get_f32_prop(&node.props, "max", 1.0);
                min + (max - min) * t.clamp(0.0, 1.0)
            }
            WidgetEvent::Custom(Value::Number(n)) => n as f32,
            WidgetEvent::Custom(Value::Nil) => return None,
            _ => return None,
        };
        let new_value = quantized_value(&node.props, new_value);
        let previous = get_f32_prop(&node.props, "value", 0.0);
        let min = get_f32_prop(&node.props, "min", 0.0);
        let max = get_f32_prop(&node.props, "max", 1.0);
        if should_trigger_integer_haptic(node.widget_id, previous, new_value, min, max) {
            trigger_level_change_haptic();
        }
        let callback = node.props.get("on-change")?.clone();
        Some(EventOutput {
            callback,
            args: vec![Value::Number(new_value as f64)],
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let label = props
            .get("label")
            .and_then(|v| match v {
                Value::String(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        let value = quantized_value(props, get_f32_prop(props, "value", 0.0));
        let decimals = get_f32_prop(props, "decimals", 2.0) as u32;
        let text = format!("{} {}", label, format_value(value as f64, decimals));
        let fg = Color {
            r: 0.9,
            g: 0.9,
            b: 0.9,
            a: 1.0,
        };
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16;
        for (i, ch) in text.chars().enumerate() {
            let c = col_start + i as u16;
            if c >= col_start + rect.width.round() as u16 {
                break;
            }
            buf.set(row, c, styled_cell(ch, fg, None));
        }
    }

    #[cfg(target_os = "macos")]
    fn renders_own_focus(&self) -> bool {
        true
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(KNOB_NUMBER_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let value = quantized_value(&node.props, get_f32_prop(&node.props, "value", 0.0));
        let decimals = get_f32_prop(&node.props, "decimals", 2.0) as u32;
        let state = get_state(node.widget_id);
        let is_focused = viewport.focused_widget_id == Some(node.widget_id);
        let font_size = get_f32_prop(&node.props, "font-size", DEFAULT_FONT_SIZE);
        let label_size = get_f32_prop(&node.props, "label-font-size", font_size * 0.88);
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
        let label_color = resolve_named_color(
            &node.props,
            "label-color",
            Color {
                r: 0.52,
                g: 0.52,
                b: 0.55,
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
        let arc_color = resolve_named_color(&node.props, "arc-color", theme::WIDGET_FOCUS_BG());
        let track_color =
            resolve_named_color(&node.props, "track-color", theme::WIDGET_KNOB_TRACK());

        let knob_size = get_f32_prop(&node.props, "knob-size", node.rect.height * 0.53)
            .max(0.72)
            .min(node.rect.height * 0.9);
        let knob_width = if viewport.cell_w > 0.0 {
            knob_size * viewport.cell_h / viewport.cell_w
        } else {
            knob_size
        };
        let label_row = node.rect.row;
        let value_row = node.rect.row + node.rect.height * 0.62;
        let knob_top = node.rect.row + node.rect.height * 0.45;
        let knob_rect = Rect {
            row: knob_top.max(node.rect.row + 0.72),
            col: node.rect.col + (node.rect.width - knob_width) * 0.5,
            width: knob_width,
            height: knob_size,
        };
        let (ndc_min, ndc_max) = ndc_bounds(knob_rect, viewport);
        let px_w = knob_rect.width * viewport.cell_w;
        let px_h = knob_rect.height * viewport.cell_h;
        let mut prims = vec![MetalPrimitive::WidgetInstance {
            widget_type: widget_type.to_string(),
            instance: WidgetInstance {
                ndc_min,
                ndc_max,
                value_t: normalized_value(&node.props),
                orientation: 0.0,
                itime: viewport.time_seconds,
                uniform_a: [if is_focused { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
                uniform_b: [0.0; 4],
                color_a: [arc_color.r, arc_color.g, arc_color.b, arc_color.a],
                color_b: [track_color.r, track_color.g, track_color.b, track_color.a],
                color_c: [edit_color.r, edit_color.g, edit_color.b, edit_color.a],
                color_d: [0.0; 4],
                corner_radius: 0.0,
                pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
            },
            is_background: false,
        }];

        if let Some(Value::String(label)) = node.props.get("label") {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: label_row,
                    col: node.rect.col + node.rect.width * 0.5
                        - (label.chars().count() as f32 * 0.31),
                    align_width: 0.0,
                    h_align: 0.0,
                    text: label.clone(),
                    font_size: label_size,
                    fg: label_color,
                    bg: Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    },
                },
            ));
        }

        let (display_text, fg) = if state.editing {
            (state.edit_text.clone(), edit_color)
        } else if is_focused {
            (format_value(value as f64, decimals), edit_color)
        } else {
            (format_value(value as f64, decimals), text_color)
        };
        let text_col = knob_rect.col + knob_width * 0.55;
        let text_row = value_row;
        prims.push(MetalPrimitive::ProportionalText(
            MetalProportionalTextPrimitive {
                row: text_row,
                col: text_col,
                align_width: 0.0,
                h_align: 0.0,
                text: display_text.clone(),
                font_size,
                fg,
                bg: Color {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                },
            },
        ));

        if is_focused && state.editing {
            let cursor_x =
                cursor_x_from_cache(&display_text, state.cursor_pos, font_size, viewport.cell_w);
            prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: Rect {
                    row: text_row + 0.08,
                    col: text_col + cursor_x,
                    width: 0.08,
                    height: node.rect.height * 0.36,
                },
                color: cursor_color,
            }));
        }

        prims
    }
}

#[cfg(target_os = "macos")]
const KNOB_NUMBER_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float2 p = float2((uv.x - 0.5) * 2.0, (uv.y - 0.5) * 2.0);
    float r = length(p);
    float a = atan2(p.y, p.x);

    float start = 1.57079633;
    float sweep = 4.71238898;
    float rel = fmod((a - start + 6.2831853), 6.2831853);
    float inRange = step(rel, sweep);
    float active = step(rel, sweep * in.value_t);

    float ring = abs(r - 0.74) - 0.088;
    float aa = max(fwidth(r), 0.0015);
    float ringMask = smoothstep(aa, -aa, ring) * inRange;
    float activeMask = ringMask * active;

    float notchAngle = start + sweep * in.value_t;
    float2 n = float2(cos(notchAngle), sin(notchAngle));
    float notch = length(p - n * 0.74) - 0.084;
    float notchMask = smoothstep(aa, -aa, notch);
    float lineAlong = dot(p, n);
    float lineAcross = abs(p.x * n.y - p.y * n.x);
    float lineSegment = step(0.0, lineAlong) * step(lineAlong, 0.68);
    float line = lineAcross - 0.088;
    float lineMask = smoothstep(aa, -aa, line) * lineSegment;

    float4 col = float4(0.0);
    col = mix(col, in.color_b, ringMask);
    col = mix(col, in.color_a, activeMask);
    col = mix(col, in.color_b, lineMask);
    col = mix(col, in.color_a, notchMask);
    if (col.a < 0.01) { discard_fragment(); }
    return col;
}
"#;
