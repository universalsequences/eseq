//! Context menu: a pointer-anchored action-menu overlay (modal family).
//!
//! Unlike the `dropdown` (a value picker attached to a control), a
//! `context-menu` is an action menu opened at a pointer event — typically a
//! right-click. It reuses the modal overlay machinery (overlay stack entry of
//! kind `Modal`, subtree diversion into the overlay pass, focus/dismissal
//! intercepts keyed off `is_overlay_panel_widget`) with an anchored placement
//! policy instead of a centered one, and without a scrim.
//!
//! ```lisp
//! (context-menu :is-open (menu-open?) :anchor-col c :anchor-row r
//!               :on-close (fn () (close-menu!))
//!   (menu-item "Rename" :shortcut "⌘R" :on-select (fn (info) ...))
//!   (menu-separator)
//!   (menu-item "Delete" :disabled true :on-select (fn (info) ...)))
//! ```

use std::collections::HashMap;

use super::WidgetDefinition;
use super::{
    GpuPrimitive, GpuProportionalTextPrimitive, GpuRectPrimitive, WidgetViewport,
    resolve_named_color,
};
use crate::backend::Color;
use crate::layout::{
    Constraints, DEFAULT_FONT_SIZE, LayoutCtx, LayoutNode, MeasureCtx, Rect, Size,
    current_frame_viewport, f64_to_f32, get_prop_num, get_prop_str,
};
use crate::vm::Value;

use super::{EventOutput, MouseEventOutcome, WidgetEvent};
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};
use std::cell::RefCell;
use std::rc::Rc;

/// Menu rows span the panel width; text and hover insets come from the shared
/// popup-menu style rather than an additional context-menu-only gutter.
pub(crate) const PANEL_PADDING_COLS: f32 = 0.0;
/// Vertical content padding inside the panel, in rows.
pub(crate) const PANEL_PADDING_ROWS: f32 = super::menu_style::PANEL_PADDING_V;
/// Height of a `menu-item` row.
const ITEM_ROW_HEIGHT: f32 = super::menu_style::ROW_HEIGHT;
/// Height of a `menu-separator` row.
const SEPARATOR_ROW_HEIGHT: f32 = 0.6;
/// Minimum gap between an item's label and its right-aligned shortcut hint.
const SHORTCUT_GAP_COLS: f32 = 2.5;
/// Minimum panel content width, in columns.
const MIN_CONTENT_WIDTH: f32 = 8.0;
/// Fallback per-character width (cells at DEFAULT_FONT_SIZE) when no text
/// measurer is available (layout tests, TUI).
const APPROX_CHAR_WIDTH: f32 = super::menu_style::APPROX_CHAR_WIDTH;
/// Inner horizontal padding of a `menu-item` row (label inset / shortcut inset).
const ITEM_PADDING_COLS: f32 = super::menu_style::TEXT_PADDING_H;
pub struct ContextMenuWidget;
pub struct MenuItemWidget;
pub struct MenuSeparatorWidget;

pub static CONTEXT_MENU_WIDGET: ContextMenuWidget = ContextMenuWidget;
pub static MENU_ITEM_WIDGET: MenuItemWidget = MenuItemWidget;
pub static MENU_SEPARATOR_WIDGET: MenuSeparatorWidget = MenuSeparatorWidget;

fn value_is_open(node: &Value) -> bool {
    let Value::Map(map) = node else {
        return false;
    };
    match map.get("is-open").map(|value| value.borrow().clone()) {
        Some(Value::Bool(open)) => open,
        Some(Value::Number(n)) => n > 0.5,
        Some(Value::ReactiveRef { slot, .. }) => crate::reactive::read_float_slot(&slot) > 0.5,
        _ => false,
    }
}

/// Numeric prop that may be a plain number or a reactive binding (the anchor
/// props accept bindings so apps can point them straight at state cells).
fn prop_f32(node: &Value, key: &str) -> Option<f32> {
    let Value::Map(map) = node else {
        return None;
    };
    match map.get(key).map(|value| value.borrow().clone()) {
        Some(Value::Number(n)) => Some(n as f32),
        Some(Value::ReactiveRef { slot, .. }) => {
            Some(crate::reactive::read_float_slot(&slot) as f32)
        }
        _ => None,
    }
}

