use std::collections::HashMap;

use crossterm::event::{MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, MouseEventOutcome, WidgetDefinition, WidgetEvent, get_f32_prop,
    styled_cell,
};
use crate::layout::{Constraints, DEFAULT_FONT_SIZE, LayoutNode, MeasureCtx, Rect, Size};
use crate::theme;
use crate::vm::Value;

pub struct TabsWidget;

pub static TABS_WIDGET: TabsWidget = TabsWidget;

/// Height of the tab bar header (text row + underline row)
const HEADER_HEIGHT: f32 = 2.0;

fn header_height(aspect: f32) -> f32 {
    aspect * HEADER_HEIGHT
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

    let row_u16 = rect.row.round() as u16;
    let col_u16 = rect.col.round() as u16 + pad;
    let width_u16 = (rect.width.round() as u16).saturating_sub(pad * 2);
    let tab_width = width_u16 / items.len().max(1) as u16;

    for (i, label) in items.iter().enumerate() {
        let tab_col = col_u16 + (i as u16) * tab_width;
        let is_selected = i == selected;
        let fg = if is_selected {
            theme::GREEN()
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
                theme::GREEN()
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
        let _ = node;
        let header_h = header_height(constraints.aspect);
        let inner = Constraints {
            min_width: constraints.min_width,
            max_width: constraints.max_width,
            min_height: 0.0,
            max_height: (constraints.max_height - header_h).max(0.0),
            aspect: constraints.aspect,
        };
        let max_child_height = children
            .iter()
            .filter_map(|child| measure_child(child, inner))
            .map(|s| s.height)
            .fold(0.0_f32, f32::max);
        Some(Size {
            width: constraints.max_width,
            height: header_h + max_child_height,
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
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        let selected = crate::layout::get_prop_num(node, "value")
            .map(crate::layout::f64_to_f32)
            .unwrap_or(0.0) as usize;

        let child_constraints = Constraints {
            min_width: 0.0,
            max_width: area.width,
            min_height: 0.0,
            max_height: area.height,
            aspect: 1.0,
        };
        let tallest_child_height = children
            .iter()
            .filter_map(|child| measure_child(child, child_constraints))
            .map(|size| size.height)
            .fold(0.0_f32, f32::max);
        let header_h = (area.height - tallest_child_height).clamp(HEADER_HEIGHT, area.height);
        let child_h = (area.height - header_h).max(0.0);

        let child_area = Rect {
            row: area.row + header_h,
            col: area.col,
            width: area.width,
            height: child_h,
        };

        children
            .get(selected)
            .map(|child| build_child(child, child_area))
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
                let tab_width = node.rect.width / items.len() as f32;
                let clicked = (rel_col / tab_width).floor() as usize;
                let clicked = clicked.min(items.len() - 1);
                MouseEventOutcome::Dispatch(WidgetEvent::Custom(Value::Number(clicked as f64)))
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
        let header_h = header_height(aspect);
        let header_rect = Rect {
            row: node.rect.row,
            col: node.rect.col,
            width: node.rect.width,
            height: header_h,
        };

        let inner_width = (header_rect.width - padding * 2.0).max(0.0);
        let tab_col_start = header_rect.col + padding;
        let tab_width = inner_width / items.len() as f32;
        let mut prims = Vec::new();

        // Header background
        prims.push(super::MetalPrimitive::Rect(super::MetalRectPrimitive {
            rect: header_rect,
            color: theme::BG(),
        }));

        for (i, label) in items.iter().enumerate() {
            let is_selected = i == selected;
            let tab_col = tab_col_start + (i as f32) * tab_width;
            let fg = if is_selected {
                theme::GREEN()
            } else {
                theme::BRIGHT_BLACK()
            };

            let label_len = label.chars().count() as f32;
            let text_pad = ((tab_width - label_len) / 2.0).max(0.0);
            prims.push(super::MetalPrimitive::ProportionalText(
                super::MetalProportionalTextPrimitive {
                    row: header_rect.row,
                    col: tab_col + text_pad,
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
                    color: theme::GREEN(),
                }));
            }
        }

        prims
    }
}
