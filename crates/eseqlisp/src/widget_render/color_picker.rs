//! `color-picker`: a saturation/value square over a hue strip and an alpha
//! strip, the familiar HSV picker.
//!
//! The widget is stateless about color: it displays `:h :s :v :a` (all
//! `0..1`) and reports `(h s v a)` to `:on-change` on every press and drag.
//! The caller keeps HSV rather than RGB so a drag through grey (saturation 0)
//! does not lose the hue. `hsv->rgb` / `rgb->hsv` convert at the edges.
//!
//! Which part a drag edits is decided by where it started, so sweeping the SV
//! square past its bottom edge keeps editing S/V instead of jumping into the
//! hue strip. Gradients are Gouraud-shaded meshes in the foreground layer
//! (exact for the piecewise-linear hue ramp and the alpha ramp, a fine grid
//! for the bilinear SV square), so there is no fragment shader to keep in
//! sync between Metal and WGSL. Markers sit in the same layer, after the
//! gradients, so they draw on top.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

use super::{
    CellBuffer, EventOutput, GpuPrimitive, GpuRectPrimitive, GpuShadedMeshPrimitive,
    GpuShadedVertex, MouseEventOutcome, WidgetDefinition, WidgetEvent, WidgetViewport,
    get_f32_prop, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::vm::Value;

pub struct ColorPickerWidget;

pub static COLOR_PICKER_WIDGET: ColorPickerWidget = ColorPickerWidget;

const DEFAULT_WIDTH: f32 = 24.0;
/// Each strip's height and the gaps around it, as fractions of the widget
/// height.
const STRIP_FRACTION: f32 = 0.085;
const GAP_FRACTION: f32 = 0.035;
/// SV square mesh resolution. The SV gradient is bilinear in (s, v), so a
/// linear-interpolating triangle grid approximates it; 16x16 is visually exact.
const SV_GRID: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerRegion {
    SaturationValue,
    Hue,
    Alpha,
}

/// The three sub-rects of a picker occupying `rect`: SV square, hue strip,
/// alpha strip, top to bottom.
pub fn picker_regions(rect: Rect) -> (Rect, Rect, Rect) {
    let strip = rect.height * STRIP_FRACTION;
    let gap = rect.height * GAP_FRACTION;
    let sv_height = (rect.height - 2.0 * strip - 2.0 * gap).max(0.0);
    let sv = Rect { height: sv_height, ..rect };
    let hue = Rect {
        row: rect.row + sv_height + gap,
        height: strip,
        ..rect
    };
    let alpha = Rect {
        row: hue.row + strip + gap,
        height: strip,
        ..rect
    };
    (sv, hue, alpha)
}

/// The region a point belongs to: the SV square above the first gap's
/// midpoint, the hue strip down to the second gap's midpoint, alpha below.
pub fn region_at(rect: Rect, row: f32) -> PickerRegion {
    let (sv, hue, alpha) = picker_regions(rect);
    if row < (sv.row + sv.height + hue.row) * 0.5 {
        PickerRegion::SaturationValue
    } else if row < (hue.row + hue.height + alpha.row) * 0.5 {
        PickerRegion::Hue
    } else {
        PickerRegion::Alpha
    }
}

/// `(h s v a)` from the props, each clamped to `0..1`.
pub fn picker_hsva(props: &HashMap<String, Value>) -> [f32; 4] {
    [
        get_f32_prop(props, "h", 0.0).clamp(0.0, 1.0),
        get_f32_prop(props, "s", 0.0).clamp(0.0, 1.0),
        get_f32_prop(props, "v", 1.0).clamp(0.0, 1.0),
        get_f32_prop(props, "a", 1.0).clamp(0.0, 1.0),
    ]
}

/// HSV (all `0..1`) to RGB (all `0..1`).
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = (h.rem_euclid(1.0)) * 6.0;
    let sector = h.floor();
    let f = h - sector;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match sector as i32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// RGB (all `0..1`) to HSV (all `0..1`). Grey reports hue 0.
pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let v = max;
    let s = if max > 0.0 { delta / max } else { 0.0 };
    if delta <= f32::EPSILON {
        return [0.0, s, v];
    }
    let h = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    [h / 6.0, s, v]
}