/// True when a `menu-item` layout node is disabled.
pub(crate) fn item_disabled(props: &HashMap<String, Value>) -> bool {
    match props.get("disabled") {
        Some(Value::Bool(disabled)) => *disabled,
        Some(Value::Number(n)) => *n > 0.5,
        _ => false,
    }
}

/// Anchored placement with flip + clamp: the panel opens with its top-left
/// corner at the anchor; if it would overflow the frame's right/bottom edge
/// it flips to the left/above the anchor, and is finally clamped fully inside
/// the frame on both axes.
pub(crate) fn anchored_panel_rect(
    anchor_col: f32,
    anchor_row: f32,
    width: f32,
    height: f32,
    frame: Rect,
) -> Rect {
    let width = width.clamp(1.0, frame.width.max(1.0));
    let height = height.clamp(1.0, frame.height.max(1.0));
    let right = frame.col + frame.width;
    let bottom = frame.row + frame.height;

    let col = if anchor_col + width <= right {
        anchor_col
    } else if anchor_col - width >= frame.col {
        anchor_col - width
    } else {
        right - width
    };
    let row = if anchor_row + height <= bottom {
        anchor_row
    } else if anchor_row - height >= frame.row {
        anchor_row - height
    } else {
        bottom - height
    };

    Rect {
        row: row.clamp(frame.row, (bottom - height).max(frame.row)),
        col: col.clamp(frame.col, (right - width).max(frame.col)),
        width,
        height,
    }
}

/// The panel rect implied by a laid-out context-menu's children: their
/// bounding box expanded by the panel padding. Used by `build_layout_node` to
/// inject the overlay geometry props (the render path cannot re-run child
/// measurement).
pub(crate) fn panel_rect_from_children(children: &[LayoutNode]) -> Option<Rect> {
    let first = children.first()?;
    let mut top = first.rect.row;
    let mut left = first.rect.col;
    let mut bottom = first.rect.row + first.rect.height;
    let mut right = first.rect.col + first.rect.width;
    for child in &children[1..] {
        top = top.min(child.rect.row);
        left = left.min(child.rect.col);
        bottom = bottom.max(child.rect.row + child.rect.height);
        right = right.max(child.rect.col + child.rect.width);
    }
    Some(Rect {
        row: top - PANEL_PADDING_ROWS,
        col: left - PANEL_PADDING_COLS,
        width: (right - left) + PANEL_PADDING_COLS * 2.0,
        height: (bottom - top) + PANEL_PADDING_ROWS * 2.0,
    })
}

fn measured_text_width(text: &str, font_size: f32, ctx: &MeasureCtx<'_>) -> f32 {
    match ctx.text_measurer {
        Some(measurer) if ctx.cell_w > 0.0 => {
            measurer.measure_text_px(text, font_size) / ctx.cell_w
        }
        _ => text.chars().count() as f32 * APPROX_CHAR_WIDTH * (font_size / DEFAULT_FONT_SIZE),
    }
}

fn menu_item_select_info(node: &LayoutNode) -> Value {
    let mut info = HashMap::new();
    let mut insert = |key: &str, value: Value| {
        info.insert(key.to_string(), Rc::new(RefCell::new(value)));
    };
    insert("phase", Value::String("select".to_string()));
    if let Some(Value::String(text)) = node.props.get("text") {
        insert("text", Value::String(text.clone()));
    }
    if let Some(value) = node.props.get("value") {
        insert("value", value.clone());
    }
    Value::Map(info)
}

/// Panel border + background. Same chrome family as the dropdown menu and the
/// modal panel, with a tighter corner radius; no scrim behind it.
pub(crate) fn emit_menu_chrome(
    props: &HashMap<String, Value>,
    panel_rect: Rect,
    viewport: WidgetViewport,
) {
    let border_color =
        resolve_named_color(props, "border-color", crate::theme::DROPDOWN_MENU_BORDER());
    let panel_bg = resolve_named_color(props, "background-color", crate::theme::DROPDOWN_MENU_BG());
    super::menu_style::emit_panel_chrome(panel_rect, panel_bg, border_color, viewport);
}

