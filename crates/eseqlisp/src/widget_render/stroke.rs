//! Anti-aliased stroke geometry shared by curve widgets.
//!
//! Curves used to be drawn by scattering one small disc per sampled column,
//! which breaks into a dotted trail wherever the curve climbs faster than the
//! column spacing. These helpers instead build a real ribbon: a fully opaque
//! core quad per segment plus a fringe quad on each side whose outer edge fades
//! to alpha 0. The fringe is what supplies the anti-aliasing — the shaded-mesh
//! primitive interpolates vertex color across each face, so the rasterizer
//! produces a linear coverage ramp roughly one pixel wide without needing an
//! SDF pipeline or MSAA.
//!
//! Joins are mitered while the miter stays short (see [`MITER_LIMIT`]) and are
//! filled with a bevel/round wedge otherwise, so the ribbon keeps its full
//! perpendicular width at any join angle — including the near-hairpin apex of a
//! high-Q resonant peak, which a clamped miter would have pinched to a sliver.
//!
//! Everything accumulates into a [`ShadedMesh`], which is emitted as a single
//! `GpuPrimitive::ForegroundMesh`. One primitive per widget keeps the
//! per-frame primitive clone/offset work in the backends proportional to
//! widgets rather than to triangles. The mesh draws in the late pass (above
//! widget instances, live spectrograms and circles), so a response curve stays
//! on top of the spectrum it is layered over; within one mesh, geometry pushed
//! later paints over geometry pushed earlier.

use std::f32::consts::{FRAC_PI_4, PI, TAU};

use super::{GpuPrimitive, GpuShadedMeshPrimitive, GpuShadedVertex, WidgetViewport};
use crate::backend::Color;

/// A join is mitered while `1 / cos(half-angle)` stays at or below this; past
/// it the miter tip would shoot off as a long spike, so a bevel/round wedge
/// fills the outer gap instead. Unlike a clamped miter, the wedge keeps the
/// ribbon's perpendicular width exactly nominal on both sides of the join.
const MITER_LIMIT: f32 = 2.0;

/// Widest angular step used when filling a join wedge or a disc. A wedge
/// narrower than this is a single bevel triangle; a near-hairpin join gets
/// subdivided into a rounded fan instead of one degenerate sliver.
const MAX_ARC_STEP_RADIANS: f32 = FRAC_PI_4;

/// Width of the anti-aliasing fringe on each side of the core, in design
/// pixels. Just under a pixel: wide enough to cover the samples a hard edge
/// would have had to decide, narrow enough that the line still reads crisp.
const FRINGE_PX: f32 = 0.9;

/// Segments around a disc. Small enough to keep the vertex count of a handle
/// modest, large enough that the fringe hides the faceting at handle radii.
const DISC_SEGMENTS: usize = 16;

fn transparent(color: Color) -> Color {
    Color::rgba(color.r, color.g, color.b, 0.0)
}

/// Unit vector, or `[1, 0]` for a degenerate input. The fallback is
/// direction-blind: coincident consecutive points get an arbitrary orientation
/// rather than an inherited one. That only shows up as a stub of ribbon at a
/// duplicated sample, which no caller here produces.
fn normalize(vector: [f32; 2]) -> [f32; 2] {
    let length = vector[0].hypot(vector[1]);
    if length > 1.0e-6 {
        [vector[0] / length, vector[1] / length]
    } else {
        [1.0, 0.0]
    }
}

fn wrap_angle(mut angle: f32) -> f32 {
    while angle <= -PI {
        angle += TAU;
    }
    while angle > PI {
        angle -= TAU;
    }
    angle
}

/// How a vertex offsets the rails of the segments meeting there.
#[derive(Clone, Copy)]
enum Join {
    /// Both segments share one offset vertex `point + direction * reach *
    /// multiplier`. `multiplier` is `1 / cos(half-angle)`, which is exactly the
    /// factor that puts the shared vertex at perpendicular distance `reach`
    /// from *both* segments.
    Miter {
        direction: [f32; 2],
        multiplier: f32,
    },
    /// The miter would be too long, so each segment uses its own perpendicular
    /// rail and a wedge fills the gap on the outer side.
    Bevel,
}

