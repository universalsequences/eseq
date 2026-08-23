//! Backend-neutral geometry for GPU primitive runs.
//!
//! Render backends consume the retained [`GpuPrimitiveRun`] display list. This
//! module owns the conversion from cell-space rectangles/quads to GPU instances
//! and the clip-stack math so backend implementations do not drift apart.

use std::ops::Range;

use crate::layout::Rect;
use crate::widget_render::{self, GpuPrimitive, GpuPrimitiveRun};

/// Vertex layout used by the tuned Metal text pipeline. Solid geometry is
/// expanded from the same `SolidQuadInstance` representation consumed by wgpu.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Vertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub fg: [f32; 4],
    pub bg: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "wgpu", derive(bytemuck::Pod, bytemuck::Zeroable))]
pub(crate) struct SolidQuadInstance {
    /// Left, top, right, bottom in NDC. Y decreases towards the bottom.
    pub ndc_bounds: [f32; 4],
    pub color: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PrimitiveRunOp {
    Draw(Range<u32>),
    PushClip(Rect),
    PopClip,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PrimitiveRunGeometry {
    pub instances: Vec<SolidQuadInstance>,
    pub ops: Vec<PrimitiveRunOp>,
}

pub(crate) fn build_primitive_run_geometry(
    run: &GpuPrimitiveRun,
    cell_w: f32,
    cell_h: f32,
    viewport_w: f32,
    viewport_h: f32,
) -> PrimitiveRunGeometry {
    build_primitive_geometry(&run.primitives, cell_w, cell_h, viewport_w, viewport_h)
}

pub(crate) fn build_primitive_geometry(
    primitives: &[GpuPrimitive],
    cell_w: f32,
    cell_h: f32,
    viewport_w: f32,
    viewport_h: f32,
) -> PrimitiveRunGeometry {
    let mut geometry = PrimitiveRunGeometry::default();
    // Instances of one clip segment, tagged with the z index they were authored
    // at. Metal buckets a segment's primitives into a `BTreeMap<i32, Vec<_>>`
    // and draws the buckets in ascending key order
    // (`metal_backend::z_ordered_primitive_layers`); a stable sort by z index
    // reproduces that ordering, including the "untagged primitives sit at z 0"
    // rule that `effective_z_index` encodes.
    let mut segment: Vec<(i32, SolidQuadInstance)> = Vec::new();

    fn flush_segment(
        geometry: &mut PrimitiveRunGeometry,
        segment: &mut Vec<(i32, SolidQuadInstance)>,
    ) {
        if segment.is_empty() {
            return;
        }
        segment.sort_by_key(|(z_index, _)| *z_index);
        let start = geometry.instances.len() as u32;
        geometry
            .instances
            .extend(segment.drain(..).map(|(_, instance)| instance));
        let end = geometry.instances.len() as u32;
        geometry.ops.push(PrimitiveRunOp::Draw(start..end));
    }

    for primitive in primitives {
        let z_index = widget_render::effective_z_index(primitive);
        match widget_render::innermost_primitive(primitive) {
            GpuPrimitive::Rect(rect) | GpuPrimitive::ForegroundRect(rect) => {
                if let Some(instance) = solid_rect_instance(
                    rect.rect,
                    rect.color.to_rgba(),
                    cell_w,
                    cell_h,
                    viewport_w,
                    viewport_h,
                ) {
                    segment.push((z_index, instance));
                }
            }
            GpuPrimitive::Quad(quad) => {
                let rect = Rect {
                    row: quad.y,
                    col: quad.x,
                    width: quad.width,
                    height: quad.height,
                };
                if let Some(instance) = solid_rect_instance(
                    rect,
                    quad.color.to_rgba(),
                    cell_w,
                    cell_h,
                    viewport_w,
                    viewport_h,
                ) {
                    segment.push((z_index, instance));
                }
            }
            GpuPrimitive::PushClipRect(rect) => {
                flush_segment(&mut geometry, &mut segment);
                geometry.ops.push(PrimitiveRunOp::PushClip(*rect));
            }
            GpuPrimitive::PopClipRect => {
                flush_segment(&mut geometry, &mut segment);
                geometry.ops.push(PrimitiveRunOp::PopClip);
            }
            _ => {}
        }
    }
    flush_segment(&mut geometry, &mut segment);
    geometry
}

pub(crate) fn push_solid_rect_vertices(
    rect: Rect,
    color: crate::backend::Color,
    cell_w: f32,
    cell_h: f32,
    viewport_w: f32,
    viewport_h: f32,
    vertices: &mut Vec<Vertex>,
) {
    if let Some(instance) = solid_rect_instance(
        rect,
        color.to_rgba(),
        cell_w,
        cell_h,
        viewport_w,
        viewport_h,
    ) {
        push_instance_vertices(instance, vertices);
    }
}

pub(crate) fn push_solid_quad_vertices(
    quad: crate::widget_render::GpuQuadPrimitive,
    cell_w: f32,
    cell_h: f32,
    viewport_w: f32,
    viewport_h: f32,
    vertices: &mut Vec<Vertex>,
) {
    push_solid_rect_vertices(
        Rect {
            row: quad.y,
            col: quad.x,
            width: quad.width,
            height: quad.height,
        },
        quad.color,
        cell_w,
        cell_h,
        viewport_w,
        viewport_h,
        vertices,
    );
}

fn push_instance_vertices(instance: SolidQuadInstance, vertices: &mut Vec<Vertex>) {
    let [left, top, right, bottom] = instance.ndc_bounds;
    let vertex = |position| Vertex {
        position,
        uv: [0.0, 0.0],
        fg: instance.color,
        bg: instance.color,
    };
    vertices.extend_from_slice(&[
        vertex([left, top]),
        vertex([left, bottom]),
        vertex([right, top]),
        vertex([right, top]),
        vertex([left, bottom]),
        vertex([right, bottom]),
    ]);
}

fn solid_rect_instance(
    rect: Rect,
    color: [f32; 4],
    cell_w: f32,
    cell_h: f32,
    viewport_w: f32,
    viewport_h: f32,
) -> Option<SolidQuadInstance> {
    if !rect.row.is_finite()
        || !rect.col.is_finite()
        || !rect.width.is_finite()
        || !rect.height.is_finite()
        || rect.width <= 0.0
        || rect.height <= 0.0
        || viewport_w <= 0.0
        || viewport_h <= 0.0
    {
        return None;
    }
    let ndc_x = |px: f32| px / viewport_w * 2.0 - 1.0;
    let ndc_y = |px: f32| 1.0 - px / viewport_h * 2.0;
    Some(SolidQuadInstance {
        ndc_bounds: [
            ndc_x(rect.col * cell_w),
            ndc_y(rect.row * cell_h),
            ndc_x((rect.col + rect.width) * cell_w),
            ndc_y((rect.row + rect.height) * cell_h),
        ],
        color,
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ScissorRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl ScissorRect {
    pub fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    pub fn from_cells(rect: Rect, cell_w: f32, cell_h: f32) -> Self {
        let left = (rect.col * cell_w).floor().max(0.0);
        let top = (rect.row * cell_h).floor().max(0.0);
        let right = ((rect.col + rect.width) * cell_w).ceil().max(left);
        let bottom = ((rect.row + rect.height) * cell_h).ceil().max(top);
        Self {
            x: left.min(u32::MAX as f32) as u32,
            y: top.min(u32::MAX as f32) as u32,
            width: (right - left).min(u32::MAX as f32) as u32,
            height: (bottom - top).min(u32::MAX as f32) as u32,
        }
    }

    pub fn intersect(self, other: Self) -> Self {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = self
            .x
            .saturating_add(self.width)
            .min(other.x.saturating_add(other.width));
        let y2 = self
            .y
            .saturating_add(self.height)
            .min(other.y.saturating_add(other.height));
        Self {
            x: x1,
            y: y1,
            width: x2.saturating_sub(x1),
            height: y2.saturating_sub(y1),
        }
    }
}

pub(crate) struct ClipStack {
    stack: Vec<ScissorRect>,
}

impl ClipStack {
    pub fn new(base: ScissorRect) -> Self {
        Self { stack: vec![base] }
    }

    pub fn current(&self) -> ScissorRect {
        *self.stack.last().expect("clip stack always has a base")
    }

    pub fn push_cells(&mut self, rect: Rect, cell_w: f32, cell_h: f32) {
        let nested = ScissorRect::from_cells(rect, cell_w, cell_h);
        self.stack.push(self.current().intersect(nested));
    }

    pub fn pop(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Color;
    use crate::widget_render::{GpuQuadPrimitive, GpuRectPrimitive};

    #[test]
    fn run_geometry_preserves_draw_order_and_nested_clip_commands() {
        let run = GpuPrimitiveRun {
            widget_id: 7,
            ordinal: 0,
            widget_type: "box".into(),
            ancestor_widget_ids: vec![],
            primitives: vec![
                GpuPrimitive::Rect(GpuRectPrimitive {
                    rect: Rect {
                        row: 0.0,
                        col: 0.0,
                        width: 4.0,
                        height: 2.0,
                    },
                    color: Color::rgb(1.0, 0.0, 0.0),
                }),
                GpuPrimitive::PushClipRect(Rect {
                    row: 0.5,
                    col: 1.0,
                    width: 2.0,
                    height: 1.0,
                }),
                GpuPrimitive::Quad(GpuQuadPrimitive {
                    x: 1.0,
                    y: 0.5,
                    width: 2.0,
                    height: 1.0,
                    color: Color::rgb(0.0, 1.0, 0.0),
                }),
                GpuPrimitive::PopClipRect,
            ],
            reused_from_previous: false,
        };

        let geometry = build_primitive_run_geometry(&run, 10.0, 20.0, 100.0, 100.0);
        assert_eq!(geometry.instances.len(), 2);
        assert_eq!(
            geometry.ops,
            vec![
                PrimitiveRunOp::Draw(0..1),
                PrimitiveRunOp::PushClip(Rect {
                    row: 0.5,
                    col: 1.0,
                    width: 2.0,
                    height: 1.0,
                }),
                PrimitiveRunOp::Draw(1..2),
                PrimitiveRunOp::PopClip,
            ]
        );
    }

    #[test]
    fn z_layers_sort_within_a_clip_segment_but_never_across_one() {
        let rect = |row: f32| {
            GpuPrimitive::Rect(GpuRectPrimitive {
                rect: Rect {
                    row,
                    col: 0.0,
                    width: 1.0,
                    height: 1.0,
                },
                color: Color::rgb(row / 10.0, 0.0, 0.0),
            })
        };
        let clip = Rect {
            row: 0.0,
            col: 0.0,
            width: 4.0,
            height: 4.0,
        };
        // Authored out of z order, and straddling a clip segment boundary.
        let geometry = build_primitive_geometry(
            &[
                widget_render::z_layer(5, rect(1.0)),
                rect(2.0),
                widget_render::z_layer(-1, rect(3.0)),
                GpuPrimitive::PushClipRect(clip),
                widget_render::z_layer(5, rect(4.0)),
                widget_render::z_layer(1, rect(5.0)),
                GpuPrimitive::PopClipRect,
            ],
            10.0,
            10.0,
            100.0,
            100.0,
        );

        // Metal buckets each clip segment by z index and draws the buckets in
        // ascending order; untagged primitives sit at z 0.
        let rows: Vec<i32> = geometry
            .instances
            .iter()
            .map(|instance| (((1.0 - instance.ndc_bounds[1]) * 50.0 / 10.0).round()) as i32)
            .collect();
        assert_eq!(rows, vec![3, 2, 1, 5, 4]);
        assert_eq!(
            geometry.ops,
            vec![
                PrimitiveRunOp::Draw(0..3),
                PrimitiveRunOp::PushClip(clip),
                PrimitiveRunOp::Draw(3..5),
                PrimitiveRunOp::PopClip,
            ]
        );
    }

    #[test]
    fn clip_stack_intersects_and_never_pops_the_viewport() {
        let mut clips = ClipStack::new(ScissorRect::full(100, 80));
        clips.push_cells(
            Rect {
                row: 1.0,
                col: 2.0,
                width: 20.0,
                height: 10.0,
            },
            10.0,
            10.0,
        );
        assert_eq!(
            clips.current(),
            ScissorRect {
                x: 20,
                y: 10,
                width: 80,
                height: 70
            }
        );
        clips.pop();
        clips.pop();
        assert_eq!(clips.current(), ScissorRect::full(100, 80));
    }
}
