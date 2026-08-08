#[cfg(target_os = "macos")]
use std::collections::HashMap;

use super::WidgetDefinition;
#[cfg(target_os = "macos")]
use super::{
    MetalPrimitive, MetalProportionalTextPrimitive, MetalRectPrimitive, WidgetInstance,
    WidgetViewport, get_f32_prop, ndc_bounds, push_overlay_primitive, resolve_named_color,
};
#[cfg(target_os = "macos")]
use crate::backend::Color;
use crate::layout::{
    Constraints, LayoutCtx, LayoutNode, MeasureCtx, Rect, Size, current_frame_viewport,
    current_layout_cell_dims, f64_to_f32, get_prop_num, get_prop_str,
};
use crate::vm::Value;

/// Fraction of the frame viewport a modal occupies on each axis when no
/// explicit `:width`/`:height` is given.
const DEFAULT_VIEWPORT_FRACTION: f32 = 0.7;
/// Horizontal content padding inside the panel, in columns.
const CONTENT_PADDING: f32 = 1.5;
/// Vertical content padding inside the panel, in rows (fixed rather than
/// aspect-derived so the inset is stable across cell geometries).
const CONTENT_PADDING_ROWS: f32 = 0.7;
/// Rows reserved for the `:title` header above the child content.
const TITLE_ROWS: f32 = 1.6;
#[cfg(target_os = "macos")]
const SCRIM_ALPHA: f32 = 0.45;
#[cfg(target_os = "macos")]
const PANEL_CORNER_RADIUS_PX: f32 = 30.0;

pub struct ModalWidget;

pub static MODAL_WIDGET: ModalWidget = ModalWidget;

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

/// Centered modal rect within `frame` (both in the same tile-local layout
/// coordinates). Explicit dimensions are clamped to the frame.
pub(crate) fn compute_modal_rect(frame: Rect, width: Option<f32>, height: Option<f32>) -> Rect {
    let width = width
        .unwrap_or(frame.width * DEFAULT_VIEWPORT_FRACTION)
        .clamp(1.0, frame.width.max(1.0));
    let height = height
        .unwrap_or(frame.height * DEFAULT_VIEWPORT_FRACTION)
        .clamp(1.0, frame.height.max(1.0));
    Rect {
        row: frame.row + (frame.height - height) / 2.0,
        col: frame.col + (frame.width - width) / 2.0,
        width,
        height,
    }
}

/// Modal rect for a widget-tree node laid out against `frame`.
pub(crate) fn modal_rect_for_value(node: &Value, frame: Rect) -> Rect {
    let (cell_w, cell_h) = current_layout_cell_dims().unwrap_or((1.0, 1.0));
    let dimension = |cell_prop: &str, pixel_prop: &str, cell_px: f32| {
        get_prop_num(node, pixel_prop)
            .map(f64_to_f32)
            .map(|pixels| pixels / cell_px.max(1.0))
            .or_else(|| get_prop_num(node, cell_prop).map(f64_to_f32))
    };
    compute_modal_rect(
        frame,
        dimension("width", "width-px", cell_w),
        dimension("height", "height-px", cell_h),
    )
}

/// The frame viewport + modal rect recorded on the layout node at layout
/// time (see `build_layout_node` in `ui/layout.rs`). Layout coordinates.
#[cfg(target_os = "macos")]
pub(crate) fn overlay_rects_from_props(props: &HashMap<String, Value>) -> Option<(Rect, Rect)> {
    let rect = |prefix: &str| -> Option<Rect> {
        let read = |suffix: &str| -> Option<f32> {
            match props.get(&format!("{prefix}{suffix}")) {
                Some(Value::Number(n)) => Some(*n as f32),
                _ => None,
            }
        };
        Some(Rect {
            row: read("row")?,
            col: read("col")?,
            width: read("width")?,
            height: read("height")?,
        })
    };
    Some((rect("_frame_")?, rect("_modal_")?))
}