fn unit(value: f32, start: f32, extent: f32) -> f32 {
    ((value - start) / extent.max(0.001)).clamp(0.0, 1.0)
}

fn pick_event(region: PickerRegion, node: &LayoutNode, col: f32, row: f32) -> WidgetEvent {
    let [mut h, mut s, mut v, mut a] = picker_hsva(&node.props);
    let (sv, hue, alpha) = picker_regions(node.rect);
    match region {
        PickerRegion::SaturationValue => {
            s = unit(col, sv.col, sv.width);
            v = 1.0 - unit(row, sv.row, sv.height);
        }
        PickerRegion::Hue => h = unit(col, hue.col, hue.width),
        PickerRegion::Alpha => a = unit(col, alpha.col, alpha.width),
    }
    WidgetEvent::Custom(Value::List(
        [h, s, v, a]
            .into_iter()
            .map(|n| Rc::new(RefCell::new(Value::Number(n as f64))))
            .collect(),
    ))
}

fn color(rgb: [f32; 3], a: f32) -> Color {
    Color::rgba(rgb[0], rgb[1], rgb[2], a)
}

fn push_quad(vertices: &mut Vec<GpuShadedVertex>, rect: Rect, corners: [Color; 4]) {
    // corners: top-left, top-right, bottom-left, bottom-right
    let tl = [rect.col, rect.row];
    let tr = [rect.col + rect.width, rect.row];
    let bl = [rect.col, rect.row + rect.height];
    let br = [rect.col + rect.width, rect.row + rect.height];
    for (point, color) in [
        (tl, corners[0]),
        (tr, corners[1]),
        (bl, corners[2]),
        (tr, corners[1]),
        (br, corners[3]),
        (bl, corners[2]),
    ] {
        vertices.push(GpuShadedVertex { point, color });
    }
}

fn outline(rect: Rect, color: Color, line_w: f32, line_h: f32, out: &mut Vec<GpuPrimitive>) {
    for edge in [
        Rect { height: line_h, ..rect },
        Rect {
            row: rect.row + rect.height - line_h,
            height: line_h,
            ..rect
        },
        Rect { width: line_w, ..rect },
        Rect {
            col: rect.col + rect.width - line_w,
            width: line_w,
            ..rect
        },
    ] {
        out.push(GpuPrimitive::ForegroundRect(GpuRectPrimitive { rect: edge, color }));
    }
}

/// A two-tone marker (dark outer ring, light inner ring) that reads on any
/// color underneath.
fn marker(rect: Rect, line_w: f32, line_h: f32, out: &mut Vec<GpuPrimitive>) {
    outline(rect, Color::rgba(0.0, 0.0, 0.0, 0.85), line_w, line_h, out);
    let inner = Rect {
        col: rect.col + line_w,
        row: rect.row + line_h,
        width: (rect.width - 2.0 * line_w).max(0.0),
        height: (rect.height - 2.0 * line_h).max(0.0),
    };
    outline(inner, Color::rgba(1.0, 1.0, 1.0, 0.95), line_w, line_h, out);
}

fn checkerboard(rect: Rect, cell_w: f32, cell_h: f32, out: &mut Vec<GpuPrimitive>) {
    let light = Color::rgba(0.78, 0.78, 0.80, 1.0);
    let dark = Color::rgba(0.50, 0.50, 0.53, 1.0);
    out.push(GpuPrimitive::ForegroundRect(GpuRectPrimitive { rect, color: light }));
    // Squares sized to the strip height in pixels so they stay square.
    let side_px = (rect.height * cell_h * 0.5).max(2.0);
    let side_w = side_px / cell_w;
    let side_h = side_px / cell_h;
    let cols = (rect.width / side_w).ceil() as usize;
    for row in 0..2 {
        for col in 0..cols {
            if (row + col) % 2 == 0 {
                continue;
            }
            let x = rect.col + col as f32 * side_w;
            out.push(GpuPrimitive::ForegroundRect(GpuRectPrimitive {
                rect: Rect {
                    col: x,
                    row: rect.row + row as f32 * side_h,
                    width: side_w.min(rect.col + rect.width - x),
                    height: side_h.min(rect.height - row as f32 * side_h),
                },
                color: dark,
            }));
        }
    }
}