/// Accumulates anti-aliased geometry for one widget into a single shaded mesh.
///
/// All widths and radii are **design pixels**; scaling to device pixels happens
/// inside, so callers never call `ui_design_px` themselves. Point coordinates
/// stay in cell units, like every other primitive.
#[derive(Default)]
pub struct ShadedMesh {
    vertices: Vec<GpuShadedVertex>,
}

impl ShadedMesh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    #[cfg(test)]
    pub fn vertices(&self) -> &[GpuShadedVertex] {
        &self.vertices
    }

    /// Emit the accumulated geometry as one `ForegroundMesh` primitive. A mesh
    /// with nothing in it pushes nothing.
    pub fn push_into(self, primitives: &mut Vec<GpuPrimitive>) {
        if self.vertices.is_empty() {
            return;
        }
        primitives.push(GpuPrimitive::ForegroundMesh(GpuShadedMeshPrimitive {
            vertices: self.vertices,
        }));
    }

    fn triangle(&mut self, points: [[f32; 2]; 3], colors: [Color; 3]) {
        for (point, color) in points.into_iter().zip(colors) {
            self.vertices.push(GpuShadedVertex { point, color });
        }
    }

    fn quad(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2], colors: [Color; 4]) {
        self.triangle([a, b, c], [colors[0], colors[1], colors[2]]);
        self.triangle([a, c, d], [colors[0], colors[2], colors[3]]);
    }

    /// Fan of `steps` core wedges plus their fringe, sweeping `sweep` radians
    /// from `start_angle` around `center_px`. Used both for join wedges and for
    /// whole discs.
    #[allow(clippy::too_many_arguments)]
    fn arc(
        &mut self,
        center_px: [f32; 2],
        cell_w: f32,
        cell_h: f32,
        start_angle: f32,
        sweep: f32,
        steps: usize,
        core_radius: f32,
        outer_radius: f32,
        color: Color,
    ) {
        if steps == 0 {
            return;
        }
        let fade = transparent(color);
        let center_cell = [center_px[0] / cell_w, center_px[1] / cell_h];
        let point = |angle: f32, radius: f32| {
            [
                (center_px[0] + angle.cos() * radius) / cell_w,
                (center_px[1] + angle.sin() * radius) / cell_h,
            ]
        };
        for step in 0..steps {
            let a0 = start_angle + sweep * (step as f32 / steps as f32);
            let a1 = start_angle + sweep * ((step + 1) as f32 / steps as f32);
            let core0 = point(a0, core_radius);
            let core1 = point(a1, core_radius);
            self.triangle([center_cell, core0, core1], [color, color, color]);
            let outer0 = point(a0, outer_radius);
            let outer1 = point(a1, outer_radius);
            self.triangle([core0, outer0, outer1], [color, fade, fade]);
            self.triangle([core0, outer1, core1], [color, fade, color]);
        }
    }

    /// Stroke `points` (cell units) as an anti-aliased ribbon whose opaque core
    /// is `half_width` design pixels to each side of the path.
    ///
    /// Layout of the emitted vertices, which the tests rely on: 18 vertices per
    /// segment in path order (core quad, left fringe quad, right fringe quad),
    /// then any join wedges.
    pub fn push_polyline(
        &mut self,
        points: &[[f32; 2]],
        color: Color,
        viewport: WidgetViewport,
        half_width: f32,
    ) {
        if points.len() < 2 || color.a <= 0.0 {
            return;
        }
        let cell_w = viewport.cell_w.max(1.0);
        let cell_h = viewport.cell_h.max(1.0);
        let half_width_px = super::ui_design_px(half_width).max(0.1);
        let fringe_px = super::ui_design_px(FRINGE_PX).max(0.5);

        let points_px: Vec<[f32; 2]> = points
            .iter()
            .map(|point| [point[0] * cell_w, point[1] * cell_h])
            .collect();
        // Unit direction and left normal of every segment.
        let directions: Vec<[f32; 2]> = points_px
            .windows(2)
            .map(|pair| normalize([pair[1][0] - pair[0][0], pair[1][1] - pair[0][1]]))
            .collect();
        let normals: Vec<[f32; 2]> = directions
            .iter()
            .map(|direction| [-direction[1], direction[0]])
            .collect();

        let joins: Vec<Join> = (0..points_px.len())
            .map(|index| {
                if index == 0 {
                    return Join::Miter {
                        direction: normals[0],
                        multiplier: 1.0,
                    };
                }
                if index == directions.len() {
                    return Join::Miter {
                        direction: normals[index - 1],
                        multiplier: 1.0,
                    };
                }
                let previous = normals[index - 1];
                let next = normals[index];
                let sum = [previous[0] + next[0], previous[1] + next[1]];
                // A hairpin cancels the two normals out; there is no meaningful
                // miter direction there, only a wedge.
                if sum[0].hypot(sum[1]) < 1.0e-3 {
                    return Join::Bevel;
                }
                let direction = normalize(sum);
                let cosine = direction[0] * next[0] + direction[1] * next[1];
                if cosine <= 0.0 || 1.0 / cosine > MITER_LIMIT {
                    Join::Bevel
                } else {
                    Join::Miter {
                        direction,
                        multiplier: 1.0 / cosine,
                    }
                }
            })
            .collect();

        // Cell-space rail vertex for `vertex` as seen by `segment`. At a miter
        // join both adjacent segments hit the identical expression with the
        // identical arguments, so they share bitwise-identical vertices and the
        // ribbon cannot gap; at a bevel join each segment keeps its own
        // perpendicular rail and the wedge below covers the gap.
        let rail = |vertex: usize, segment: usize, reach_px: f32| -> [f32; 2] {
            let (direction, multiplier) = match joins[vertex] {
                Join::Miter {
                    direction,
                    multiplier,
                } => (direction, multiplier),
                Join::Bevel => (normals[segment], 1.0),
            };
            [
                (points_px[vertex][0] + direction[0] * reach_px * multiplier) / cell_w,
                (points_px[vertex][1] + direction[1] * reach_px * multiplier) / cell_h,
            ]
        };

        let fade = transparent(color);
        let edge_px = half_width_px + fringe_px;
        for segment in 0..directions.len() {
            let (start, end) = (segment, segment + 1);
            let left_core_start = rail(start, segment, half_width_px);
            let left_core_end = rail(end, segment, half_width_px);
            let right_core_start = rail(start, segment, -half_width_px);
            let right_core_end = rail(end, segment, -half_width_px);
            let left_edge_start = rail(start, segment, edge_px);
            let left_edge_end = rail(end, segment, edge_px);
            let right_edge_start = rail(start, segment, -edge_px);
            let right_edge_end = rail(end, segment, -edge_px);

            self.quad(
                left_core_start,
                left_core_end,
                right_core_end,
                right_core_start,
                [color, color, color, color],
            );
            self.quad(
                left_edge_start,
                left_edge_end,
                left_core_end,
                left_core_start,
                [fade, fade, color, color],
            );
            self.quad(
                right_core_start,
                right_core_end,
                right_edge_end,
                right_edge_start,
                [color, color, fade, fade],
            );
        }

        // Fill the outer side of every beveled join. The wedge spans from the
        // previous segment's rail to the next one's, so the ink stays a full
        // `half_width_px` thick right through the apex.
        for vertex in 1..directions.len() {
            if !matches!(joins[vertex], Join::Bevel) {
                continue;
            }
            let previous_direction = directions[vertex - 1];
            let next_direction = directions[vertex];
            let cross = previous_direction[0] * next_direction[1]
                - previous_direction[1] * next_direction[0];
            // The turn's outer corner is the side the normals fan apart on,
            // which is opposite the direction of the turn.
            let side = if cross > 0.0 { -1.0 } else { 1.0 };
            let previous_normal = normals[vertex - 1];
            let next_normal = normals[vertex];
            let start_angle = (previous_normal[1] * side).atan2(previous_normal[0] * side);
            let end_angle = (next_normal[1] * side).atan2(next_normal[0] * side);
            let sweep = wrap_angle(end_angle - start_angle);
            let steps = ((sweep.abs() / MAX_ARC_STEP_RADIANS).ceil() as usize).max(1);
            self.arc(
                points_px[vertex],
                cell_w,
                cell_h,
                start_angle,
                sweep,
                steps,
                half_width_px,
                edge_px,
                color,
            );
        }
    }

    /// Anti-aliased filled disc of `radius` design pixels.
    pub fn push_disc(
        &mut self,
        center: [f32; 2],
        radius: f32,
        color: Color,
        viewport: WidgetViewport,
    ) {
        if radius <= 0.0 || color.a <= 0.0 {
            return;
        }
        let cell_w = viewport.cell_w.max(1.0);
        let cell_h = viewport.cell_h.max(1.0);
        let radius_px = super::ui_design_px(radius);
        let center_px = [center[0] * cell_w, center[1] * cell_h];
        let fringe_px = super::ui_design_px(FRINGE_PX).max(0.5);
        let inner = (radius_px - fringe_px * 0.5).max(radius_px * 0.35);
        self.arc(
            center_px,
            cell_w,
            cell_h,
            0.0,
            TAU,
            DISC_SEGMENTS,
            inner,
            inner + fringe_px,
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL_W: f32 = 10.0;
    const CELL_H: f32 = 20.0;

    fn viewport() -> WidgetViewport {
        WidgetViewport {
            cell_w: CELL_W,
            cell_h: CELL_H,
            vp_w: 800.0,
            vp_h: 600.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 24.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        }
    }

    fn stroke(points: &[[f32; 2]], half_width: f32) -> ShadedMesh {
        let mut mesh = ShadedMesh::new();
        mesh.push_polyline(
            points,
            Color::rgba(1.0, 1.0, 1.0, 1.0),
            viewport(),
            half_width,
        );
        mesh
    }

    /// Perpendicular half width of the opaque core at the start of every
    /// segment, in device pixels: the rail-to-rail vector projected onto that
    /// segment's own normal. This is the number the rasterizer actually inks,
    /// and it is what a clamped miter silently collapses at a sharp join.
    fn perpendicular_half_widths(points: &[[f32; 2]], half_width: f32) -> Vec<f32> {
        let mesh = stroke(points, half_width);
        let vertices = mesh.vertices();
        (0..points.len() - 1)
            .map(|segment| {
                let base = segment * 18;
                // Core quad vertex order: left_start, left_end, right_end,
                // left_start, right_end, right_start.
                let left = vertices[base].point;
                let right = vertices[base + 5].point;
                let dx = (left[0] - right[0]) * CELL_W;
                let dy = (left[1] - right[1]) * CELL_H;
                let direction = normalize([
                    (points[segment + 1][0] - points[segment][0]) * CELL_W,
                    (points[segment + 1][1] - points[segment][1]) * CELL_H,
                ]);
                let normal = [-direction[1], direction[0]];
                (dx * normal[0] + dy * normal[1]).abs() * 0.5
            })
            .collect()
    }

    #[test]
    fn a_near_hairpin_apex_keeps_its_full_perpendicular_width() {
        // The eq8 case that motivated this: a Q=18 / +24 dB bell sampled every
        // ~1.5 px jumps ~17 px between the two samples either side of the peak,
        // so the apex angle is a few degrees. A miter clamped to 2x reach inks
        // only `2 * cos(half-angle)` of the nominal width there — about 17% —
        // and the peak renders thin and dim.
        let half_width = 0.75;
        let expected = super::super::ui_design_px(half_width).max(0.1);
        let apex_x = 30.0 / CELL_W;
        let points = vec![
            [0.0, 40.0 / CELL_H],
            [apex_x - 1.5 / CELL_W, 17.0 / CELL_H],
            [apex_x, 0.0],
            [apex_x + 1.5 / CELL_W, 17.0 / CELL_H],
            [60.0 / CELL_W, 40.0 / CELL_H],
        ];
        for width in perpendicular_half_widths(&points, half_width) {
            assert!(
                (width - expected).abs() < 1.0e-3,
                "perpendicular half width {width} should stay at the nominal {expected}"
            );
        }
    }

    #[test]
    fn shallow_and_steep_joins_both_ink_the_nominal_width() {
        let half_width = 1.6;
        let expected = super::super::ui_design_px(half_width).max(0.1);
        let points = vec![[0.0, 1.0], [0.1, 1.0], [0.2, 0.1], [0.3, 1.0], [0.4, 1.0]];
        let widths = perpendicular_half_widths(&points, half_width);
        assert_eq!(widths.len(), 4);
        for width in widths {
            assert!(
                (width - expected).abs() < 1.0e-3,
                "perpendicular half width {width} should stay at the nominal {expected}"
            );
        }
    }

    #[test]
    fn a_gentle_join_miters_and_shares_its_vertices_so_the_ribbon_never_gaps() {
        // Shallow enough that 1/cos(half-angle) stays under the miter limit.
        let points = vec![[0.0, 1.0], [0.1, 0.98], [0.2, 1.0]];
        let mesh = stroke(&points, 1.6);
        let vertices = mesh.vertices();
        // Two segments, no join wedge: pure quads.
        assert_eq!(vertices.len(), 2 * 18);
        // Segment 0's far rail vertices are segment 1's near rail vertices.
        assert_eq!(vertices[1].point, vertices[18].point);
        assert_eq!(vertices[2].point, vertices[23].point);
    }

    #[test]
    fn a_sharp_join_bevels_instead_of_spiking() {
        let points = vec![[0.0, 1.0], [0.1, 0.2], [0.2, 0.9]];
        let mesh = stroke(&points, 1.6);
        let vertices = mesh.vertices();
        // Beyond the two segments' quads there is wedge geometry filling the
        // outer side of the join.
        assert!(vertices.len() > 2 * 18);
        // And no vertex runs away from the path the way a raw miter would: the
        // apex sits 16 px from the plot origin, everything stays near it.
        let apex = [0.1 * CELL_W, 0.2 * CELL_H];
        let reach = super::super::ui_design_px(1.6).max(0.1)
            + super::super::ui_design_px(FRINGE_PX).max(0.5);
        for vertex in &vertices[2 * 18..] {
            let dx = vertex.point[0] * CELL_W - apex[0];
            let dy = vertex.point[1] * CELL_H - apex[1];
            assert!(dx.hypot(dy) <= reach + 1.0e-3);
        }
    }

    #[test]
    fn the_fringe_fades_to_zero_alpha_on_its_outer_edge() {
        let points = vec![[0.0, 1.0], [1.0, 0.5]];
        let mut mesh = ShadedMesh::new();
        mesh.push_polyline(&points, Color::rgba(1.0, 1.0, 1.0, 0.9), viewport(), 1.6);
        let vertices = mesh.vertices();
        // Core quad: fully opaque.
        assert!(vertices[..6].iter().all(|vertex| vertex.color.a == 0.9));
        // Left fringe quad: one edge opaque, the outer edge transparent.
        let fringe = &vertices[6..12];
        assert!(fringe.iter().any(|vertex| vertex.color.a == 0.0));
        assert!(fringe.iter().any(|vertex| vertex.color.a == 0.9));
    }

    #[test]
    fn a_degenerate_path_emits_nothing() {
        let mut primitives = Vec::new();
        let mut mesh = ShadedMesh::new();
        mesh.push_polyline(
            &[[0.0, 0.0]],
            Color::rgba(1.0, 1.0, 1.0, 1.0),
            viewport(),
            1.6,
        );
        assert!(mesh.is_empty());
        mesh.push_into(&mut primitives);
        assert!(primitives.is_empty());
    }

    #[test]
    fn a_whole_widget_batches_into_one_primitive() {
        let mut mesh = ShadedMesh::new();
        mesh.push_polyline(
            &[[0.0, 1.0], [1.0, 0.5], [2.0, 0.9]],
            Color::rgba(1.0, 1.0, 1.0, 1.0),
            viewport(),
            0.75,
        );
        mesh.push_disc(
            [1.0, 0.5],
            10.0,
            Color::rgba(1.0, 0.0, 0.0, 1.0),
            viewport(),
        );
        let mut primitives = Vec::new();
        mesh.push_into(&mut primitives);
        assert_eq!(primitives.len(), 1);
        let GpuPrimitive::ForegroundMesh(batched) = &primitives[0] else {
            panic!("expected one batched mesh");
        };
        assert_eq!(batched.vertices.len() % 3, 0);
        // A 16-segment disc is 3 triangles per segment.
        assert_eq!(batched.vertices.len(), 2 * 18 + DISC_SEGMENTS * 9);
    }
}
