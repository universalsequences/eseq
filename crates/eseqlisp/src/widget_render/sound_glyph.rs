//! `sound-glyph` widget (sound-glyph spec P2): renders a sound's plant glyph
//! from a host-published [`SoundGlyphFrame`](crate::sound_glyph_data).
//!
//! The widget is deliberately dumb: its `:source` prop is an opaque frame
//! key (the host mints one per sound, e.g. `sound-glyph:track:0:patch:3`);
//! geometry lives in the frame in glyph unit space (0..1 square, y = 0 at
//! the top) and is scaled here into the largest centered square that fits
//! the layout rect, working in pixel space because cells are not square.
//! Monochrome for now — P3 adds per-branch diff tints to the frame. Inert:
//! no input handlers, no animation frames, nothing in the TUI.

use std::collections::HashMap;

use super::{resolve_named_color, CellBuffer, WidgetDefinition};
use crate::backend::Color;
use crate::layout::{f64_to_f32, get_prop_num, Constraints, MeasureCtx, Size};
use crate::vm::Value;

#[cfg(target_os = "macos")]
use super::{
    MetalCirclePrimitive, MetalCircleVisibleHalf, MetalPrimitive, MetalRectPrimitive,
    MetalTrianglePrimitive, WidgetViewport,
};
#[cfg(target_os = "macos")]
use crate::layout::LayoutNode;

pub struct SoundGlyphWidget;

pub static SOUND_GLYPH_WIDGET: SoundGlyphWidget = SoundGlyphWidget;

/// Tip stroke width as a fraction of the root width (taper).
#[cfg(target_os = "macos")]
const TIP_TAPER: f32 = 0.35;

pub fn source_key(props: &HashMap<String, Value>) -> Option<String> {
    match props.get("source") {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

impl WidgetDefinition for SoundGlyphWidget {
    fn names(&self) -> &'static [&'static str] {
        &["sound-glyph"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or(constraints.max_width)
            .clamp(4.0, constraints.max_width.max(4.0));
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(6.0)
            .max(2.0);
        Some(Size { width, height })
    }

    fn tui_render(
        &self,
        _props: &HashMap<String, Value>,
        _rect: crate::layout::Rect,
        _buf: &mut CellBuffer,
    ) {
        // Nothing in the TUI (spec P2).
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let mut primitives = Vec::new();
        let background = resolve_named_color(
            &node.props,
            "background-color",
            Color::rgba(0.0, 0.0, 0.0, 0.0),
        );
        if background.a > 0.001 {
            primitives.push(MetalPrimitive::Rect(MetalRectPrimitive {
                rect: node.rect,
                color: background,
            }));
        }
        let Some(frame) =
            source_key(&node.props).and_then(|k| crate::sound_glyph_data::sound_glyph_frame(&k))
        else {
            return primitives;
        };
        let stroke_color =
            resolve_named_color(&node.props, "color", Color::rgba(0.66, 0.70, 0.66, 0.95));
        let mark_color = Color::rgba(
            stroke_color.r,
            stroke_color.g,
            stroke_color.b,
            stroke_color.a * 0.75,
        );

        // Largest centered square inside the rect, in pixel space.
        let (cell_w, cell_h) = (viewport.cell_w.max(1.0), viewport.cell_h.max(1.0));
        let px_w = node.rect.width * cell_w;
        let px_h = node.rect.height * cell_h;
        let side = px_w.min(px_h);
        if side < 4.0 {
            return primitives;
        }
        let origin_px = [
            node.rect.col * cell_w + (px_w - side) * 0.5,
            node.rect.row * cell_h + (px_h - side) * 0.5,
        ];
        let to_px =
            |p: [f32; 2]| -> [f32; 2] { [origin_px[0] + p[0] * side, origin_px[1] + p[1] * side] };
        let to_cells = |p: [f32; 2]| -> [f32; 2] { [p[0] / cell_w, p[1] / cell_h] };

        for stroke in &frame.strokes {
            if stroke.points.len() < 2 {
                continue;
            }
            let points_px: Vec<[f32; 2]> = stroke.points.iter().map(|&p| to_px(p)).collect();
            let root_half = (stroke.width * side * 0.5).max(0.5);
            emit_tapered_stroke(
                &points_px,
                root_half,
                root_half * TIP_TAPER,
                stroke_color,
                &to_cells,
                &mut primitives,
            );
        }
        for mark in &frame.marks {
            let center_px = to_px(mark.pos);
            primitives.push(MetalPrimitive::Circle(MetalCirclePrimitive {
                center: to_cells(center_px),
                radius_px: (mark.radius * side).max(1.0),
                color: mark_color,
                visible_half: MetalCircleVisibleHalf::Full,
            }));
        }
        primitives
    }
}

/// Miter-joined tapered stroke (same construction as the compressor
/// gain-reduction trace): offset each vertex along its joined normal by a
/// half-width interpolated root→tip, emit shared-vertex triangles, and cap
/// joints with small circles so bends stay smooth at glyph scale.
#[cfg(target_os = "macos")]
fn emit_tapered_stroke(
    points_px: &[[f32; 2]],
    root_half: f32,
    tip_half: f32,
    color: Color,
    to_cells: &dyn Fn([f32; 2]) -> [f32; 2],
    primitives: &mut Vec<MetalPrimitive>,
) {
    let n = points_px.len();
    let normalize = |v: [f32; 2]| -> [f32; 2] {
        let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
        if len > 1.0e-6 {
            [v[0] / len, v[1] / len]
        } else {
            [1.0, 0.0]
        }
    };
    let mut left = Vec::with_capacity(n);
    let mut right = Vec::with_capacity(n);
    for i in 0..n {
        let dir_prev = if i > 0 {
            normalize([
                points_px[i][0] - points_px[i - 1][0],
                points_px[i][1] - points_px[i - 1][1],
            ])
        } else {
            normalize([
                points_px[i + 1][0] - points_px[i][0],
                points_px[i + 1][1] - points_px[i][1],
            ])
        };
        let dir_next = if i + 1 < n {
            normalize([
                points_px[i + 1][0] - points_px[i][0],
                points_px[i + 1][1] - points_px[i][1],
            ])
        } else {
            dir_prev
        };
        let n0 = [-dir_prev[1], dir_prev[0]];
        let n1 = [-dir_next[1], dir_next[0]];
        let miter = normalize([n0[0] + n1[0], n0[1] + n1[1]]);
        // Miter limit 3: sharp corners widen at most 3x before beveling.
        let denom = (miter[0] * n1[0] + miter[1] * n1[1]).max(1.0 / 3.0);
        let t = i as f32 / (n - 1) as f32;
        let half = root_half + (tip_half - root_half) * t;
        let reach = half / denom;
        left.push(to_cells([
            points_px[i][0] + miter[0] * reach,
            points_px[i][1] + miter[1] * reach,
        ]));
        right.push(to_cells([
            points_px[i][0] - miter[0] * reach,
            points_px[i][1] - miter[1] * reach,
        ]));
    }
    for i in 1..n {
        primitives.push(MetalPrimitive::Triangle(MetalTrianglePrimitive {
            points: [left[i - 1], left[i], right[i]],
            color,
        }));
        primitives.push(MetalPrimitive::Triangle(MetalTrianglePrimitive {
            points: [left[i - 1], right[i], right[i - 1]],
            color,
        }));
    }
}