impl WidgetDefinition for ContextMenuWidget {
    fn names(&self) -> &'static [&'static str] {
        &["context-menu"]
    }

    fn is_container(&self) -> bool {
        true
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["is-open", "anchor-col", "anchor-row", "width"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["is-open", "anchor-col", "anchor-row"]
    }

    fn renders_own_focus(&self) -> bool {
        true
    }

    fn measure(
        &self,
        _node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        // Zero parent footprint, like the modal: opening the menu must never
        // shift the surrounding layout.
        Some(Size {
            width: 0.0,
            height: 0.0,
        })
    }

    fn layout_children(
        &self,
        node: &Value,
        area: Rect,
        children: &[Value],
        aspect: f32,
        _layout_ctx: LayoutCtx,
        measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect, LayoutCtx) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        if !value_is_open(node) {
            return vec![];
        }
        let frame = current_frame_viewport().unwrap_or(area);
        let max_content_width = (frame.width - PANEL_PADDING_COLS * 2.0).max(1.0);
        let constraints = Constraints {
            min_width: 0.0,
            max_width: max_content_width,
            min_height: 0.0,
            max_height: f32::INFINITY,
            aspect,
        };
        let sizes: Vec<Size> = children
            .iter()
            .map(|child| {
                measure_child(child, constraints).unwrap_or(Size {
                    width: 0.0,
                    height: ITEM_ROW_HEIGHT,
                })
            })
            .collect();
        let content_width = sizes
            .iter()
            .map(|size| size.width)
            .fold(MIN_CONTENT_WIDTH, f32::max)
            .max(
                get_prop_num(node, "width")
                    .map(f64_to_f32)
                    .map(|w| w - PANEL_PADDING_COLS * 2.0)
                    .unwrap_or(0.0),
            )
            .min(max_content_width);
        let content_height: f32 = sizes.iter().map(|size| size.height).sum();

        let anchor_col = prop_f32(node, "anchor-col").unwrap_or(frame.col);
        let anchor_row = prop_f32(node, "anchor-row").unwrap_or(frame.row);
        let panel = anchored_panel_rect(
            anchor_col,
            anchor_row,
            content_width + PANEL_PADDING_COLS * 2.0,
            content_height + PANEL_PADDING_ROWS * 2.0,
            frame,
        );

        let mut cursor = panel.row + PANEL_PADDING_ROWS;
        children
            .iter()
            .zip(sizes)
            .map(|(child, size)| {
                let rect = Rect {
                    row: cursor,
                    col: panel.col + PANEL_PADDING_COLS,
                    width: content_width,
                    height: size.height,
                };
                cursor += size.height;
                build_child(child, rect, LayoutCtx::default())
            })
            .collect()
    }
}