impl WidgetDefinition for ColorPickerWidget {
    fn names(&self) -> &'static [&'static str] {
        &["color-picker"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["h", "s", "v", "a"]
    }

    fn doc(&self) -> &'static str {
        "An HSV colour picker: a saturation/value square above a hue strip and an \
alpha strip. It shows :h :s :v :a, each 0..1 (defaults h 0, s 0, v 1, a 1, all \
bindable), and does not change them itself. Every press and drag calls :on-change \
with four numbers, (h s v a), each 0..1; only the part being dragged differs from \
the props. Which part a drag edits is fixed by where it started, so overshooting \
the square does not jump into the hue strip. Keep the colour as HSV in your state \
(so dragging through grey keeps its hue) and convert with hsv->rgb / rgb->hsv. It \
is :width cells wide (default 24, or :width :fill for the available width) and \
tall enough for a square SV area unless :height is given. Example: \
(color-picker :h h :s s :v v :a a :width 20 \
:on-change (lambda (h s v a) (set-color! h s v a)))"
    }

    fn completion_props(&self) -> &'static [&'static str] {
        &["h", "s", "v", "a", "width", "height", "on-change"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[&Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let fill = crate::layout::prop_is_keyword(node, "width", "fill")
            && constraints.max_width.is_finite();
        let width = if fill {
            constraints.max_width
        } else {
            get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or(DEFAULT_WIDTH)
        }
        .max(1.0);
        // Square SV area by default; the strips add their share below it.
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or_else(|| {
                let sv = width / constraints.aspect.max(f32::EPSILON);
                sv / (1.0 - 2.0 * (STRIP_FRACTION + GAP_FRACTION))
            })
            .max(1.0);
        Some(Size { width, height })
    }

    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let [h, s, v, _] = picker_hsva(props);
        let fg = color(hsv_to_rgb(h, s, v), 1.0);
        let row0 = rect.row.round() as u16;
        let col0 = rect.col.round() as u16;
        let width = rect.width.round().max(1.0) as u16;
        for col in 0..width {
            buf.set(row0, col0 + col, styled_cell('█', fg, None));
        }
    }

    fn mouse_event(
        &self,
        node: &LayoutNode,
        mouse_kind: MouseEventKind,
        local_col: f32,
        local_row: f32,
        drag_start: Option<(f32, f32)>,
        _gesture: Option<&Value>,
        _modifiers: KeyModifiers,
        _cell_w: f32,
        _cell_h: f32,
    ) -> MouseEventOutcome {
        let local_row = super::scroll::event_content_row(local_row);
        match mouse_kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let region = region_at(node.rect, local_row);
                MouseEventOutcome::Dispatch(pick_event(region, node, local_col, local_row))
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let start_row = drag_start
                    .map_or(local_row, |(_, row)| super::scroll::event_content_row(row));
                let region = region_at(node.rect, start_row);
                MouseEventOutcome::Dispatch(pick_event(region, node, local_col, local_row))
            }
            MouseEventKind::Up(MouseButton::Left) => MouseEventOutcome::Consume,
            _ => MouseEventOutcome::Ignore,
        }
    }

    fn captures_drag(&self) -> bool {
        true
    }

    fn unclamped_drag(&self) -> bool {
        true
    }

    fn handle_event(&self, node: &LayoutNode, event: WidgetEvent) -> Option<EventOutput> {
        let WidgetEvent::Custom(Value::List(items)) = event else {
            return None;
        };
        let callback = node.props.get("on-change")?.clone();
        let args = items
            .iter()
            .map(|item| match &*item.borrow() {
                Value::Number(n) => Value::Number(n.clamp(0.0, 1.0)),
                _ => Value::Number(0.0),
            })
            .collect();
        Some(EventOutput { callback, args })
    }

    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let rect = node.rect;
        if !(rect.width.is_finite() && rect.height.is_finite())
            || rect.width <= 0.0
            || rect.height <= 0.0
        {
            return Vec::new();
        }
        let [h, s, v, a] = picker_hsva(&node.props);
        let (sv, hue, alpha) = picker_regions(rect);
        let cell_w = viewport.cell_w.max(1.0);
        let cell_h = viewport.cell_h.max(1.0);
        let line_w = super::ui_design_px(1.0) / cell_w;
        let line_h = super::ui_design_px(1.0) / cell_h;

        let mut prims = Vec::new();
        let mut mesh = Vec::with_capacity(SV_GRID * SV_GRID * 6 + 6 * 6 + 6);

        // SV square: s across, v down (1 at the top).
        let n = SV_GRID as f32;
        for gy in 0..SV_GRID {
            for gx in 0..SV_GRID {
                let s0 = gx as f32 / n;
                let s1 = (gx + 1) as f32 / n;
                let v0 = 1.0 - gy as f32 / n;
                let v1 = 1.0 - (gy + 1) as f32 / n;
                let cell = Rect {
                    col: sv.col + s0 * sv.width,
                    row: sv.row + (gy as f32 / n) * sv.height,
                    width: sv.width / n,
                    height: sv.height / n,
                };
                push_quad(
                    &mut mesh,
                    cell,
                    [
                        color(hsv_to_rgb(h, s0, v0), 1.0),
                        color(hsv_to_rgb(h, s1, v0), 1.0),
                        color(hsv_to_rgb(h, s0, v1), 1.0),
                        color(hsv_to_rgb(h, s1, v1), 1.0),
                    ],
                );
            }
        }

        // Hue strip: six linear segments between the primaries/secondaries.
        for segment in 0..6 {
            let h0 = segment as f32 / 6.0;
            let h1 = (segment + 1) as f32 / 6.0;
            let c0 = color(hsv_to_rgb(h0, 1.0, 1.0), 1.0);
            // `hsv_to_rgb(1.0)` wraps to red, which is the right endpoint.
            let c1 = color(hsv_to_rgb(h1 - 1e-6, 1.0, 1.0), 1.0);
            push_quad(
                &mut mesh,
                Rect {
                    col: hue.col + h0 * hue.width,
                    width: hue.width / 6.0,
                    ..hue
                },
                [c0, c1, c0, c1],
            );
        }

        // The checkerboard goes under the alpha ramp, so it precedes the
        // mesh; the mesh is emitted next, then the markers over it.
        checkerboard(alpha, cell_w, cell_h, &mut prims);
        let rgb = hsv_to_rgb(h, s, v);
        push_quad(
            &mut mesh,
            alpha,
            [color(rgb, 0.0), color(rgb, 1.0), color(rgb, 0.0), color(rgb, 1.0)],
        );
        prims.push(GpuPrimitive::ForegroundMesh(GpuShadedMeshPrimitive { vertices: mesh }));

        let border = Color::rgba(0.0, 0.0, 0.0, 0.45);
        for area in [sv, hue, alpha] {
            outline(area, border, line_w, line_h, &mut prims);
        }

        let ring_px = super::ui_design_px(10.0);
        let ring_w = ring_px / cell_w;
        let ring_h = ring_px / cell_h;
        marker(
            Rect {
                col: sv.col + s * sv.width - ring_w * 0.5,
                row: sv.row + (1.0 - v) * sv.height - ring_h * 0.5,
                width: ring_w,
                height: ring_h,
            },
            line_w,
            line_h,
            &mut prims,
        );
        let bar_w = super::ui_design_px(6.0) / cell_w;
        for (strip, t) in [(hue, h), (alpha, a)] {
            marker(
                Rect {
                    col: strip.col + t * strip.width - bar_w * 0.5,
                    row: strip.row - line_h,
                    width: bar_w,
                    height: strip.height + 2.0 * line_h,
                },
                line_w,
                line_h,
                &mut prims,
            );
        }
        prims
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::LayoutAnimationHints;

    fn node(props: HashMap<String, Value>) -> LayoutNode {
        LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "color-picker".to_string(),
            rect: Rect {
                col: 0.0,
                row: 0.0,
                width: 20.0,
                height: 20.0,
            },
            props,
            children: Vec::new(),
            focusable: false,
            animation: LayoutAnimationHints::default(),
        }
    }

    fn numbers(output: &EventOutput) -> Vec<f64> {
        output
            .args
            .iter()
            .map(|arg| match arg {
                Value::Number(n) => *n,
                other => panic!("expected number, got {other:?}"),
            })
            .collect()
    }

    fn dispatch(node: &LayoutNode, kind: MouseEventKind, col: f32, row: f32, start: Option<(f32, f32)>) -> Vec<f64> {
        let MouseEventOutcome::Dispatch(event) = COLOR_PICKER_WIDGET.mouse_event(
            node, kind, col, row, start, None, KeyModifiers::NONE, 1.0, 1.0,
        ) else {
            panic!("expected dispatch");
        };
        numbers(&COLOR_PICKER_WIDGET.handle_event(node, event).expect("on-change"))
    }

    #[test]
    fn hsv_round_trips_through_rgb() {
        for rgb in [[1.0, 0.0, 0.0], [0.2, 0.6, 0.4], [0.9, 0.9, 0.1], [0.3, 0.1, 0.8]] {
            let [h, s, v] = rgb_to_hsv(rgb[0], rgb[1], rgb[2]);
            let back = hsv_to_rgb(h, s, v);
            for i in 0..3 {
                assert!((back[i] - rgb[i]).abs() < 1e-5, "{rgb:?} -> {back:?}");
            }
        }
        assert_eq!(rgb_to_hsv(0.5, 0.5, 0.5), [0.0, 0.0, 0.5]);
    }

    #[test]
    fn press_in_the_square_sets_saturation_and_value_only() {
        let mut props = HashMap::new();
        props.insert("on-change".to_string(), Value::Keyword("cb".to_string()));
        props.insert("h".to_string(), Value::Number(0.3));
        props.insert("a".to_string(), Value::Number(0.5));
        let node = node(props);
        let (sv, _, _) = picker_regions(node.rect);
        let out = dispatch(&node, MouseEventKind::Down(MouseButton::Left), sv.width * 0.25, sv.height * 0.25, None);
        assert!((out[0] - 0.3).abs() < 1e-6, "hue untouched");
        assert!((out[1] - 0.25).abs() < 1e-4, "s from x");
        assert!((out[2] - 0.75).abs() < 1e-4, "v from y, top is 1");
        assert!((out[3] - 0.5).abs() < 1e-6, "alpha untouched");
    }

    #[test]
    fn drag_stays_in_the_region_it_started_in() {
        let mut props = HashMap::new();
        props.insert("on-change".to_string(), Value::Keyword("cb".to_string()));
        props.insert("s".to_string(), Value::Number(0.4));
        props.insert("v".to_string(), Value::Number(0.6));
        let node = node(props);
        let (_, hue, alpha) = picker_regions(node.rect);
        // A drag that began on the hue strip and wandered into the alpha strip
        // still edits hue.
        let out = dispatch(
            &node,
            MouseEventKind::Drag(MouseButton::Left),
            15.0,
            alpha.row + alpha.height * 0.5,
            Some((2.0, hue.row + hue.height * 0.5)),
        );
        assert!((out[0] - 0.75).abs() < 1e-4, "hue from x");
        assert!((out[1] - 0.4).abs() < 1e-6 && (out[2] - 0.6).abs() < 1e-6);
        // Pressing on alpha edits alpha.
        let out = dispatch(&node, MouseEventKind::Down(MouseButton::Left), 5.0, alpha.row + 0.1, None);
        assert!((out[3] - 0.25).abs() < 1e-4);
    }

    #[test]
    fn gradients_are_one_foreground_mesh_with_markers_after_it() {
        let node = node(HashMap::new());
        let prims = COLOR_PICKER_WIDGET.build_primitives(
            "color-picker",
            &node,
            WidgetViewport {
                cell_w: 8.0,
                cell_h: 16.0,
                vp_w: 640.0,
                vp_h: 360.0,
                time_seconds: 0.0,
                focused_widget_id: None,
                focused_branch: false,
                overlay_viewport_bottom: 36.0,
                scroll_top: 0.0,
                scroll_left: 0.0,
                inherited_hover: false,
            },
        );
        let mesh_at = prims
            .iter()
            .position(|p| matches!(p, GpuPrimitive::ForegroundMesh(_)))
            .expect("one gradient mesh");
        assert_eq!(
            prims.iter().filter(|p| matches!(p, GpuPrimitive::ForegroundMesh(_))).count(),
            1
        );
        assert!(prims[mesh_at + 1..]
            .iter()
            .all(|p| matches!(p, GpuPrimitive::ForegroundRect(_))));
    }
}