/// Props injected on the open modal's layout node so the render path can
/// reconstruct the scrim + panel geometry without re-deriving the frame
/// viewport (which is only known during layout).
pub(crate) fn injected_layout_props(frame: Rect, modal_rect: Rect) -> [(String, Value); 8] {
    let entry = |key: &str, value: f32| (key.to_string(), Value::Number(value as f64));
    [
        entry("_frame_row", frame.row),
        entry("_frame_col", frame.col),
        entry("_frame_width", frame.width),
        entry("_frame_height", frame.height),
        entry("_modal_row", modal_rect.row),
        entry("_modal_col", modal_rect.col),
        entry("_modal_width", modal_rect.width),
        entry("_modal_height", modal_rect.height),
    ]
}

#[cfg(target_os = "macos")]
fn normalized_corner_radius(rect: Rect, viewport: WidgetViewport, radius_px: f32) -> f32 {
    if radius_px <= 0.0 {
        return 0.001;
    }
    let px_h = (rect.height * viewport.cell_h).max(1.0);
    ((radius_px * 2.0) / px_h).clamp(0.001, 0.5)
}

#[cfg(target_os = "macos")]
fn emit_rounded_rect_overlay(rect: Rect, color: Color, radius_px: f32, viewport: WidgetViewport) {
    let (ndc_min, ndc_max) = ndc_bounds(rect, viewport);
    let px_w = rect.width * viewport.cell_w;
    let px_h = rect.height * viewport.cell_h;
    push_overlay_primitive(MetalPrimitive::WidgetInstance {
        widget_type: "dropdown".to_string(),
        instance: WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: 0.0,
            orientation: 0.0,
            itime: viewport.time_seconds,
            uniform_a: [0.0; 4],
            uniform_b: [0.0; 4],
            uniform_c: [0.0; 4],
            uniform_d: [0.0; 4],
            color_a: [color.r, color.g, color.b, color.a],
            color_b: [0.0; 4],
            color_c: [0.0; 4],
            color_d: [0.0; 4],
            corner_radius: normalized_corner_radius(rect, viewport, radius_px),
            pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
        },
        is_background: true,
    });
}

/// Full-frame scrim behind the panel.
#[cfg(target_os = "macos")]
pub(crate) fn emit_modal_scrim(
    props: &HashMap<String, Value>,
    frame_rect: Rect,
    _viewport: WidgetViewport,
) {
    let scrim = resolve_named_color(
        props,
        "scrim-color",
        Color::rgba(0.0, 0.0, 0.0, SCRIM_ALPHA),
    );
    push_overlay_primitive(MetalPrimitive::Rect(MetalRectPrimitive {
        rect: frame_rect,
        color: scrim,
    }));
}

/// Panel border + background, matching the dropdown menu chrome.
#[cfg(target_os = "macos")]
pub(crate) fn emit_modal_panel_chrome(
    props: &HashMap<String, Value>,
    modal_rect: Rect,
    viewport: WidgetViewport,
) {
    let border_px = 1.0;
    let border_row = border_px / viewport.cell_h.max(1.0);
    let border_col = border_px / viewport.cell_w.max(1.0);
    let border_rect = Rect {
        row: modal_rect.row - border_row,
        col: modal_rect.col - border_col,
        width: modal_rect.width + border_col * 2.0,
        height: modal_rect.height + border_row * 2.0,
    };
    let border_color =
        resolve_named_color(props, "border-color", crate::theme::DROPDOWN_MENU_BORDER());
    emit_rounded_rect_overlay(
        border_rect,
        border_color,
        PANEL_CORNER_RADIUS_PX + border_px,
        viewport,
    );
    let panel_bg = resolve_named_color(props, "background-color", crate::theme::DROPDOWN_MENU_BG());
    emit_rounded_rect_overlay(modal_rect, panel_bg, PANEL_CORNER_RADIUS_PX, viewport);
}