impl WidgetDefinition for MenuItemWidget {
    fn names(&self) -> &'static [&'static str] {
        &["menu-item"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["text", "shortcut", "font-size"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let font_size = super::menu_style::menu_font_size_from_node(node);
        let text = get_prop_str(node, "text").unwrap_or_default();
        let mut width = ITEM_PADDING_COLS * 2.0 + measured_text_width(&text, font_size, ctx);
        if let Some(shortcut) = get_prop_str(node, "shortcut")
            && !shortcut.is_empty()
        {
            width += SHORTCUT_GAP_COLS + measured_text_width(&shortcut, font_size, ctx);
        }
        Some(Size {
            width,
            height: ITEM_ROW_HEIGHT,
        })
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
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if item_disabled(&node.props) {
                    return MouseEventOutcome::Consume;
                }
                if node.props.contains_key("on-select") {
                    return MouseEventOutcome::Dispatch(WidgetEvent::Custom(
                        menu_item_select_info(node),
                    ));
                }
                MouseEventOutcome::Consume
            }
            MouseEventKind::Up(MouseButton::Left) => MouseEventOutcome::Consume,
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(value) = event else {
            return None;
        };
        let callback = node.props.get("on-select")?.clone();
        Some(EventOutput {
            callback,
            args: vec![value],
        })
    }

    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        _viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let mut prims = Vec::new();
        let disabled = item_disabled(&node.props);
        let dim = |color: Color| {
            if disabled {
                Color::rgba(color.r, color.g, color.b, color.a * 0.4)
            } else {
                color
            }
        };
        if !disabled && super::pointer_hovered(node.widget_id) {
            prims.push(GpuPrimitive::Rect(GpuRectPrimitive {
                rect: super::menu_style::row_highlight_rect(node.rect),
                color: resolve_named_color(
                    &node.props,
                    "hover-color",
                    crate::theme::DROPDOWN_HOVER_BG(),
                ),
            }));
        }
        let font_size = super::menu_style::menu_font_size_from_props(&node.props);
        let text_row = node.rect.row + (ITEM_ROW_HEIGHT - 1.0) * 0.5;
        if let Some(Value::String(text)) = node.props.get("text") {
            prims.push(GpuPrimitive::ProportionalText(
                GpuProportionalTextPrimitive {
                    row: text_row,
                    col: node.rect.col + ITEM_PADDING_COLS,
                    align_width: 0.0,
                    h_align: 0.0,
                    text: text.clone(),
                    font_size,
                    scale: 1.0,
                    fg: dim(resolve_named_color(
                        &node.props,
                        "text-color",
                        crate::theme::DROPDOWN_FG(),
                    )),
                    bg: Color::rgba(0.0, 0.0, 0.0, 0.0),
                },
            ));
        }
        if let Some(Value::String(shortcut)) = node.props.get("shortcut")
            && !shortcut.is_empty()
        {
            let fg = resolve_named_color(&node.props, "shortcut-color", {
                let base = crate::theme::DROPDOWN_FG();
                Color::rgba(base.r, base.g, base.b, base.a * 0.55)
            });
            prims.push(GpuPrimitive::ProportionalText(
                GpuProportionalTextPrimitive {
                    row: text_row,
                    col: node.rect.col + ITEM_PADDING_COLS,
                    align_width: (node.rect.width - ITEM_PADDING_COLS * 2.0).max(0.0),
                    h_align: 1.0,
                    text: shortcut.clone(),
                    font_size,
                    scale: 1.0,
                    fg: dim(fg),
                    bg: Color::rgba(0.0, 0.0, 0.0, 0.0),
                },
            ));
        }
        prims
    }
}

