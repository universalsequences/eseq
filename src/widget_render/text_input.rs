use std::cell::RefCell;
use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetKeyEvent,
    resolve_named_color, styled_cell,
};
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num,
    get_prop_str,
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
const TEXT_PADDING_V: f32 = 0.15;
const RING_WIDTH: f32 = 0.15;
const ICON_GAP: f32 = 0.3;

// ── Internal state ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
struct TextInputState {
    cursor_pos: usize,
}

thread_local! {
    static STATES: RefCell<HashMap<u64, TextInputState>> = RefCell::new(HashMap::new());
    /// Cursor X offsets cached during measure().
    /// Key: (text_value, font_size_bits) → offsets[i] = cell-width of text[0..i].
    static CURSOR_X_CACHE: RefCell<HashMap<(String, u32), Vec<f32>>> =
        RefCell::new(HashMap::new());
}

fn get_state(widget_id: u64) -> TextInputState {
    STATES.with(|s| s.borrow().get(&widget_id).cloned().unwrap_or_default())
}

fn set_state(widget_id: u64, state: TextInputState) {
    STATES.with(|s| s.borrow_mut().insert(widget_id, state));
    super::bump_widget_state_generation();
}

fn get_text(props: &HashMap<String, Value>) -> String {
    match props.get("value") {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn get_placeholder(props: &HashMap<String, Value>) -> String {
    match props.get("placeholder") {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn cached_cursor_x(text: &str, font_size: f32, cursor_pos: usize) -> Option<f32> {
    CURSOR_X_CACHE.with(|c| {
        c.borrow()
            .get(&(text.to_string(), font_size.to_bits()))
            .and_then(|offsets| offsets.get(cursor_pos).copied())
    })
}

#[cfg(target_os = "macos")]
fn approx_cursor_x(cursor_pos: usize, font_size: f32, cell_w: f32) -> f32 {
    cursor_pos as f32 * font_size * 0.55 / cell_w
}

// ── Widget definition ───────────────────────────────────────────────────────

pub struct TextInputWidget;
pub static TEXT_INPUT_WIDGET: TextInputWidget = TextInputWidget;

impl WidgetDefinition for TextInputWidget {
    fn names(&self) -> &'static [&'static str] {
        &["text-input"]
    }

    fn is_container(&self) -> bool {
        true // first child is positioned as a leading icon
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["value", "placeholder", "width", "height", "font-size"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or_else(|| {
                if constraints.max_width < f32::MAX {
                    constraints.max_width
                } else {
                    20.0
                }
            });

        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(1.5);

        // Cache per-character cursor offsets for proportional text
        if let Some(measurer) = ctx.text_measurer {
            let font_size = get_prop_num(node, "font-size")
                .map(f64_to_f32)
                .unwrap_or(ctx.inherited_font_size);
            let text = get_prop_str(node, "value").unwrap_or_default();
            let chars: Vec<char> = text.chars().collect();
            let mut offsets = Vec::with_capacity(chars.len() + 1);
            offsets.push(0.0);
            for i in 1..=chars.len() {
                let substr: String = chars[..i].iter().collect();
                let px_w = measurer.measure_text_px(&substr, font_size);
                offsets.push(px_w / ctx.cell_w);
            }
            CURSOR_X_CACHE.with(|c| {
                c.borrow_mut()
                    .insert((text, font_size.to_bits()), offsets);
            });
        }

        Some(Size { width, height })
    }

    fn layout_children(
        &self,
        _node: &Value,
        area: Rect,
        children: &[Value],
        _aspect: f32,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        if children.is_empty() {
            return vec![];
        }
        let child = &children[0];
        let child_constraints = Constraints {
            min_width: 0.0,
            max_width: area.height * 2.0,
            min_height: 0.0,
            max_height: area.height,
            aspect: 1.0,
        };
        let child_size = measure_child(child, child_constraints)
            .unwrap_or(Size {
                width: 1.0,
                height: 1.0,
            });
        let child_rect = Rect {
            row: area.row + (area.height - child_size.height) * 0.5,
            col: area.col + TEXT_PADDING_H,
            width: child_size.width,
            height: child_size.height,
        };
        vec![build_child(child, child_rect)]
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        _local_col: f32,
        _local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        _modifiers: KeyModifiers,
    ) -> MouseEventOutcome {
        if matches!(mouse_kind, MouseEventKind::Down(MouseButton::Left)) {
            let text = get_text(&node.props);
            let mut state = get_state(node.widget_id);
            state.cursor_pos = text.chars().count();
            set_state(node.widget_id, state);
        }
        MouseEventOutcome::Consume
    }

    fn key_event(&self, node: &LayoutNode, key: WidgetKeyEvent) -> Option<WidgetEvent> {
        let text = get_text(&node.props);
        let char_count = text.chars().count();
        let mut state = get_state(node.widget_id);
        state.cursor_pos = state.cursor_pos.min(char_count);

        match key.code {
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                let chars: Vec<char> = text.chars().collect();
                let mut new = Vec::with_capacity(chars.len() + 1);
                new.extend_from_slice(&chars[..state.cursor_pos]);
                new.push(c);
                new.extend_from_slice(&chars[state.cursor_pos..]);
                state.cursor_pos += 1;
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::String(
                    new.into_iter().collect(),
                )))
            }
            KeyCode::Backspace => {
                if state.cursor_pos > 0 {
                    let chars: Vec<char> = text.chars().collect();
                    let mut new = Vec::with_capacity(chars.len() - 1);
                    new.extend_from_slice(&chars[..state.cursor_pos - 1]);
                    new.extend_from_slice(&chars[state.cursor_pos..]);
                    state.cursor_pos -= 1;
                    set_state(node.widget_id, state);
                    Some(WidgetEvent::Custom(Value::String(
                        new.into_iter().collect(),
                    )))
                } else {
                    Some(WidgetEvent::Custom(Value::Nil))
                }
            }
            KeyCode::Delete => {
                if state.cursor_pos < char_count {
                    let chars: Vec<char> = text.chars().collect();
                    let mut new = Vec::with_capacity(chars.len() - 1);
                    new.extend_from_slice(&chars[..state.cursor_pos]);
                    new.extend_from_slice(&chars[state.cursor_pos + 1..]);
                    set_state(node.widget_id, state);
                    Some(WidgetEvent::Custom(Value::String(
                        new.into_iter().collect(),
                    )))
                } else {
                    Some(WidgetEvent::Custom(Value::Nil))
                }
            }
            KeyCode::Left => {
                if state.cursor_pos > 0 {
                    state.cursor_pos -= 1;
                    set_state(node.widget_id, state);
                }
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Right => {
                if state.cursor_pos < char_count {
                    state.cursor_pos += 1;
                    set_state(node.widget_id, state);
                }
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::Home => {
                state.cursor_pos = 0;
                set_state(node.widget_id, state);
                Some(WidgetEvent::Custom(Value::Nil))
            }
            KeyCode::End => {
                state.cursor_pos = char_count;
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
        if matches!(value, Value::Nil) {
            return None;
        }
        let Value::String(new_text) = value else {
            return None;
        };
        let callback = node
            .props
            .get("on-change")
            .filter(|v| !matches!(v, Value::Nil | Value::Bool(false)))
            .cloned()?;
        Some(EventOutput {
            callback,
            args: vec![Value::String(new_text.clone())],
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let text = get_text(props);
        let placeholder = get_placeholder(props);
        let display = if text.is_empty() { &placeholder } else { &text };

        let fg = crate::backend::Color {
            r: if text.is_empty() { 0.4 } else { 0.9 },
            g: if text.is_empty() { 0.4 } else { 0.9 },
            b: if text.is_empty() { 0.45 } else { 0.92 },
            a: 1.0,
        };
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16 + 1;
        let max_col = rect.col.round() as u16 + rect.width.round() as u16;
        for (i, ch) in display.chars().enumerate() {
            let c = col_start + i as u16;
            if c >= max_col {
                break;
            }
            buf.set(row, c, styled_cell(ch, fg, None));
        }
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(TEXT_INPUT_BG_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let text = get_text(&node.props);
        let placeholder = get_placeholder(&node.props);
        let state = get_state(node.widget_id);
        let is_focused = viewport.focused_widget_id == Some(node.widget_id);

        let font_size = node
            .props
            .get("font-size")
            .and_then(|v| match v {
                Value::Number(n) => Some(*n as f32),
                _ => None,
            })
            .unwrap_or(DEFAULT_FONT_SIZE);

        let bg_transparent = matches!(
            node.props.get("bg"),
            Some(Value::Keyword(k)) if k == "transparent"
        );

        let bg_color = resolve_named_color(
            &node.props,
            "bg-color",
            Color {
                r: 0.20,
                g: 0.21,
                b: 0.23,
                a: 1.0,
            },
        );
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
        let placeholder_color = resolve_named_color(
            &node.props,
            "placeholder-color",
            Color {
                r: 0.40,
                g: 0.40,
                b: 0.45,
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
        let cursor_color = resolve_named_color(
            &node.props,
            "cursor-color",
            Color {
                r: 0.25,
                g: 0.52,
                b: 0.96,
                a: 1.0,
            },
        );

        let mut prims = Vec::new();

        // ── Focus ring (larger blue rounded rect behind background) ──
        if is_focused && !bg_transparent {
            // Cells are typically taller than wide; scale horizontal margin
            // so the ring looks the same thickness on all sides.
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
                widget_type: "text-input".to_string(),
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

        // ── Background rounded rect ──
        if !bg_transparent {
            let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
            let px_w = node.rect.width * viewport.cell_w;
            let px_h = node.rect.height * viewport.cell_h;
            prims.push(MetalPrimitive::WidgetInstance {
                widget_type: "text-input".to_string(),
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

        // ── Text position ──
        let icon_width = if !node.children.is_empty() {
            node.children[0].rect.width + ICON_GAP
        } else {
            0.0
        };
        let text_col = node.rect.col + TEXT_PADDING_H + icon_width;
        let text_row = node.rect.row + (node.rect.height - 1.0) * 0.5;
        let transparent = Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };

        // ── Display text or placeholder ──
        let (display_text, display_color) = if text.is_empty() {
            (placeholder.clone(), placeholder_color)
        } else {
            (text.clone(), text_color)
        };

        if !display_text.is_empty() {
            prims.push(MetalPrimitive::ProportionalText(
                MetalProportionalTextPrimitive {
                    row: text_row,
                    col: text_col,
                    text: display_text,
                    font_size,
                    fg: display_color,
                    bg: transparent,
                },
            ));
        }

        // ── Cursor ──
        if is_focused {
            let cursor_pos = state.cursor_pos.min(text.chars().count());
            let cursor_x_offset = cached_cursor_x(&text, font_size, cursor_pos)
                .unwrap_or_else(|| approx_cursor_x(cursor_pos, font_size, viewport.cell_w));

            let cursor_col = text_col + cursor_x_offset;
            let cursor_rect = Rect {
                row: node.rect.row + TEXT_PADDING_V + 0.05,
                col: cursor_col,
                width: 0.08,
                height: node.rect.height - TEXT_PADDING_V * 2.0 - 0.1,
            };
            prims.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: cursor_rect,
                color: cursor_color,
            }));
        }

        prims
    }
}

// ── Metal shader ─────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
const TEXT_INPUT_BG_SHADER: &str = r#"
fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = in.aspect;
    float4 col = in.color_a;

    // Aspect-corrected [-aspect, aspect] x [-1, 1]
    float2 p = float2((uv.x - 0.5) * 2.0 * aspect, (uv.y - 0.5) * 2.0);

    // Rounded rect SDF
    float r = 0.75;
    float2 half_size = float2(aspect - r, 1.0 - r);
    float2 q = abs(p) - half_size;
    float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;

    float edge = fwidth(d) * 1.2;
    float mask = smoothstep(edge, -edge, d);

    if (mask < 0.002) { discard_fragment(); }
    return float4(col.rgb, col.a * mask);
}
"#;
