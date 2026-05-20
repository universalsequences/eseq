use std::collections::HashMap;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, get_f32_prop,
    styled_cell,
};
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutCtx, LayoutNode, MeasureCtx, Rect, Size,
};
use crate::theme;
use crate::vm::Value;

pub struct TabsWidget;

pub static TABS_WIDGET: TabsWidget = TabsWidget;

/// Height of the tab bar header (text row + underline row)
const HEADER_HEIGHT: f32 = 2.0;

fn get_bool_prop(props: &HashMap<String, Value>, key: &str, default: bool) -> bool {
    match props.get(key) {
        Some(Value::Bool(value)) => *value,
        _ => default,
    }
}

fn header_height(props: &HashMap<String, Value>, _aspect: f32) -> f32 {
    get_f32_prop(props, "header-height", HEADER_HEIGHT)
}

fn compact_tab_widths(props: &HashMap<String, Value>, items: &[String]) -> Vec<f32> {
    let tab_pad = get_f32_prop(props, "tab-padding", 0.75);
    items
        .iter()
        .map(|label| label.chars().count() as f32 + tab_pad * 2.0)
        .collect()
}

fn compact_gap(props: &HashMap<String, Value>) -> f32 {
    get_f32_prop(props, "gap", 0.75)
}

fn compact_total_width(props: &HashMap<String, Value>, items: &[String]) -> f32 {
    let widths = compact_tab_widths(props, items);
    let gap = compact_gap(props);
    widths.iter().sum::<f32>() + gap * (widths.len().saturating_sub(1) as f32)
}

fn compact_hit_index(
    props: &HashMap<String, Value>,
    items: &[String],
    rel_col: f32,
) -> Option<usize> {
    let widths = compact_tab_widths(props, items);
    let gap = compact_gap(props);
    let mut cursor = get_f32_prop(props, "padding", 0.0);
    for (idx, width) in widths.iter().enumerate() {
        if rel_col >= cursor && rel_col < cursor + *width {
            return Some(idx);
        }
        cursor += *width + gap;
    }
    None
}

fn header_height_for_value(node: &Value, aspect: f32) -> f32 {
    let props = crate::layout::get_map(node).unwrap_or_default();
    header_height(&props, aspect)
}

fn selected_index(node: &Value, children_len: usize) -> usize {
    (crate::layout::get_prop_num(node, "value")
        .map(crate::layout::f64_to_f32)
        .unwrap_or(0.0) as usize)
        .min(children_len.saturating_sub(1))
}