/// Optional `:title` header at the top of the panel.
#[cfg(target_os = "macos")]
pub(crate) fn emit_modal_title(
    props: &HashMap<String, Value>,
    modal_rect: Rect,
    _viewport: WidgetViewport,
) {
    let Some(Value::String(title)) = props.get("title") else {
        return;
    };
    if title.is_empty() {
        return;
    }
    let font_size = get_f32_prop(props, "font-size", crate::layout::DEFAULT_FONT_SIZE) * 1.1;
    push_overlay_primitive(MetalPrimitive::ProportionalText(
        MetalProportionalTextPrimitive {
            row: modal_rect.row + (TITLE_ROWS - 1.0) * 0.5,
            col: modal_rect.col + CONTENT_PADDING,
            align_width: 0.0,
            h_align: 0.0,
            text: title.clone(),
            font_size,
            scale: 1.0,
            fg: resolve_named_color(props, "title-color", crate::theme::DROPDOWN_FG()),
            bg: Color::rgba(0.0, 0.0, 0.0, 0.0),
        },
    ));
}

fn has_title(node: &Value) -> bool {
    get_prop_str(node, "title").is_some_and(|title| !title.is_empty())
}

impl WidgetDefinition for ModalWidget {
    fn names(&self) -> &'static [&'static str] {
        &["modal"]
    }

    fn is_container(&self) -> bool {
        true
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &[
            "is-open",
            "width",
            "height",
            "width-px",
            "height-px",
            "title",
        ]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["is-open"]
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
        // Zero parent footprint: opening a modal must never shift the
        // surrounding layout. The subtree is anchored to the frame viewport
        // in layout_children instead.
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
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
        build_child: &mut dyn FnMut(&Value, Rect, LayoutCtx) -> LayoutNode,
    ) -> Vec<LayoutNode> {
        if !value_is_open(node) {
            return vec![];
        }
        // Tile-local frame viewport installed by the layout engine; the
        // engine falls back to the tile's own root area when the backend
        // did not provide one (tests, capture, TUI).
        let frame = current_frame_viewport().unwrap_or(area);
        let modal_rect = modal_rect_for_value(node, frame);
        let _ = aspect;
        let pad_x = CONTENT_PADDING;
        let pad_y = CONTENT_PADDING_ROWS;
        let title_rows = if has_title(node) { TITLE_ROWS } else { 0.0 };
        let inner = Rect {
            row: modal_rect.row + pad_y + title_rows,
            col: modal_rect.col + pad_x,
            width: (modal_rect.width - pad_x * 2.0).max(0.0),
            height: (modal_rect.height - pad_y * 2.0 - title_rows).max(0.0),
        };
        children
            .first()
            .map(|child| build_child(child, inner, LayoutCtx::default()))
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_rect_defaults_to_seventy_percent_centered() {
        let frame = Rect {
            row: 0.0,
            col: 0.0,
            width: 100.0,
            height: 40.0,
        };
        let rect = compute_modal_rect(frame, None, None);
        assert!((rect.width - 70.0).abs() < 0.001);
        assert!((rect.height - 28.0).abs() < 0.001);
        assert!((rect.col - 15.0).abs() < 0.001);
        assert!((rect.row - 6.0).abs() < 0.001);
    }

    #[test]
    fn explicit_dimensions_are_clamped_to_the_frame() {
        let frame = Rect {
            row: -4.0,
            col: -10.0,
            width: 60.0,
            height: 20.0,
        };
        let rect = compute_modal_rect(frame, Some(500.0), Some(6.0));
        assert!((rect.width - 60.0).abs() < 0.001);
        assert!((rect.height - 6.0).abs() < 0.001);
        // Centered inside the (offset) frame.
        assert!((rect.col - (-10.0)).abs() < 0.001);
        assert!((rect.row - (-4.0 + 7.0)).abs() < 0.001);
    }
}