impl WidgetDefinition for MenuSeparatorWidget {
    fn names(&self) -> &'static [&'static str] {
        &["menu-separator"]
    }

    fn measure(
        &self,
        _node: &Value,
        _children: &[Value],
        _constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        // Minimal width so a separator never drives the menu width.
        Some(Size {
            width: 1.0,
            height: SEPARATOR_ROW_HEIGHT,
        })
    }

    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let line_height = 1.0 / viewport.cell_h.max(1.0);
        vec![GpuPrimitive::Rect(GpuRectPrimitive {
            rect: Rect {
                row: node.rect.row + (node.rect.height - line_height) * 0.5,
                col: node.rect.col + ITEM_PADDING_COLS,
                width: (node.rect.width - ITEM_PADDING_COLS * 2.0).max(0.0),
                height: line_height,
            },
            color: resolve_named_color(&node.props, "color", crate::theme::DROPDOWN_MENU_BORDER()),
        })]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Rect = Rect {
        row: 0.0,
        col: 0.0,
        width: 100.0,
        height: 40.0,
    };

    #[test]
    fn panel_opens_below_and_right_of_the_anchor_when_it_fits() {
        let rect = anchored_panel_rect(10.0, 5.0, 20.0, 8.0, FRAME);
        assert!((rect.col - 10.0).abs() < 0.001);
        assert!((rect.row - 5.0).abs() < 0.001);
    }

    #[test]
    fn panel_flips_left_and_above_near_the_far_edges() {
        let rect = anchored_panel_rect(95.0, 38.0, 20.0, 8.0, FRAME);
        assert!((rect.col - 75.0).abs() < 0.001);
        assert!((rect.row - 30.0).abs() < 0.001);
    }

    #[test]
    fn panel_is_clamped_fully_inside_the_frame() {
        // Anchor near the corner of a frame too small to flip cleanly.
        let small = Rect {
            row: 0.0,
            col: 0.0,
            width: 15.0,
            height: 6.0,
        };
        let rect = anchored_panel_rect(14.0, 5.5, 12.0, 5.0, small);
        assert!(rect.col >= small.col - 0.001);
        assert!(rect.row >= small.row - 0.001);
        assert!(rect.col + rect.width <= small.col + small.width + 0.001);
        assert!(rect.row + rect.height <= small.row + small.height + 0.001);
    }

    #[test]
    fn oversized_panel_is_clamped_to_the_frame_dimensions() {
        let rect = anchored_panel_rect(10.0, 5.0, 500.0, 90.0, FRAME);
        assert!((rect.width - FRAME.width).abs() < 0.001);
        assert!((rect.height - FRAME.height).abs() < 0.001);
        assert!((rect.col - FRAME.col).abs() < 0.001);
        assert!((rect.row - FRAME.row).abs() < 0.001);
    }


    fn widget_node(kind: &str, props: &[(&str, Value)], children: Vec<Value>) -> Value {
        let mut map: HashMap<String, Rc<RefCell<Value>>> = HashMap::new();
        map.insert(
            "type".to_string(),
            Rc::new(RefCell::new(Value::String(kind.to_string()))),
        );
        for (key, value) in props {
            map.insert(key.to_string(), Rc::new(RefCell::new(value.clone())));
        }
        if !children.is_empty() {
            map.insert(
                "children".to_string(),
                Rc::new(RefCell::new(Value::List(
                    children
                        .into_iter()
                        .map(|child| Rc::new(RefCell::new(child)))
                        .collect(),
                ))),
            );
        }
        Value::Map(map)
    }

    fn find_widget<'a>(node: &'a LayoutNode, widget_type: &str) -> Option<&'a LayoutNode> {
        if node.widget_type == widget_type {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find_widget(child, widget_type))
    }

    /// Regression (eseq-swo follow-up): the measure pass sized the panel from
    /// the shared popup font while the render pass drew the label at the tile's
    /// *inherited* font, so labels clipped (or the panel sat over-wide).
    /// Both sides must now resolve the same size.
    #[test]
    fn panel_width_and_row_font_agree_under_a_non_default_tile_font() {
        const LABEL: &str = "Rename this drum rack pad";
        let tile_font = 12.0_f32;
        assert!((tile_font - DEFAULT_FONT_SIZE).abs() > 0.01);
        assert!((tile_font - super::super::menu_style::MENU_FONT_SIZE).abs() > 0.01);

        let tree = widget_node(
            "box",
            &[("font-size", Value::Number(tile_font as f64))],
            vec![widget_node(
                "context-menu",
                &[
                    ("is-open", Value::Bool(true)),
                    ("anchor-col", Value::Number(2.0)),
                    ("anchor-row", Value::Number(2.0)),
                ],
                vec![widget_node(
                    "menu-item",
                    &[("text", Value::String(LABEL.to_string()))],
                    vec![],
                )],
            )],
        );

        let layout = crate::layout::LayoutEngine::new(120, 40, 1.0)
            .layout(&tree)
            .expect("layout");
        let item = find_widget(&layout, "menu-item").expect("menu-item laid out");

        // The layout pass really did inject the inherited tile font — without
        // it this test would pass vacuously.
        assert!(
            matches!(item.props.get("font-size"), Some(Value::Number(n)) if (*n as f32 - tile_font).abs() < 0.01),
            "expected the inherited tile font to be injected into the row props"
        );

        // Render side resolves to the shared popup font, not the tile font.
        let rendered = super::super::menu_style::menu_font_size_from_props(&item.props);
        assert!(
            (rendered - super::super::menu_style::MENU_FONT_SIZE).abs() < 0.001,
            "render font {rendered} should be the shared popup font"
        );

        // Measure side (which drives the panel width) used the same value.
        let width_at = |font_size: f32| {
            ITEM_PADDING_COLS * 2.0
                + LABEL.chars().count() as f32 * APPROX_CHAR_WIDTH * (font_size / DEFAULT_FONT_SIZE)
        };
        let expected = width_at(rendered);
        assert!(expected > MIN_CONTENT_WIDTH, "label must clear the min width");
        assert!(
            (item.rect.width - expected).abs() < 0.001,
            "panel row width {} should come from the rendered font ({expected}), not the tile font ({})",
            item.rect.width,
            width_at(tile_font)
        );
    }

    fn menu_item_node(props: &[(&str, Value)]) -> Value {
        let mut map = HashMap::new();
        for (key, value) in props {
            map.insert(key.to_string(), Rc::new(RefCell::new(value.clone())));
        }
        Value::Map(map)
    }

    fn measure_menu_item(node: &Value, inherited_font_size: f32) -> Size {
        let ctx = MeasureCtx {
            text_measurer: None,
            cell_w: 0.0,
            cell_h: 0.0,
            inherited_font_size,
        };
        MENU_ITEM_WIDGET
            .measure(
                node,
                &[],
                Constraints {
                    min_width: 0.0,
                    max_width: f32::INFINITY,
                    min_height: 0.0,
                    max_height: f32::INFINITY,
                    aspect: 1.0,
                },
                &ctx,
                &mut |_, _| None,
            )
            .unwrap()
    }

    #[test]
    fn menu_item_ignores_the_inherited_tile_font_size() {
        // The context menu is chrome: it must match a dropdown popup whatever
        // font the tile it was opened from happens to use.
        let node = menu_item_node(&[("text", Value::String("Rename".to_string()))]);
        let small = measure_menu_item(&node, 8.0);
        let large = measure_menu_item(&node, 24.0);
        assert!((small.width - large.width).abs() < 0.001);

        let expected = ITEM_PADDING_COLS * 2.0
            + "Rename".chars().count() as f32
                * APPROX_CHAR_WIDTH
                * (super::super::menu_style::MENU_FONT_SIZE / DEFAULT_FONT_SIZE);
        assert!((small.width - expected).abs() < 0.001);
    }

    #[test]
    fn explicit_font_size_still_overrides_the_shared_menu_default() {
        let node = menu_item_node(&[
            ("text", Value::String("Rename".to_string())),
            ("font-size", Value::Number(20.0)),
        ]);
        let measured = measure_menu_item(&node, 8.0);
        let expected = ITEM_PADDING_COLS * 2.0
            + "Rename".chars().count() as f32 * APPROX_CHAR_WIDTH * (20.0 / DEFAULT_FONT_SIZE);
        assert!((measured.width - expected).abs() < 0.001);
    }

    #[test]
    fn panel_rect_from_children_expands_their_bounding_box_by_the_padding() {
        let child = |row: f32, height: f32| LayoutNode {
            widget_id: 0,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "menu-item".to_string(),
            rect: Rect {
                row,
                col: 4.0,
                width: 12.0,
                height,
            },
            props: HashMap::new(),
            children: vec![],
            focusable: false,
            animation: Default::default(),
        };
        let rect = panel_rect_from_children(&[child(2.0, 1.4), child(3.4, 0.6)]).unwrap();
        assert!((rect.row - (2.0 - PANEL_PADDING_ROWS)).abs() < 0.001);
        assert!((rect.col - (4.0 - PANEL_PADDING_COLS)).abs() < 0.001);
        assert!((rect.width - (12.0 + PANEL_PADDING_COLS * 2.0)).abs() < 0.001);
        assert!((rect.height - (2.0 + PANEL_PADDING_ROWS * 2.0)).abs() < 0.001);
    }
}