fn get_items(props: &HashMap<String, Value>) -> Vec<String> {
    match props.get("items") {
        Some(Value::List(list)) => list
            .iter()
            .filter_map(|v| match &*v.borrow() {
                Value::String(s) => Some(s.clone()),
                Value::Keyword(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

fn tui_render_header(props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
    let items = get_items(props);
    if items.is_empty() {
        return;
    }
    let selected = get_f32_prop(props, "value", 0.0) as usize;
    let pad = get_f32_prop(props, "padding", 0.0).round() as u16;
    let compact = get_bool_prop(props, "compact", false);

    let row_u16 = rect.row.round() as u16;
    let col_u16 = rect.col.round() as u16;

    let spans: Vec<(u16, u16)> = if compact {
        let gap = compact_gap(props).round() as u16;
        let tab_pad = get_f32_prop(props, "tab-padding", 0.75).round() as u16;
        let mut cursor = col_u16 + pad;
        items
            .iter()
            .map(|label| {
                let width = label.chars().count() as u16 + tab_pad * 2;
                let span = (cursor, width);
                cursor = cursor.saturating_add(width + gap);
                span
            })
            .collect()
    } else {
        let width_u16 = (rect.width.round() as u16).saturating_sub(pad * 2);
        let tab_width = width_u16 / items.len().max(1) as u16;
        items
            .iter()
            .enumerate()
            .map(|(i, _)| (col_u16 + pad + (i as u16) * tab_width, tab_width))
            .collect()
    };

    for (i, label) in items.iter().enumerate() {
        let (tab_col, tab_width) = spans[i];
        let is_selected = i == selected;
        let fg = if is_selected {
            theme::WHITE()
        } else {
            theme::BRIGHT_BLACK()
        };

        let label_len = label.chars().count() as u16;
        let pad = tab_width.saturating_sub(label_len) / 2;

        for c in 0..tab_width {
            buf.set(row_u16, tab_col + c, styled_cell(' ', fg, None));
        }
        for (j, ch) in label.chars().enumerate() {
            let col = tab_col + pad + j as u16;
            if col < tab_col + tab_width {
                buf.set(row_u16, col, styled_cell(ch, fg, None));
            }
        }
        // Underline row
        let underline_row = row_u16 + 1;
        for c in 0..tab_width {
            let ch = if is_selected { '\u{2500}' } else { ' ' };
            let line_fg = if is_selected {
                theme::WHITE()
            } else {
                theme::BRIGHT_BLACK()
            };
            buf.set(underline_row, tab_col + c, styled_cell(ch, line_fg, None));
        }
    }
}

impl WidgetDefinition for TabsWidget {
    fn names(&self) -> &'static [&'static str] {
        &["tabs"]
    }

    fn is_container(&self) -> bool {
        true
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["items", "value"]
    }

    fn measure(
        &self,
        node: &Value,
        children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let props = crate::layout::get_map(node).unwrap_or_default();
        let compact = get_bool_prop(&props, "compact", false);
        let items = get_items(&props);
        let header_h = header_height(&props, constraints.aspect);
        let inner = Constraints {
            min_width: constraints.min_width,
            max_width: constraints.max_width,
            min_height: 0.0,
            max_height: (constraints.max_height - header_h).max(0.0),
            aspect: constraints.aspect,
        };
        let selected_size = children
            .get(selected_index(node, children.len()))
            .and_then(|child| measure_child(child, inner))
            .unwrap_or(Size {
                width: 0.0,
                height: 0.0,
            });
        Some(Size {
            width: if compact {
                let header_width = compact_total_width(&props, &items)
                    + get_f32_prop(&props, "padding", 0.0) * 2.0;
                selected_size.width.max(header_width)
            } else {
                constraints.max_width
            },
            height: header_h + selected_size.height,
        })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        tui_render_header(props, rect, buf);
    }

    fn layout_children(
        &self,
        node: &Value,
        area: Rect,
        children: &[Value],
        _aspect: f32,
        _layout_ctx: LayoutCtx,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect, LayoutCtx) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let selected = selected_index(node, children.len());
        // Keep the tab header at its explicit/natural height. Previously this
        // used leftover height (`area.height - selected_child_height`), which
        // made the header expand whenever a parent gave tabs more vertical
        // space than their selected child naturally needed. Custom instrument
        // UIs often nest a tabs widget inside a stretched panel, causing the
        // tab labels/content to be pushed down.
        let header_h = header_height_for_value(node, 1.0).min(area.height);
        let child_h = (area.height - header_h).max(0.0);

        let child_area = Rect {
            row: area.row + header_h,
            col: area.col,
            width: area.width,
            height: child_h,
        };

        children
            .get(selected)
            .map(|child| build_child(child, child_area, LayoutCtx::default()))
            .into_iter()
            .collect()
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        _drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        // Only respond to clicks in the header area (above children)
        let header_bottom = node
            .children
            .first()
            .map(|c| c.rect.row)
            .unwrap_or(node.rect.row + node.rect.height);
        if local_row >= header_bottom {
            return MouseEventOutcome::Ignore;
        }
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let items = get_items(&node.props);
                if items.is_empty() {
                    return MouseEventOutcome::Consume;
                }
                let rel_col = local_col - node.rect.col;
                let clicked = if get_bool_prop(&node.props, "compact", false) {
                    compact_hit_index(&node.props, &items, rel_col)
                } else {
                    let tab_width = node.rect.width / items.len() as f32;
                    Some(((rel_col / tab_width).floor() as usize).min(items.len() - 1))
                };
                if let Some(clicked) = clicked {
                    MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::Number(clicked as f64)))
                } else {
                    MouseEventOutcome::Consume
                }
            }
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(Value::Number(idx)) = event else {
            return None;
        };
        let callback = node.props.get("on-change")?.clone();
        Some(EventOutput {
            callback,
            args: vec![Value::Number(idx)],
        })
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: super::WidgetViewport,
    ) -> Vec<super::MetalPrimitive> {
        let items = get_items(&node.props);
        if items.is_empty() {
            return Vec::new();
        }
        let selected = get_f32_prop(&node.props, "value", 0.0) as usize;
        let aspect = if viewport.cell_w > 0.0 {
            viewport.cell_h / viewport.cell_w
        } else {
            1.0
        };

        // Only render the header portion (HEADER_HEIGHT text rows)
        let padding = get_f32_prop(&node.props, "padding", 0.0);
        let header_h = header_height(&node.props, aspect);
        let header_rect = Rect {
            row: node.rect.row,
            col: node.rect.col,
            width: node.rect.width,
            height: header_h,
        };

        let inner_width = (header_rect.width - padding * 2.0).max(0.0);
        let tab_col_start = header_rect.col + padding;
        let compact = get_bool_prop(&node.props, "compact", false);
        let compact_widths = compact_tab_widths(&node.props, &items);
        let gap = compact_gap(&node.props);
        let mut prims = Vec::new();

        for (i, label) in items.iter().enumerate() {
            let is_selected = i == selected;
            let (tab_col, tab_width) = if compact {
                let tab_col =
                    tab_col_start + compact_widths.iter().take(i).sum::<f32>() + gap * i as f32;
                (tab_col, compact_widths[i])
            } else {
                let tab_width = inner_width / items.len() as f32;
                (tab_col_start + (i as f32) * tab_width, tab_width)
            };
            let fg = if is_selected {
                theme::WHITE()
            } else {
                theme::BRIGHT_BLACK()
            };

            let label_len = label.chars().count() as f32;
            let text_pad = ((tab_width - label_len) / 2.0).max(0.0);
            prims.push(super::MetalPrimitive::ProportionalText(
                super::MetalProportionalTextPrimitive {
                    row: header_rect.row,
                    col: tab_col + text_pad,
                    align_width: 0.0,
                    h_align: 0.0,
                    text: label.clone(),
                    font_size: DEFAULT_FONT_SIZE,
                    fg,
                    bg: theme::BG(),
                },
            ));

            if is_selected {
                let underline_height = 0.15;
                prims.push(super::MetalPrimitive::Quad(super::MetalQuadPrimitive {
                    x: tab_col * viewport.cell_w,
                    y: (header_rect.row + header_h - underline_height) * viewport.cell_w,
                    width: tab_width * viewport.cell_w,
                    height: underline_height * viewport.cell_w,
                    color: theme::WHITE(),
                }));
            }
        }

        prims
    }
}
